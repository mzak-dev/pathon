use std::collections::{BTreeMap, HashMap};

use pathon_rhi_types::{DeviceApi, ShaderModuleDesc, ShaderModuleId};

use crate::{ShaderComposer, ShaderError, ShaderFeatures};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShaderKey {
    pub root: String,
    pub features: ShaderFeatures,
}

/// Composes and creates each permutation once; one library serves one device.
/// Registering a module or root drops the cached modules, so the next request sees the new source.
#[derive(Debug, Default)]
pub struct ShaderLibrary {
    composer: ShaderComposer,
    roots: BTreeMap<String, String>,
    modules: HashMap<ShaderKey, ShaderModuleId>,
}

impl ShaderLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, path: &str, source: &str) -> Result<(), ShaderError> {
        self.composer.add_module(path, source)?;
        self.modules.clear();
        Ok(())
    }

    pub fn add_root(&mut self, name: &str, source: &str) {
        self.roots.insert(name.to_owned(), source.to_owned());
        self.modules.clear();
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
