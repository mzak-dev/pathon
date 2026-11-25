use pathon_api::rhi::{BindingResource, BufferDesc, BufferId, BufferUsage, DeviceApi, RhiError};

/// A piece of the arena buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArenaSlice {
    pub buffer: BufferId,
    pub offset: u64,
    pub size: u64,
}

impl ArenaSlice {
    pub fn binding(self) -> BindingResource {
        BindingResource::Buffer {
            buffer: self.buffer,
            offset: self.offset,
            size: Some(self.size),
        }
    }
}

/// Per-record scratch for uniform and storage data written by the CPU: frame constants, instances, lights. A queue
/// write lands before the next submission, so the cursor restarts with every record attempt (`record_epoch`). The
/// buffer grows by doubling; the one it replaces is returned for `RecordCtx::release_later`, because slices handed out
/// earlier in the same record still point into it.
#[derive(Debug, Default)]
pub struct BufferArena {
    buffer: Option<(BufferId, u64)>,
    cursor: u64,
    epoch: Option<u64>,
}

impl BufferArena {
    const MIN_CAPACITY: u64 = 64 << 10;
    const MIN_BINDING: u64 = 256;

    pub fn new() -> Self {
        Self::default()
    }

    /// Writes `bytes` and returns where they went, plus the buffer that was replaced to make room.
    pub fn write(
        &mut self,
        device: &dyn DeviceApi,
        record_epoch: u64,
        bytes: &[u8],
    ) -> Result<(ArenaSlice, Option<BufferId>), RhiError> {
        if self.epoch != Some(record_epoch) {
            self.epoch = Some(record_epoch);
            self.cursor = 0;
        }
        let limits = &device.caps().limits;
        let alignment = u64::from(limits.min_uniform_buffer_offset_alignment.max(limits.min_storage_buffer_offset_alignment));
        let offset = self.cursor.next_multiple_of(alignment);
        // A storage binding must hold at least one element of the array the shader declares (the widest is 160 bytes),
        // so an empty list still gets a block of this size.
        let size = (bytes.len() as u64).max(Self::MIN_BINDING);
        let needed = offset + size;
        let mut retired = None;
        let buffer = match self.buffer {
            Some((buffer, capacity)) if needed <= capacity => buffer,
            existing => {
                let capacity = needed.next_power_of_two().max(Self::MIN_CAPACITY).max(existing.map_or(0, |(_, c)| c * 2));
                let buffer = device.create_buffer(&BufferDesc {
                    label: "pbr frame data",
                    size: capacity,
                    usage: BufferUsage::UNIFORM | BufferUsage::STORAGE | BufferUsage::COPY_DST,
                })?;
                retired = existing.map(|(old, _)| old);
                self.buffer = Some((buffer, capacity));
                buffer
            }
        };
        if !bytes.is_empty() {
            device.write_buffer(buffer, offset, bytes)?;
        }
        self.cursor = offset + size;
        Ok((ArenaSlice { buffer, offset, size }, retired))
    }

    /// After device loss the buffer is dead.
    pub fn forget_gpu(&mut self) {
        self.buffer = None;
        self.cursor = 0;
        self.epoch = None;
    }
}

#[cfg(test)]
mod tests {
    use pathon_rhi::NullDevice;

    use super::*;

    #[test]
    fn slices_are_aligned_do_not_overlap_and_restart_each_record() {
        let device = NullDevice::new();
        let mut arena = BufferArena::new();

        let (first, _) = arena.write(&device, 1, &[1; 20]).expect("write");
        let (second, _) = arena.write(&device, 1, &[2; 20]).expect("write");
        let (restarted, _) = arena.write(&device, 2, &[3; 20]).expect("write");

        assert_eq!(first.offset, 0);
        assert!(second.offset >= first.offset + first.size);
        assert_eq!(second.offset % 256, 0);
        assert_eq!(restarted.offset, 0);
    }

    #[test]
    fn growing_hands_back_the_old_buffer_and_keeps_earlier_slices_valid() {
        let device = NullDevice::new();
        let mut arena = BufferArena::new();
        let (small, retired) = arena.write(&device, 1, &[1; 64]).expect("write");
        assert!(retired.is_none());

        let (big, retired) = arena.write(&device, 1, &vec![2; 200 << 10]).expect("write");

        assert_eq!(retired, Some(small.buffer));
        assert_ne!(big.buffer, small.buffer);
        assert!(device.write_buffer(small.buffer, 0, &[0; 4]).is_ok(), "not destroyed here");
    }
}
