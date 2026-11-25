use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use pathon_rhi_types::{DeviceApi, ShaderModuleDesc, ShaderModuleId};

use crate::references::package_paths;
use crate::snapshot::LibrarySnapshot;
use crate::{Composed, ShaderComposer, ShaderDiagnostic, ShaderError, ShaderFeatures};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShaderKey {
    pub root: String,
    pub features: ShaderFeatures,
}

/// Composes and creates each permutation once; one library serves one device.
///
/// Registering a new module path or root name leaves every cached module alone. Replacing a module with different
/// source evicts the permutations of the roots that import it, directly or through other modules (found by the
/// `package::` paths in the sources; any `super::` import makes it evict everything), replacing a root evicts its own,
/// and both hand the module ids back, because the library has no device to destroy them with; pipelines built from them
/// stay in `PipelineCache` until `PipelineCache::evict_modules` is given those ids (`pathon-hot` does that, ADR-0040).
#[derive(Debug, Default)]
pub struct ShaderLibrary {
    composer: Arc<ShaderComposer>,
    module_sources: BTreeMap<String, String>,
    roots: BTreeMap<String, Root>,
    composed: HashMap<ShaderKey, Arc<str>>,
    modules: HashMap<ShaderKey, ShaderModuleId>,
    warnings: Vec<ShaderDiagnostic>,
    generation: u64,
    root_generations: HashMap<String, u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Root {
    pub(crate) source: Arc<str>,
    pub(crate) file: Arc<str>,
}

impl Root {
    pub(crate) fn new(source: &str, file: &str) -> Self {
        Self {
            source: Arc::from(source),
            file: Arc::from(file),
        }
    }
}

/// Everything composing one permutation needs, detached from the library so it can run on another thread.
#[derive(Debug)]
pub(crate) struct ComposeJob {
    composer: Arc<ShaderComposer>,
    source: Arc<str>,
    file: Arc<str>,
    key: ShaderKey,
}

impl ComposeJob {
    pub(crate) fn run(&self) -> Result<Composed, ShaderError> {
        self.composer
            .compose_checked(&self.source, &self.file, &self.key.features)
    }
}

impl ShaderLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, path: &str, source: &str) -> Result<Vec<ShaderModuleId>, ShaderError> {
        Arc::make_mut(&mut self.composer).add_module(path, source)?;
        Ok(self.module_registered(path, source))
    }

    /// Like [`Self::add_module`], with the file the source came from, which diagnostics show.
    pub fn add_module_file(
        &mut self,
        path: &str,
        source: &str,
        file: &str,
    ) -> Result<Vec<ShaderModuleId>, ShaderError> {
        Arc::make_mut(&mut self.composer).add_module_file(path, source, file)?;
        Ok(self.module_registered(path, source))
    }

    pub fn add_root(&mut self, name: &str, source: &str) -> Vec<ShaderModuleId> {
        self.add_root_file(name, source, name)
    }

    /// Like [`Self::add_root`], with the file the source came from, which diagnostics show instead of the root name.
    pub fn add_root_file(&mut self, name: &str, source: &str, file: &str) -> Vec<ShaderModuleId> {
        let previous = self.roots.insert(name.to_owned(), Root::new(source, file));
        match previous {
            Some(previous) if *previous.source != *source => self.evict_roots(&HashSet::from([name.to_owned()])),
            _ => Vec::new(),
        }
    }

    /// Forgets a module whose file was deleted; evicts like replacing it does.
    pub fn remove_module(&mut self, path: &str) -> Vec<ShaderModuleId> {
        Arc::make_mut(&mut self.composer).remove_module(path);
        let dependents = self.roots_using(path);
        match self.module_sources.remove(path) {
            Some(_) => self.evict_dependents(dependents),
            None => Vec::new(),
        }
    }

    /// Forgets a shader root whose file was deleted; its cached permutations go with it.
    pub fn remove_root(&mut self, name: &str) -> Vec<ShaderModuleId> {
        match self.roots.remove(name) {
            Some(_) => self.evict_roots(&HashSet::from([name.to_owned()])),
            None => Vec::new(),
        }
    }

    /// The composed WGSL of a permutation, composed once per key; reflection reads the same text the module is made of.
    pub fn wgsl(&mut self, key: &ShaderKey) -> Result<Arc<str>, ShaderError> {
        if let Some(wgsl) = self.composed.get(key) {
            return Ok(Arc::clone(wgsl));
        }
        let composed = self.compose_job(key)?.run()?;
        self.warnings.extend(composed.warnings);
        self.composed.insert(key.clone(), Arc::clone(&composed.wgsl));
        Ok(composed.wgsl)
    }

    pub fn module(&mut self, device: &dyn DeviceApi, key: &ShaderKey) -> Result<ShaderModuleId, ShaderError> {
        if let Some(id) = self.modules.get(key) {
            return Ok(*id);
        }
        let wgsl = self.wgsl(key)?;
        let id = device
            .create_shader_module(&ShaderModuleDesc {
                label: "permutation",
                wgsl: &wgsl,
            })
            .map_err(ShaderError::Rhi)?;
        self.modules.insert(key.clone(), id);
        Ok(id)
    }

    /// What the library composes from and has built, to try a change on another thread before applying it.
    #[must_use]
    pub fn snapshot(&self) -> LibrarySnapshot {
        let keys: HashSet<&ShaderKey> = self.modules.keys().chain(self.composed.keys()).collect();
        let keys = keys.into_iter().cloned().collect();
        LibrarySnapshot::new(Arc::clone(&self.composer), self.roots.clone(), keys)
    }

    /// Warnings (S0106) found while composing since the last call; log them with `to_diagnostic`.
    pub fn take_warnings(&mut self) -> Vec<ShaderDiagnostic> {
        std::mem::take(&mut self.warnings)
    }
}

impl ShaderLibrary {
    // Bumped whenever cached modules are evicted, so work started against older sources can be told apart.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    // Bumped when this root's cached modules are evicted, so a build of it from before can be told apart from one
    // that is still current.
    pub(crate) fn root_generation(&self, root: &str) -> u64 {
        self.root_generations.get(root).copied().unwrap_or(0)
    }

    pub(crate) fn compose_job(&self, key: &ShaderKey) -> Result<ComposeJob, ShaderError> {
        let root = self
            .roots
            .get(&key.root)
            .ok_or_else(|| ShaderError::UnknownRoot { name: key.root.clone() })?;
        Ok(ComposeJob {
            composer: Arc::clone(&self.composer),
            source: Arc::clone(&root.source),
            file: Arc::clone(&root.file),
            key: key.clone(),
        })
    }

    pub(crate) fn composed(&self, key: &ShaderKey) -> Option<Arc<str>> {
        self.composed.get(key).map(Arc::clone)
    }

    /// The WGSL a created module was made from, to check pipeline descriptions against it.
    pub(crate) fn wgsl_of_module(&self, module: ShaderModuleId) -> Option<Arc<str>> {
        let key = self.modules.iter().find(|(_, id)| **id == module).map(|(key, _)| key)?;
        self.composed(key)
    }

    pub(crate) fn cached_module(&self, key: &ShaderKey) -> Option<ShaderModuleId> {
        self.modules.get(key).copied()
    }

    pub(crate) fn store(&mut self, key: ShaderKey, composed: Composed, module: ShaderModuleId) {
        self.warnings.extend(composed.warnings);
        self.composed.insert(key.clone(), composed.wgsl);
        self.modules.insert(key, module);
    }

    pub(crate) fn drain_modules(&mut self) -> Vec<ShaderModuleId> {
        self.modules.drain().map(|(_, id)| id).collect()
    }

    fn module_registered(&mut self, path: &str, source: &str) -> Vec<ShaderModuleId> {
        let previous = self.module_sources.insert(path.to_owned(), source.to_owned());
        match previous {
            Some(previous) if previous != source => {
                let dependents = self.roots_using(path);
                self.evict_dependents(dependents)
            }
            _ => Vec::new(),
        }
    }

    // The roots that reach `module` through `package::` paths, or `None` when the sources use relative imports and
    // the answer cannot be read off the text.
    fn roots_using(&self, module: &str) -> Option<HashSet<String>> {
        if self.module_sources.values().any(|source| source.contains("super::")) {
            return None;
        }
        let edges: HashMap<&str, Vec<&str>> = self
            .module_sources
            .iter()
            .map(|(name, source)| (name.as_str(), self.modules_referenced_by(source)))
            .collect();
        let mut users = HashSet::new();
        for (name, root) in &self.roots {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut pending = self.modules_referenced_by(&root.source);
            while let Some(next) = pending.pop() {
                if next == module {
                    users.insert(name.clone());
                    break;
                }
                if seen.insert(next) {
                    pending.extend(edges.get(next).into_iter().flatten().copied());
                }
            }
        }
        Some(users)
    }

    fn modules_referenced_by(&self, source: &str) -> Vec<&str> {
        let mut found = Vec::new();
        for segments in package_paths(source) {
            let module = (1..=segments.len()).rev().find_map(|length| {
                self.module_sources
                    .get_key_value(segments[..length].join("::").as_str())
            });
            if let Some((name, _)) = module {
                found.push(name.as_str());
            }
        }
        found
    }

    fn evict_dependents(&mut self, dependents: Option<HashSet<String>>) -> Vec<ShaderModuleId> {
        match dependents {
            Some(roots) => self.evict_roots(&roots),
            None => self.evict_all(),
        }
    }

    fn evict_roots(&mut self, roots: &HashSet<String>) -> Vec<ShaderModuleId> {
        self.generation += 1;
        for root in roots {
            *self.root_generations.entry(root.clone()).or_insert(0) += 1;
        }
        self.composed.retain(|key, _| !roots.contains(&key.root));
        let mut evicted = Vec::new();
        self.modules.retain(|key, id| {
            let evict = roots.contains(&key.root);
            if evict {
                evicted.push(*id);
            }
            !evict
        });
        evicted
    }

    fn evict_all(&mut self) -> Vec<ShaderModuleId> {
        self.generation += 1;
        for root in self.roots.keys() {
            *self.root_generations.entry(root.clone()).or_insert(0) += 1;
        }
        self.composed.clear();
        self.drain_modules()
    }
}
