#![doc = include_str!("../../../docs/crates/pathon-profile.md")]

mod gpu;
mod profiler;
mod sink;

#[cfg(feature = "puffin")]
#[cfg(not(target_arch = "wasm32"))]
mod puffin_sink;
#[cfg(feature = "tracing")]
mod tracing_sink;
#[cfg(feature = "tracy")]
#[cfg(not(target_arch = "wasm32"))]
mod tracy_sink;

pub use profiler::{Profiler, ProfilerBuilder};
#[cfg(feature = "puffin")]
#[cfg(not(target_arch = "wasm32"))]
pub use puffin_sink::PuffinSink;
pub use sink::{Collected, CollectingSink, GpuPassTiming, ProfileSink, SpanEvent};
#[cfg(feature = "tracing")]
pub use tracing_sink::{GPU_TARGET, PASS_TARGET, TracingSink};
#[cfg(feature = "tracy")]
#[cfg(not(target_arch = "wasm32"))]
pub use tracy_sink::TracySink;
