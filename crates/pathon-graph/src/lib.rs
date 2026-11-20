#![doc = include_str!("../../../docs/crates/pathon-graph.md")]

mod compile;
mod execute;
mod graph;
mod order;
mod pool;

pub use compile::{CompiledGraph, GraphStats};
pub use execute::{Execution, PassContext};
pub use graph::{
    FrameGraph, FrameSizes, GraphSize, GraphTextureDesc, PassBuilder, PassDesc, RecordFn, Stage, TextureHandle,
};
pub use order::{OrderNode, sort_nodes};
pub use pool::TexturePool;
