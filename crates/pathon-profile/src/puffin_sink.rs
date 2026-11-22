use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use puffin::{GlobalProfiler, ScopeDetails, ScopeId, StreamInfo, ThreadInfo, ThreadProfiler};

use crate::sink::{GpuPassTiming, ProfileSink};

thread_local! {
    static OPEN_SCOPES: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

// CPU scopes named after the graph passes, and a `pathon GPU` thread whose scopes carry the GPU durations. Turns puffin
// scopes on and ends puffin's frame when the renderer's frame is committed. The GPU scopes of a frame are laid out
// from the moment they are reported, so only their lengths and order are exact.
#[derive(Debug, Default)]
pub struct PuffinSink {
    scope_ids: Mutex<HashMap<String, ScopeId>>,
}

impl PuffinSink {
    pub fn new() -> Self {
        puffin::set_scopes_on(true);
        Self::default()
    }

    fn scope_id(&self, name: &str) -> ScopeId {
        let mut ids = self.scope_ids.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(id) = ids.get(name) {
            return *id;
        }
        let details = ScopeDetails::from_scope_name(Cow::Owned(name.to_owned()));
        let id = GlobalProfiler::lock().register_user_scopes(&[details])[0];
        ids.insert(name.to_owned(), id);
        id
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a frame's GPU time in nanoseconds fits an i64"
)]
fn offset_ns(ticks: u64, period_ns: f32) -> i64 {
    (ticks as f64 * f64::from(period_ns)) as i64
}

impl ProfileSink for PuffinSink {
    fn pass_begin(&self, pass: &'static str) {
        if !puffin::are_scopes_on() {
            return;
        }
        let id = self.scope_id(pass);
        let offset = ThreadProfiler::call(|profiler| profiler.begin_scope(id, ""));
        OPEN_SCOPES.with_borrow_mut(|open| open.push(offset));
    }

    fn pass_end(&self, _pass: &'static str) {
        if let Some(offset) = OPEN_SCOPES.with_borrow_mut(Vec::pop) {
            ThreadProfiler::call(|profiler| profiler.end_scope(offset));
        }
    }

    fn gpu_timings(&self, _frame: u64, period_ns: f32, passes: &[GpuPassTiming]) {
        let Some(first) = passes.first() else {
            return;
        };
        if !puffin::are_scopes_on() {
            return;
        }
        let origin = puffin::now_ns();
        let to_ns = |ticks: u64| origin + offset_ns(ticks.saturating_sub(first.begin_ticks), period_ns);
        let mut stream = puffin::Stream::default();
        for timing in passes {
            let id = self.scope_id(&format!("{} (GPU: {})", timing.pass, timing.label));
            let (offset, _) = stream.begin_scope(|| to_ns(timing.begin_ticks), id, "");
            stream.end_scope(offset, to_ns(timing.end_ticks));
        }
        if let Ok(info) = StreamInfo::parse(stream) {
            let thread = ThreadInfo {
                start_time_ns: Some(origin),
                name: "pathon GPU".to_owned(),
            };
            GlobalProfiler::lock().report_user_scopes(thread, &info.as_stream_into_ref());
        }
    }

    fn frame_end(&self, _frame: u64) {
        GlobalProfiler::lock().new_frame();
    }
}
