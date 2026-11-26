#![doc = include_str!("../../../docs/crates/pathon-pbr.md")]

pub mod area;
mod arena;
mod cluster;
mod context;
mod depth_pyramid;
mod draw;
mod frame;
mod light;
mod lighting;
pub mod ltc;
mod ltc_table;
mod pack;
mod prepass;
pub mod reference;
mod settings;
mod shaders;

pub use arena::{ArenaSlice, BufferArena};
pub use cluster::{
    COUNTERS_SIZE, ClusterBuffers, ClusterError, ClusterGrid, ClusterParams, ClusterPipeline, LAST_SLICE_END,
    RANGE_SIZE,
};
pub use context::PbrContext;
pub use depth_pyramid::{DepthPyramidFeature, pyramid_levels};
pub use draw::{GpuInstance, SceneDraws};
pub use frame::{FrameUniforms, ViewMatrices};
pub use light::{GpuLight, PackedLights};
pub use lighting::LightingFeature;
pub use pack::PbrPack;
pub use prepass::PrepassFeature;
pub use settings::{ClusterSettings, LightAssignment, PbrSettings};
pub use shaders::{
    AREA_LIGHTS_FLAG, ForwardKey, HDR_FORMAT, IBL_FIRST_BINDING, LTC_BINDING, PbrShaders, PrepassKey, RenderPipeline,
    SHADOW_FIRST_BINDING, SHADOWS_FLAG, TEXTURE_GROUP,
};
