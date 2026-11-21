#![doc = include_str!("../../../docs/crates/pathon-core.md")]

mod builder;
mod error;
mod frame;
mod registry;
mod renderer;
mod runner;
mod target;

pub use builder::RendererBuilder;
pub use error::RenderError;
pub use frame::{FrameInput, FrameReport, ViewInput};
pub use registry::{FeatureInfo, FeatureStatus};
pub use renderer::Renderer;
pub use target::TargetId;
