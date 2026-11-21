#![doc = include_str!("../../../docs/crates/pathon-graph.md")]

mod cache;
mod capture;
mod compile;
mod dot;
mod execute;
mod graph;
mod history;
mod order;
mod pool;

pub use cache::GraphCache;
pub use capture::{
    AccessCapture, BarrierCapture, CapturedKind, CapturedTarget, GraphCapture, PassCapture, ResourceCapture,
};
pub use compile::{CompiledGraph, GraphStats};
pub use dot::{DotPass, render_dot, to_dot};
pub use execute::{Execution, PassContext};
pub use graph::{
    BufferHandle, FrameGraph, FrameSizes, GraphBufferDesc, GraphSize, GraphTextureDesc, PassBuilder, PassDesc,
    RecordFn, Stage, TextureHandle,
};
pub use history::{HistoryStore, HistoryTexture};
pub use order::{OrderNode, sort_nodes};
pub use pool::ResourcePool;
