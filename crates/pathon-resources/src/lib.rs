#![doc = include_str!("../../../docs/crates/pathon-resources.md")]

mod deferred;
mod gpu_array;
mod range_allocator;
mod uniform_ring;

pub use deferred::DeferredRelease;
pub use gpu_array::GpuArray;
pub use range_allocator::RangeAllocator;
pub use uniform_ring::{UniformRing, UniformSlice};
