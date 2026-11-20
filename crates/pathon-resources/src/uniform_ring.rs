use pathon_rhi_types::{BindingResource, BufferDesc, BufferId, BufferUsage, DeviceApi, RhiError};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct UniformSlice {
    pub buffer: BufferId,
    pub offset: u64,
    pub size: u64,
}

impl UniformSlice {
    pub fn binding(self) -> BindingResource {
        BindingResource::Buffer {
            buffer: self.buffer,
            offset: self.offset,
            size: Some(self.size),
        }
    }
}

// Per-frame linear allocator for small uniform data. Queue writes are ordered before the next submission, so the ring
// can restart at offset zero every frame.
#[derive(Debug)]
pub struct UniformRing {
    buffer: BufferId,
    capacity: u64,
    alignment: u64,
    cursor: u64,
}

impl UniformRing {
    pub fn new(device: &dyn DeviceApi, label: &'static str, capacity: u64) -> Result<Self, RhiError> {
        let buffer = device.create_buffer(&BufferDesc {
            label,
            size: capacity,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        })?;
        let alignment = u64::from(device.caps().limits.min_uniform_buffer_offset_alignment);
        Ok(Self {
            buffer,
            capacity,
            alignment,
            cursor: 0,
        })
    }

    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    pub fn begin_frame(&mut self) {
        self.cursor = 0;
    }

    pub fn write(&mut self, device: &dyn DeviceApi, bytes: &[u8]) -> Result<UniformSlice, RhiError> {
        let offset = self.cursor.next_multiple_of(self.alignment);
        let size = bytes.len() as u64;
        if offset + size > self.capacity {
            return Err(RhiError::validation("R0108", "uniform ring is full for this frame"));
        }
        device.write_buffer(self.buffer, offset, bytes)?;
        self.cursor = offset + size;
        Ok(UniformSlice {
            buffer: self.buffer,
            offset,
            size,
        })
    }
}

#[cfg(test)]
mod tests {
    use pathon_rhi_null::NullDevice;

    use super::*;

    #[test]
    fn writes_are_aligned_and_ring_restarts_each_frame() {
        let device = NullDevice::new();
        let mut ring = UniformRing::new(&device, "frame uniforms", 1024).expect("ring");
        let first = ring.write(&device, &[1; 16]).expect("first");
        let second = ring.write(&device, &[2; 16]).expect("second");

        assert_eq!(first.offset, 0);
        assert_eq!(second.offset, 256);
        ring.begin_frame();
        assert_eq!(ring.write(&device, &[3; 16]).expect("third").offset, 0);
    }

    #[test]
    fn overflow_is_an_error() {
        let device = NullDevice::new();
        let mut ring = UniformRing::new(&device, "small", 300).expect("ring");
        ring.write(&device, &[0; 16]).expect("fits");
        assert!(ring.write(&device, &[0; 64]).is_err());
    }
}
