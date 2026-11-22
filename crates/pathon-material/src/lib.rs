#![doc = include_str!("../../../docs/crates/pathon-material.md")]

mod data;
mod error;
mod shaders;
mod standard;
mod table;
mod variant;

pub use data::{MATERIAL_DATA_FIELDS, MaterialData, NO_TEXTURE, flag_bits};
pub use error::MaterialError;
pub use shaders::{HOOKS_MODULE, MaterialHooks, STANDARD_MODULE, SURFACE_MODULE, TEXTURE_SOURCE_MODULE};
pub use standard::{AlphaMode, DiffuseModel, MaterialTextures, StandardMaterial, TextureSlot};
pub use table::{MaterialId, MaterialTable};
pub use variant::{AlphaKind, MaterialVariant, flags};
