// The browser runner (ADR-0014): one thread, the renderer is built asynchronously and frames follow the platform's
// `requestAnimationFrame` callbacks. Frames are skipped until the renderer exists.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pathon_core::{FrameInput, FrameReport, RenderError, Renderer, RendererBuilder, TargetId, ViewInput};
use pathon_platform::{App, Platform, PlatformCtx, PlatformEvent, PlatformWindow, WindowDesc, WindowId};
use pathon_rhi::{MaybeSend, PhysicalSize, RhiError, SurfaceTarget};
use wasm_bindgen_futures::spawn_local;
use web_sys::Element;

use super::{FrameContext, RunError, rhi_size, surface_target};

const CONTAINER_ID: &str = "pathon";
const MESSAGE_ID: &str = "pathon-no-webgpu";
const NO_WEBGPU_MESSAGE: &str = "Ta strona wymaga przeglądarki z WebGPU (Chrome, Edge, Safari 26, Firefox).\nThis page needs a browser with WebGPU (Chrome, Edge, Safari 26, Firefox).";
const DEVICE_CREATION_MESSAGE: &str = "Nie udało się utworzyć urządzenia WebGPU (szczegóły w konsoli).\nCould not create a WebGPU device (see the console).";

type Slot<T> = Rc<RefCell<Option<T>>>;
type FrameHook = Box<dyn FnMut(&mut Renderer, &FrameReport)>;
type WindowEntry = (WindowId, TargetId, PhysicalSize);

enum State {
    Waiting(Option<RendererBuilder>),
    Starting {
        built: Slot<Result<Renderer, RenderError>>,
        surfaces: Vec<(Arc<dyn PlatformWindow>, SurfaceTarget)>,
    },
    Running(Box<Running>),
    Done,
}

struct Running {
    // `None` while the renderer is away in an asynchronous device recovery.
    renderer: Option<Renderer>,
    recovery: Option<Slot<(Renderer, Result<(), RenderError>)>>,
    windows: Vec<WindowEntry>,
}

struct RunnerApp<Scene> {
    state: State,
    hook: FrameHook,
    windows: Vec<WindowDesc>,
    scene: Scene,
    start_ms: f64,
    last_frame_ms: f64,
    frame_index: u64,
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

fn browser_has_webgpu() -> bool {
    web_sys::window()
        .and_then(|window| js_sys::Reflect::get(&window.navigator(), &"gpu".into()).ok())
        .is_some_and(|gpu| !gpu.is_undefined() && !gpu.is_null())
}

fn show_message(text: &str) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    if document.get_element_by_id(MESSAGE_ID).is_some() {
        return;
    }
    let Some(container) = document
        .get_element_by_id(CONTAINER_ID)
        .or_else(|| document.body().map(Element::from))
    else {
        return;
    };
    let canvases = container.get_elements_by_tag_name("canvas");
    while let Some(canvas) = canvases.item(0) {
        canvas.remove();
    }
    if let Ok(message) = document.create_element("p") {
        message.set_id(MESSAGE_ID);
        message.set_text_content(Some(text));
        let _ = message.set_attribute("style", "white-space: pre-line");
        let _ = container.append_child(&message);
    }
}

fn message_for(error: &RenderError) -> Option<&'static str> {
    match error {
        RenderError::Rhi(RhiError::NoAdapter(_)) => Some(NO_WEBGPU_MESSAGE),
        RenderError::Rhi(RhiError::DeviceCreation(_)) => Some(DEVICE_CREATION_MESSAGE),
        _ => None,
    }
}

impl<Scene: FnMut(&FrameContext<'_>) -> Vec<ViewInput>> RunnerApp<Scene> {
    fn begin_start(&mut self, ctx: &mut dyn PlatformCtx, builder: RendererBuilder) -> Result<State, RunError> {
        let mut surfaces = Vec::with_capacity(self.windows.len());
        for desc in &self.windows {
            let window = ctx.create_window(desc).map_err(RunError::Platform)?;
            let surface = surface_target(&window);
            surfaces.push((window, surface));
        }
        let builder = match surfaces.first() {
            Some((_, first)) => builder.with_compatible_surface(first.clone()),
            None => builder,
        };
        let built = Rc::new(RefCell::new(None));
        let result_slot = Rc::clone(&built);
        spawn_local(async move {
            *result_slot.borrow_mut() = Some(builder.build_async().await);
        });
        Ok(State::Starting { built, surfaces })
    }

    fn finish_start(
        mut renderer: Renderer,
        surfaces: Vec<(Arc<dyn PlatformWindow>, SurfaceTarget)>,
    ) -> Result<Running, RunError> {
        let mut windows = Vec::with_capacity(surfaces.len());
        for (window, mut surface) in surfaces {
            // The window may have been sized while the device was being requested.
            surface.size = rhi_size(window.inner_size());
            let size = surface.size;
            let target = renderer.add_window(surface).map_err(RunError::Render)?;
            windows.push((window.id(), target, size));
        }
        Ok(Running {
            renderer: Some(renderer),
            recovery: None,
            windows,
        })
    }

    fn stop(&mut self, ctx: &mut dyn PlatformCtx) {
        self.state = State::Done;
        ctx.exit();
    }

    fn fail(&mut self, ctx: &mut dyn PlatformCtx, error: &RunError) {
        log::error!("{error}");
        if let RunError::Render(render_error) = error
            && let Some(text) = message_for(render_error)
        {
            show_message(text);
        }
        self.stop(ctx);
    }

    fn poll_start(&mut self, ctx: &mut dyn PlatformCtx) {
        let State::Starting { built, surfaces } = &mut self.state else {
            return;
        };
        let Some(result) = built.borrow_mut().take() else {
            return;
        };
        let surfaces = std::mem::take(surfaces);
        let started = result
            .map_err(RunError::Render)
            .and_then(|renderer| Self::finish_start(renderer, surfaces));
        match started {
            Ok(running) => self.state = State::Running(Box::new(running)),
            Err(error) => self.fail(ctx, &error),
        }
    }

    fn poll_recovery(&mut self, ctx: &mut dyn PlatformCtx) -> bool {
        let State::Running(running) = &mut self.state else {
            return false;
        };
        let Some(recovery) = &running.recovery else {
            return running.renderer.is_some();
        };
        let Some((mut renderer, result)) = recovery.borrow_mut().take() else {
            return false;
        };
        running.recovery = None;
        match result {
            Ok(()) => {
                for (_, target, size) in &running.windows {
                    if let Err(error) = renderer.resize_target(*target, *size) {
                        log::error!("resize after recovery failed: {error}");
                    }
                }
                running.renderer = Some(renderer);
                true
            }
            Err(error) => {
                self.fail(ctx, &RunError::Render(error));
                false
            }
        }
    }

    fn begin_recovery(running: &mut Running) {
        let Some(mut renderer) = running.renderer.take() else {
            return;
        };
        log::warn!("device lost, rebuilding");
        let recovery = Rc::new(RefCell::new(None));
        let result_slot = Rc::clone(&recovery);
        running.recovery = Some(recovery);
        spawn_local(async move {
            let result = renderer.recover_async().await;
            *result_slot.borrow_mut() = Some((renderer, result));
        });
    }

    #[expect(clippy::cast_possible_truncation, reason = "a frame delta fits f32")]
    fn render(&mut self) {
        let State::Running(running) = &mut self.state else {
            return;
        };
        let Some(renderer) = running.renderer.as_mut() else {
            return;
        };
        let now = now_ms();
        let windows: Vec<(TargetId, PhysicalSize)> = running
            .windows
            .iter()
            .map(|(_, target, size)| (*target, *size))
            .collect();
        let frame = FrameContext {
            frame_index: self.frame_index,
            time_seconds: (now - self.start_ms) / 1000.0,
            delta_seconds: ((now - self.last_frame_ms) / 1000.0) as f32,
            windows: &windows,
        };
        let input = FrameInput {
            frame_index: frame.frame_index,
            time_seconds: frame.time_seconds,
            delta_seconds: frame.delta_seconds,
            views: (self.scene)(&frame),
        };
        self.last_frame_ms = now;
        self.frame_index += 1;
        match renderer.render_frame(&input) {
            Ok(report) => (self.hook)(renderer, &report),
            Err(RenderError::DeviceLost) => Self::begin_recovery(running),
            Err(error) => log::error!("frame failed: {error}"),
        }
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
                match self.begin_start(ctx, builder) {
                    Ok(state) => self.state = state,
                    Err(error) => self.fail(ctx, &error),
                }
            }
            PlatformEvent::Resized { window, size } => {
                if let State::Running(running) = &mut self.state
                    && let Some(entry) = running.windows.iter_mut().find(|(id, _, _)| *id == window)
                {
                    entry.2 = rhi_size(size);
                    if let Some(renderer) = running.renderer.as_mut()
                        && let Err(error) = renderer.resize_target(entry.1, entry.2)
                    {
                        log::error!("resize failed: {error}");
                    }
                }
            }
            PlatformEvent::CloseRequested(_) => self.stop(ctx),
            _ => {}
        }
    }

    fn on_frame(&mut self, ctx: &mut dyn PlatformCtx) {
        self.poll_start(ctx);
        if self.poll_recovery(ctx) {
            self.render();
        }
    }
}

// Opens `windows` on the page, builds the renderer asynchronously and renders on every animation frame; returns as soon
// as the browser owns the loop. A browser without WebGPU gets a message in the page instead (`Ok`, nothing panics).
pub fn run(
    platform: impl Platform,
    builder: RendererBuilder,
    windows: Vec<WindowDesc>,
    scene: impl FnMut(&FrameContext<'_>) -> Vec<ViewInput> + 'static,
) -> Result<(), RunError> {
    run_with_hook(platform, builder, windows, scene, |_, _| {})
}

// Like `run`, with `hook` called after every rendered frame.
pub fn run_with_hook(
    platform: impl Platform,
    builder: RendererBuilder,
    windows: Vec<WindowDesc>,
    scene: impl FnMut(&FrameContext<'_>) -> Vec<ViewInput> + 'static,
    hook: impl FnMut(&mut Renderer, &FrameReport) + MaybeSend + 'static,
) -> Result<(), RunError> {
    if !browser_has_webgpu() {
        log::error!("navigator.gpu is missing: this browser has no WebGPU");
        show_message(NO_WEBGPU_MESSAGE);
        return Ok(());
    }
    let now = now_ms();
    let app = RunnerApp {
        state: State::Waiting(Some(builder)),
        hook: Box::new(hook),
        windows,
        scene,
        start_ms: now,
        last_frame_ms: now,
        frame_index: 0,
    };
    platform.run(Box::new(app)).map_err(RunError::Platform)
}
