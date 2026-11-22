use std::cell::RefCell;

use tracing::span::EnteredSpan;

use crate::sink::{GpuPassTiming, ProfileSink};

pub const PASS_TARGET: &str = "pathon::pass";
pub const GPU_TARGET: &str = "pathon::gpu";

thread_local! {
    // The guards are not `Send`, and a pass begins and ends on the thread that records it.
    static ENTERED: RefCell<Vec<EnteredSpan>> = const { RefCell::new(Vec::new()) };
}

// A `pass` span with the graph pass in its `name` field, and one `gpu_pass` event per GPU pass with `name`, `label` and
// `duration_ns`. A subscriber decides where they go (`tracing-tracy`, `tracing-subscriber`, ...).
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingSink;

impl ProfileSink for TracingSink {
    fn pass_begin(&self, pass: &'static str) {
        let span = tracing::info_span!(target: PASS_TARGET, "pass", name = pass).entered();
        ENTERED.with_borrow_mut(|entered| entered.push(span));
    }

    fn pass_end(&self, _pass: &'static str) {
        ENTERED.with_borrow_mut(Vec::pop);
    }

    fn gpu_timings(&self, frame: u64, period_ns: f32, passes: &[GpuPassTiming]) {
        for timing in passes {
            tracing::info!(
                target: GPU_TARGET,
                frame,
                name = timing.pass,
                label = timing.label,
                duration_ns = timing.duration_ns(period_ns),
                "gpu_pass"
            );
        }
    }
}
