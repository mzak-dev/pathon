#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

use pathon_golden::{announce_adapter, gpu_or_skip};
use pathon_rhi::{Backend, BufferCopy, BufferDesc, BufferId, BufferUsage, CommandList, Device, DeviceApi, DeviceRequest, RhiError};

pub fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_TEST_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        other => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
    }
}

/// A real device, or `None` (the test is skipped) when the machine has no adapter.
pub fn gpu(test: &str) -> Option<Device> {
    let built = Device::build(&DeviceRequest {
        label: "pbr test",
        backend: backend_from_environment(),
        ..Default::default()
    });
    let device = gpu_or_skip(test, built, |error| matches!(error, RhiError::NoAdapter(_)))?;
    announce_adapter(test, device.caps());
    Some(device)
}

pub fn buffer(device: &dyn DeviceApi, label: &'static str, size: u64, usage: BufferUsage) -> BufferId {
    device
        .create_buffer(&BufferDesc { label, size, usage })
        .expect("buffer")
}

/// Copies `size` bytes of `source` (in the `COPY_SRC` state) into `readback` at `offset`.
pub fn copy_out(list: &mut CommandList, source: BufferId, readback: BufferId, size: u64) {
    list.copy_buffer_to_buffer(source, 0, readback, 0, size);
}

/// A tiny deterministic generator, so a failing scene can be reproduced from its seed.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }

    pub fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next_f32()
    }
}

pub fn _unused(_: BufferCopy) {}

pub mod probe;
pub mod scene;
