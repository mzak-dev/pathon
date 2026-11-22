use std::ops::Range;

use pathon_rhi_types::MaybeSendSync;

// ADR-0008: blocking parallel-for over index ranges, so tasks may borrow frame data. Maps onto Bevy's
// `ComputeTaskPool::scope` and Godot's `WorkerThreadPool` group tasks.
pub trait TaskPool: MaybeSendSync {
    fn parallel_for(&self, len: usize, task: &(dyn Fn(Range<usize>) + Sync));

    fn thread_count(&self) -> usize;

    // ADR-0024: starts `job` without waiting for it; a pool with no free thread hands it back, so the caller decides
    // when to run it (wasm and `SerialTaskPool` always do).
    fn try_spawn(&self, job: BackgroundJob) -> Result<(), BackgroundJob> {
        Err(job)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub type BackgroundJob = Box<dyn FnOnce() + Send + 'static>;

#[cfg(target_arch = "wasm32")]
pub type BackgroundJob = Box<dyn FnOnce() + 'static>;

const _: Option<&dyn TaskPool> = None;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct SerialTaskPool;

impl TaskPool for SerialTaskPool {
    fn parallel_for(&self, len: usize, task: &(dyn Fn(Range<usize>) + Sync)) {
        if len > 0 {
            task(0..len);
        }
    }

    fn thread_count(&self) -> usize {
        1
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn serial_pool_hands_a_background_job_back_unrun() {
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let job: BackgroundJob = Box::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });

        let Err(returned) = SerialTaskPool.try_spawn(job) else {
            panic!("a pool without threads must refuse background jobs");
        };

        assert_eq!(ran.load(Ordering::Relaxed), 0);
        returned();
        assert_eq!(ran.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn serial_pool_covers_the_whole_range_once() {
        let visited = AtomicUsize::new(0);
        SerialTaskPool.parallel_for(10, &|range| {
            visited.fetch_add(range.len(), Ordering::Relaxed);
        });
        assert_eq!(visited.load(Ordering::Relaxed), 10);
    }
}
