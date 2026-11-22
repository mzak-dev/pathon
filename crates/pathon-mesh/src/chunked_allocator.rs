use std::ops::Range;

use pathon_api::resources::RangeAllocator;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ChunkPolicy {
    pub(crate) chunk_capacity: u64,
    pub(crate) max_chunk_capacity: u64,
    pub(crate) max_total: u64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Allocation {
    pub(crate) chunk: u32,
    pub(crate) range: Range<u64>,
    // Set when this allocation had to open a chunk: the caller creates its GPU buffers.
    pub(crate) opened_capacity: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AllocationError {
    Empty,
    TooLarge { max: u64 },
    Exhausted { free: u64 },
    Fragmented { free: u64, largest_free: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct AllocatorStats {
    pub(crate) chunks: usize,
    pub(crate) capacity: u64,
    pub(crate) allocated: u64,
    pub(crate) free: u64,
    pub(crate) largest_free: u64,
}

#[derive(Debug)]
struct Chunk {
    ranges: RangeAllocator,
    allocated: u64,
}

// Sub-allocation across fixed-size chunks; the pool owns one GPU buffer set per chunk. Everything is counted in
// units (vertices or indices), so a unit range means the same thing in every stream of a chunk.
#[derive(Debug)]
pub(crate) struct ChunkedAllocator {
    policy: ChunkPolicy,
    chunks: Vec<Option<Chunk>>,
}

impl ChunkedAllocator {
    pub(crate) fn new(policy: ChunkPolicy) -> Self {
        Self {
            policy,
            chunks: Vec::new(),
        }
    }

    pub(crate) fn policy(&self) -> ChunkPolicy {
        self.policy
    }

    pub(crate) fn allocate(&mut self, count: u64) -> Result<Allocation, AllocationError> {
        if count == 0 {
            return Err(AllocationError::Empty);
        }
        if count > self.policy.max_chunk_capacity {
            return Err(AllocationError::TooLarge {
                max: self.policy.max_chunk_capacity,
            });
        }
        for (index, chunk) in self.chunks.iter_mut().enumerate() {
            let Some(chunk) = chunk else { continue };
            if let Some(range) = chunk.ranges.allocate(count, 1) {
                chunk.allocated += count;
                return Ok(Allocation {
                    chunk: chunk_number(index),
                    range,
                    opened_capacity: None,
                });
            }
        }
        self.allocate_in_new_chunk(count)
    }

    fn allocate_in_new_chunk(&mut self, count: u64) -> Result<Allocation, AllocationError> {
        let stats = self.stats();
        let budget_left = self.policy.max_total.saturating_sub(stats.capacity);
        let capacity = count.max(self.policy.chunk_capacity.min(budget_left));
        if budget_left < count || capacity > self.policy.max_chunk_capacity {
            return Err(if stats.free >= count {
                AllocationError::Fragmented {
                    free: stats.free,
                    largest_free: stats.largest_free,
                }
            } else {
                AllocationError::Exhausted { free: stats.free }
            });
        }
        let mut ranges = RangeAllocator::new(capacity);
        let range = ranges
            .allocate(count, 1)
            .expect("an empty chunk of at least `count` units fits the request");
        let chunk = Chunk {
            ranges,
            allocated: count,
        };
        let index = if let Some(index) = self.chunks.iter().position(Option::is_none) {
            self.chunks[index] = Some(chunk);
            index
        } else {
            self.chunks.push(Some(chunk));
            self.chunks.len() - 1
        };
        Ok(Allocation {
            chunk: chunk_number(index),
            range,
            opened_capacity: Some(capacity),
        })
    }

    // For a chunk that was just opened but whose GPU buffers could not be created: nothing lives in it.
    pub(crate) fn abandon_opened(&mut self, chunk: u32) {
        if let Some(slot) = self.chunks.get_mut(chunk as usize) {
            *slot = None;
        }
    }

    // Returns the chunk that became empty and was released, so the pool can destroy its buffers.
    pub(crate) fn free(&mut self, chunk: u32, range: Range<u64>) -> Option<u32> {
        let live_chunks = self.chunks.iter().flatten().count();
        let slot = self.chunks.get_mut(chunk as usize)?;
        let entry = slot.as_mut()?;
        entry.allocated -= range.end - range.start;
        entry.ranges.free(range);
        let is_standard = entry.ranges.size() <= self.policy.chunk_capacity;
        if entry.allocated == 0 && (live_chunks > 1 || !is_standard) {
            *slot = None;
            return Some(chunk);
        }
        None
    }

    pub(crate) fn stats(&self) -> AllocatorStats {
        self.chunks
            .iter()
            .flatten()
            .fold(AllocatorStats::default(), |stats, chunk| AllocatorStats {
                chunks: stats.chunks + 1,
                capacity: stats.capacity + chunk.ranges.size(),
                allocated: stats.allocated + chunk.allocated,
                free: stats.free + chunk.ranges.available(),
                largest_free: stats.largest_free.max(chunk.ranges.largest_free()),
            })
    }
}

fn chunk_number(index: usize) -> u32 {
    u32::try_from(index).expect("a pool never holds u32::MAX chunks")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allocator(chunk: u64, max_chunk: u64, total: u64) -> ChunkedAllocator {
        ChunkedAllocator::new(ChunkPolicy {
            chunk_capacity: chunk,
            max_chunk_capacity: max_chunk,
            max_total: total,
        })
    }

    #[test]
    fn first_allocation_opens_a_chunk_and_later_ones_share_it() {
        let mut allocator = allocator(100, 100, 300);
        let a = allocator.allocate(30).expect("a");
        let b = allocator.allocate(30).expect("b");

        assert_eq!(a.opened_capacity, Some(100));
        assert_eq!(b.opened_capacity, None);
        assert_eq!((a.chunk, b.chunk), (0, 0));
        assert_eq!((a.range, b.range), (0..30, 30..60));
    }

    #[test]
    fn a_request_that_does_not_fit_opens_the_next_chunk() {
        let mut allocator = allocator(100, 100, 300);
        allocator.allocate(70).expect("first");
        let second = allocator.allocate(50).expect("second");

        assert_eq!((second.chunk, second.opened_capacity), (1, Some(100)));
    }

    #[test]
    fn exactly_one_chunk_fits_and_one_more_is_dedicated() {
        let mut allocator = allocator(100, 250, 1000);
        let exact = allocator.allocate(100).expect("exact");
        let oversize = allocator.allocate(101).expect("oversize");
        let largest = allocator.allocate(250).expect("largest");

        assert_eq!(exact.opened_capacity, Some(100));
        assert_eq!(oversize.opened_capacity, Some(101));
        assert_eq!(largest.opened_capacity, Some(250));
        assert_eq!(allocator.allocate(251), Err(AllocationError::TooLarge { max: 250 }));
    }

    #[test]
    fn zero_is_rejected() {
        assert_eq!(allocator(10, 10, 10).allocate(0), Err(AllocationError::Empty));
    }

    #[test]
    fn budget_exhaustion_and_fragmentation_are_told_apart() {
        let mut allocator = allocator(100, 100, 100);
        let a = allocator.allocate(40).expect("a");
        let b = allocator.allocate(20).expect("b");
        let c = allocator.allocate(40).expect("c");
        assert_eq!(allocator.allocate(1), Err(AllocationError::Exhausted { free: 0 }));

        allocator.free(a.chunk, a.range);
        allocator.free(c.chunk, c.range);
        assert_eq!(
            allocator.allocate(50),
            Err(AllocationError::Fragmented {
                free: 80,
                largest_free: 40
            })
        );
        assert_eq!(allocator.allocate(81), Err(AllocationError::Exhausted { free: 80 }));
        allocator.free(b.chunk, b.range);
        assert!(allocator.allocate(100).is_ok());
    }

    #[test]
    fn the_last_chunk_may_shrink_to_the_remaining_budget() {
        let mut allocator = allocator(100, 100, 130);
        allocator.allocate(100).expect("first");
        let tail = allocator.allocate(30).expect("tail");

        assert_eq!(tail.opened_capacity, Some(30));
        assert_eq!(allocator.allocate(1), Err(AllocationError::Exhausted { free: 0 }));
    }

    #[test]
    fn an_empty_extra_chunk_is_released_but_the_last_standard_one_stays() {
        let mut allocator = allocator(100, 100, 300);
        let a = allocator.allocate(100).expect("a");
        let b = allocator.allocate(100).expect("b");

        assert_eq!(allocator.free(b.chunk, b.range), Some(1));
        assert_eq!(allocator.free(a.chunk, a.range), None);
        assert_eq!(allocator.stats().chunks, 1);
    }

    #[test]
    fn a_dedicated_chunk_is_always_released_and_its_slot_reused() {
        let mut allocator = allocator(100, 500, 1000);
        let big = allocator.allocate(300).expect("big");
        assert_eq!(allocator.free(big.chunk, big.range), Some(0));
        assert_eq!(allocator.stats().chunks, 0);

        let next = allocator.allocate(10).expect("next");
        assert_eq!((next.chunk, next.opened_capacity), (0, Some(100)));
    }

    #[test]
    fn an_abandoned_chunk_disappears_even_when_it_is_the_only_one() {
        let mut allocator = allocator(100, 100, 100);
        let opened = allocator.allocate(10).expect("opened");

        allocator.abandon_opened(opened.chunk);

        assert_eq!(allocator.stats(), AllocatorStats::default());
        assert_eq!(allocator.allocate(100).expect("budget is free again").range, 0..100);
    }

    #[test]
    fn ranges_never_overlap_under_churn() {
        let mut allocator = allocator(64, 64, 256);
        let mut live: Vec<Allocation> = Vec::new();
        let mut seed = 7u64;
        for _ in 0..500 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let size = (seed >> 58) + 1;
            if seed & (1 << 20) != 0 && !live.is_empty() {
                let victim = live.swap_remove(usize::try_from(seed >> 40).expect("fits") % live.len());
                allocator.free(victim.chunk, victim.range);
            } else if let Ok(allocation) = allocator.allocate(size) {
                live.push(allocation);
            }
            for (index, a) in live.iter().enumerate() {
                for b in &live[index + 1..] {
                    let disjoint = a.chunk != b.chunk || a.range.end <= b.range.start || b.range.end <= a.range.start;
                    assert!(disjoint, "{a:?} overlaps {b:?}");
                }
            }
        }
    }
}
