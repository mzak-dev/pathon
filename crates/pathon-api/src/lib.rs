#![doc = include_str!("../../../docs/crates/pathon-api.md")]

mod context;
mod feature;
mod pack;
mod shared;
mod slot;
mod task_pool;
mod view;

pub use context::{PrepareCtx, RecordCtx, SetupCtx, TemporalSettings};
pub use feature::{Negotiation, RenderFeature, Requirements, Role};
pub use glam;
pub use pack::{Pack, PackRegistrar};
pub use pathon_diag::{Diagnostic, Severity};
pub use shared::Shared;
pub use slot::{Slot, Slots};
pub use task_pool::{SerialTaskPool, TaskPool};
pub use view::{Camera, ViewCtx, ViewId};

// Everything a regular pack needs, so packs depend on `pathon-api` alone.
pub mod rhi {
    pub use pathon_rhi_types::*;
}

pub mod graph {
    pub use pathon_graph::*;
}

pub mod resources {
    pub use pathon_resources::*;
}
