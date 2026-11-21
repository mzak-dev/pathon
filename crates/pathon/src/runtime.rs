// The optional runner for integration mode C in an own engine (ADR-0004): natively the platform loop stays on the main
// thread and the renderer lives on a render thread, in the browser everything runs on the one thread (ADR-0014).

use pathon_core::{RenderError, TargetId};
use pathon_platform::PlatformError;
use pathon_rhi::PhysicalSize;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use native::{StdTaskPool, run, run_with_hook};
#[cfg(target_arch = "wasm32")]
pub use web::{run, run_with_hook};

#[derive(Debug)]
pub enum RunError {
    Platform(PlatformError),
    Render(RenderError),
    RenderThreadPanicked,
}

impl std::error::Error for RunError {}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Platform(error) => write!(f, "platform: {error}"),
            Self::Render(error) => write!(f, "renderer: {error}"),
            Self::RenderThreadPanicked => write!(f, "the render thread panicked"),
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

fn rhi_size(size: pathon_platform::PhysicalSize) -> PhysicalSize {
    PhysicalSize::new(size.width, size.height)
}

#[cfg_attr(
    target_arch = "wasm32",
    expect(clippy::arc_with_non_send_sync, reason = "one thread on the web (ADR-0004)")
)]
fn surface_target(window: &std::sync::Arc<dyn pathon_platform::PlatformWindow>) -> pathon_rhi::SurfaceTarget {
    let source: std::sync::Arc<dyn pathon_rhi::WindowSource> = std::sync::Arc::new(std::sync::Arc::clone(window));
    pathon_rhi::SurfaceTarget::new(source, rhi_size(window.inner_size()), window.scale_factor())
}
