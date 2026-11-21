#![doc = include_str!("../../../docs/crates/pathon-graph.md")]

mod cache;
mod capture;
mod compile;
mod execute;
mod graph;
mod order;
mod pool;

pub use cache::GraphCache;
pub use capture::{AccessCapture, BarrierCapture, GraphCapture, PassCapture, ResourceCapture};
pub use compile::{CompiledGraph, GraphStats};
pub use execute::{Execution, PassContext};
pub use graph::{
    BufferHandle, FrameGraph, FrameSizes, GraphBufferDesc, GraphSize, GraphTextureDesc, PassBuilder, PassDesc,
    RecordFn, Stage, TextureHandle,
};
pub use order::{OrderNode, sort_nodes};
pub use pool::ResourcePool;
