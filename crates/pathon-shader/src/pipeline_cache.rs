use std::collections::HashMap;

use pathon_rhi_types::{
    ComputePipelineDesc, ComputePipelineId, DeviceApi, RenderPipelineDesc, RenderPipelineId, RhiError, ShaderStage,
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
        let mut key = desc.clone();
        sort_constants(&mut key.vertex);
        key.fragment.iter_mut().for_each(sort_constants);
        if let Some(id) = self.render.get(&key) {
            return Ok(*id);
        }
        let id = device.create_render_pipeline(&key)?;
        self.render.insert(key, id);
        Ok(id)
    }

    pub fn compute(
        &mut self,
        device: &dyn DeviceApi,
        desc: &ComputePipelineDesc,
    ) -> Result<ComputePipelineId, RhiError> {
        let mut key = desc.clone();
        sort_constants(&mut key.shader);
        if let Some(id) = self.compute.get(&key) {
            return Ok(*id);
        }
        let id = device.create_compute_pipeline(&key)?;
        self.compute.insert(key, id);
        Ok(id)
    }
}

fn sort_constants(stage: &mut ShaderStage) {
    stage.constants.sort_by_key(|(name, _)| *name);
}
