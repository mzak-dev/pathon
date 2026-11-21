#![doc = include_str!("../../../docs/crates/pathon.md")]

pub use pathon_api as api;
pub use pathon_core::{
    FeatureStatus, FrameInput, FrameReport, RenderError, Renderer, RendererBuilder, TargetId, ViewInput,
};
pub use pathon_diag as diag;
#[cfg(feature = "runtime")]
pub use pathon_platform as platform;
#[cfg(feature = "winit")]
pub use pathon_platform_winit::WinitPlatform;
pub use pathon_rhi as rhi;

#[cfg(feature = "runtime")]
pub mod runtime;
