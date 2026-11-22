// M2-17: the host owns two winit windows and renders them plus a 256x256 offscreen target with one renderer —
// integration mode C without the runner. The offscreen target is read back every 120 frames.

use std::sync::Arc;
use std::time::Instant;

use pathon::rhi::{
    Access, BufferCopy, BufferDesc, BufferId, BufferUsage, CommandList, DeviceApi, DeviceRequest, Extent3d, Format,
    PhysicalSize, SubresourceRange, SurfaceTarget, TexelLayout, TextureCopy, WindowSource,
};
use pathon::{FrameInput, Renderer, TargetId, ViewInput};
use pathon_test::UnlitMeshPack;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const OFFSCREEN_SIZE: u32 = 256;
const READBACK_EVERY_FRAMES: u64 = 120;

struct HostWindow {
    window: Arc<Window>,
    target: TargetId,
    time_offset_seconds: f64,
}

struct Running {
    windows: Vec<HostWindow>,
    renderer: Renderer,
    offscreen: TargetId,
    readback: BufferId,
}

struct MultiView {
    running: Option<Running>,
    start: Instant,
    frame_index: u64,
}

fn surface_for(window: &Arc<Window>) -> SurfaceTarget {
    let size = window.inner_size();
    let source: Arc<dyn WindowSource> = Arc::clone(window) as Arc<dyn WindowSource>;
    SurfaceTarget::new(
        source,
        PhysicalSize::new(size.width, size.height),
        window.scale_factor(),
    )
}

fn create_window(event_loop: &ActiveEventLoop, title: &str) -> Arc<Window> {
    let attributes = Window::default_attributes().with_title(title);
    Arc::new(event_loop.create_window(attributes).expect("window"))
}

// Blocks until the GPU finishes; acceptable in an example.
fn read_pixel_at_centre(renderer: &Renderer, target: TargetId, readback: BufferId) -> [u8; 4] {
    let device = renderer.device();
    let texture = renderer.target_texture(target).expect("offscreen texture");
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(texture),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: OFFSCREEN_SIZE * 4,
                rows_per_image: OFFSCREEN_SIZE,
            },
        },
        Extent3d::new_2d(OFFSCREEN_SIZE, OFFSCREEN_SIZE),
    );
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_SRC, Access::SAMPLED);
    device.submit(list).expect("copy");
    let pixels = device.read_buffer(readback).expect("read");
    let start = ((OFFSCREEN_SIZE / 2 * OFFSCREEN_SIZE + OFFSCREEN_SIZE / 2) * 4) as usize;
    pixels[start..start + 4].try_into().expect("four bytes")
}

impl MultiView {
    fn start(event_loop: &ActiveEventLoop) -> Running {
        let first = create_window(event_loop, "pathon — multi view, left");
        let second = create_window(event_loop, "pathon — multi view, right");
        let first_surface = surface_for(&first);
        let second_surface = surface_for(&second);
        let mut renderer = Renderer::builder()
            .with_feature(UnlitMeshPack::new())
            .with_device_request(DeviceRequest {
                compatible_surface: Some(first_surface.clone()),
                ..DeviceRequest::default()
            })
            .build()
            .expect("renderer");
        let first_target = renderer.add_window(first_surface).expect("first window target");
        let second_target = renderer.add_window(second_surface).expect("second window target");
        let offscreen = renderer
            .add_offscreen_target(PhysicalSize::new(OFFSCREEN_SIZE, OFFSCREEN_SIZE), Format::Rgba8Unorm)
            .expect("offscreen target");
        let readback = renderer
            .device()
            .create_buffer(&BufferDesc {
                label: "offscreen readback",
                size: u64::from(OFFSCREEN_SIZE * OFFSCREEN_SIZE * 4),
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback buffer");
        Running {
            windows: vec![
                HostWindow {
                    window: first,
                    target: first_target,
                    time_offset_seconds: 0.0,
                },
                HostWindow {
                    window: second,
                    target: second_target,
                    time_offset_seconds: 2.0,
                },
            ],
            renderer,
            offscreen,
            readback,
        }
    }

    fn render(&mut self) {
        let Some(running) = &mut self.running else {
            return;
        };
        let time_seconds = self.start.elapsed().as_secs_f64();
        let mut views: Vec<ViewInput> = running
            .windows
            .iter()
            .map(|host| {
                let size = host.window.inner_size();
                let aspect = size.width as f32 / size.height.max(1) as f32;
                ViewInput {
                    target: host.target,
                    camera: pathon_examples::orbit_camera(time_seconds + host.time_offset_seconds, aspect),
                }
            })
            .collect();
        views.push(ViewInput {
            target: running.offscreen,
            camera: pathon_examples::orbit_camera(time_seconds + 4.0, 1.0),
        });
        let input = FrameInput {
            time_seconds,
            delta_seconds: 0.0,
            views,
        };
        if let Err(error) = running.renderer.render_frame(&input) {
            log::error!("frame failed: {error}");
        }
        if self.frame_index.is_multiple_of(READBACK_EVERY_FRAMES) {
            let rgba = read_pixel_at_centre(&running.renderer, running.offscreen, running.readback);
            log::info!("offscreen pixel at frame {}: {rgba:?}", self.frame_index);
        }
        self.frame_index += 1;
    }
}

impl ApplicationHandler for MultiView {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_none() {
            self.running = Some(Self::start(event_loop));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let Some(running) = &mut self.running else {
                    return;
                };
                let Some(host) = running.windows.iter().find(|host| host.window.id() == id) else {
                    return;
                };
                if let Err(error) = running
                    .renderer
                    .resize_target(host.target, PhysicalSize::new(size.width, size.height))
                {
                    log::error!("resize failed: {error}");
                }
            }
            WindowEvent::RedrawRequested => {
                let is_first = self
                    .running
                    .as_ref()
                    .is_some_and(|running| running.windows.first().is_some_and(|host| host.window.id() == id));
                // One render_frame records every view, so a redraw of the first window is enough.
                if is_first {
                    self.render();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(running) = &self.running
            && let Some(first) = running.windows.first()
        {
            first.window.request_redraw();
        }
    }
}

fn main() {
    pathon_examples::init_logging();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = MultiView {
        running: None,
        start: Instant::now(),
        frame_index: 0,
    };
    event_loop.run_app(&mut app).expect("event loop ran");
}
