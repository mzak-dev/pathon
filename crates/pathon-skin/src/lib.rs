#![doc = include_str!("../../../docs/crates/pathon-skin.md")]

mod deformed;
mod error;
mod frame;
mod gpu;
mod morph;
mod pack;
pub mod reference;
mod sample;

pub use deformed::{DEFORMED_SURFACE_STRIDE, DeformedSurface, SkinTargets, deformed_vertex_buffers};
pub use error::SkinError;
pub use frame::{BoneSlice, MorphBinding, MotionHistory, SkinFrame, SkinInstance, SkinOutput, SkinSource};
pub use morph::{MorphTarget, MorphTargets};
pub use pack::SkinPack;
pub use sample::SampleArm;
