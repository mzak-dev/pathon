use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use pathon_rhi_types::{DeviceApi, ShaderModuleDesc, ShaderModuleId};

use crate::{ShaderComposer, ShaderError, ShaderFeatures};

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
    roots: BTreeMap<String, Arc<str>>,
    composed: HashMap<ShaderKey, Arc<str>>,
    modules: HashMap<ShaderKey, ShaderModuleId>,
    generation: u64,
}

/// Everything composing one permutation needs, detached from the library so it can run on another thread.
#[derive(Debug)]
pub(crate) struct ComposeJob {
    composer: Arc<ShaderComposer>,
    source: Arc<str>,
    key: ShaderKey,
}

impl ComposeJob {
    pub(crate) fn run(&self) -> Result<Arc<str>, ShaderError> {
        self.composer
            .compose_with(&self.source, &self.key.root, &self.key.features)
            .map(Arc::from)
    }
}

impl ShaderLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, path: &str, source: &str) -> Result<Vec<ShaderModuleId>, ShaderError> {
        Arc::make_mut(&mut self.composer).add_module(path, source)?;
        let previous = self.module_sources.insert(path.to_owned(), source.to_owned());
        Ok(self.evict_if_changed(previous.as_deref(), source))
    }

    pub fn add_root(&mut self, name: &str, source: &str) -> Vec<ShaderModuleId> {
        let previous = self.roots.insert(name.to_owned(), Arc::from(source));
        self.evict_if_changed(previous.as_deref(), source)
    }

    /// The composed WGSL of a permutation, composed once per key; reflection reads the same text the module is made of.
    pub fn wgsl(&mut self, key: &ShaderKey) -> Result<Arc<str>, ShaderError> {
        if let Some(wgsl) = self.composed.get(key) {
            return Ok(Arc::clone(wgsl));
        }
        let wgsl = self.compose_job(key)?.run()?;
        self.composed.insert(key.clone(), Arc::clone(&wgsl));
        Ok(wgsl)
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
}

impl ShaderLibrary {
    // Bumped whenever cached modules are evicted, so work started against older sources can be told apart.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn compose_job(&self, key: &ShaderKey) -> Result<ComposeJob, ShaderError> {
        let source = self
            .roots
            .get(&key.root)
            .ok_or_else(|| ShaderError::UnknownRoot { name: key.root.clone() })?;
        Ok(ComposeJob {
            composer: Arc::clone(&self.composer),
            source: Arc::clone(source),
            key: key.clone(),
        })
    }

    pub(crate) fn composed(&self, key: &ShaderKey) -> Option<Arc<str>> {
        self.composed.get(key).map(Arc::clone)
    }

    pub(crate) fn cached_module(&self, key: &ShaderKey) -> Option<ShaderModuleId> {
        self.modules.get(key).copied()
    }

    pub(crate) fn store(&mut self, key: ShaderKey, wgsl: Arc<str>, module: ShaderModuleId) {
        self.composed.insert(key.clone(), wgsl);
        self.modules.insert(key, module);
    }

    pub(crate) fn drain_modules(&mut self) -> Vec<ShaderModuleId> {
        self.modules.drain().map(|(_, id)| id).collect()
    }

    fn evict_if_changed(&mut self, previous: Option<&str>, source: &str) -> Vec<ShaderModuleId> {
        match previous {
            Some(previous) if previous != source => {
                self.generation += 1;
                self.composed.clear();
                self.drain_modules()
            }
            _ => Vec::new(),
        }
    }
}
