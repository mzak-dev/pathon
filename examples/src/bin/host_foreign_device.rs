// M2-19: a raw wgpu + winit host creates the device, the window and the swapchain itself. pathon adopts the device,
// renders the scene into a texture the host owns (mode B), and the host copies it to its swapchain and presents.

use std::sync::Arc;
use std::time::Instant;

use pathon::rhi::{Access, Device};
use pathon::{FrameInput, Renderer, TargetId, ViewInput};
use pathon_examples::block_on_ready;
use pathon_rhi_wgpu::interop;
use pathon_test::UnlitMeshPack;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct Host {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pathon_device: Arc<Device>,
    renderer: Renderer,
    color: wgpu::Texture,
    target: TargetId,
}

impl Host {
    fn start(event_loop: &ActiveEventLoop) -> Self {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("pathon — host device"))
                .expect("window"),
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(Arc::clone(&window)).expect("surface");
        let adapter = block_on_ready(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let (device, queue) =
            block_on_ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface config");
        config.usage |= wgpu::TextureUsages::COPY_DST;
        surface.configure(&device, &config);

        let pathon_device = Arc::new(Device::Wgpu(interop::adopt(
            instance,
            adapter,
            device.clone(),
            queue.clone(),
        )));
        let mut renderer = Renderer::builder()
            .with_device(Arc::clone(&pathon_device))
            .with_feature(UnlitMeshPack::new())
            .build()
            .expect("renderer on the host device");
        let (color, target) = Self::host_texture(&device, &pathon_device, &mut renderer, &config);
        Self {
            window,
            surface,
            config,
            device,
            queue,
            pathon_device,
            renderer,
            color,
            target,
        }
    }

    fn host_texture(
        device: &wgpu::Device,
        pathon_device: &Device,
        renderer: &mut Renderer,
        config: &wgpu::SurfaceConfiguration,
    ) -> (wgpu::Texture, TargetId) {
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("host color"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let wgpu_device = pathon_device.as_wgpu().expect("adopted wgpu device");
        let texture = wgpu_device.import_texture("host color", color.clone()).expect("import");
        let target = renderer
            .import_texture(texture, Access::COLOR_ATTACHMENT, Access::COPY_SRC)
            .expect("target");
        (color, target)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        if let Some(old) = self.renderer.target_texture(self.target) {
            let _ = self.renderer.remove_target(self.target);
            if let Some(wgpu_device) = self.pathon_device.as_wgpu() {
                wgpu_device.forget_texture(old);
            }
        }
        let (color, target) = Self::host_texture(&self.device, &self.pathon_device, &mut self.renderer, &self.config);
        self.color = color;
        self.target = target;
    }

    fn redraw(&mut self, time_seconds: f64) {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("host background"),
        });
        let view = self.color.create_view(&wgpu::TextureViewDescriptor::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("host background"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.3,
                        g: 0.1,
                        b: 0.1,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        }));
        self.queue.submit([encoder.finish()]);

        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let camera = pathon_examples::orbit_camera(time_seconds, aspect);
        let input = FrameInput {
            time_seconds,
            delta_seconds: 0.0,
            views: vec![ViewInput {
                target: self.target,
                camera,
            }],
        };
        if let Err(error) = self.renderer.render_frame(&input) {
            log::error!("pathon frame failed: {error}");
            return;
        }

        let (wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame)) =
            self.surface.get_current_texture()
        else {
            return;
        };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("host present"),
        });
        encoder.copy_texture_to_texture(
            self.color.as_image_copy(),
            frame.texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.config.width,
                height: self.config.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }
}

struct App {
    host: Option<Host>,
    start: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.host.is_none() {
            self.host = Some(Host::start(event_loop));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(host) = &mut self.host else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => host.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                host.redraw(self.start.elapsed().as_secs_f64());
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(host) = &self.host {
            host.window.request_redraw();
        }
    }
}

fn main() {
    pathon_examples::init_logging();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        host: None,
        start: Instant::now(),
    };
    event_loop.run_app(&mut app).expect("event loop ran");
}
