#![doc = include_str!("../../../docs/crates/pathon-hot.md")]
// Browsers have no files to watch; the crate is empty there (ADR-0040).
#![cfg(not(target_arch = "wasm32"))]

mod change;
mod config;
mod diagnostics;
mod error;
mod guard;
mod reloader;
mod scan;
mod worker;

pub use config::{DirRole, HotConfig, SourceLimits};
pub use error::HotError;
pub use reloader::{HotReloader, HotReport};
