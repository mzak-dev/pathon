// The optional runner for integration mode C in an own engine (ADR-0004): the platform loop stays on the main thread
// and creates surfaces, the renderer lives on a named render thread at most one frame behind.

use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crossbeam_channel::{Sender, bounded};
use pathon_api::TaskPool;
use pathon_core::{FrameInput, FrameReport, RenderError, Renderer, RendererBuilder, TargetId, ViewInput};
use pathon_platform::{App, Platform, PlatformCtx, PlatformError, PlatformEvent, PlatformWindow, WindowDesc, WindowId};
use pathon_rhi::{PhysicalSize, SurfaceTarget, WindowSource};

// Ceiling: spawns threads on every call; switch to persistent workers when M4-17 measurements show the cost.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StdTaskPool {
    threads: usize,
}

impl StdTaskPool {
    pub fn new() -> Self {
        Self {
            threads: thread::available_parallelism().map_or(1, NonZeroUsize::get),
        }
    }
}

impl Default for StdTaskPool {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskPool for StdTaskPool {
    fn parallel_for(&self, len: usize, task: &(dyn Fn(Range<usize>) + Sync)) {
        let workers = self.threads.min(len);
        if workers <= 1 {
            if len > 0 {
                task(0..len);
            }
            return;
        }
        let chunk = len.div_ceil(workers);
        thread::scope(|scope| {
            for start in (chunk..len).step_by(chunk) {
                let end = (start + chunk).min(len);
                thread::Builder::new()
                    .name("pathon-task".into())
                    .spawn_scoped(scope, move || task(start..end))
                    .expect("the OS can start a worker thread");
            }
            task(0..chunk);
        });
    }

    fn thread_count(&self) -> usize {
        self.threads
    }
}

#[derive(Debug)]
pub enum RunError {
    Platform(PlatformError),
    Render(RenderError),
}

impl std::error::Error for RunError {}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Platform(error) => write!(f, "platform: {error}"),
            Self::Render(error) => write!(f, "renderer: {error}"),
        }
    }
}

#[derive(Debug)]
pub struct FrameContext<'run> {
    pub frame_index: u64,
    pub time_seconds: f64,
    pub delta_seconds: f32,
    pub windows: &'run [(TargetId, PhysicalSize)],
}

enum Message {
    Frame(FrameInput),
    Resize(TargetId, PhysicalSize),
    Exit,
}

struct Running {
    windows: Vec<(WindowId, TargetId, PhysicalSize)>,
    sender: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

enum State {
    Waiting(Option<RendererBuilder>),
    Running(Running),
    Done,
}

// Runs on the render thread after every frame (e.g. pathon-devtools publishes its snapshot here).
type FrameHook = Box<dyn FnMut(&mut Renderer, &FrameReport) + Send>;

struct RunnerApp<Scene> {
    state: State,
    hook: Option<FrameHook>,
    windows: Vec<WindowDesc>,
    scene: Scene,
    error: Arc<Mutex<Option<RunError>>>,
    start: Instant,
    last_frame: Instant,
    frame_index: u64,
}

fn lock_error(error: &Mutex<Option<RunError>>) -> MutexGuard<'_, Option<RunError>> {
    error
        .lock()
        .expect("the error slot is only written by plain assignments, which do not panic")
}

fn rhi_size(size: pathon_platform::PhysicalSize) -> PhysicalSize {
    PhysicalSize::new(size.width, size.height)
}

fn surface_target(window: &Arc<dyn PlatformWindow>) -> SurfaceTarget {
    let source: Arc<dyn WindowSource> = Arc::new(Arc::clone(window));
    SurfaceTarget::new(source, rhi_size(window.inner_size()), window.scale_factor())
}

fn render_loop(
    mut renderer: Renderer,
    receiver: &crossbeam_channel::Receiver<Message>,
    mut hook: FrameHook,
    error_slot: &Mutex<Option<RunError>>,
) {
    for message in receiver {
        match message {
            Message::Frame(input) => match renderer.render_frame(&input) {
                Ok(report) => hook(&mut renderer, &report),
                Err(RenderError::DeviceLost) => {
                    log::warn!("device lost, rebuilding");
                    if let Err(error) = renderer.recover() {
                        log::error!("device recovery failed: {error}");
                        *lock_error(error_slot) = Some(RunError::Render(error));
                        return;
                    }
                }
                Err(error) => log::error!("frame failed: {error}"),
            },
            Message::Resize(target, size) => {
                if let Err(error) = renderer.resize_target(target, size) {
                    log::error!("resize failed: {error}");
                }
            }
            Message::Exit => return,
        }
    }
}

impl<Scene: FnMut(&FrameContext<'_>) -> Vec<ViewInput>> RunnerApp<Scene> {
    fn start(&mut self, ctx: &mut dyn PlatformCtx, builder: RendererBuilder) -> Result<Running, RunError> {
        let mut created = Vec::with_capacity(self.windows.len());
        for desc in &self.windows {
            created.push(ctx.create_window(desc).map_err(RunError::Platform)?);
        }
        // One SurfaceTarget per window, so the surface made for adapter selection is the one the window keeps.
        let targets: Vec<SurfaceTarget> = created.iter().map(surface_target).collect();
        let builder = match targets.first() {
            Some(first) => builder.with_compatible_surface(first.clone()),
            None => builder,
        };
        let mut renderer = builder.build().map_err(RunError::Render)?;
        let mut windows = Vec::with_capacity(created.len());
        for (window, surface) in created.iter().zip(targets) {
            let target = renderer.add_window(surface).map_err(RunError::Render)?;
            windows.push((window.id(), target, rhi_size(window.inner_size())));
        }
        let (sender, receiver) = bounded(1);
        let hook = self.hook.take().unwrap_or_else(|| Box::new(|_, _| {}));
        let error_slot = Arc::clone(&self.error);
        let thread = thread::Builder::new()
            .name("pathon-render".into())
            .spawn(move || render_loop(renderer, &receiver, hook, &error_slot))
            .expect("the OS can start the render thread");
        Ok(Running {
            windows,
            sender,
            thread: Some(thread),
        })
    }

    fn stop(&mut self, ctx: &mut dyn PlatformCtx) {
        if let State::Running(running) = &mut self.state {
            let _ = running.sender.send(Message::Exit);
            if let Some(thread) = running.thread.take()
                && thread.join().is_err()
            {
                log::error!("render thread panicked");
            }
        }
        self.state = State::Done;
        ctx.exit();
    }
}

impl<Scene: FnMut(&FrameContext<'_>) -> Vec<ViewInput>> App for RunnerApp<Scene> {
    fn on_event(&mut self, ctx: &mut dyn PlatformCtx, event: PlatformEvent) {
        match event {
            PlatformEvent::SurfacesAvailable => {
                let State::Waiting(builder) = &mut self.state else {
                    return;
                };
                let Some(builder) = builder.take() else {
                    return;
                };
                match self.start(ctx, builder) {
                    Ok(running) => self.state = State::Running(running),
                    Err(error) => {
                        *lock_error(&self.error) = Some(error);
                        self.stop(ctx);
                    }
                }
            }
            PlatformEvent::Resized { window, size } => {
                if let State::Running(running) = &mut self.state
                    && let Some(entry) = running.windows.iter_mut().find(|(id, _, _)| *id == window)
                {
                    entry.2 = rhi_size(size);
                    let _ = running.sender.send(Message::Resize(entry.1, entry.2));
                }
            }
            PlatformEvent::CloseRequested(_) => self.stop(ctx),
            PlatformEvent::SurfacesLost => log::warn!("surfaces lost: the runner supports desktop platforms only"),
            _ => {}
        }
    }

    fn on_frame(&mut self, ctx: &mut dyn PlatformCtx) {
        let State::Running(running) = &self.state else {
            return;
        };
        let now = Instant::now();
        let windows: Vec<(TargetId, PhysicalSize)> = running
            .windows
            .iter()
            .map(|(_, target, size)| (*target, *size))
            .collect();
        let frame = FrameContext {
            frame_index: self.frame_index,
            time_seconds: now.duration_since(self.start).as_secs_f64(),
            delta_seconds: now.duration_since(self.last_frame).as_secs_f32(),
            windows: &windows,
        };
        let views = (self.scene)(&frame);
        let input = FrameInput {
            frame_index: frame.frame_index,
            time_seconds: frame.time_seconds,
            delta_seconds: frame.delta_seconds,
            views,
        };
        self.last_frame = now;
        self.frame_index += 1;
        if running.sender.send(Message::Frame(input)).is_err() {
            self.stop(ctx);
        }
    }
}

// Opens `windows`, builds the renderer on the main thread (surfaces need it on macOS) and renders on a render thread.
// `scene` runs on the main thread every frame and returns the views to draw.
pub fn run(
    platform: impl Platform,
    builder: RendererBuilder,
    windows: Vec<WindowDesc>,
    scene: impl FnMut(&FrameContext<'_>) -> Vec<ViewInput> + 'static,
) -> Result<(), RunError> {
    run_with_hook(platform, builder, windows, scene, |_, _| {})
}

// Like `run`, with `hook` called on the render thread after every rendered frame.
pub fn run_with_hook(
    platform: impl Platform,
    builder: RendererBuilder,
    windows: Vec<WindowDesc>,
    scene: impl FnMut(&FrameContext<'_>) -> Vec<ViewInput> + 'static,
    hook: impl FnMut(&mut Renderer, &FrameReport) + Send + 'static,
) -> Result<(), RunError> {
    let error = Arc::new(Mutex::new(None));
    let now = Instant::now();
    let app = RunnerApp {
        state: State::Waiting(Some(builder)),
        hook: Some(Box::new(hook)),
        windows,
        scene,
        error: Arc::clone(&error),
        start: now,
        last_frame: now,
        frame_index: 0,
    };
    platform.run(Box::new(app)).map_err(RunError::Platform)?;
    match lock_error(&error).take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn std_pool_covers_every_index_exactly_once() {
        let pool = StdTaskPool::new();
        let visits: Vec<AtomicUsize> = (0..1000).map(|_| AtomicUsize::new(0)).collect();
        pool.parallel_for(visits.len(), &|range| {
            for index in range {
                visits[index].fetch_add(1, Ordering::Relaxed);
            }
        });
        assert!(visits.iter().all(|visit| visit.load(Ordering::Relaxed) == 1));
    }
}
