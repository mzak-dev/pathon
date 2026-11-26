#![doc = include_str!("../../../docs/crates/pathon-shadow.md")]

mod atlas;
pub mod cascade;
mod feature;
mod frame;
mod input;
mod local;
mod local_feature;
mod local_frame;
mod pack;
mod plan;
mod settings;
mod shaders;

pub use feature::ShadowFeature;
pub use frame::{SHADOW_MAP, SHADOW_MAP_ACCESS, ShadowBindings, ShadowFrame, ShadowUniforms};
pub use input::{DirectionalShadowLight, LocalLightFilter, ShadowCaster, ShadowScene};
pub use local::{Frustum, LocalShadowLight, LocalShadowShape, MAX_SPOT_HALF_ANGLE};
pub use local_frame::{
    LocalLightReport, LocalShadowBindings, LocalShadowFrame, LocalShadowReport, LocalShadowUniforms, SHADOW_LOCAL_MAP,
};
pub use pack::ShadowPack;
pub use settings::{CascadeFit, ShadowFilter, ShadowSettings, SplitScheme};
pub use shaders::{SAMPLE_MODULE, modules};
