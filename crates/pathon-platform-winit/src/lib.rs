#![doc = include_str!("../../../docs/crates/pathon-platform-winit.md")]

use std::collections::HashMap;
use std::sync::Arc;

use pathon_platform::{
    App, ElementState, InputEvent, MouseButton, PhysicalPosition, PhysicalSize, Platform, PlatformCtx, PlatformError,
    PlatformEvent, PlatformWindow, WindowDesc, WindowId,
};
use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::Window;

#[cfg(target_arch = "wasm32")]
mod web;

#[derive(Debug)]
struct WinitWindow(Arc<Window>);

impl HasWindowHandle for WinitWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for WinitWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

impl PlatformWindow for WinitWindow {
    fn id(&self) -> WindowId {
        WindowId(u64::from(self.0.id()))
    }

    fn inner_size(&self) -> PhysicalSize {
        let size = self.0.inner_size();
        PhysicalSize {
            width: size.width,
            height: size.height,
        }
    }

    fn scale_factor(&self) -> f64 {
        self.0.scale_factor()
    }

    fn request_redraw(&self) {
        self.0.request_redraw();
    }
}

#[derive(Debug, Default)]
pub struct WinitPlatform;

impl WinitPlatform {
    pub fn new() -> Self {
        Self
    }
}

impl Platform for WinitPlatform {
    fn run(self, app: Box<dyn App>) -> Result<(), PlatformError> {
        let event_loop = EventLoop::new().map_err(|error| PlatformError::EventLoop(error.to_string()))?;
        let handler = Handler {
            app,
            windows: HashMap::new(),
        };
        run_loop(event_loop, handler)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_loop(event_loop: EventLoop<()>, mut handler: Handler) -> Result<(), PlatformError> {
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop
        .run_app(&mut handler)
        .map_err(|error| PlatformError::EventLoop(error.to_string()))
}

// The browser owns the loop: `spawn_app` returns at once and frames follow `requestAnimationFrame` (ADR-0014).
#[expect(clippy::unnecessary_wraps, reason = "same signature as the native loop")]
#[cfg(target_arch = "wasm32")]
fn run_loop(event_loop: EventLoop<()>, handler: Handler) -> Result<(), PlatformError> {
    use winit::platform::web::EventLoopExtWebSys;

    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.spawn_app(handler);
    Ok(())
}

struct Handler {
    app: Box<dyn App>,
    windows: HashMap<WindowId, Arc<dyn PlatformWindow>>,
}

struct Ctx<'loop_> {
    event_loop: &'loop_ ActiveEventLoop,
    windows: &'loop_ mut HashMap<WindowId, Arc<dyn PlatformWindow>>,
}

impl PlatformCtx for Ctx<'_> {
    fn create_window(&mut self, desc: &WindowDesc) -> Result<Arc<dyn PlatformWindow>, PlatformError> {
        #[cfg(not(target_arch = "wasm32"))]
        let attributes = native_attributes(desc);
        #[cfg(target_arch = "wasm32")]
        let attributes = web_attributes(desc)?;
        let window = self
            .event_loop
            .create_window(attributes)
            .map_err(|error| PlatformError::WindowCreation(error.to_string()))?;
        #[cfg(target_arch = "wasm32")]
        window.request_redraw();
        let window: Arc<dyn PlatformWindow> = Arc::new(WinitWindow(Arc::new(window)));
        self.windows.insert(window.id(), Arc::clone(&window));
        Ok(window)
    }

    fn window(&self, id: WindowId) -> Option<Arc<dyn PlatformWindow>> {
        self.windows.get(&id).map(Arc::clone)
    }

    fn close_window(&mut self, id: WindowId) {
        self.windows.remove(&id);
    }

    fn exit(&mut self) {
        self.event_loop.exit();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn native_attributes(desc: &WindowDesc) -> winit::window::WindowAttributes {
    Window::default_attributes()
        .with_title(desc.title.clone())
        .with_inner_size(winit::dpi::LogicalSize::new(desc.size.width, desc.size.height))
        .with_resizable(desc.resizable == pathon_platform::Resizable::Yes)
}

#[cfg(target_arch = "wasm32")]
fn web_attributes(desc: &WindowDesc) -> Result<winit::window::WindowAttributes, PlatformError> {
    use winit::platform::web::WindowAttributesExtWebSys;

    let canvas = web::create_canvas(desc)?;
    Ok(Window::default_attributes()
        .with_title(desc.title.clone())
        .with_canvas(Some(canvas))
        .with_append(false))
}

#[cfg(target_arch = "wasm32")]
impl Handler {
    // A redraw request is the only thing winit ties to `requestAnimationFrame`, so it drives the frame and re-arms.
    fn run_web_frame(&mut self, event_loop: &ActiveEventLoop) {
        let mut ctx = Ctx {
            event_loop,
            windows: &mut self.windows,
        };
        self.app.on_frame(&mut ctx);
        for window in self.windows.values() {
            window.request_redraw();
        }
    }
}

impl Handler {
    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: PlatformEvent) {
        let mut ctx = Ctx {
            event_loop,
            windows: &mut self.windows,
        };
        self.app.on_event(&mut ctx, event);
    }
}

fn element_state(state: winit::event::ElementState) -> ElementState {
    match state {
        winit::event::ElementState::Pressed => ElementState::Pressed,
        winit::event::ElementState::Released => ElementState::Released,
    }
}

fn translate(window: WindowId, event: WindowEvent) -> Option<PlatformEvent> {
    let input = |event| Some(PlatformEvent::Input { window, event });
    match event {
        WindowEvent::Resized(size) => Some(PlatformEvent::Resized {
            window,
            size: PhysicalSize {
                width: size.width,
                height: size.height,
            },
        }),
        WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            Some(PlatformEvent::ScaleChanged { window, scale_factor })
        }
        WindowEvent::RedrawRequested => Some(PlatformEvent::RedrawRequested(window)),
        WindowEvent::CloseRequested => Some(PlatformEvent::CloseRequested(window)),
        WindowEvent::Focused(focused) => input(InputEvent::Focused(focused)),
        WindowEvent::CursorMoved { position, .. } => input(InputEvent::CursorMoved {
            position: PhysicalPosition {
                x: position.x,
                y: position.y,
            },
        }),
        WindowEvent::MouseInput { state, button, .. } => {
            let button = match button {
                winit::event::MouseButton::Left => MouseButton::Left,
                winit::event::MouseButton::Right => MouseButton::Right,
                winit::event::MouseButton::Middle => MouseButton::Middle,
                winit::event::MouseButton::Back => MouseButton::Other(3),
                winit::event::MouseButton::Forward => MouseButton::Other(4),
                winit::event::MouseButton::Other(other) => MouseButton::Other(other),
            };
            input(InputEvent::MouseButton {
                button,
                state: element_state(state),
            })
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let (delta_x, delta_y) = match delta {
                MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
                MouseScrollDelta::PixelDelta(position) => (position.x, position.y),
            };
            input(InputEvent::MouseWheel { delta_x, delta_y })
        }
        WindowEvent::KeyboardInput { event, .. } => {
            let name = match &event.logical_key {
                winit::keyboard::Key::Character(text) => text.to_string(),
                other => format!("{other:?}"),
            };
            input(InputEvent::Key {
                name,
                state: element_state(event.state),
            })
        }
        _ => None,
    }
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.dispatch(event_loop, PlatformEvent::SurfacesAvailable);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.dispatch(event_loop, PlatformEvent::SurfacesLost);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: winit::window::WindowId, event: WindowEvent) {
        #[cfg(target_arch = "wasm32")]
        if matches!(event, WindowEvent::RedrawRequested) {
            self.run_web_frame(event_loop);
            return;
        }
        if let Some(event) = translate(WindowId(u64::from(window_id)), event) {
            self.dispatch(event_loop, event);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut ctx = Ctx {
            event_loop,
            windows: &mut self.windows,
        };
        self.app.on_frame(&mut ctx);
    }
}
