#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

use std::collections::HashSet;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use pathon_api::{BackgroundJob, TaskPool};
use pathon_rhi::Device;
use pathon_rhi_types::{
    BindGroupDesc, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutId, BufferDesc, BufferId, Caps, CommandList,
    ComputePipelineDesc, ComputePipelineId, DeviceApi, Extent3d, Format, PipelineCacheData, QuerySetDesc, QuerySetId,
    RenderPipelineDesc, RenderPipelineId, Resource, RhiError, SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId,
    SurfaceConfig, SurfaceFrame, SurfaceId, SurfaceTarget, TexelLayout, TextureCopy, TextureDesc, TextureId,
    TextureViewDesc, TextureViewId,
};

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("a test thread panicked while holding the lock")
}

// Compiles block here until the test opens the gate, so a test decides exactly when builds are running.
#[derive(Debug)]
pub struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    pub fn closed() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            changed: Condvar::new(),
        })
    }

    pub fn open(&self) {
        *lock(&self.open) = true;
        self.changed.notify_all();
    }

    fn pass(&self) {
        let mut open = lock(&self.open);
        while !*open {
            open = self
                .changed
                .wait(open)
                .expect("a test thread panicked while holding the lock");
        }
    }
}

#[derive(Debug, Default)]
pub struct Stats {
    pub modules: AtomicUsize,
    pub pipelines: AtomicUsize,
    pub entered: AtomicUsize,
    pub running: AtomicUsize,
    pub max_running: AtomicUsize,
    pub created: Mutex<Vec<Resource>>,
    pub destroyed: Mutex<Vec<Resource>>,
    pub threads: Mutex<HashSet<ThreadId>>,
}

impl Stats {
    pub fn count(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }

    pub fn leaked(&self) -> Vec<Resource> {
        let destroyed = lock(&self.destroyed);
        lock(&self.created)
            .iter()
            .filter(|resource| !destroyed.contains(resource))
            .copied()
            .collect()
    }
}

struct Running<'stats>(&'stats Stats);

impl<'stats> Running<'stats> {
    fn enter(stats: &'stats Stats) -> Self {
        stats.entered.fetch_add(1, Ordering::SeqCst);
        let now = stats.running.fetch_add(1, Ordering::SeqCst) + 1;
        stats.max_running.fetch_max(now, Ordering::SeqCst);
        lock(&stats.threads).insert(thread::current().id());
        Self(stats)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::SeqCst);
    }
}

// The null device with a compile cost: every create_shader_module / create_*_pipeline waits for the gate, sleeps
// `delay`, and then fails or panics when the label says so.
pub struct TestDevice {
    inner: Device,
    pub delay: Duration,
    pub gate: Option<Arc<Gate>>,
    pub fail_label: Option<&'static str>,
    pub panic_label: Option<&'static str>,
    pub stats: Arc<Stats>,
    // What `pipeline_cache_data` returns, so a test decides what the "driver" holds.
    pub cache_blob: Mutex<Option<Vec<u8>>>,
}

impl TestDevice {
    pub fn new(delay: Duration) -> Self {
        Self {
            inner: Device::null(),
            delay,
            gate: None,
            fail_label: None,
            panic_label: None,
            stats: Arc::new(Stats::default()),
            cache_blob: Mutex::new(None),
        }
    }

    fn compile<T: Into<Resource> + Copy>(
        &self,
        label: &str,
        counter: &AtomicUsize,
        create: impl FnOnce() -> Result<T, RhiError>,
    ) -> Result<T, RhiError> {
        let _running = Running::enter(&self.stats);
        if let Some(gate) = &self.gate {
            gate.pass();
        }
        thread::sleep(self.delay);
        assert_ne!(self.panic_label, Some(label), "injected panic in `{label}`");
        if self.fail_label == Some(label) {
            return Err(RhiError::Backend(format!("injected failure in `{label}`")));
        }
        let id = create()?;
        counter.fetch_add(1, Ordering::SeqCst);
        lock(&self.stats.created).push(id.into());
        Ok(id)
    }
}

impl DeviceApi for TestDevice {
    fn caps(&self) -> &Caps {
        self.inner.caps()
    }
    fn is_lost(&self) -> bool {
        self.inner.is_lost()
    }
    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferId, RhiError> {
        self.inner.create_buffer(desc)
    }
    fn write_buffer(&self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), RhiError> {
        self.inner.write_buffer(buffer, offset, data)
    }
    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureId, RhiError> {
        self.inner.create_texture(desc)
    }
    fn texture_desc(&self, texture: TextureId) -> Result<TextureDesc, RhiError> {
        self.inner.texture_desc(texture)
    }
    fn write_texture(
        &self,
        dst: &TextureCopy,
        data: &[u8],
        layout: TexelLayout,
        size: Extent3d,
    ) -> Result<(), RhiError> {
        self.inner.write_texture(dst, data, layout, size)
    }
    fn create_texture_view(&self, texture: TextureId, desc: &TextureViewDesc) -> Result<TextureViewId, RhiError> {
        self.inner.create_texture_view(texture, desc)
    }
    fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerId, RhiError> {
        self.inner.create_sampler(desc)
    }
    fn create_shader_module(&self, desc: &ShaderModuleDesc<'_>) -> Result<ShaderModuleId, RhiError> {
        self.compile(desc.label, &self.stats.modules, || {
            self.inner.create_shader_module(desc)
        })
    }
    fn create_bind_group_layout(&self, desc: &BindGroupLayoutDesc) -> Result<BindGroupLayoutId, RhiError> {
        self.inner.create_bind_group_layout(desc)
    }
    fn create_bind_group(&self, desc: &BindGroupDesc) -> Result<BindGroupId, RhiError> {
        self.inner.create_bind_group(desc)
    }
    fn create_render_pipeline(&self, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError> {
        self.compile(desc.label, &self.stats.pipelines, || {
            self.inner.create_render_pipeline(desc)
        })
    }
    fn create_compute_pipeline(&self, desc: &ComputePipelineDesc) -> Result<ComputePipelineId, RhiError> {
        self.compile(desc.label, &self.stats.pipelines, || {
            self.inner.create_compute_pipeline(desc)
        })
    }
    fn create_query_set(&self, desc: &QuerySetDesc) -> Result<QuerySetId, RhiError> {
        self.inner.create_query_set(desc)
    }
    fn destroy(&self, resource: Resource) {
        lock(&self.stats.destroyed).push(resource);
        self.inner.destroy(resource);
    }
    fn create_surface(&self, target: &SurfaceTarget) -> Result<SurfaceId, RhiError> {
        self.inner.create_surface(target)
    }
    fn configure_surface(&self, surface: SurfaceId, config: &SurfaceConfig) -> Result<Format, RhiError> {
        self.inner.configure_surface(surface, config)
    }
    fn acquire_surface(&self, surface: SurfaceId) -> Result<SurfaceFrame, RhiError> {
        self.inner.acquire_surface(surface)
    }
    fn present_surface(&self, surface: SurfaceId) -> Result<(), RhiError> {
        self.inner.present_surface(surface)
    }
    fn discard_surface(&self, surface: SurfaceId) {
        self.inner.discard_surface(surface);
    }
    fn destroy_surface(&self, surface: SurfaceId) {
        self.inner.destroy_surface(surface);
    }
    fn submit(&self, commands: CommandList) -> Result<(), RhiError> {
        self.inner.submit(commands)
    }
    fn read_buffer(&self, buffer: BufferId) -> Result<Vec<u8>, RhiError> {
        self.inner.read_buffer(buffer)
    }
    fn wait_idle(&self) -> Result<(), RhiError> {
        self.inner.wait_idle()
    }
    fn pipeline_cache_data(&self) -> Option<PipelineCacheData> {
        let blob = lock(&self.cache_blob).clone()?;
        // SAFETY: the bytes only travel to a test store and back; no driver ever receives them.
        Some(unsafe { PipelineCacheData::from_bytes(blob) })
    }
}

// A pool that runs `try_spawn` on a fresh OS thread per job, like the runner's `StdTaskPool`.
#[derive(Debug)]
pub struct ThreadedPool {
    threads: usize,
}

impl ThreadedPool {
    pub fn new(threads: usize) -> Self {
        Self { threads }
    }
}

impl TaskPool for ThreadedPool {
    fn parallel_for(&self, len: usize, task: &(dyn Fn(Range<usize>) + Sync)) {
        if len > 0 {
            task(0..len);
        }
    }

    fn thread_count(&self) -> usize {
        self.threads
    }

    fn try_spawn(&self, job: BackgroundJob) -> Result<(), BackgroundJob> {
        thread::Builder::new()
            .name("test-compile".into())
            .spawn(job)
            .expect("the test can start a thread");
        Ok(())
    }
}

pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(1));
    }
}

// p99 by nearest rank; with at least 100 frames the single worst frame is ignored.
pub fn p99(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    let rank = (samples.len() * 99).div_ceil(100);
    samples[rank.saturating_sub(1)]
}
