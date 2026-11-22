use std::sync::{Mutex, PoisonError};

use pathon_api::rhi::MaybeSendSync;

// What the GPU measured for one GPU pass of the frame. `pass` is the name of the graph pass that recorded it, `label`
// the label of the GPU pass itself (a graph pass may record several).
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct GpuPassTiming {
    pub pass: &'static str,
    pub label: &'static str,
    pub begin_ticks: u64,
    pub end_ticks: u64,
}

impl GpuPassTiming {
    pub const fn new(pass: &'static str, label: &'static str, begin_ticks: u64, end_ticks: u64) -> Self {
        Self {
            pass,
            label,
            begin_ticks,
            end_ticks,
        }
    }

    pub fn duration_ticks(&self) -> u64 {
        self.end_ticks.saturating_sub(self.begin_ticks)
    }

    pub fn duration_ns(&self, period_ns: f32) -> f64 {
        self.duration_ticks() as f64 * f64::from(period_ns)
    }
}

// Where spans and timings go: `tracing`, Tracy, puffin or a test. Every method may run on any thread that records.
pub trait ProfileSink: MaybeSendSync {
    // CPU: the graph pass starts and ends recording its commands. Calls are nested properly per thread.
    fn pass_begin(&self, _pass: &'static str) {}

    fn pass_end(&self, _pass: &'static str) {}

    // GPU: results of `frame`, read back a few frames after it was recorded. `period_ns` converts ticks to nanoseconds.
    fn gpu_timings(&self, _frame: u64, _period_ns: f32, _passes: &[GpuPassTiming]) {}

    // The renderer committed `frame` (its commands reached the GPU, or the host).
    fn frame_end(&self, _frame: u64) {}
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpanEvent {
    Begin(&'static str),
    End(&'static str),
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Collected {
    pub spans: Vec<SpanEvent>,
    pub gpu: Vec<(u64, GpuPassTiming)>,
    pub ended_frames: Vec<u64>,
}

// Keeps everything it is told; for tests and tools that want the numbers instead of a profiler UI.
#[derive(Debug, Default)]
pub struct CollectingSink {
    collected: Mutex<Collected>,
}

impl CollectingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take(&self) -> Collected {
        std::mem::take(&mut *self.collected.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn with(&self, update: impl FnOnce(&mut Collected)) {
        update(&mut self.collected.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

impl ProfileSink for CollectingSink {
    fn pass_begin(&self, pass: &'static str) {
        self.with(|collected| collected.spans.push(SpanEvent::Begin(pass)));
    }

    fn pass_end(&self, pass: &'static str) {
        self.with(|collected| collected.spans.push(SpanEvent::End(pass)));
    }

    fn gpu_timings(&self, frame: u64, _period_ns: f32, passes: &[GpuPassTiming]) {
        self.with(|collected| collected.gpu.extend(passes.iter().map(|timing| (frame, *timing))));
    }

    fn frame_end(&self, frame: u64) {
        self.with(|collected| collected.ended_frames.push(frame));
    }
}
