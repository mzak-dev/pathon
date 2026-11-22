use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use pathon_rhi_types::{DeviceApi, ShaderModuleDesc, ShaderModuleId};

use crate::{Composed, ShaderComposer, ShaderDiagnostic, ShaderError, ShaderFeatures};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShaderKey {
    pub root: String,
    pub features: ShaderFeatures,
}

/// Composes and creates each permutation once; one library serves one device.
///
/// Registering a new module path or root name leaves every cached module alone. Replacing a module or root with
/// different source evicts all cached modules and composed sources (any root may import the module) and hands the
/// module ids back, because the library has no device to destroy them with; pipelines in `PipelineCache` built from
/// them stay until M3-06.
#[derive(Debug, Default)]
pub struct ShaderLibrary {
    composer: Arc<ShaderComposer>,
    module_sources: BTreeMap<String, String>,
    roots: BTreeMap<String, Root>,
    composed: HashMap<ShaderKey, Arc<str>>,
    modules: HashMap<ShaderKey, ShaderModuleId>,
    warnings: Vec<ShaderDiagnostic>,
    generation: u64,
}

#[derive(Debug)]
struct Root {
    source: Arc<str>,
    file: Arc<str>,
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
        let root = Root {
            source: Arc::from(source),
            file: Arc::from(file),
        };
        let previous = self.roots.insert(name.to_owned(), root);
        self.evict_if_changed(previous.as_ref().map(|root| &*root.source), source)
    }

    /// Forgets a module whose file was deleted; evicts like replacing it does.
    pub fn remove_module(&mut self, path: &str) -> Vec<ShaderModuleId> {
        Arc::make_mut(&mut self.composer).remove_module(path);
        match self.module_sources.remove(path) {
            Some(_) => self.evict_all(),
            None => Vec::new(),
        }
    }

    /// Forgets a shader root whose file was deleted; its cached permutations go with it.
    pub fn remove_root(&mut self, name: &str) -> Vec<ShaderModuleId> {
        match self.roots.remove(name) {
            Some(_) => self.evict_all(),
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
        self.evict_if_changed(previous.as_deref(), source)
    }

    fn evict_if_changed(&mut self, previous: Option<&str>, source: &str) -> Vec<ShaderModuleId> {
        match previous {
            Some(previous) if previous != source => self.evict_all(),
            _ => Vec::new(),
        }
    }

    fn evict_all(&mut self) -> Vec<ShaderModuleId> {
        self.generation += 1;
        self.composed.clear();
        self.drain_modules()
    }
}
