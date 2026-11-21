use std::any::Any;
use std::fmt;
use std::sync::Arc;

use crate::binding::{BindGroupDesc, BindGroupLayoutDesc};
use crate::caps::{Backend, Caps, Features};
use crate::command::CommandList;
use crate::desc::{
    BufferDesc, Extent3d, QuerySetDesc, SamplerDesc, TexelLayout, TextureCopy, TextureDesc, TextureViewDesc,
};
use crate::error::RhiError;
use crate::format::Format;
use crate::id::{
    BindGroupId, BindGroupLayoutId, BufferId, ComputePipelineId, QuerySetId, RenderPipelineId, SamplerId,
    ShaderModuleId, SurfaceId, TextureId, TextureViewId,
};
use crate::maybe_send::MaybeSendSync;
use crate::pipeline::{ComputePipelineDesc, RenderPipelineDesc, ShaderModuleDesc};
use crate::surface::{SurfaceConfig, SurfaceFrame, SurfaceTarget};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum PowerPreference {
    #[default]
    HighPerformance,
    LowPower,
}

#[derive(Clone, Default)]
pub struct DeviceRequest {
    pub label: &'static str,
    pub backend: Option<Backend>,
    pub power: PowerPreference,
    pub required_features: Features,
    pub optional_features: Features,
    pub compatible_surface: Option<SurfaceTarget>,
    // Backend-specific hooks, downcast by the backend that understands them (e.g. extra Vulkan extensions).
    pub extensions: Vec<Arc<dyn Any + Send + Sync>>,
}

impl fmt::Debug for DeviceRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceRequest")
            .field("label", &self.label)
            .field("backend", &self.backend)
            .field("power", &self.power)
            .field("required_features", &self.required_features)
            .field("optional_features", &self.optional_features)
            .field("compatible_surface", &self.compatible_surface)
            .field("extensions", &self.extensions.len())
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Resource {
    Buffer(BufferId),
    Texture(TextureId),
    TextureView(TextureViewId),
    Sampler(SamplerId),
    ShaderModule(ShaderModuleId),
    BindGroupLayout(BindGroupLayoutId),
    BindGroup(BindGroupId),
    RenderPipeline(RenderPipelineId),
    ComputePipeline(ComputePipelineId),
    QuerySet(QuerySetId),
}

macro_rules! resource_from_id {
    ($($variant:ident($id:ty)),* $(,)?) => {$(
        impl From<$id> for Resource {
            fn from(id: $id) -> Self {
                Self::$variant(id)
            }
        }
    )*};
}

resource_from_id!(
    Buffer(BufferId),
    Texture(TextureId),
    TextureView(TextureViewId),
    Sampler(SamplerId),
    ShaderModule(ShaderModuleId),
    BindGroupLayout(BindGroupLayoutId),
    BindGroup(BindGroupId),
    RenderPipeline(RenderPipelineId),
    ComputePipeline(ComputePipelineId),
    QuerySet(QuerySetId),
);

// Every method takes `&self`: the device is shared between the main thread (surface creation) and the render thread
// (ADR-0004), so backends keep their object pools behind locks.
pub trait DeviceApi: MaybeSendSync {
    fn caps(&self) -> &Caps;

    fn is_lost(&self) -> bool;

    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferId, RhiError>;

    fn write_buffer(&self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), RhiError>;

    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureId, RhiError>;

    fn texture_desc(&self, texture: TextureId) -> Result<TextureDesc, RhiError>;

    fn write_texture(
        &self,
        dst: &TextureCopy,
        data: &[u8],
        layout: TexelLayout,
        size: Extent3d,
    ) -> Result<(), RhiError>;

    fn create_texture_view(&self, texture: TextureId, desc: &TextureViewDesc) -> Result<TextureViewId, RhiError>;

    fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerId, RhiError>;

    fn create_shader_module(&self, desc: &ShaderModuleDesc<'_>) -> Result<ShaderModuleId, RhiError>;

    fn create_bind_group_layout(&self, desc: &BindGroupLayoutDesc) -> Result<BindGroupLayoutId, RhiError>;

    fn create_bind_group(&self, desc: &BindGroupDesc) -> Result<BindGroupId, RhiError>;

    fn create_render_pipeline(&self, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError>;

    fn create_compute_pipeline(&self, desc: &ComputePipelineDesc) -> Result<ComputePipelineId, RhiError>;

    fn create_query_set(&self, desc: &QuerySetDesc) -> Result<QuerySetId, RhiError>;

    fn destroy(&self, resource: Resource);

    fn create_surface(&self, target: &SurfaceTarget) -> Result<SurfaceId, RhiError>;

    fn configure_surface(&self, surface: SurfaceId, config: &SurfaceConfig) -> Result<Format, RhiError>;

    fn acquire_surface(&self, surface: SurfaceId) -> Result<SurfaceFrame, RhiError>;

    fn present_surface(&self, surface: SurfaceId) -> Result<(), RhiError>;

    // Gives an acquired frame back without showing it, e.g. when the frame failed to record.
    fn discard_surface(&self, surface: SurfaceId);

    fn destroy_surface(&self, surface: SurfaceId);

    fn submit(&self, commands: CommandList) -> Result<(), RhiError>;

    // Blocks until the GPU has written the buffer; not available on wasm.
    fn read_buffer(&self, buffer: BufferId) -> Result<Vec<u8>, RhiError>;

    fn wait_idle(&self) -> Result<(), RhiError>;
}
