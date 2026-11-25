use std::collections::BTreeMap;
use std::sync::Arc;

use pathon_diag::Diagnostic;

use crate::config::DirRole;

/// A module is registered before the roots that import it, and removed after them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Kind {
    Module,
    Root,
}

impl From<DirRole> for Kind {
    fn from(role: DirRole) -> Self {
        match role {
            DirRole::Shaders => Self::Root,
            DirRole::Modules => Self::Module,
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct EntryKey {
    pub(crate) kind: Kind,
    pub(crate) name: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Entry {
    pub(crate) label: String,
    pub(crate) source: Arc<str>,
}

pub(crate) type Files = BTreeMap<EntryKey, Entry>;

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    Upsert(Arc<str>),
    Remove,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Change {
    pub(crate) key: EntryKey,
    pub(crate) label: String,
    pub(crate) action: Action,
}

impl Change {
    // Modules first, so a root that imports a new module never meets a library without it.
    fn order(&self) -> (bool, Kind) {
        (matches!(self.action, Action::Remove), self.key.kind)
    }
}

/// What differs between the files the library has accepted and the ones on disk now.
pub(crate) fn changes_between(accepted: &Files, working: &Files) -> Vec<Change> {
    let mut changes = Vec::new();
    for (key, entry) in working {
        if accepted.get(key) != Some(entry) {
            changes.push(Change {
                key: key.clone(),
                label: entry.label.clone(),
                action: Action::Upsert(Arc::clone(&entry.source)),
            });
        }
    }
    for (key, entry) in accepted {
        if !working.contains_key(key) {
            changes.push(Change {
                key: key.clone(),
                label: entry.label.clone(),
                action: Action::Remove,
            });
        }
    }
    changes.sort_by_key(Change::order);
    changes
}

pub(crate) fn apply_to(files: &mut Files, change: &Change) {
    match &change.action {
        Action::Upsert(source) => {
            files.insert(
                change.key.clone(),
                Entry {
                    label: change.label.clone(),
                    source: Arc::clone(source),
                },
            );
        }
        Action::Remove => {
            files.remove(&change.key);
        }
    }
}

/// What the worker found, for the frame thread to apply.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    pub(crate) changes: Vec<Change>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}
