#![doc = include_str!("../../../docs/crates/pathon-rhi-types.md")]

mod access;
mod binding;
mod caps;
mod color_space;
mod command;
mod desc;
mod device;
mod error;
mod format;
mod id;
mod indirect;
mod maybe_send;
mod pipeline;
mod surface;

pub use access::{Access, SubresourceRange};
pub use binding::{
    BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
    SamplerBindingType, ShaderStages, StorageAccess, TextureSampleType,
};
pub use caps::{AdapterInfo, Backend, Caps, DeviceType, Features, FormatCaps, Limits, Profile};
pub use color_space::{
    DisplayHdr, SurfaceCaps, SurfaceChoice, SurfaceColorSpace, SurfaceColorSpaces, SurfaceFormatCaps, SurfaceOutput,
};
pub use command::{
    Color, ColorAttachment, Command, CommandList, ComputePassDesc, ComputePassEncoder, DeclaredAccess, DepthAccess,
    DepthAttachment, ExternalCallback, ExternalCommand, LoadOp, PassTimestamps, RenderPassDesc, RenderPassEncoder,
    ResourceRef, ScissorRect, StoreOp, Transfer, Viewport,
};
pub use desc::{
    AddressMode, BufferCopy, BufferDesc, BufferUsage, CompareFunction, Extent3d, FilterMode, Origin3d, QuerySetDesc,
    SamplerDesc, TexelLayout, TextureAspect, TextureCopy, TextureDesc, TextureDimension, TextureUsage, TextureViewDesc,
    TextureViewDimension,
};
pub use device::{DeviceApi, DeviceRequest, PipelineCacheData, PowerPreference, Resource};
pub use error::RhiError;
pub use format::{Compression, Format, FormatKind, SampleKind};
pub use id::{
    BindGroupId, BindGroupLayoutId, BufferId, ComputePipelineId, Id, Pool, QuerySetId, RawHandle, RenderPipelineId,
    SamplerId, ShaderModuleId, SurfaceId, TextureId, TextureViewId,
};
pub use indirect::{
    DispatchIndirectArgs, DrawIndexedIndirectArgs, DrawIndirectArgs, INDIRECT_ALIGNMENT, INDIRECT_COUNT_SIZE,
    MultiDrawCount,
};
pub use maybe_send::{MaybeSend, MaybeSendSync};
pub use pipeline::{
    BlendComponent, BlendFactor, BlendOperation, BlendState, ColorTargetState, ColorWrites, ComputePipelineDesc,
    CullMode, DepthBias, DepthState, FrontFace, IndexFormat, OverrideValue, PolygonMode, PrimitiveState, PrimitiveTopology,
    RenderPipelineDesc, ShaderModuleDesc, ShaderStage, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexStepMode,
};
pub use surface::{PhysicalSize, PresentMode, SurfaceConfig, SurfaceFrame, SurfaceTarget, WindowSource};
