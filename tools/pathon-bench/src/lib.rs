#![doc = include_str!("../../../docs/crates/pathon-bench.md")]

mod alloc;
mod budgets;
mod metric;
mod quantity;
mod stats;

pub use alloc::{AllocationCount, CountingAllocator, count_allocations};

pub use budgets::{Budgets, Regression};
pub use metric::{LINE_PREFIX, Metric, emit};
pub use quantity::{Quantity, Unit, format_value};
pub use stats::{median, minimum};
