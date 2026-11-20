use std::collections::VecDeque;

use pathon_rhi_types::{DeviceApi, Resource};

// The GPU may still read a resource for `frames_in_flight` frames after the CPU stops using it.
#[derive(Debug)]
pub struct DeferredRelease {
    frames_in_flight: u64,
    pending: VecDeque<(u64, Resource)>,
}

impl DeferredRelease {
    pub fn new(frames_in_flight: u64) -> Self {
        Self {
            frames_in_flight,
            pending: VecDeque::new(),
        }
    }

    pub fn release(&mut self, frame: u64, resource: impl Into<Resource>) {
        self.pending.push_back((frame, resource.into()));
    }

    pub fn collect(&mut self, frame: u64, device: &dyn DeviceApi) -> usize {
        let mut destroyed = 0;
        while let Some((released_at, resource)) = self.pending.front().copied() {
            if frame < released_at + self.frames_in_flight {
                break;
            }
            device.destroy(resource);
            self.pending.pop_front();
            destroyed += 1;
        }
        destroyed
    }

    // After device loss every handle is already invalid, so nothing is destroyed.
    pub fn forget_all(&mut self) {
        self.pending.clear();
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use pathon_rhi_null::NullDevice;
    use pathon_rhi_types::{BufferDesc, BufferUsage};

    use super::*;

    #[test]
    fn resources_are_destroyed_only_after_frames_in_flight() {
        let device = NullDevice::new();
        let buffer = device
            .create_buffer(&BufferDesc {
                label: "old",
                size: 4,
                usage: BufferUsage::UNIFORM,
            })
            .expect("buffer");
        let mut release = DeferredRelease::new(2);
        release.release(10, buffer);

        assert_eq!(release.collect(11, &device), 0);
        assert!(device.write_buffer(buffer, 0, &[0; 4]).is_ok());
        assert_eq!(release.collect(12, &device), 1);
        assert!(device.write_buffer(buffer, 0, &[0; 4]).is_err());
        assert!(release.is_empty());
    }
}
