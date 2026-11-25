#![doc = include_str!("../../../docs/crates/pathon-shadow.md")]

pub mod cascade;
mod feature;
mod frame;
mod input;
mod pack;
mod plan;
mod settings;
mod shaders;

pub use feature::ShadowFeature;
pub use frame::{SHADOW_MAP, SHADOW_MAP_ACCESS, ShadowBindings, ShadowFrame, ShadowUniforms};
pub use input::{DirectionalShadowLight, ShadowCaster, ShadowScene};
pub use pack::ShadowPack;
pub use settings::{CascadeFit, ShadowFilter, ShadowSettings, SplitScheme};
pub use shaders::{SAMPLE_MODULE, modules};
