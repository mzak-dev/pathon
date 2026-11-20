use std::mem;
use std::ops::Range;

use bytemuck::Pod;
use pathon_rhi_types::{BufferDesc, BufferId, BufferUsage, DeviceApi, RhiError};

use crate::deferred::DeferredRelease;

// CPU mirror of a storage buffer (instances, materials, lights). `sync` uploads only the dirty range and grows the
// buffer by doubling; the previous buffer goes through deferred release because the GPU may still read it.
#[derive(Debug)]
pub struct GpuArray<T> {
    label: &'static str,
    items: Vec<T>,
    buffer: Option<BufferId>,
    capacity: usize,
    dirty: Option<Range<usize>>,
}

impl<T: Pod> GpuArray<T> {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            items: Vec::new(),
            buffer: None,
            capacity: 0,
            dirty: None,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index)
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    pub fn buffer(&self) -> Option<BufferId> {
        self.buffer
    }

    pub fn push(&mut self, item: T) -> usize {
        let index = self.items.len();
        self.items.push(item);
        self.mark_dirty(index..index + 1);
        index
    }

    pub fn set(&mut self, index: usize, item: T) -> bool {
        let Some(slot) = self.items.get_mut(index) else {
            return false;
        };
        *slot = item;
        self.mark_dirty(index..index + 1);
        true
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.dirty = None;
    }

    fn mark_dirty(&mut self, range: Range<usize>) {
        self.dirty = Some(match self.dirty.take() {
            Some(dirty) => dirty.start.min(range.start)..dirty.end.max(range.end),
            None => range,
        });
    }

    pub fn sync(
        &mut self,
        device: &dyn DeviceApi,
        release: &mut DeferredRelease,
        frame: u64,
    ) -> Result<BufferId, RhiError> {
        let needed = self.items.len().max(1);
        if self.buffer.is_none() || needed > self.capacity {
            let capacity = needed.next_power_of_two();
            let buffer = device.create_buffer(&BufferDesc {
                label: self.label,
                size: (capacity * mem::size_of::<T>()) as u64,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_DST | BufferUsage::COPY_SRC,
            })?;
            if let Some(old) = self.buffer.replace(buffer) {
                release.release(frame, old);
            }
            self.capacity = capacity;
            self.dirty = (!self.items.is_empty()).then_some(0..self.items.len());
        }
        let buffer = self.buffer.expect("buffer was created above when missing");
        if let Some(dirty) = self.dirty.take() {
            let offset = (dirty.start * mem::size_of::<T>()) as u64;
            device.write_buffer(buffer, offset, bytemuck::cast_slice(&self.items[dirty]))?;
        }
        Ok(buffer)
    }

    // After device loss the buffer handle is dead; the next `sync` re-creates and re-uploads everything.
    pub fn forget_gpu_buffer(&mut self) {
        self.buffer = None;
        self.capacity = 0;
    }
}

#[cfg(test)]
mod tests {
    use pathon_rhi_null::NullDevice;
    use pathon_rhi_types::CommandList;

    use super::*;

    fn read_back(device: &NullDevice, buffer: BufferId, len: usize) -> Vec<u32> {
        let size = (len * 4) as u64;
        let readback = device
            .create_buffer(&BufferDesc {
                label: "readback",
                size,
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback");
        let mut list = CommandList::new();
        list.copy_buffer_to_buffer(buffer, 0, readback, 0, size);
        device.submit(list).expect("copy");
        bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec()
    }

    #[test]
    fn growing_reuploads_everything_and_releases_the_old_buffer() {
        let device = NullDevice::new();
        let mut release = DeferredRelease::new(2);
        let mut array: GpuArray<u32> = GpuArray::new("values");
        array.push(7);
        let first = array.sync(&device, &mut release, 0).expect("sync");
        array.push(8);
        array.push(9);
        let second = array.sync(&device, &mut release, 1).expect("sync");

        assert_ne!(first, second);
        assert_eq!(release.len(), 1);
        assert_eq!(read_back(&device, second, 3), vec![7, 8, 9]);
    }

    #[test]
    fn set_uploads_only_the_changed_item() {
        let device = NullDevice::new();
        let mut release = DeferredRelease::new(2);
        let mut array: GpuArray<u32> = GpuArray::new("values");
        for value in [1, 2, 3, 4] {
            array.push(value);
        }
        let buffer = array.sync(&device, &mut release, 0).expect("sync");
        array.set(2, 30);
        array.sync(&device, &mut release, 1).expect("sync");

        assert_eq!(read_back(&device, buffer, 4), vec![1, 2, 30, 4]);
        assert!(release.is_empty());
    }
}
