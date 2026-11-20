#![doc = include_str!("../../../docs/devtools.md")]

mod capture;
mod demo;
mod protocol;
mod server;
pub mod ui;

pub use capture::snapshot;
pub use demo::spawn_demo;
pub use protocol::{
    AccessSnapshot, BarrierSnapshot, DiagnosticSnapshot, FeatureSnapshot, PassSnapshot, ResourceSnapshot, Snapshot,
    StatsSnapshot, ViewSnapshot,
};
pub use server::{ClientEvent, DEFAULT_ADDRESS, DevtoolsServer, connect};
