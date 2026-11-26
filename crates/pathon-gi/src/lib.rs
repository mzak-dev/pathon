#![doc = include_str!("../../../docs/crates/pathon-gi.md")]

mod feature;
mod frame;
mod gpu;
mod input;
mod lights;
mod pack;
pub mod reference;
mod settings;
mod shaders;
mod status;
mod volume;
mod voxel;

pub use feature::GiFeature;
pub use frame::{GI_TEXTURE_ACCESS, GiBindings, GiFrame};
pub use lights::{GiLight, pack_lights};
pub use pack::GiPack;
pub use settings::{GiSettings, MAX_RAYS_PER_FRAME, MAX_VOXELS};
pub use shaders::{COMMON_MODULE, SAMPLE_MODULE, modules};
pub use status::GiStatus;
pub use volume::{GiVolumeUniforms, IRRADIANCE_RES, IRRADIANCE_TILE, ProbeLayout, VISIBILITY_RES, VISIBILITY_TILE};
pub use voxel::{BYTES_PER_CELL, VoxelGridUniforms, VoxelLayout};
