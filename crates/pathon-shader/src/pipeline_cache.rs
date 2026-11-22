use std::borrow::Cow;
use std::collections::HashMap;

use pathon_rhi_types::{
    ComputePipelineDesc, ComputePipelineId, DeviceApi, RenderPipelineDesc, RenderPipelineId, Resource, RhiError,
    ShaderStage,
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
