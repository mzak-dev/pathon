#![doc = include_str!("../../../docs/crates/pathon-golden.md")]

mod compare;
mod error;
mod gpu;
mod image;
mod png_io;
mod readback;
mod store;

pub use compare::{Comparison, PixelDifference, Tolerance, compare, diff_image};
pub use error::{Artifacts, GoldenError, Mismatch};
pub use gpu::{announce_adapter, gpu_or_skip};
pub use image::GoldenImage;
pub use readback::read_texture;
pub use store::{GoldenStore, Outcome, assert_golden};
