use std::collections::VecDeque;

use pathon_api::resources::RangeAllocator;
use pathon_api::rhi::{BufferDesc, BufferId, BufferUsage, DeviceApi, Resource};

use crate::error::{ArenaKind, MeshError};

// One `vec4<f32>` per vertex: xyz in object space, w = 1.
pub const DEFORMED_POSITION_STRIDE: u64 = 16;

// A skinned instance's slice of a bank; the same slice is valid in both banks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct DeformedRange {
    pub first_vertex: u32,
    pub vertex_count: u32,
}

#[derive(Debug)]
struct PendingFree {
    frame: u64,
    range: DeformedRange,
}

// Layout of skinned positions for motion vectors (ADR-0027, written by M4-12): a storage buffer with two banks of
// `bank_capacity` vertices. In frame `f` skinning writes bank `f & 1`; the other bank still holds last frame's
// positions. The buffer is also a vertex buffer, so a rasterization pass can bind a bank as the position stream.
#[derive(Debug)]
pub struct DeformedPositions {
    label: &'static str,
    buffer: BufferId,
    bank_capacity: u32,
    frames_in_flight: u64,
    allocator: RangeAllocator,
    pending: VecDeque<PendingFree>,
    pending_vertices: u64,
}

impl DeformedPositions {
    pub fn new(
        device: &dyn DeviceApi,
        label: &'static str,
        bank_capacity: u32,
        frames_in_flight: u64,
    ) -> Result<Self, MeshError> {
        let limits = &device.caps().limits;
        let largest_buffer = limits.max_buffer_size.min(limits.max_storage_buffer_binding_size);
        let max_bank = largest_buffer / (2 * DEFORMED_POSITION_STRIDE);
        if u64::from(bank_capacity) > max_bank {
            return Err(MeshError::TooLarge {
                label,
                kind: ArenaKind::DeformedVertices,
                requested: u64::from(bank_capacity),
                max: max_bank,
            });
        }
        let buffer = device.create_buffer(&BufferDesc {
            label,
            size: 2 * u64::from(bank_capacity) * DEFORMED_POSITION_STRIDE,
            usage: BufferUsage::STORAGE | BufferUsage::VERTEX | BufferUsage::COPY_DST | BufferUsage::COPY_SRC,
        })?;
        Ok(Self {
            label,
            buffer,
            bank_capacity,
            frames_in_flight,
            allocator: RangeAllocator::new(u64::from(bank_capacity)),
            pending: VecDeque::new(),
            pending_vertices: 0,
        })
    }

    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    pub fn bank_capacity(&self) -> u32 {
        self.bank_capacity
    }

    pub fn allocate(&mut self, vertex_count: u32) -> Result<DeformedRange, MeshError> {
        let requested = u64::from(vertex_count);
        let range = (requested > 0).then(|| self.allocator.allocate(requested, 1)).flatten();
        match range {
            Some(range) => Ok(DeformedRange {
                first_vertex: u32::try_from(range.start).expect("bank capacity is u32"),
                vertex_count,
            }),
            None if requested == 0 => Err(MeshError::InvalidData {
                label: self.label,
                reason: "a deformed range needs at least one vertex".to_owned(),
            }),
            None if self.allocator.available() >= requested => Err(MeshError::Fragmented {
                pool: self.label,
                kind: ArenaKind::DeformedVertices,
                requested,
                free: self.allocator.available(),
                largest_free: self.allocator.largest_free(),
            }),
            None => Err(MeshError::Exhausted {
                pool: self.label,
                kind: ArenaKind::DeformedVertices,
                requested,
                free: self.allocator.available(),
                pending: self.pending_vertices,
            }),
        }
    }

    // Skinning may still write or read the range for `frames_in_flight` frames, so it is reusable only later.
    pub fn free(&mut self, range: DeformedRange, frame: u64) {
        self.pending_vertices += u64::from(range.vertex_count);
        self.pending.push_back(PendingFree { frame, range });
    }

    pub fn begin_frame(&mut self, frame: u64) {
        while let Some(removal) = self.pending.front() {
            if frame < removal.frame + self.frames_in_flight {
                break;
            }
            let range = removal.range;
            self.pending.pop_front();
            let start = u64::from(range.first_vertex);
            self.pending_vertices -= u64::from(range.vertex_count);
            self.allocator.free(start..start + u64::from(range.vertex_count));
        }
    }

    pub fn current_bank(frame: u64) -> u32 {
        u32::from(frame & 1 == 1)
    }

    pub fn previous_bank(frame: u64) -> u32 {
        Self::current_bank(frame) ^ 1
    }

    pub fn current_offset(&self, range: DeformedRange, frame: u64) -> u64 {
        self.bank_offset(range, Self::current_bank(frame))
    }

    pub fn previous_offset(&self, range: DeformedRange, frame: u64) -> u64 {
        self.bank_offset(range, Self::previous_bank(frame))
    }

    fn bank_offset(&self, range: DeformedRange, bank: u32) -> u64 {
        (u64::from(bank) * u64::from(self.bank_capacity) + u64::from(range.first_vertex)) * DEFORMED_POSITION_STRIDE
    }

    // After device loss the buffer is dead; create a new `DeformedPositions` and allocate the ranges again.
    pub fn release(self, device: &dyn DeviceApi) {
        device.destroy(Resource::Buffer(self.buffer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banks_alternate_and_previous_is_the_other_one() {
        assert_eq!(
            [DeformedPositions::current_bank(4), DeformedPositions::current_bank(5)],
            [0, 1]
        );
        for frame in 0..4 {
            assert_ne!(
                DeformedPositions::current_bank(frame),
                DeformedPositions::previous_bank(frame)
            );
            assert_eq!(
                DeformedPositions::previous_bank(frame + 1),
                DeformedPositions::current_bank(frame)
            );
        }
    }
}
