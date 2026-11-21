// M0-11: the host owns the window and the event loop (raw winit, no pathon-platform) and calls the renderer on its
// own thread — integration mode C in an own engine without the runner.

use std::sync::Arc;
use std::time::Instant;

use pathon::rhi::{DeviceRequest, PhysicalSize, SurfaceTarget, WindowSource};
use pathon::{FrameInput, Renderer, TargetId, ViewInput};
use pathon_test::UnlitMeshPack;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct Running {
    window: Arc<Window>,
    renderer: Renderer,
    target: TargetId,
}

struct ExternalWindow {
    running: Option<Running>,
    start: Instant,
}

impl ExternalWindow {
    fn start(event_loop: &ActiveEventLoop) -> Running {
        let attributes = Window::default_attributes().with_title("pathon — external window");
        let window = Arc::new(event_loop.create_window(attributes).expect("window"));
        let size = window.inner_size();
        let source: Arc<dyn WindowSource> = Arc::clone(&window) as Arc<dyn WindowSource>;
        let surface = SurfaceTarget::new(
            source,
            PhysicalSize::new(size.width, size.height),
            window.scale_factor(),
        );
        let mut renderer = Renderer::builder()
            .with_feature(UnlitMeshPack::new())
            .with_device_request(DeviceRequest {
                compatible_surface: Some(surface.clone()),
                ..DeviceRequest::default()
            })
            .build()
            .expect("renderer");
        let target = renderer.add_window(surface).expect("window target");
        Running {
            window,
            renderer,
            target,
        }
    }
}

impl ApplicationHandler for ExternalWindow {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_none() {
            self.running = Some(Self::start(event_loop));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(running) = &mut self.running else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Err(error) = running
                    .renderer
                    .resize_target(running.target, PhysicalSize::new(size.width, size.height))
                {
                    log::error!("resize failed: {error}");
                }
            }
            WindowEvent::RedrawRequested => {
                let size = running.window.inner_size();
                let time_seconds = self.start.elapsed().as_secs_f64();
                let camera = pathon_examples::orbit_camera(time_seconds, size.width as f32 / size.height.max(1) as f32);
                let input = FrameInput {
                    time_seconds,
                    delta_seconds: 0.0,
                    views: vec![ViewInput {
                        target: running.target,
                        camera,
                    }],
                };
                if let Err(error) = running.renderer.render_frame(&input) {
                    log::error!("frame failed: {error}");
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(running) = &self.running {
            running.window.request_redraw();
        }
    }
}

fn main() {
    pathon_examples::init_logging();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = ExternalWindow {
        running: None,
        start: Instant::now(),
    };
    event_loop.run_app(&mut app).expect("event loop ran");
}
