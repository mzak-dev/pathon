use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::{fs, io};

use crossbeam_channel::{Receiver, Sender};
use notify::event::{AccessKind, AccessMode};
use notify::{Event, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use pathon_diag::{Diagnostic, Severity};
use pathon_rhi_types::Resource;
use pathon_shader::{BackgroundCompiler, LibrarySnapshot};

use crate::change::{Action, Batch, Change, Files, Kind};
use crate::config::HotConfig;
use crate::diagnostics;
use crate::error::HotError;
use crate::scan::WatchedDir;
use crate::worker::{Signal, WORKER_STACK_BYTES, Worker};

/// What one [`HotReloader::pump`] did.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct HotReport {
    /// Files whose new source the compiler took, as labelled in diagnostics.
    pub reloaded: Vec<String>,
    /// Files that were deleted and whose shaders or modules the compiler dropped.
    pub removed: Vec<String>,
    /// Everything worth showing: skipped files (K0001 to K0005), changes that did not compile and were not applied
    /// (the located S01xx errors and K0006), builds that failed in the background, shader warnings. Each is also
    /// logged.
    pub diagnostics: Vec<Diagnostic>,
    /// Modules and pipelines the new sources replaced, to destroy with `RecordCtx::release_later` (ADR-0022). Empty
    /// until the replacements are built and nothing is pending.
    pub retired: Vec<Resource>,
}

impl HotReport {
    /// A source was taken or dropped: whatever was built from it is built again.
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.reloaded.is_empty() || !self.removed.is_empty()
    }
}

/// Watches directories of `.wgsl` and `.wesl` files and keeps a [`BackgroundCompiler`] in step with them, without
/// ever taking away a shader that works (ADR-0040, `docs/crates/pathon-hot.md`).
#[derive(Debug)]
pub struct HotReloader {
    batches: Receiver<Batch>,
    snapshot_requested: Arc<AtomicBool>,
    snapshots: Sender<LibrarySnapshot>,
    shutdown: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    retired: Vec<Resource>,
    idle_pumps: u32,
    settle_pumps: u32,
    // A background build failed since the last change: the old pipelines may still be the ones in use.
    build_failed: bool,
}

impl HotReloader {
    /// Reads the directories once and starts watching them. The first [`Self::pump`] hands what was read to the
    /// compiler, so call it before asking the compiler for a shader.
    pub fn new(config: &HotConfig) -> Result<Self, HotError> {
        if config.dirs.is_empty() {
            return Err(HotError::NoDirectories);
        }
        let dirs = watched_dirs(config)?;
        let (signals_out, signals) = crossbeam_channel::unbounded();
        let (watcher, watcher_notice) = start_watcher(&dirs, &signals_out, config)?;
        let (batches_out, batches) = crossbeam_channel::unbounded();
        let (shutdown_out, shutdown) = crossbeam_channel::bounded(0);
        let (snapshots_out, snapshots) = crossbeam_channel::unbounded();
        let snapshot_requested = Arc::new(AtomicBool::new(false));
        let mut worker = Worker {
            dirs,
            limits: config.limits,
            debounce: config.debounce,
            signals,
            shutdown,
            snapshot_requested: Arc::clone(&snapshot_requested),
            snapshots,
            batches: batches_out.clone(),
            _watcher: watcher,
            working: BTreeMap::new(),
            accepted: Files::new(),
            reported_problems: HashMap::new(),
            reported_rejections: HashMap::new(),
            reported_duplicates: HashSet::new(),
        };
        // Nothing is built from the files yet, so the first read needs no vetting; the first pump delivers it.
        let initial = worker.read_initial(watcher_notice);
        batches_out
            .send(initial)
            .map_err(|_| HotError::Thread(io::Error::other("the batch channel closed")))?;
        let thread = thread::Builder::new()
            .name("pathon-hot".into())
            .stack_size(WORKER_STACK_BYTES)
            .spawn(move || worker.run())
            .map_err(HotError::Thread)?;
        Ok(Self {
            batches,
            snapshot_requested,
            snapshots: snapshots_out,
            shutdown: Some(shutdown_out),
            thread: Some(thread),
            retired: Vec::new(),
            idle_pumps: 0,
            settle_pumps: config.settle_pumps,
            build_failed: false,
        })
    }

    /// The per-frame call, right after `BackgroundCompiler::pump`: gives the compiler the changed sources the watcher
    /// thread has vetted, evicts what they replace, and reports. It reads no file and composes nothing.
    ///
    /// It takes the compiler's failures and warnings (`take_failures`, `take_warnings`) to report them with the rest.
    pub fn pump(&mut self, compiler: &mut BackgroundCompiler) -> HotReport {
        let mut report = HotReport::default();
        let mut evicted = Vec::new();
        let mut changed = false;
        while let Ok(batch) = self.batches.try_recv() {
            report.diagnostics.extend(batch.diagnostics);
            for change in batch.changes {
                changed = true;
                apply(compiler, change, &mut report, &mut evicted);
            }
        }
        if self.snapshot_requested.swap(false, Ordering::AcqRel) {
            // A send fails only while this reloader is being dropped.
            drop(self.snapshots.send(compiler.snapshot()));
        }
        if changed {
            self.build_failed = false;
            self.retired.extend(compiler.evict_pipelines(&evicted));
            self.retired.extend(evicted.into_iter().map(Resource::from));
        }
        for failure in compiler.take_failures() {
            self.build_failed = true;
            report.diagnostics.push(failure.diagnostic());
        }
        report.diagnostics.extend(
            compiler
                .take_warnings()
                .iter()
                .map(pathon_shader::ShaderDiagnostic::to_diagnostic),
        );
        self.settle(compiler.pending(), changed, &mut report);
        for diagnostic in &report.diagnostics {
            log_diagnostic(diagnostic);
        }
        report
    }

    /// Everything replaced so far, settled or not, for shutdown; the device is about to be destroyed anyway, so this
    /// matters only to a host that keeps the device.
    pub fn take_retired(&mut self) -> Vec<Resource> {
        std::mem::take(&mut self.retired)
    }

    // The old modules and pipelines stay alive while a draw may still use them: until every replacement is built
    // (nothing pending for `settle_pumps` pumps in a row) and no background build has failed.
    fn settle(&mut self, pending: usize, changed: bool, report: &mut HotReport) {
        if changed || pending > 0 {
            self.idle_pumps = 0;
        } else {
            self.idle_pumps = self.idle_pumps.saturating_add(1);
        }
        if self.idle_pumps >= self.settle_pumps && !self.build_failed {
            report.retired = std::mem::take(&mut self.retired);
        }
    }
}

impl Drop for HotReloader {
    fn drop(&mut self) {
        self.shutdown = None;
        if let Some(thread) = self.thread.take() {
            // The thread only ends early by panicking, which it has already reported.
            drop(thread.join());
        }
    }
}

fn apply(
    compiler: &mut BackgroundCompiler,
    change: Change,
    report: &mut HotReport,
    evicted: &mut Vec<pathon_rhi_types::ShaderModuleId>,
) {
    let name = change.key.name.as_str();
    let removed = matches!(change.action, Action::Remove);
    match (change.key.kind, &change.action) {
        (Kind::Module, Action::Upsert(source)) => match compiler.add_module_file(name, source, &change.label) {
            Ok(ids) => evicted.extend(ids),
            Err(error) => {
                report
                    .diagnostics
                    .push(diagnostics::unusable_name(&change.label, &error.to_string()));
                return;
            }
        },
        (Kind::Root, Action::Upsert(source)) => evicted.extend(compiler.add_root_file(name, source, &change.label)),
        (Kind::Module, Action::Remove) => evicted.extend(compiler.remove_module(name)),
        (Kind::Root, Action::Remove) => evicted.extend(compiler.remove_root(name)),
    }
    if removed {
        log::info!("hot reload: `{}` was deleted, its shader is dropped", change.label);
        report.removed.push(change.label);
    } else {
        log::info!("hot reload: `{}` reloaded", change.label);
        report.reloaded.push(change.label);
    }
}

fn log_diagnostic(diagnostic: &Diagnostic) {
    match diagnostic.severity {
        Severity::Error => log::error!("{diagnostic}"),
        Severity::Warning => log::warn!("{diagnostic}"),
        Severity::Note => log::info!("{diagnostic}"),
    }
}

fn watched_dirs(config: &HotConfig) -> Result<Vec<WatchedDir>, HotError> {
    config
        .dirs
        .iter()
        .map(|(configured, role)| {
            let canonical = fs::canonicalize(configured).map_err(|source| HotError::DirectoryUnavailable {
                path: configured.clone(),
                source,
            })?;
            Ok(WatchedDir {
                configured: configured.clone(),
                canonical,
                role: *role,
            })
        })
        .collect()
}

fn handler(signals: Sender<Signal>) -> impl FnMut(notify::Result<Event>) + Send + 'static {
    move |result| {
        let signal = match result {
            Ok(event) if is_relevant(&event) => Signal::Changed,
            Ok(_) => return,
            Err(error) => Signal::Problem(format!("the file watcher reported: {error}")),
        };
        // A send fails only while the reloader is being dropped.
        drop(signals.send(signal));
    }
}

// Reading a file raises access events on some systems; only the end of a write counts among them.
fn is_relevant(event: &Event) -> bool {
    match event.kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

fn start_watcher(
    dirs: &[WatchedDir],
    signals: &Sender<Signal>,
    config: &HotConfig,
) -> Result<(Box<dyn Watcher + Send>, Option<Diagnostic>), HotError> {
    let native = RecommendedWatcher::new(handler(signals.clone()), notify::Config::default())
        .and_then(|watcher| watch_all(watcher, dirs));
    match native {
        Ok(watcher) => Ok((Box::new(watcher), None)),
        Err(error) => {
            let notice = diagnostics::watcher_problem(&format!(
                "the system's file notification is unavailable ({error}); the directories are polled every {:?}",
                config.poll_interval
            ));
            let poll_config = notify::Config::default().with_poll_interval(config.poll_interval);
            let watcher = PollWatcher::new(handler(signals.clone()), poll_config).map_err(HotError::Watcher)?;
            let watcher = watch_all(watcher, dirs).map_err(HotError::Watcher)?;
            Ok((Box::new(watcher), Some(notice)))
        }
    }
}

fn watch_all<W: Watcher>(mut watcher: W, dirs: &[WatchedDir]) -> notify::Result<W> {
    for dir in dirs {
        watcher.watch(&dir.canonical, RecursiveMode::Recursive)?;
    }
    Ok(watcher)
}
