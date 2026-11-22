#![doc = include_str!("../../../docs/crates/pathon-platform.md")]

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WindowId(pub u64);

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LogicalSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PhysicalPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Resizable {
    Yes,
    No,
}

#[derive(Clone, PartialEq, Debug)]
pub struct WindowDesc {
    pub title: String,
    pub size: LogicalSize,
    pub resizable: Resizable,
}

impl WindowDesc {
    pub fn new(title: impl Into<String>, width: f64, height: f64) -> Self {
        Self {
            title: title.into(),
            size: LogicalSize { width, height },
            resizable: Resizable::Yes,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ElementState {
    Pressed,
    Released,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

// Raw input only: mapping keys to actions is out of scope (plan, "Poza zakresem").
#[derive(Clone, PartialEq, Debug)]
pub enum InputEvent {
    Key { name: String, state: ElementState },
    CursorMoved { position: PhysicalPosition },
    MouseButton { button: MouseButton, state: ElementState },
    MouseWheel { delta_x: f64, delta_y: f64 },
    Focused(bool),
}

// Surfaces may only exist between `SurfacesAvailable` and `SurfacesLost` (mobile and web suspend them).
#[derive(Clone, PartialEq, Debug)]
pub enum PlatformEvent {
    SurfacesAvailable,
    SurfacesLost,
    Resized { window: WindowId, size: PhysicalSize },
    ScaleChanged { window: WindowId, scale_factor: f64 },
    RedrawRequested(WindowId),
    CloseRequested(WindowId),
    Input { window: WindowId, event: InputEvent },
}

#[cfg(not(target_arch = "wasm32"))]
pub trait PlatformWindow: HasWindowHandle + HasDisplayHandle + Send + Sync {
    fn id(&self) -> WindowId;
    fn inner_size(&self) -> PhysicalSize;
    fn scale_factor(&self) -> f64;
    fn request_redraw(&self);
    // A request: whatever the backend, the platform answers with `PlatformEvent::Resized` when the size changes.
    fn request_inner_size(&self, size: PhysicalSize);
}

#[cfg(target_arch = "wasm32")]
pub trait PlatformWindow: HasWindowHandle + HasDisplayHandle {
    fn id(&self) -> WindowId;
    fn inner_size(&self) -> PhysicalSize;
    fn scale_factor(&self) -> f64;
    fn request_redraw(&self);
    // A request: whatever the backend, the platform answers with `PlatformEvent::Resized` when the size changes.
    fn request_inner_size(&self, size: PhysicalSize);
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlatformError {
    #[error("window creation failed: {0}")]
    WindowCreation(String),
    #[error("event loop failed: {0}")]
    EventLoop(String),
    #[error("windows can only be created while surfaces are available")]
    SurfacesUnavailable,
}

// Valid only inside a callback; the platform owns the event loop.
pub trait PlatformCtx {
    fn create_window(&mut self, desc: &WindowDesc) -> Result<Arc<dyn PlatformWindow>, PlatformError>;
    fn window(&self, id: WindowId) -> Option<Arc<dyn PlatformWindow>>;
    fn close_window(&mut self, id: WindowId);
    fn exit(&mut self);
}

pub trait App {
    fn on_event(&mut self, ctx: &mut dyn PlatformCtx, event: PlatformEvent);
    fn on_frame(&mut self, ctx: &mut dyn PlatformCtx);
}

pub trait Platform {
    fn run(self, app: Box<dyn App>) -> Result<(), PlatformError>;
}

const _: Option<&dyn App> = None;
const _: Option<&dyn PlatformCtx> = None;
const _: Option<&dyn PlatformWindow> = None;
