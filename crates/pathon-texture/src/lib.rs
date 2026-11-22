#![doc = include_str!("../../../docs/crates/pathon-texture.md")]

mod choice;
mod error;
mod half;
#[cfg(feature = "ktx2")]
mod ktx2;
mod loader;
mod mipmap;
pub mod reference;
mod source;
#[cfg(test)]
mod test_caps;
mod texture;

pub use choice::FormatChoice;
pub use error::TextureError;
#[cfg(feature = "ktx2")]
pub use ktx2::Ktx2Image;
pub use loader::TextureLoader;
pub use mipmap::{MipMode, MipmapGenerator, can_generate_mips};
pub use source::{ColorSpace, TextureSource, Variant};
pub use texture::Texture;
