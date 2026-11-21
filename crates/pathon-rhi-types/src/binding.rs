use std::num::NonZeroU32;

use bitflags::bitflags;

use crate::desc::TextureViewDimension;
use crate::format::Format;
use crate::id::{BindGroupLayoutId, BufferId, SamplerId, TextureViewId};

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct ShaderStages: u32 {
        const VERTEX = 1 << 0;
        const FRAGMENT = 1 << 1;
        const COMPUTE = 1 << 2;
        const VERTEX_FRAGMENT = Self::VERTEX.bits() | Self::FRAGMENT.bits();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextureSampleType {
    Float,
    UnfilterableFloat,
    Depth,
    Uint,
    Sint,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StorageAccess {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SamplerBindingType {
    Filtering,
    NonFiltering,
    Comparison,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum BindingType {
    UniformBuffer,
    StorageBuffer {
        access: StorageAccess,
    },
    SampledTexture {
        sample_type: TextureSampleType,
        dimension: TextureViewDimension,
        multisampled: bool,
    },
    StorageTexture {
        access: StorageAccess,
        format: Format,
        dimension: TextureViewDimension,
    },
    Sampler(SamplerBindingType),
}

impl BindingType {
    pub const fn texture_2d() -> Self {
        Self::SampledTexture {
            sample_type: TextureSampleType::Float,
            dimension: TextureViewDimension::D2,
            multisampled: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BindGroupLayoutEntry {
    pub binding: u32,
    pub visibility: ShaderStages,
    pub ty: BindingType,
    pub count: Option<NonZeroU32>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BindGroupLayoutDesc {
    pub label: &'static str,
    pub entries: Vec<BindGroupLayoutEntry>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum BindingResource {
    Buffer {
        buffer: BufferId,
        offset: u64,
        size: Option<u64>,
    },
    TextureView(TextureViewId),
    Sampler(SamplerId),
    TextureViewArray(Vec<TextureViewId>),
    SamplerArray(Vec<SamplerId>),
}

impl BindingResource {
    pub const fn whole_buffer(buffer: BufferId) -> Self {
        Self::Buffer {
            buffer,
            offset: 0,
            size: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BindGroupEntry {
    pub binding: u32,
    pub resource: BindingResource,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BindGroupDesc {
    pub label: &'static str,
    pub layout: BindGroupLayoutId,
    pub entries: Vec<BindGroupEntry>,
}
