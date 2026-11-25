#![doc = include_str!("../../../docs/crates/pathon-bindless.md")]

mod binder;
mod error;
mod path;
mod sampler;
mod table;
mod wgsl;

pub use binder::{BinderDesc, TextureBinder};
pub use error::{BindlessError, BindlessShortfall};
pub use path::{
    BINDLESS_FEATURES, DEFAULT_CAPACITY, MIN_BINDLESS_CAPACITY, PathChoice, PathPreference, TexturePath, choose_path,
};
pub use sampler::SamplerPreset;
pub use table::{MAX_CAPACITY, TextureHandle, TextureTable, decode_shader_value};
pub use wgsl::MAX_MATERIAL_TEXTURES;
