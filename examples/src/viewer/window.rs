// The windowed viewer: an `App` of pathon-platform (so any platform backend runs it) that builds the renderer once
// surfaces exist, uploads the scene and turns the raw input events into camera movement.

use std::sync::Arc;
use std::time::Instant;

use pathon::TargetId;
use pathon::platform::{App, PhysicalSize, PlatformCtx, PlatformEvent, PlatformWindow, WindowDesc};
use pathon::rhi::{DeviceApi, PhysicalSize as RhiSize, SurfaceTarget, WindowSource};

use super::controls::{Command, Controls};
use super::session::{Session, viewer_builder};
use crate::gltf_loader::LoadedScene;

const WHEEL_SPEED_FACTOR: f32 = 1.15;
const FRAMES_PER_REPORT: u64 = 120;

struct Running {
    window: Arc<dyn PlatformWindow>,
    target: TargetId,
    session: Session,
    controls: Controls,
    last_update: Instant,
    frames: u64,
    frame_limit: Option<u64>,
    measured_since: Instant,
}

pub struct ViewerWindow {
    scene: Option<LoadedScene>,
    title: String,
    running: Option<Running>,
    failure: Arc<std::sync::Mutex<Option<String>>>,
}

impl std::fmt::Debug for ViewerWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerWindow")
            .field("title", &self.title)
            .field("running", &self.running.is_some())
            .finish_non_exhaustive()
    }
}

fn surface_target(window: &Arc<dyn PlatformWindow>) -> SurfaceTarget {
    let source: Arc<dyn WindowSource> = Arc::new(Arc::clone(window));
    let size = window.inner_size();
    SurfaceTarget::new(source, RhiSize::new(size.width, size.height), window.scale_factor())
}

impl ViewerWindow {
    /// `failure` receives the reason when the viewer stops because of an error, so `main` can exit with a code.
    pub fn new(scene: LoadedScene, title: String, failure: Arc<std::sync::Mutex<Option<String>>>) -> Self {
        Self {
            scene: Some(scene),
            title,
            running: None,
            failure,
        }
    }

    fn fail(&self, ctx: &mut dyn PlatformCtx, reason: String) {
        log::error!("{reason}");
        if let Ok(mut slot) = self.failure.lock() {
            *slot = Some(reason);
        }
        ctx.exit();
    }

    fn start(&mut self, ctx: &mut dyn PlatformCtx) -> Result<Running, String> {
        let scene = self.scene.take().ok_or("the scene was already taken")?;
        let window = ctx
            .create_window(&WindowDesc::new(self.title.clone(), 1280.0, 720.0))
            .map_err(|error| error.to_string())?;
        let surface = surface_target(&window);
        let mut renderer = viewer_builder()
            .with_compatible_surface(surface.clone())
            .build()
            .map_err(|error| error.to_string())?;
        log::info!("adapter: {}", renderer.device().caps().adapter.name);
        let target = renderer.add_window(surface).map_err(|error| error.to_string())?;
        let session = Session::new(renderer, &scene).map_err(|error| error.to_string())?;
        log::info!(
            "controls: WASD/arrows move, Q/E down/up, Shift fast, mouse drag look, wheel speed, 1-9 file cameras, \
             +/- exposure, [ ] ambient, T sun shadows, I info, Esc quit"
        );
        Ok(Running {
            window,
            target,
            session,
            controls: Controls::default(),
            last_update: Instant::now(),
            frames: 0,
            frame_limit: crate::frame_limit_from_env(),
            measured_since: Instant::now(),
        })
    }
}

fn aspect_ratio(size: PhysicalSize) -> f32 {
    size.width as f32 / size.height.max(1) as f32
}

impl Running {
    fn apply(&mut self, command: Command) -> bool {
        let session = &mut self.session;
        match command {
            Command::SwitchCamera(index) => {
                if !session.use_scene_camera(index) {
                    log::info!("the file has no camera {}", index + 1);
                }
            }
            Command::ExposureUp => session.settings.adjust_exposure(1.0),
            Command::ExposureDown => session.settings.adjust_exposure(-1.0),
            Command::AmbientUp => session.settings.adjust_ambient(1.0),
            Command::AmbientDown => session.settings.adjust_ambient(-1.0),
            Command::ToggleShadows => session.settings.sun_shadows = !session.settings.sun_shadows,
            Command::PrintInfo => log::info!("{}", session.describe()),
            Command::Exit => return false,
        }
        true
    }

    fn update_camera(&mut self) {
        let now = Instant::now();
        let seconds = now.duration_since(self.last_update).as_secs_f32().min(0.1);
        self.last_update = now;
        let (look_x, look_y) = self.controls.take_look();
        let wheel = self.controls.take_wheel();
        let camera = &mut self.session.camera;
        camera.look(look_x, look_y);
        camera.speed *= WHEEL_SPEED_FACTOR.powf(wheel);
        camera.walk(self.controls.movement(), seconds);
    }
}

impl App for ViewerWindow {
    fn on_event(&mut self, ctx: &mut dyn PlatformCtx, event: PlatformEvent) {
        match event {
            PlatformEvent::SurfacesAvailable if self.running.is_none() => match self.start(ctx) {
                Ok(running) => self.running = Some(running),
                Err(reason) => self.fail(ctx, reason),
            },
            PlatformEvent::Resized { size, .. } => {
                if let Some(running) = &mut self.running
                    && let Err(error) = running
                        .session
                        .renderer
                        .resize_target(running.target, RhiSize::new(size.width, size.height))
                {
                    log::error!("resize failed: {error}");
                }
            }
            PlatformEvent::CloseRequested(_) => ctx.exit(),
            PlatformEvent::Input { event, .. } => {
                let Some(running) = &mut self.running else {
                    return;
                };
                if let Some(command) = running.controls.handle(&event)
                    && !running.apply(command)
                {
                    ctx.exit();
                }
            }
            _ => {}
        }
    }

    fn on_frame(&mut self, ctx: &mut dyn PlatformCtx) {
        let Some(running) = &mut self.running else {
            return;
        };
        let size = running.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        running.update_camera();
        match running.session.render(running.target, aspect_ratio(size)) {
            Ok(report) => {
                for diagnostic in &report.diagnostics {
                    log::warn!("{diagnostic}");
                }
                running.frames += 1;
                if running.frames.is_multiple_of(FRAMES_PER_REPORT) {
                    let seconds = running.measured_since.elapsed().as_secs_f32();
                    log::info!(
                        "{:.1} fps ({:.2} ms per frame)",
                        FRAMES_PER_REPORT as f32 / seconds,
                        seconds * 1000.0 / FRAMES_PER_REPORT as f32
                    );
                    running.measured_since = Instant::now();
                }
                if running.frame_limit.is_some_and(|limit| running.frames >= limit) {
                    log::info!("rendered {} frames, exiting", running.frames);
                    ctx.exit();
                }
            }
            Err(error) => {
                let reason = format!("frame failed: {error}");
                self.fail(ctx, reason);
            }
        }
    }
}
