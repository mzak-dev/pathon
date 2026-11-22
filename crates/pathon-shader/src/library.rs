use std::collections::{BTreeMap, HashMap};

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
/// different source evicts all cached modules (any root may import the module) and hands their ids back, because the
/// library has no device to destroy them with; pipelines in `PipelineCache` built from them stay until M3-06.
#[derive(Debug, Default)]
pub struct ShaderLibrary {
    composer: ShaderComposer,
    module_sources: BTreeMap<String, String>,
    roots: BTreeMap<String, String>,
    modules: HashMap<ShaderKey, ShaderModuleId>,
}

impl ShaderLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, path: &str, source: &str) -> Result<Vec<ShaderModuleId>, ShaderError> {
        self.composer.add_module(path, source)?;
        let previous = self.module_sources.insert(path.to_owned(), source.to_owned());
        Ok(self.evict_if_changed(previous.as_deref(), source))
    }

    pub fn add_root(&mut self, name: &str, source: &str) -> Vec<ShaderModuleId> {
        let previous = self.roots.insert(name.to_owned(), source.to_owned());
        self.evict_if_changed(previous.as_deref(), source)
    }

    pub fn module(&mut self, device: &dyn DeviceApi, key: &ShaderKey) -> Result<ShaderModuleId, ShaderError> {
        if let Some(id) = self.modules.get(key) {
            return Ok(*id);
        }
        let source = self
            .roots
            .get(&key.root)
            .ok_or_else(|| ShaderError::UnknownRoot { name: key.root.clone() })?;
        let wgsl = self.composer.compose_with(source, &key.root, &key.features)?;
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
    fn evict_if_changed(&mut self, previous: Option<&str>, source: &str) -> Vec<ShaderModuleId> {
        match previous {
            Some(previous) if previous != source => self.modules.drain().map(|(_, id)| id).collect(),
            _ => Vec::new(),
        }
    }
}
