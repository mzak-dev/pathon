// The thread that reads the files. It owns everything slow (reading, measuring, composing a changed shader) so the frame
// thread only applies what comes out (ADR-0040). Composition runs here because wesl recurses deeply: this thread has a
// large stack, and `guard` keeps inputs well below what even that could not hold.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, select};
use notify::Watcher;
use pathon_diag::Diagnostic;
use pathon_shader::LibrarySnapshot;

use crate::change::{Action, Batch, Change, Entry, EntryKey, Files, Kind, apply_to, changes_between};
use crate::config::SourceLimits;
use crate::scan::{self, WatchedDir};
use crate::{diagnostics, guard};

pub(crate) const WORKER_STACK_BYTES: usize = 64 << 20;
// A checkout that rewrites hundreds of files is taken whole or not at all; trying each file alone would cost a check per file.
const MAX_CHANGES_TRIED_ONE_BY_ONE: usize = 32;

#[derive(Debug)]
pub(crate) enum Signal {
    Changed,
    Problem(String),
}

type FileId = (usize, PathBuf);

pub(crate) struct Worker {
    pub(crate) dirs: Vec<WatchedDir>,
    pub(crate) limits: SourceLimits,
    pub(crate) debounce: Duration,
    pub(crate) signals: Receiver<Signal>,
    pub(crate) shutdown: Receiver<()>,
    pub(crate) snapshot_requested: Arc<AtomicBool>,
    pub(crate) snapshots: Receiver<LibrarySnapshot>,
    pub(crate) batches: Sender<Batch>,
    // Held so the watch lasts as long as the thread.
    pub(crate) _watcher: Box<dyn Watcher + Send>,
    /// What is on disk now, file by file.
    pub(crate) working: BTreeMap<FileId, (EntryKey, Entry)>,
    /// What the library has been given.
    pub(crate) accepted: Files,
    pub(crate) reported_problems: HashMap<FileId, Diagnostic>,
    pub(crate) reported_rejections: HashMap<EntryKey, Vec<Diagnostic>>,
    pub(crate) reported_duplicates: HashSet<String>,
}

enum Vetted {
    Accepted(Vec<Change>),
    Partly {
        accepted: Vec<Change>,
        rejected: Vec<(Change, Vec<Diagnostic>)>,
    },
}

impl Worker {
    pub(crate) fn run(mut self) {
        while self.wait_for_quiet() {
            if !self.process() {
                return;
            }
        }
    }

    // Blocks for the first signal, then until none arrived for `debounce`. False once the reloader is gone.
    fn wait_for_quiet(&mut self) -> bool {
        let mut problems = Vec::new();
        select! {
            recv(self.signals) -> signal => match signal {
                Ok(signal) => note(signal, &mut problems),
                Err(_) => return false,
            },
            recv(self.shutdown) -> _ => return false,
        }
        loop {
            match self.signals.recv_timeout(self.debounce) {
                Ok(signal) => note(signal, &mut problems),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
        if !problems.is_empty() {
            let diagnostics = problems.iter().map(|problem| diagnostics::watcher_problem(problem)).collect();
            self.send(Batch {
                changes: Vec::new(),
                diagnostics,
            });
        }
        true
    }

    // What is on disk when hot reload starts is accepted without vetting: nothing is built from it yet, so there is no
    // working version to protect.
    pub(crate) fn read_initial(&mut self, watcher_notice: Option<Diagnostic>) -> Batch {
        let mut diagnostics: Vec<Diagnostic> = watcher_notice.into_iter().collect();
        diagnostics.extend(self.rescan());
        let working = self.working_files(&mut diagnostics);
        let (accepted, rejected) = match self.vet(None, changes_between(&self.accepted, &working)) {
            Vetted::Accepted(changes) => (changes, Vec::new()),
            Vetted::Partly { accepted, rejected } => (accepted, rejected),
        };
        for change in &accepted {
            apply_to(&mut self.accepted, change);
        }
        self.report_rejections(rejected, &mut diagnostics);
        Batch {
            changes: accepted,
            diagnostics,
        }
    }

    fn send(&self, batch: Batch) -> bool {
        self.batches.send(batch).is_ok()
    }

    // False when the reloader went away while this waited.
    fn process(&mut self) -> bool {
        let mut diagnostics = self.rescan();
        let working = self.working_files(&mut diagnostics);
        let pending = changes_between(&self.accepted, &working);
        if pending.is_empty() {
            self.reported_rejections.clear();
            return self.send_if_any(Vec::new(), diagnostics);
        }
        let Some(snapshot) = self.fetch_snapshot() else {
            return false;
        };
        let (accepted, rejected) = match self.vet(Some(&snapshot), pending) {
            Vetted::Accepted(changes) => (changes, Vec::new()),
            Vetted::Partly { accepted, rejected } => (accepted, rejected),
        };
        for change in &accepted {
            apply_to(&mut self.accepted, change);
            self.reported_rejections.remove(&change.key);
        }
        self.report_rejections(rejected, &mut diagnostics);
        self.send_if_any(accepted, diagnostics)
    }

    // A change that stays rejected is reported once, not on every file event that retries it.
    fn report_rejections(&mut self, rejected: Vec<(Change, Vec<Diagnostic>)>, diagnostics: &mut Vec<Diagnostic>) {
        for (change, problems) in rejected {
            if self.reported_rejections.get(&change.key) != Some(&problems) {
                diagnostics.extend(problems.iter().cloned());
                diagnostics.push(self::diagnostics::reload_rejected(&change.label));
                self.reported_rejections.insert(change.key, problems);
            }
        }
    }

    fn send_if_any(&self, changes: Vec<Change>, diagnostics: Vec<Diagnostic>) -> bool {
        if changes.is_empty() && diagnostics.is_empty() {
            return true;
        }
        self.send(Batch { changes, diagnostics })
    }

    // Files that cannot be accepted keep their last good content, so a bad edit never reads as a deletion.
    fn rescan(&mut self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut seen = HashSet::new();
        for (index, dir) in self.dirs.iter().enumerate() {
            for scanned in scan::scan(dir, &self.limits) {
                let id = (index, scanned.relative);
                seen.insert(id.clone());
                match scanned.outcome {
                    Ok(file) => {
                        self.working.insert(id.clone(), file);
                        self.reported_problems.remove(&id);
                    }
                    Err(problem) => {
                        if self.reported_problems.get(&id) != Some(&problem) {
                            diagnostics.push(problem.clone());
                            self.reported_problems.insert(id, problem);
                        }
                    }
                }
            }
        }
        self.working.retain(|id, _| seen.contains(id));
        self.reported_problems.retain(|id, _| seen.contains(id));
        diagnostics
    }

    // Two files that would register the same name (`a.wgsl` and `a.wesl`): the first in path order wins.
    fn working_files(&mut self, diagnostics: &mut Vec<Diagnostic>) -> Files {
        let mut files = Files::new();
        for (key, entry) in self.working.values() {
            if files.contains_key(key) {
                if self.reported_duplicates.insert(entry.label.clone()) {
                    diagnostics.push(self::diagnostics::unusable_name(
                        &entry.label,
                        &format!("another file already provides `{}`", key.name),
                    ));
                }
            } else {
                files.insert(key.clone(), entry.clone());
            }
        }
        files
    }

    fn fetch_snapshot(&self) -> Option<LibrarySnapshot> {
        self.snapshot_requested.store(true, Ordering::Release);
        select! {
            recv(self.snapshots) -> snapshot => snapshot.ok(),
            recv(self.shutdown) -> _ => None,
        }
    }

    // Everything at once when it compiles, else change by change in rounds: one broken file must not hold back the
    // others, and a module and the root that needs it may only compile together.
    fn vet(&self, snapshot: Option<&LibrarySnapshot>, pending: Vec<Change>) -> Vetted {
        let Err(problems) = self.check(snapshot, &pending) else {
            return Vetted::Accepted(pending);
        };
        if pending.len() > MAX_CHANGES_TRIED_ONE_BY_ONE {
            let rejected = pending.into_iter().map(|change| (change, problems.clone())).collect();
            return Vetted::Partly {
                accepted: Vec::new(),
                rejected,
            };
        }
        let mut accepted: Vec<Change> = Vec::new();
        let mut waiting = pending;
        loop {
            let mut still_waiting = Vec::new();
            let mut progressed = false;
            for change in waiting {
                let mut attempt = accepted.clone();
                attempt.push(change.clone());
                if self.check(snapshot, &attempt).is_ok() {
                    accepted.push(change);
                    progressed = true;
                } else {
                    still_waiting.push(change);
                }
            }
            waiting = still_waiting;
            if !progressed || waiting.is_empty() {
                break;
            }
        }
        let rejected = waiting
            .into_iter()
            .map(|change| {
                let mut attempt = accepted.clone();
                attempt.push(change.clone());
                let problems = self.check(snapshot, &attempt).err().unwrap_or_default();
                (change, problems)
            })
            .collect();
        Vetted::Partly { accepted, rejected }
    }

    // The sources the library would hold after `changes` must nest and import within the limits, and every
    // permutation the library has built must still compile.
    fn check(&self, snapshot: Option<&LibrarySnapshot>, changes: &[Change]) -> Result<(), Vec<Diagnostic>> {
        let mut candidate = self.accepted.clone();
        for change in changes {
            apply_to(&mut candidate, change);
        }
        self.check_import_depth(&candidate)?;
        let Some(snapshot) = snapshot else {
            return Ok(());
        };
        let mut snapshot = snapshot.clone();
        for change in changes {
            apply_to_snapshot(&mut snapshot, change)?;
        }
        let mut problems: Vec<Diagnostic> = Vec::new();
        for (_, error) in snapshot.check() {
            let problem = error.diagnostic().map_or_else(
                || Diagnostic::error("S0111", error.to_string()),
                pathon_shader::ShaderDiagnostic::to_diagnostic,
            );
            if !problems.contains(&problem) {
                problems.push(problem);
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }

    fn check_import_depth(&self, candidate: &Files) -> Result<(), Vec<Diagnostic>> {
        let modules: BTreeMap<&str, &str> = candidate
            .iter()
            .filter(|(key, _)| key.kind == Kind::Module)
            .map(|(key, entry)| (key.name.as_str(), &*entry.source))
            .collect();
        let roots = candidate
            .iter()
            .filter(|(key, _)| key.kind == Kind::Root)
            .map(|(_, entry)| &*entry.source);
        let (depth, chain) = guard::longest_import_chain(&modules, roots);
        if depth > self.limits.max_import_depth {
            let shown = chain.iter().take(4).copied().collect::<Vec<_>>().join(" -> ");
            let chain = format!("{shown} -> ... ({} modules)", chain.len());
            return Err(vec![diagnostics::too_deep_imports(
                &chain,
                depth,
                self.limits.max_import_depth,
            )]);
        }
        Ok(())
    }
}

fn note(signal: Signal, problems: &mut Vec<String>) {
    if let Signal::Problem(problem) = signal {
        problems.push(problem);
    }
}

fn apply_to_snapshot(snapshot: &mut LibrarySnapshot, change: &Change) -> Result<(), Vec<Diagnostic>> {
    let name = change.key.name.as_str();
    match (change.key.kind, &change.action) {
        (Kind::Module, Action::Upsert(source)) => snapshot
            .add_module_file(name, source, &change.label)
            .map_err(|error| vec![diagnostics::unusable_name(&change.label, &error.to_string())]),
        (Kind::Root, Action::Upsert(source)) => {
            snapshot.add_root_file(name, source, &change.label);
            Ok(())
        }
        (Kind::Module, Action::Remove) => {
            snapshot.remove_module(name);
            Ok(())
        }
        (Kind::Root, Action::Remove) => {
            snapshot.remove_root(name);
            Ok(())
        }
    }
}
