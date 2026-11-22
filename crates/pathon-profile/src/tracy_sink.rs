use std::cell::RefCell;
use std::sync::{Mutex, PoisonError};

use tracy_client::{Client, GpuContext, GpuContextType, Span};

use crate::sink::{GpuPassTiming, ProfileSink};

thread_local! {
    // Tracy zones are not `Send`; dropping one ends it.
    static ZONES: RefCell<Vec<Span>> = const { RefCell::new(Vec::new()) };
}

// CPU zones named after the graph passes, a frame mark per frame and a GPU context whose zones carry the timestamps the
// GPU wrote. Starts the Tracy client (once per process).
//
// The GPU clock is calibrated with the first timestamp that arrives, a few frames after it was written, so GPU zones
// sit a constant offset from the CPU zones; their lengths and order are exact.
pub struct TracySink {
    client: Client,
    gpu: Mutex<Option<GpuContext>>,
}

impl std::fmt::Debug for TracySink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracySink").finish_non_exhaustive()
    }
}

impl Default for TracySink {
    fn default() -> Self {
        Self::new()
    }
}

impl TracySink {
    pub fn new() -> Self {
        Self {
            client: Client::start(),
            gpu: Mutex::new(None),
        }
    }

    fn gpu_context(&self, first_timestamp: u64, period_ns: f32) -> Option<GpuContext> {
        let mut context = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        if context.is_none() {
            let calibration = i64::try_from(first_timestamp).unwrap_or(i64::MAX);
            *context = self
                .client
                .clone()
                .new_gpu_context(Some("pathon GPU"), GpuContextType::Invalid, calibration, period_ns)
                .ok();
        }
        context.clone()
    }
}

impl ProfileSink for TracySink {
    fn pass_begin(&self, pass: &'static str) {
        let zone = self
            .client
            .clone()
            .span_alloc(Some(pass), "pathon::pass", file!(), line!(), 0);
        ZONES.with_borrow_mut(|zones| zones.push(zone));
    }

    fn pass_end(&self, _pass: &'static str) {
        ZONES.with_borrow_mut(Vec::pop);
    }

    fn gpu_timings(&self, _frame: u64, period_ns: f32, passes: &[GpuPassTiming]) {
        let Some(first) = passes.first() else {
            return;
        };
        let Some(context) = self.gpu_context(first.begin_ticks, period_ns) else {
            return;
        };
        for timing in passes {
            let Ok(mut span) = context.span_alloc(timing.pass, timing.label, file!(), line!()) else {
                return;
            };
            span.end_zone();
            span.upload_timestamp_start(i64::try_from(timing.begin_ticks).unwrap_or(i64::MAX));
            span.upload_timestamp_end(i64::try_from(timing.end_ticks).unwrap_or(i64::MAX));
        }
    }

    fn frame_end(&self, _frame: u64) {
        self.client.frame_mark();
    }
}
