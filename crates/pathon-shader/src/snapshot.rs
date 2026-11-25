use std::collections::BTreeMap;
use std::sync::Arc;

use crate::library::Root;
use crate::{ShaderComposer, ShaderError, ShaderKey};

/// A copy of what a library composes shaders from, plus the permutations it has built, so a change can be tried out
/// (on another thread) before the library takes it: a source that no longer compiles never reaches the library, and the
/// permutations already built stay in use (ADR-0040).
#[derive(Clone, Debug)]
pub struct LibrarySnapshot {
    composer: Arc<ShaderComposer>,
    roots: BTreeMap<String, Root>,
    keys: Vec<ShaderKey>,
}

impl LibrarySnapshot {
    pub(crate) fn new(composer: Arc<ShaderComposer>, roots: BTreeMap<String, Root>, keys: Vec<ShaderKey>) -> Self {
        Self { composer, roots, keys }
    }

    /// The permutations the library had built when the snapshot was taken.
    #[must_use]
    pub fn keys(&self) -> &[ShaderKey] {
        &self.keys
    }

    pub fn add_module_file(&mut self, path: &str, source: &str, file: &str) -> Result<(), ShaderError> {
        Arc::make_mut(&mut self.composer).add_module_file(path, source, file)
    }

    pub fn remove_module(&mut self, path: &str) {
        Arc::make_mut(&mut self.composer).remove_module(path);
    }

    pub fn add_root_file(&mut self, name: &str, source: &str, file: &str) {
        self.roots.insert(name.to_owned(), Root::new(source, file));
    }

    pub fn remove_root(&mut self, name: &str) {
        self.roots.remove(name);
    }

    /// Composes and validates every permutation the library had built against the changed sources and returns the
    /// ones that no longer compile. A permutation whose shader was removed is not an error.
    #[must_use]
    pub fn check(&self) -> Vec<(ShaderKey, ShaderError)> {
        let mut failures = Vec::new();
        for key in &self.keys {
            let Some(root) = self.roots.get(&key.root) else {
                continue;
            };
            if let Err(error) = self.composer.compose_checked(&root.source, &root.file, &key.features) {
                failures.push((key.clone(), error));
            }
        }
        failures
    }
}
