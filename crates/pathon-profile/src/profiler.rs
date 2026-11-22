use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pathon_api::graph::FrameObserver;
use pathon_api::rhi::{CommandList, DeviceApi, PassTimestamps};

use crate::gpu::GpuTimer;
use crate::sink::{GpuPassTiming, ProfileSink};

const DEFAULT_GPU_PASS_CAPACITY: u32 = 128;
const DEFAULT_GPU_LATENCY_FRAMES: u64 = 2;

// Set on a renderer (`Renderer::set_frame_observer`): names every pass after the graph, times every GPU pass and hands
// both to the sinks.
pub struct Profiler {
    sinks: Vec<Arc<dyn ProfileSink>>,
    gpu: Option<Mutex<GpuTimer>>,
    frame: AtomicU64,
}

impl std::fmt::Debug for Profiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profiler")
            .field("sinks", &self.sinks.len())
            .field("gpu_timings", &self.gpu.is_some())
            .finish_non_exhaustive()
    }
}

pub struct ProfilerBuilder {
    sinks: Vec<Arc<dyn ProfileSink>>,
    gpu: Option<(u32, u64)>,
}

impl std::fmt::Debug for ProfilerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfilerBuilder")
            .field("sinks", &self.sinks.len())
            .field("gpu", &self.gpu)
            .finish()
    }
}

impl Default for ProfilerBuilder {
    fn default() -> Self {
        Self {
            sinks: Vec::new(),
            gpu: Some((DEFAULT_GPU_PASS_CAPACITY, DEFAULT_GPU_LATENCY_FRAMES)),
        }
    }
}

impl ProfilerBuilder {
    #[must_use]
    pub fn with_sink(mut self, sink: Arc<dyn ProfileSink>) -> Self {
        self.sinks.push(sink);
        self
    }

    // GPU passes timed per frame; further ones of the frame are skipped with T0002.
    #[must_use]
    pub fn with_gpu_pass_capacity(mut self, capacity: u32) -> Self {
        let latency = self.gpu.map_or(DEFAULT_GPU_LATENCY_FRAMES, |(_, latency)| latency);
        self.gpu = Some((capacity, latency));
        self
    }

    // Frames between recording a frame and reading its timings. Reading blocks until the GPU has finished the frame,
    // so a larger value keeps CPU and GPU more parallel.
    #[must_use]
    pub fn with_gpu_latency(mut self, frames: u64) -> Self {
        let capacity = self.gpu.map_or(DEFAULT_GPU_PASS_CAPACITY, |(capacity, _)| capacity);
        self.gpu = Some((capacity, frames));
        self
    }

    // CPU spans only: no query sets, no readback.
    #[must_use]
    pub fn without_gpu_timings(mut self) -> Self {
        self.gpu = None;
        self
    }

    #[cfg_attr(
        target_arch = "wasm32",
        expect(clippy::arc_with_non_send_sync, reason = "one thread on the web (ADR-0004)")
    )]
    pub fn build(self) -> Arc<Profiler> {
        Arc::new(Profiler {
            frame: AtomicU64::new(0),
            sinks: self.sinks,
            gpu: self
                .gpu
                .map(|(capacity, latency)| Mutex::new(GpuTimer::new(capacity, latency))),
        })
    }
}

impl Profiler {
    pub fn builder() -> ProfilerBuilder {
        ProfilerBuilder::default()
    }

    // Reads every pending frame now, waiting for the GPU. For shutdown and for tests, which do not render `latency`
    // more frames.
    pub fn flush(&self, device: &dyn DeviceApi) {
        let ready = self
            .with_gpu(|timer| timer.read_ready(device, None))
            .unwrap_or_default();
        self.report(ready);
    }

    // Destroys the GPU objects the profiler made. Call it before the renderer's device is dropped or reused.
    pub fn release(&self, device: &dyn DeviceApi) {
        self.with_gpu(|timer| timer.release(device));
    }

    fn with_gpu<T>(&self, use_timer: impl FnOnce(&mut GpuTimer) -> T) -> Option<T> {
        let timer = self.gpu.as_ref()?;
        let mut timer: MutexGuard<'_, GpuTimer> = timer.lock().unwrap_or_else(PoisonError::into_inner);
        Some(use_timer(&mut timer))
    }

    fn report(&self, frames: Vec<(u64, f32, Vec<GpuPassTiming>)>) {
        for (frame, period_ns, passes) in frames {
            for sink in &self.sinks {
                sink.gpu_timings(frame, period_ns, &passes);
            }
        }
    }
}

impl FrameObserver for Profiler {
    fn begin_frame(&self, device: &dyn DeviceApi, frame: u64) {
        self.frame.store(frame, Ordering::Relaxed);
        self.with_gpu(|timer| timer.begin_frame(device, frame));
    }

    fn begin_pass(&self, pass: &'static str) {
        for sink in &self.sinks {
            sink.pass_begin(pass);
        }
    }

    fn end_pass(&self, pass: &'static str) {
        for sink in self.sinks.iter().rev() {
            sink.pass_end(pass);
        }
    }

    fn gpu_pass(&self, _device: &dyn DeviceApi, pass: &'static str, label: &'static str) -> Option<PassTimestamps> {
        self.with_gpu(|timer| timer.gpu_pass(pass, label)).flatten()
    }

    fn end_frame(&self, _device: &dyn DeviceApi, commands: &mut CommandList) {
        self.with_gpu(|timer| timer.end_frame(commands));
    }

    fn frame_committed(&self, device: &dyn DeviceApi) {
        let frame = self.frame.load(Ordering::Relaxed);
        let ready = self.with_gpu(|timer| {
            timer.frame_committed();
            timer.read_ready(device, Some(frame))
        });
        self.report(ready.unwrap_or_default());
        for sink in &self.sinks {
            sink.frame_end(frame);
        }
    }

    fn frame_abandoned(&self) {
        self.with_gpu(GpuTimer::frame_abandoned);
    }

    fn device_reset(&self) {
        self.with_gpu(GpuTimer::forget_device);
    }
}
