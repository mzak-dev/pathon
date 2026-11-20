use std::ops::Range;

// First-fit sub-allocation inside one large buffer. Ceiling: O(free ranges) per call; a size-bucketed allocator
// (e.g. offset-allocator) is the upgrade if mesh streaming makes this show up in profiles.
#[derive(Clone, Debug)]
pub struct RangeAllocator {
    size: u64,
    free: Vec<Range<u64>>,
}

impl RangeAllocator {
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "the free list starts as one range covering the buffer"
    )]
    pub fn new(size: u64) -> Self {
        Self {
            size,
            free: vec![0..size],
        }
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn available(&self) -> u64 {
        self.free.iter().map(|range| range.end - range.start).sum()
    }

    pub fn allocate(&mut self, size: u64, alignment: u64) -> Option<Range<u64>> {
        let alignment = alignment.max(1);
        for index in 0..self.free.len() {
            let candidate = self.free[index].clone();
            let start = candidate.start.next_multiple_of(alignment);
            let end = start.checked_add(size)?;
            if end > candidate.end {
                continue;
            }
            self.free.remove(index);
            let mut position = index;
            if candidate.start < start {
                self.free.insert(position, candidate.start..start);
                position += 1;
            }
            if end < candidate.end {
                self.free.insert(position, end..candidate.end);
            }
            return Some(start..end);
        }
        None
    }

    pub fn free(&mut self, range: Range<u64>) {
        let index = self.free.partition_point(|free| free.start < range.start);
        self.free.insert(index, range);
        self.merge_around(index);
    }

    fn merge_around(&mut self, index: usize) {
        if index + 1 < self.free.len() && self.free[index].end == self.free[index + 1].start {
            self.free[index].end = self.free[index + 1].end;
            self.free.remove(index + 1);
        }
        if index > 0 && self.free[index - 1].end == self.free[index].start {
            self.free[index - 1].end = self.free[index].end;
            self.free.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_respect_alignment_and_do_not_overlap() {
        let mut allocator = RangeAllocator::new(1024);
        let a = allocator.allocate(10, 1).expect("a");
        let b = allocator.allocate(16, 256).expect("b");

        assert_eq!(a, 0..10);
        assert_eq!(b, 256..272);
        assert_eq!(allocator.available(), 1024 - 26);
    }

    #[test]
    fn freed_neighbours_merge_back_into_one_range() {
        let mut allocator = RangeAllocator::new(300);
        let a = allocator.allocate(100, 1).expect("a");
        let b = allocator.allocate(100, 1).expect("b");
        let c = allocator.allocate(100, 1).expect("c");
        allocator.free(a);
        allocator.free(c);
        allocator.free(b);

        assert_eq!(allocator.allocate(300, 1), Some(0..300));
    }

    #[test]
    fn exhausted_allocator_returns_none() {
        let mut allocator = RangeAllocator::new(64);
        assert!(allocator.allocate(65, 1).is_none());
        assert!(allocator.allocate(64, 1).is_some());
        assert!(allocator.allocate(1, 1).is_none());
    }
}
