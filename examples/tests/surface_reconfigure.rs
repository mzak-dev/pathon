// A window surface is reconfigured after frames were rendered into it (window resize, colour space switch) and the
// next frames must still render. On DX12 `ResizeBuffers` fails while anything still holds a back buffer, e.g. a view
// of the presented frame that waits in the deferred-release queue; the surface then stays unconfigured and every frame
// ends in R0009 (ADR-0034). Runs on the real window and GPU: PATHON_BACKEND=dx12|vulkan|metal picks the backend,
// `PATHON_REQUIRE_WINDOW=1` turns a missing display and `PATHON_REQUIRE_GPU=1` a missing adapter into a failure instead
// of a skip. Harness off: the event loop must be on the main thread.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a test without the harness reports on the console"
)]

use std::collections::VecDeque;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pathon::platform::{App, PhysicalSize, Platform, PlatformCtx, PlatformEvent, PlatformWindow, WindowDesc};
use pathon::rhi::{Backend, Color, DeviceRequest, SurfaceColorSpace, SurfaceTarget, WindowSource};
use pathon::{FrameInput, Renderer, TargetId, ViewInput, WinitPlatform};

const FRAMES_BETWEEN_STEPS: u32 = 4;
const RESIZE_TIMEOUT: Duration = Duration::from_secs(10);
const GROWTH: PhysicalSize = PhysicalSize { width: 120, height: 80 };

#[derive(Clone, Copy, Debug)]
enum Step {
    Render(u32),
    // Same parameters again: the case the DX12 error also hit.
    ConfigureAgain,
    // Resizes the real window, waits for `Resized` and tells the renderer, like the runner does.
    GrowWindow,
    ColorSpace(SurfaceColorSpace),
}

fn script() -> VecDeque<Step> {
    let spaces = [
        SurfaceColorSpace::Auto,
        SurfaceColorSpace::Srgb,
        SurfaceColorSpace::ExtendedSrgbLinear,
        SurfaceColorSpace::Hdr10St2084,
        SurfaceColorSpace::Auto,
    ];
    let mut steps = VecDeque::from([
        Step::Render(FRAMES_BETWEEN_STEPS),
        Step::ConfigureAgain,
        Step::Render(FRAMES_BETWEEN_STEPS),
        Step::GrowWindow,
        Step::Render(FRAMES_BETWEEN_STEPS),
        Step::GrowWindow,
        Step::GrowWindow,
        Step::Render(FRAMES_BETWEEN_STEPS),
    ]);
    for space in spaces {
        steps.push_back(Step::ColorSpace(space));
        steps.push_back(Step::Render(FRAMES_BETWEEN_STEPS));
    }
    steps
}

fn requested_backend() -> Option<Backend> {
    match std::env::var("PATHON_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "dx12" => Some(Backend::Dx12),
        "vulkan" => Some(Backend::Vulkan),
        "metal" => Some(Backend::Metal),
        _ => None,
    }
}

fn require_gpu() -> bool {
    std::env::var_os("PATHON_REQUIRE_GPU").is_some()
}

fn require_window() -> bool {
    std::env::var_os("PATHON_REQUIRE_WINDOW").is_some()
}

fn has_display() -> bool {
    !cfg!(target_os = "linux") || std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

struct Running {
    window: Arc<dyn PlatformWindow>,
    renderer: Renderer,
    target: TargetId,
    size: pathon::rhi::PhysicalSize,
}

type Outcome = Arc<Mutex<Option<Result<(), String>>>>;

struct Probe {
    running: Option<Running>,
    steps: VecDeque<Step>,
    frames_left: u32,
    // Window size the pending `GrowWindow` waits for.
    resize_wait: Option<(PhysicalSize, Instant)>,
    rendered: u32,
    outcome: Outcome,
}

fn rhi_size(size: PhysicalSize) -> pathon::rhi::PhysicalSize {
    pathon::rhi::PhysicalSize::new(size.width, size.height)
}

impl Probe {
    fn finish(&mut self, ctx: &mut dyn PlatformCtx, outcome: Result<(), String>) {
        self.outcome
            .lock()
            .expect("the probe never panics while holding the outcome")
            .get_or_insert(outcome);
        self.running = None;
        ctx.exit();
    }

    fn is_finished(&self) -> bool {
        self.outcome
            .lock()
            .expect("the probe never panics while holding the outcome")
            .is_some()
    }

    fn start(&mut self, ctx: &mut dyn PlatformCtx) {
        let desc = WindowDesc::new("pathon — surface reconfigure", 400.0, 300.0);
        let window = match ctx.create_window(&desc) {
            Ok(window) => window,
            Err(error) => return self.finish(ctx, Err(format!("window creation failed: {error}"))),
        };
        let size = rhi_size(window.inner_size());
        let source: Arc<dyn WindowSource> = Arc::new(Arc::clone(&window));
        let surface = SurfaceTarget::new(source, size, window.scale_factor());
        let built = Renderer::builder()
            .with_device_request(DeviceRequest {
                backend: requested_backend(),
                ..DeviceRequest::default()
            })
            .with_compatible_surface(surface.clone())
            .with_clear_color(Color::rgba(0.1, 0.2, 0.35, 1.0))
            .build();
        let mut renderer = match built {
            Ok(renderer) => renderer,
            Err(error) if !require_gpu() => {
                eprintln!("surface_reconfigure: skipped, no usable GPU backend: {error}");
                return self.finish(ctx, Ok(()));
            }
            Err(error) => return self.finish(ctx, Err(format!("renderer build failed: {error}"))),
        };
        match renderer.add_window(surface) {
            Ok(target) => {
                self.steps = script();
                self.running = Some(Running {
                    window,
                    renderer,
                    target,
                    size,
                });
            }
            Err(error) => self.finish(ctx, Err(format!("add_window failed: {error}"))),
        }
    }

    fn render_one(&mut self) -> Result<(), String> {
        let running = self.running.as_mut().ok_or("no renderer")?;
        let input = FrameInput {
            time_seconds: f64::from(self.rendered) / 60.0,
            delta_seconds: 1.0 / 60.0,
            views: vec![ViewInput {
                target: running.target,
                camera: pathon::api::Camera::default(),
            }],
        };
        let report = running
            .renderer
            .render_frame(&input)
            .map_err(|error| format!("frame {} failed: {error}", self.rendered))?;
        if report.skipped_views > 0 {
            return Err(format!("frame {} skipped its view", self.rendered));
        }
        self.rendered += 1;
        Ok(())
    }

    fn configure_again(&mut self) -> Result<(), String> {
        let running = self.running.as_mut().ok_or("no renderer")?;
        running
            .renderer
            .resize_target(running.target, running.size)
            .map_err(|error| format!("reconfiguring with the same size failed: {error}"))
    }

    fn start_growing(&mut self) -> Result<(), String> {
        let running = self.running.as_ref().ok_or("no renderer")?;
        let current = running.window.inner_size();
        let wanted = PhysicalSize {
            width: current.width + GROWTH.width,
            height: current.height + GROWTH.height,
        };
        running.window.request_inner_size(wanted);
        self.resize_wait = Some((current, Instant::now()));
        Ok(())
    }

    fn switch_color_space(&mut self, space: SurfaceColorSpace) -> Result<(), String> {
        let running = self.running.as_mut().ok_or("no renderer")?;
        running
            .renderer
            .set_target_color_space(running.target, space)
            .map_err(|error| format!("switching to {space:?} failed: {error}"))
    }

    fn advance(&mut self) -> Result<(), String> {
        if self.frames_left > 0 {
            self.frames_left -= 1;
            return self.render_one();
        }
        let Some(step) = self.steps.pop_front() else {
            return Ok(());
        };
        println!("surface_reconfigure: {step:?}");
        match step {
            Step::Render(count) => {
                self.frames_left = count;
                Ok(())
            }
            Step::ConfigureAgain => self.configure_again(),
            Step::GrowWindow => self.start_growing(),
            Step::ColorSpace(space) => self.switch_color_space(space),
        }
    }

    fn check_resize_wait(&mut self) -> Result<(), String> {
        let Some((previous, started)) = self.resize_wait else {
            return Ok(());
        };
        let running = self.running.as_mut().ok_or("no renderer")?;
        let size = running.window.inner_size();
        if size != previous {
            self.resize_wait = None;
            running.size = rhi_size(size);
            return running
                .renderer
                .resize_target(running.target, running.size)
                .map_err(|error| format!("resizing to {size:?} failed: {error}"));
        }
        if started.elapsed() > RESIZE_TIMEOUT {
            return Err(format!(
                "the window did not leave {previous:?} within {RESIZE_TIMEOUT:?}"
            ));
        }
        Ok(())
    }
}

impl App for Probe {
    fn on_event(&mut self, ctx: &mut dyn PlatformCtx, event: PlatformEvent) {
        match event {
            PlatformEvent::SurfacesAvailable if self.running.is_none() && !self.is_finished() => self.start(ctx),
            PlatformEvent::CloseRequested(_) => self.finish(ctx, Err("the window was closed".into())),
            _ => {}
        }
    }

    fn on_frame(&mut self, ctx: &mut dyn PlatformCtx) {
        if self.running.is_none() {
            return;
        }
        let result = if self.resize_wait.is_some() {
            self.check_resize_wait()
        } else {
            self.advance()
        };
        match result {
            Err(message) => self.finish(ctx, Err(message)),
            Ok(()) if self.frames_left == 0 && self.steps.is_empty() && self.resize_wait.is_none() => {
                self.finish(ctx, Ok(()));
            }
            Ok(()) => {}
        }
    }
}

fn main() -> ExitCode {
    if !has_display() {
        if require_window() {
            eprintln!("surface_reconfigure: no display and PATHON_REQUIRE_WINDOW is set");
            return ExitCode::FAILURE;
        }
        eprintln!("surface_reconfigure: skipped, no display");
        return ExitCode::SUCCESS;
    }
    pathon_examples::init_logging();
    // The probe moves into the platform, so the outcome comes back through a shared cell.
    let outcome: Outcome = Arc::default();
    let probe = Probe {
        running: None,
        steps: VecDeque::new(),
        frames_left: 0,
        resize_wait: None,
        rendered: 0,
        outcome: Arc::clone(&outcome),
    };
    if let Err(error) = WinitPlatform::new().run(Box::new(probe)) {
        eprintln!("surface_reconfigure: the platform failed: {error}");
        return ExitCode::FAILURE;
    }
    let outcome = outcome
        .lock()
        .expect("the probe never panics while holding the outcome")
        .take();
    match outcome {
        Some(Ok(())) => {
            println!("surface_reconfigure: ok");
            ExitCode::SUCCESS
        }
        Some(Err(message)) => {
            eprintln!("surface_reconfigure: FAILED: {message}");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("surface_reconfigure: FAILED: the event loop ended without an outcome");
            ExitCode::FAILURE
        }
    }
}
