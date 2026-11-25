use std::collections::{HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use pathon_api::{BackgroundJob, Diagnostic, TaskPool};
use pathon_rhi_types::{
    ComputePipelineDesc, ComputePipelineId, DeviceApi, MaybeSend, RenderPipelineDesc, RenderPipelineId, Resource,
    RhiError, ShaderModuleDesc, ShaderModuleId,
};

use crate::library::ShaderLibrary;
use crate::pipeline_cache::{PipelineCache, normalized_compute, normalized_render, render_uses_module};
use crate::snapshot::LibrarySnapshot;
use crate::{Composed, ShaderDiagnostic, ShaderError, ShaderKey, check_pipeline_constants};

/// What a pack gets back for something it asked the compiler to build.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lookup<T> {
    Ready(T),
    Pending,
    /// The build failed; the reason is in [`BackgroundCompiler::take_failures`].
    Failed,
}

/// What a draw uses while its pipeline is not ready (ADR-0024).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissPolicy<T> {
    Skip,
    /// A pipeline built synchronously beforehand, with the layout the real one will have.
    Placeholder(T),
}

impl<T: Copy> Lookup<T> {
    #[must_use]
    pub fn ready(self) -> Option<T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Pending | Self::Failed => None,
        }
    }

    /// The value to draw with: the ready one, else whatever `policy` allows (`None` skips the draw).
    #[must_use]
    pub fn resolve(self, policy: MissPolicy<T>) -> Option<T> {
        match (self, policy) {
            (Self::Ready(value), _) | (_, MissPolicy::Placeholder(value)) => Some(value),
            (Self::Pending | Self::Failed, MissPolicy::Skip) => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct BackgroundConfig {
    /// `None` is `thread_count() - 1`, at least 1.
    pub max_in_flight: Option<usize>,
    // A count, not a time: `Instant::now` panics on wasm32-unknown-unknown.
    pub inline_jobs_per_pump: usize,
}

impl BackgroundConfig {
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_in_flight: None,
            inline_jobs_per_pump: 1,
        }
    }

    #[must_use]
    pub fn with_max_in_flight(self, max_in_flight: usize) -> Self {
        Self {
            max_in_flight: Some(max_in_flight),
            ..self
        }
    }

    #[must_use]
    pub fn with_inline_jobs_per_pump(self, inline_jobs_per_pump: usize) -> Self {
        Self {
            inline_jobs_per_pump,
            ..self
        }
    }
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    #[error(transparent)]
    Shader(#[from] ShaderError),
    #[error(transparent)]
    Rhi(#[from] RhiError),
    #[error("the build panicked: {message}")]
    Panicked { message: String },
}

/// A build that did not succeed; the key stays [`Lookup::Failed`] until sources change or `retry_failed` is called.
#[derive(Debug)]
#[non_exhaustive]
pub struct BuildFailure {
    pub subject: String,
    pub error: BuildError,
    code: &'static str,
}

const SKIPPED_UNTIL_FIXED: &str =
    "the draw is skipped or uses its placeholder; fix the source, or call `retry_failed` if the cause was external";

impl BuildFailure {
    /// The code of the located shader problem when there is one (S0101 to S0111), else S0001 or S0002.
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.located().map_or(self.code, |located| located.code)
    }

    /// The problem in the source, with file, line and hint, when the failure is one.
    #[must_use]
    pub fn located(&self) -> Option<&ShaderDiagnostic> {
        match &self.error {
            BuildError::Shader(error) => error.diagnostic(),
            BuildError::Rhi(_) | BuildError::Panicked { .. } => None,
        }
    }

    #[must_use]
    pub fn diagnostic(&self) -> Diagnostic {
        let Some(located) = self.located() else {
            return Diagnostic::error(
                self.code,
                format!("{} failed to compile in the background: {}", self.subject, self.error),
            )
            .with_hint(SKIPPED_UNTIL_FIXED);
        };
        let diagnostic = located.to_diagnostic();
        let hint = match &located.hint {
            Some(hint) => format!("{hint} ({SKIPPED_UNTIL_FIXED})"),
            None => SKIPPED_UNTIL_FIXED.to_owned(),
        };
        Diagnostic {
            message: format!("{} failed to compile: {}", self.subject, diagnostic.message),
            ..diagnostic
        }
        .with_hint(hint)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
#[non_exhaustive]
pub struct PumpReport {
    pub completed: usize,
    pub started: usize,
    pub queued: usize,
    pub running: usize,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Target {
    Module(ShaderKey),
    Render(RenderPipelineDesc),
    Compute(ComputePipelineDesc),
}

impl Target {
    fn failure(self, error: BuildError) -> BuildFailure {
        let (subject, code) = match &self {
            Self::Module(key) => {
                let flags: Vec<&str> = key.features.iter().collect();
                (
                    format!("shader `{}` with flags [{}]", key.root, flags.join(", ")),
                    "S0001",
                )
            }
            Self::Render(desc) => (format!("render pipeline `{}`", desc.label), "S0002"),
            Self::Compute(desc) => (format!("compute pipeline `{}`", desc.label), "S0002"),
        };
        BuildFailure { subject, error, code }
    }
}

enum Built {
    Module {
        key: ShaderKey,
        composed: Composed,
        id: ShaderModuleId,
    },
    Render {
        desc: RenderPipelineDesc,
        id: RenderPipelineId,
    },
    Compute {
        desc: ComputePipelineDesc,
        id: ComputePipelineId,
    },
}

impl Built {
    fn resource(&self) -> Resource {
        match self {
            Self::Module { id, .. } => (*id).into(),
            Self::Render { id, .. } => (*id).into(),
            Self::Compute { id, .. } => (*id).into(),
        }
    }
}

enum Interrupted {
    Failed(BuildError),
    Cancelled,
}

impl From<ShaderError> for Interrupted {
    fn from(error: ShaderError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<RhiError> for Interrupted {
    fn from(error: RhiError) -> Self {
        Self::Failed(error.into())
    }
}

struct Finished {
    target: Target,
    generation: u64,
    result: Result<Built, BuildError>,
}

// A result nobody opened (the compiler is gone, or its channel was dropped with the result inside) destroys what the
// job created, so dropping the compiler never leaks device objects.
struct Delivery {
    device: Arc<dyn DeviceApi>,
    finished: Option<Finished>,
}

impl Delivery {
    fn open(mut self) -> Finished {
        self.finished.take().expect("a delivery is opened once")
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if let Some(Finished { result: Ok(built), .. }) = self.finished.take() {
            self.device.destroy(built.resource());
        }
    }
}

#[derive(Clone)]
struct Worker {
    device: Arc<dyn DeviceApi>,
    results: Sender<Delivery>,
    cancelled: Arc<AtomicBool>,
}

impl Worker {
    fn job<F>(&self, target: Target, generation: u64, work: F) -> BackgroundJob
    where
        F: FnOnce(&dyn DeviceApi, &AtomicBool) -> Result<Built, Interrupted> + MaybeSend + 'static,
    {
        let worker = self.clone();
        Box::new(move || worker.run(target, generation, work))
    }

    fn run<F>(&self, target: Target, generation: u64, work: F)
    where
        F: FnOnce(&dyn DeviceApi, &AtomicBool) -> Result<Built, Interrupted>,
    {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| work(self.device.as_ref(), &self.cancelled)));
        let result = match outcome {
            Ok(Ok(built)) => Ok(built),
            Ok(Err(Interrupted::Cancelled)) => return,
            Ok(Err(Interrupted::Failed(error))) => Err(error),
            Err(payload) => Err(BuildError::Panicked {
                message: panic_message(payload.as_ref()),
            }),
        };
        // Sending fails only once the compiler is gone, and the returned delivery then cleans up on drop.
        drop(self.results.send(Delivery {
            device: Arc::clone(&self.device),
            finished: Some(Finished {
                target,
                generation,
                result,
            }),
        }));
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_owned())
}

/// Builds shader permutations and pipelines off the frame thread; the frame only calls [`Self::pump`] and asks for keys
/// (ADR-0024, `docs/crates/pathon-shader.md`).
pub struct BackgroundCompiler {
    library: ShaderLibrary,
    pipelines: PipelineCache,
    pool: Arc<dyn TaskPool>,
    worker: Worker,
    results: Receiver<Delivery>,
    queue: VecDeque<BackgroundJob>,
    requested: HashSet<Target>,
    failed: HashSet<Target>,
    failures: Vec<BuildFailure>,
    // Modules whose pipelines were evicted: nothing may be built from them, and a build that finishes late is
    // destroyed.
    evicted_modules: HashSet<ShaderModuleId>,
    running: usize,
    max_in_flight: usize,
    inline_jobs_per_pump: usize,
}

impl std::fmt::Debug for BackgroundCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundCompiler")
            .field("queued", &self.queue.len())
            .field("running", &self.running)
            .field("failed", &self.failed.len())
            .finish_non_exhaustive()
    }
}

impl BackgroundCompiler {
    #[must_use]
    pub fn new(device: Arc<dyn DeviceApi>, pool: Arc<dyn TaskPool>, config: BackgroundConfig) -> Self {
        let (sender, results) = mpsc::channel();
        let max_in_flight = config
            .max_in_flight
            .unwrap_or_else(|| pool.thread_count().saturating_sub(1))
            .max(1);
        Self {
            library: ShaderLibrary::new(),
            pipelines: PipelineCache::new(),
            pool,
            worker: Worker {
                device,
                results: sender,
                cancelled: Arc::new(AtomicBool::new(false)),
            },
            results,
            queue: VecDeque::new(),
            requested: HashSet::new(),
            failed: HashSet::new(),
            failures: Vec::new(),
            evicted_modules: HashSet::new(),
            running: 0,
            max_in_flight,
            inline_jobs_per_pump: config.inline_jobs_per_pump,
        }
    }

    /// Like [`ShaderLibrary::add_module`]; builds started against the replaced source are discarded when they finish.
    pub fn add_module(&mut self, path: &str, source: &str) -> Result<Vec<ShaderModuleId>, ShaderError> {
        let generation = self.library.generation();
        let evicted = self.library.add_module(path, source)?;
        self.forget_module_failures_if_evicted(generation);
        Ok(evicted)
    }

    pub fn add_root(&mut self, name: &str, source: &str) -> Vec<ShaderModuleId> {
        let generation = self.library.generation();
        let evicted = self.library.add_root(name, source);
        self.forget_module_failures_if_evicted(generation);
        evicted
    }

    /// Like [`Self::add_module`], with the file the source came from, which diagnostics show.
    pub fn add_module_file(
        &mut self,
        path: &str,
        source: &str,
        file: &str,
    ) -> Result<Vec<ShaderModuleId>, ShaderError> {
        let generation = self.library.generation();
        let evicted = self.library.add_module_file(path, source, file)?;
        self.forget_module_failures_if_evicted(generation);
        Ok(evicted)
    }

    /// Like [`Self::add_root`], with the file the source came from, which diagnostics show.
    pub fn add_root_file(&mut self, name: &str, source: &str, file: &str) -> Vec<ShaderModuleId> {
        let generation = self.library.generation();
        let evicted = self.library.add_root_file(name, source, file);
        self.forget_module_failures_if_evicted(generation);
        evicted
    }

    /// Forgets a module whose file was deleted; builds started against it are discarded when they finish.
    pub fn remove_module(&mut self, path: &str) -> Vec<ShaderModuleId> {
        let generation = self.library.generation();
        let evicted = self.library.remove_module(path);
        self.forget_module_failures_if_evicted(generation);
        evicted
    }

    /// Forgets a shader root whose file was deleted.
    pub fn remove_root(&mut self, name: &str) -> Vec<ShaderModuleId> {
        let generation = self.library.generation();
        let evicted = self.library.remove_root(name);
        self.forget_module_failures_if_evicted(generation);
        evicted
    }

    /// What this compiler composes from and has built, to try a source change on another thread before applying it
    /// (`pathon-hot`, ADR-0040).
    #[must_use]
    pub fn snapshot(&self) -> LibrarySnapshot {
        self.library.snapshot()
    }

    /// Forgets the pipelines built from `modules` (what `add_module`, `add_root` and the file and remove variants hand
    /// back) and returns them to be destroyed with `RecordCtx::release_later` once nothing draws with them. Builds
    /// from those modules that are still running are destroyed when they finish, and no new ones start: ask for the
    /// shader key again.
    pub fn evict_pipelines(&mut self, modules: &[ShaderModuleId]) -> Vec<Resource> {
        self.evicted_modules.extend(modules.iter().copied());
        self.pipelines.evict_modules(modules)
    }

    /// Warnings (S0106) of finished permutations since the last call; each is reported once.
    pub fn take_warnings(&mut self) -> Vec<ShaderDiagnostic> {
        self.library.take_warnings()
    }

    pub fn shader_module(&mut self, key: &ShaderKey) -> Lookup<ShaderModuleId> {
        match self.library.cached_module(key) {
            Some(id) => Lookup::Ready(id),
            None => self.missing(|| Target::Module(key.clone()), Self::enqueue_module),
        }
    }

    /// The composed WGSL of a ready permutation, for reflection (M3-02) without composing again.
    #[must_use]
    pub fn wgsl(&self, key: &ShaderKey) -> Option<Arc<str>> {
        self.library.composed(key)
    }

    pub fn render_pipeline(&mut self, desc: &RenderPipelineDesc) -> Lookup<RenderPipelineId> {
        let key = normalized_render(desc);
        match self.pipelines.cached_render(key.as_ref()) {
            Some(id) => Lookup::Ready(id),
            None => self.missing(|| Target::Render(key.into_owned()), Self::enqueue_render),
        }
    }

    pub fn compute_pipeline(&mut self, desc: &ComputePipelineDesc) -> Lookup<ComputePipelineId> {
        let key = normalized_compute(desc);
        match self.pipelines.cached_compute(key.as_ref()) {
            Some(id) => Lookup::Ready(id),
            None => self.missing(|| Target::Compute(key.into_owned()), Self::enqueue_compute),
        }
    }

    /// The per-frame call: collects finished builds and starts queued ones without waiting for any of them.
    pub fn pump(&mut self) -> PumpReport {
        self.drive(self.inline_jobs_per_pump)
    }

    /// Failures since the last call; each is reported once, so the pack can turn them into diagnostics every frame.
    pub fn take_failures(&mut self) -> Vec<BuildFailure> {
        std::mem::take(&mut self.failures)
    }

    /// Lets failed keys be requested again.
    pub fn retry_failed(&mut self) {
        self.failed.clear();
    }

    /// Builds queued or running.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.requested.len()
    }

    /// Runs everything queued to the end, for warm-up and tests; waits at most `max_wait_per_build` for each running
    /// build and returns `false` if one takes longer. Without pool threads the builds run inline, so it never waits.
    pub fn finish_all(&mut self, max_wait_per_build: Duration) -> bool {
        loop {
            self.drive(usize::MAX);
            if self.running == 0 && self.queue.is_empty() {
                return true;
            }
            match self.results.recv_timeout(max_wait_per_build) {
                Ok(delivery) => self.apply(delivery.open()),
                Err(_) => return false,
            }
        }
    }

    /// Hands back every module and pipeline this compiler holds, to be destroyed with `RecordCtx::release_later`
    /// (ADR-0022): the GPU may still be using them. Keys must be asked for again afterwards.
    pub fn release_all(&mut self) -> Vec<Resource> {
        self.evicted_modules.clear();
        let mut resources: Vec<Resource> = self.library.drain_modules().into_iter().map(Resource::from).collect();
        resources.extend(self.pipelines.drain_resources());
        resources
    }
}

impl BackgroundCompiler {
    fn missing<T>(
        &mut self,
        target: impl FnOnce() -> Target,
        enqueue: fn(&mut Self, &Target) -> Option<BackgroundJob>,
    ) -> Lookup<T> {
        let target = target();
        if self.failed.contains(&target) {
            return Lookup::Failed;
        }
        if !self.requested.contains(&target) {
            match enqueue(self, &target) {
                Some(job) => {
                    self.requested.insert(target);
                    self.queue.push_back(job);
                }
                None => return Lookup::Failed,
            }
        }
        Lookup::Pending
    }

    fn enqueue_module(&mut self, target: &Target) -> Option<BackgroundJob> {
        let Target::Module(key) = target else {
            return None;
        };
        let compose = match self.library.compose_job(key) {
            Ok(compose) => compose,
            Err(error) => {
                self.record_failure(target.clone(), error.into());
                return None;
            }
        };
        let key = key.clone();
        Some(self.worker.job(
            target.clone(),
            self.library.root_generation(&key.root),
            move |device, cancelled| {
                let composed = compose.run()?;
                if cancelled.load(Ordering::Acquire) {
                    return Err(Interrupted::Cancelled);
                }
                let id = device
                    .create_shader_module(&ShaderModuleDesc {
                        label: "permutation",
                        wgsl: &composed.wgsl,
                    })
                    .map_err(ShaderError::Rhi)?;
                Ok(Built::Module { key, composed, id })
            },
        ))
    }

    fn enqueue_render(&mut self, target: &Target) -> Option<BackgroundJob> {
        let Target::Render(desc) = target else {
            return None;
        };
        if render_uses_module(desc, &self.evicted_module_list()) {
            return None;
        }
        let desc = desc.clone();
        let vertex_wgsl = self.library.wgsl_of_module(desc.vertex.module);
        let fragment_wgsl = desc
            .fragment
            .as_ref()
            .and_then(|stage| self.library.wgsl_of_module(stage.module));
        Some(self.worker.job(target.clone(), 0, move |device, cancelled| {
            if cancelled.load(Ordering::Acquire) {
                return Err(Interrupted::Cancelled);
            }
            if let Some(wgsl) = &vertex_wgsl {
                check_pipeline_constants(wgsl, &desc.vertex)?;
            }
            if let (Some(wgsl), Some(stage)) = (&fragment_wgsl, &desc.fragment) {
                check_pipeline_constants(wgsl, stage)?;
            }
            let id = device.create_render_pipeline(&desc)?;
            Ok(Built::Render { desc, id })
        }))
    }

    fn enqueue_compute(&mut self, target: &Target) -> Option<BackgroundJob> {
        let Target::Compute(desc) = target else {
            return None;
        };
        if self.evicted_modules.contains(&desc.shader.module) {
            return None;
        }
        let desc = desc.clone();
        let wgsl = self.library.wgsl_of_module(desc.shader.module);
        Some(self.worker.job(target.clone(), 0, move |device, cancelled| {
            if cancelled.load(Ordering::Acquire) {
                return Err(Interrupted::Cancelled);
            }
            if let Some(wgsl) = &wgsl {
                check_pipeline_constants(wgsl, &desc.shader)?;
            }
            let id = device.create_compute_pipeline(&desc)?;
            Ok(Built::Compute { desc, id })
        }))
    }

    fn drive(&mut self, mut inline_budget: usize) -> PumpReport {
        let mut report = PumpReport {
            completed: self.collect_finished(),
            ..PumpReport::default()
        };
        while let Some(job) = self.queue.pop_front() {
            if self.running >= self.max_in_flight {
                self.queue.push_front(job);
                break;
            }
            match self.pool.try_spawn(job) {
                Ok(()) => {
                    self.running += 1;
                    report.started += 1;
                }
                Err(job) if inline_budget > 0 => {
                    inline_budget -= 1;
                    self.running += 1;
                    report.started += 1;
                    job();
                    report.completed += self.collect_finished();
                }
                Err(job) => {
                    self.queue.push_front(job);
                    break;
                }
            }
        }
        report.completed += self.collect_finished();
        report.queued = self.queue.len();
        report.running = self.running;
        report
    }

    fn collect_finished(&mut self) -> usize {
        let mut completed = 0;
        while let Ok(delivery) = self.results.try_recv() {
            self.apply(delivery.open());
            completed += 1;
        }
        completed
    }

    fn apply(&mut self, finished: Finished) {
        let Finished {
            target,
            generation,
            result,
        } = finished;
        self.running = self.running.saturating_sub(1);
        self.requested.remove(&target);
        let outdated = self.is_outdated(&target, generation);
        match result {
            Ok(built) if outdated => self.worker.device.destroy(built.resource()),
            Ok(Built::Module { key, composed, id }) => self.library.store(key, composed, id),
            Ok(Built::Render { desc, id }) => self.pipelines.store_render(desc, id),
            Ok(Built::Compute { desc, id }) => self.pipelines.store_compute(desc, id),
            Err(_) if outdated => {}
            Err(error) => self.record_failure(target, error),
        }
    }

    fn evicted_module_list(&self) -> Vec<ShaderModuleId> {
        self.evicted_modules.iter().copied().collect()
    }

    // A module result from before a source changed, or a pipeline whose module was evicted meanwhile.
    fn is_outdated(&self, target: &Target, generation: u64) -> bool {
        match target {
            Target::Module(key) => generation != self.library.root_generation(&key.root),
            Target::Render(desc) => render_uses_module(desc, &self.evicted_module_list()),
            Target::Compute(desc) => self.evicted_modules.contains(&desc.shader.module),
        }
    }

    fn record_failure(&mut self, target: Target, error: BuildError) {
        self.failed.insert(target.clone());
        self.failures.push(target.failure(error));
    }

    fn forget_module_failures_if_evicted(&mut self, generation_before: u64) {
        if self.library.generation() != generation_before {
            self.failed.retain(|target| !matches!(target, Target::Module(_)));
        }
    }
}

impl Drop for BackgroundCompiler {
    // Nothing waits for running builds: they see the flag, stop early, and whatever they still deliver is destroyed
    // by the dropped channel.
    fn drop(&mut self) {
        self.worker.cancelled.store(true, Ordering::Release);
    }
}
