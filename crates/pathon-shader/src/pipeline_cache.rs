use std::borrow::Cow;
use std::collections::HashMap;

use pathon_rhi_types::{
    ComputePipelineDesc, ComputePipelineId, DeviceApi, RenderPipelineDesc, RenderPipelineId, Resource, RhiError,
    ShaderModuleId, ShaderStage,
};

/// Remembers pipelines by their description; one cache serves one device.
#[derive(Debug, Default)]
pub struct PipelineCache {
    render: HashMap<RenderPipelineDesc, RenderPipelineId>,
    compute: HashMap<ComputePipelineDesc, ComputePipelineId>,
}

impl PipelineCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render(&mut self, device: &dyn DeviceApi, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError> {
        let key = normalized_render(desc);
        if let Some(id) = self.render.get(key.as_ref()) {
            return Ok(*id);
        }
        let id = device.create_render_pipeline(&key)?;
        self.render.insert(key.into_owned(), id);
        Ok(id)
    }

    pub fn compute(
        &mut self,
        device: &dyn DeviceApi,
        desc: &ComputePipelineDesc,
    ) -> Result<ComputePipelineId, RhiError> {
        let key = normalized_compute(desc);
        if let Some(id) = self.compute.get(key.as_ref()) {
            return Ok(*id);
        }
        let id = device.create_compute_pipeline(&key)?;
        self.compute.insert(key.into_owned(), id);
        Ok(id)
    }

    /// Forgets every pipeline built from one of `modules` (the ids `ShaderLibrary::add_module` and friends hand back
    /// when a source changes) and returns them for the caller to destroy once the GPU is done with them.
    pub fn evict_modules(&mut self, modules: &[ShaderModuleId]) -> Vec<Resource> {
        if modules.is_empty() {
            return Vec::new();
        }
        let mut evicted = Vec::new();
        self.render.retain(|desc, id| {
            let uses_module = render_uses_module(desc, modules);
            if uses_module {
                evicted.push(Resource::from(*id));
            }
            !uses_module
        });
        self.compute.retain(|desc, id| {
            let uses_module = modules.contains(&desc.shader.module);
            if uses_module {
                evicted.push(Resource::from(*id));
            }
            !uses_module
        });
        evicted
    }
}

impl PipelineCache {
    // The keys below are already normalized, so a lookup never re-sorts and a background build stores what it built.
    pub(crate) fn cached_render(&self, key: &RenderPipelineDesc) -> Option<RenderPipelineId> {
        self.render.get(key).copied()
    }

    pub(crate) fn cached_compute(&self, key: &ComputePipelineDesc) -> Option<ComputePipelineId> {
        self.compute.get(key).copied()
    }

    pub(crate) fn store_render(&mut self, key: RenderPipelineDesc, id: RenderPipelineId) {
        self.render.insert(key, id);
    }

    pub(crate) fn store_compute(&mut self, key: ComputePipelineDesc, id: ComputePipelineId) {
        self.compute.insert(key, id);
    }

    pub(crate) fn drain_resources(&mut self) -> Vec<Resource> {
        let render = self.render.drain().map(|(_, id)| Resource::from(id));
        let compute = self.compute.drain().map(|(_, id)| Resource::from(id));
        render.chain(compute).collect()
    }
}

pub(crate) fn render_uses_module(desc: &RenderPipelineDesc, modules: &[ShaderModuleId]) -> bool {
    modules.contains(&desc.vertex.module)
        || desc
            .fragment
            .as_ref()
            .is_some_and(|stage| modules.contains(&stage.module))
}

// Borrowed when the constants are already sorted, so a lookup that hits does not clone the description.
pub(crate) fn normalized_render(desc: &RenderPipelineDesc) -> Cow<'_, RenderPipelineDesc> {
    if constants_sorted(&desc.vertex) && desc.fragment.iter().all(constants_sorted) {
        return Cow::Borrowed(desc);
    }
    let mut key = desc.clone();
    sort_constants(&mut key.vertex);
    key.fragment.iter_mut().for_each(sort_constants);
    Cow::Owned(key)
}

pub(crate) fn normalized_compute(desc: &ComputePipelineDesc) -> Cow<'_, ComputePipelineDesc> {
    if constants_sorted(&desc.shader) {
        return Cow::Borrowed(desc);
    }
    let mut key = desc.clone();
    sort_constants(&mut key.shader);
    Cow::Owned(key)
}

fn constants_sorted(stage: &ShaderStage) -> bool {
    stage.constants.is_sorted_by_key(|(name, _)| *name)
}

fn sort_constants(stage: &mut ShaderStage) {
    stage.constants.sort_by_key(|(name, _)| *name);
}

#[cfg(test)]
mod tests {
    use pathon_rhi::Device;
    use pathon_rhi_types::ShaderModuleDesc;

    use super::*;

    fn compute_desc(module: ShaderModuleId) -> ComputePipelineDesc {
        ComputePipelineDesc {
            label: "evicted",
            bind_group_layouts: vec![],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        }
    }

    #[test]
    fn evicting_a_module_forgets_only_the_pipelines_built_from_it() {
        let device = Device::null();
        let kept = device
            .create_shader_module(&ShaderModuleDesc {
                label: "kept",
                wgsl: "@compute @workgroup_size(1) fn main() {}",
            })
            .expect("module");
        let dropped = device
            .create_shader_module(&ShaderModuleDesc {
                label: "dropped",
                wgsl: "@compute @workgroup_size(1) fn main() {}",
            })
            .expect("module");
        let mut cache = PipelineCache::new();
        let kept_pipeline = cache.compute(&device, &compute_desc(kept)).expect("pipeline");
        let dropped_pipeline = cache.compute(&device, &compute_desc(dropped)).expect("pipeline");

        let evicted = cache.evict_modules(&[dropped]);

        assert_eq!(evicted, [Resource::from(dropped_pipeline)]);
        assert_eq!(cache.cached_compute(&compute_desc(kept)), Some(kept_pipeline));
        assert_eq!(cache.cached_compute(&compute_desc(dropped)), None);
    }
}
