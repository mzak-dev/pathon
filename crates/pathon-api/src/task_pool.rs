use std::ops::Range;

use pathon_rhi_types::MaybeSendSync;

// ADR-0008: blocking parallel-for over index ranges, so tasks may borrow frame data. Maps onto Bevy's
// `ComputeTaskPool::scope` and Godot's `WorkerThreadPool` group tasks.
pub trait TaskPool: MaybeSendSync {
    fn parallel_for(&self, len: usize, task: &(dyn Fn(Range<usize>) + Sync));

    fn thread_count(&self) -> usize;
}

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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn serial_pool_covers_the_whole_range_once() {
        let visited = AtomicUsize::new(0);
        SerialTaskPool.parallel_for(10, &|range| {
            visited.fetch_add(range.len(), Ordering::Relaxed);
        });
        assert_eq!(visited.load(Ordering::Relaxed), 10);
    }
}
