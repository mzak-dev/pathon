use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

/// Install with `#[global_allocator]` in a probe to count allocations. Counters are per thread, so threads of the
/// harness do not disturb a measurement: the number of allocations of a deterministic code path is deterministic.
#[derive(Debug)]
pub struct CountingAllocator;

fn note(bytes: usize) {
    ALLOCATIONS.with(|count| count.set(count.get() + 1));
    BYTES.with(|total| total.set(total.get() + bytes));
}

// SAFETY: every call is forwarded unchanged to the system allocator, which upholds the `GlobalAlloc` contract.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract for `alloc` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract for `alloc_zeroed` is passed through.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` and `layout` come from a matching `alloc` of this allocator, i.e. of `System`.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        // SAFETY: the caller's contract for `realloc` is passed through.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AllocationCount {
    pub allocations: usize,
    pub bytes: usize,
}

/// Allocations made on this thread while `action` runs (needs [`CountingAllocator`] as the global allocator).
pub fn count_allocations<R>(action: impl FnOnce() -> R) -> (R, AllocationCount) {
    let before = (ALLOCATIONS.with(Cell::get), BYTES.with(Cell::get));
    let result = action();
    let after = (ALLOCATIONS.with(Cell::get), BYTES.with(Cell::get));
    (
        result,
        AllocationCount {
            allocations: after.0 - before.0,
            bytes: after.1 - before.1,
        },
    )
}
