use bitflags::bitflags;

use crate::desc::CompareFunction;
use crate::format::Format;
use crate::id::{BindGroupLayoutId, ShaderModuleId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ShaderModuleDesc<'src> {
    pub label: &'static str,
    pub wgsl: &'src str,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ShaderStage {
    pub module: ShaderModuleId,
    pub entry_point: &'static str,
    pub constants: Vec<(&'static str, f64)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum VertexFormat {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Uint32,
    Uint32x2,
    Uint32x3,
    Uint32x4,
    Sint32,
    Unorm8x4,
    Snorm8x4,
    Float16x2,
    Float16x4,
}

impl VertexFormat {
    pub fn size_bytes(self) -> u64 {
        match self {
            Self::Float32 | Self::Uint32 | Self::Sint32 | Self::Unorm8x4 | Self::Snorm8x4 | Self::Float16x2 => 4,
            Self::Float32x2 | Self::Uint32x2 | Self::Float16x4 => 8,
            Self::Float32x3 | Self::Uint32x3 => 12,
            Self::Float32x4 | Self::Uint32x4 => 16,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum VertexStepMode {
    Vertex,
    Instance,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct VertexAttribute {
    pub format: VertexFormat,
    pub offset: u64,
    pub location: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct VertexBufferLayout {
    pub stride: u64,
    pub step_mode: VertexStepMode,
    pub attributes: Vec<VertexAttribute>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum PrimitiveTopology {
    PointList,
    LineList,
    LineStrip,
    #[default]
    TriangleList,
    TriangleStrip,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum FrontFace {
    #[default]
    Ccw,
    Cw,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CullMode {
    #[default]
    None,
    Front,
    Back,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct PrimitiveState {
    pub topology: PrimitiveTopology,
    pub front_face: FrontFace,
    pub cull_mode: CullMode,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IndexFormat {
    Uint16,
    Uint32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BlendFactor {
    Zero,
    One,
    Src,
    OneMinusSrc,
    SrcAlpha,
    OneMinusSrcAlpha,
    Dst,
    OneMinusDst,
    DstAlpha,
    OneMinusDstAlpha,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BlendOperation {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlendComponent {
    pub src_factor: BlendFactor,
    pub dst_factor: BlendFactor,
    pub operation: BlendOperation,
}

impl BlendComponent {
    pub const REPLACE: Self = Self {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::Zero,
        operation: BlendOperation::Add,
    };
    pub const OVER: Self = Self {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::OneMinusSrcAlpha,
        operation: BlendOperation::Add,
    };
    pub const ADDITIVE: Self = Self {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::One,
        operation: BlendOperation::Add,
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlendState {
    pub color: BlendComponent,
    pub alpha: BlendComponent,
}

impl BlendState {
    pub const PREMULTIPLIED_ALPHA: Self = Self {
        color: BlendComponent::OVER,
        alpha: BlendComponent::OVER,
    };
    pub const ADDITIVE: Self = Self {
        color: BlendComponent::ADDITIVE,
        alpha: BlendComponent::ADDITIVE,
    };
}

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    pub struct ColorWrites: u32 {
        const RED = 1 << 0;
        const GREEN = 1 << 1;
        const BLUE = 1 << 2;
        const ALPHA = 1 << 3;
        const ALL = Self::RED.bits() | Self::GREEN.bits() | Self::BLUE.bits() | Self::ALPHA.bits();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ColorTargetState {
    pub format: Format,
    pub blend: Option<BlendState>,
    pub write_mask: ColorWrites,
}

impl ColorTargetState {
    pub const fn opaque(format: Format) -> Self {
        Self {
            format,
            blend: None,
            write_mask: ColorWrites::ALL,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DepthState {
    pub format: Format,
    pub write_enabled: bool,
    pub compare: CompareFunction,
}

impl DepthState {
    // Reverse-Z (slot contract, M2-10): nearer fragments have larger depth.
    pub const fn reverse_z(format: Format) -> Self {
        Self {
            format,
            write_enabled: true,
            compare: CompareFunction::GreaterEqual,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct RenderPipelineDesc {
    pub label: &'static str,
    pub bind_group_layouts: Vec<BindGroupLayoutId>,
    pub vertex: ShaderStage,
    pub vertex_buffers: Vec<VertexBufferLayout>,
    pub fragment: Option<ShaderStage>,
    pub color_targets: Vec<ColorTargetState>,
    pub primitive: PrimitiveState,
    pub depth: Option<DepthState>,
    pub sample_count: u32,
    pub immediate_size: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ComputePipelineDesc {
    pub label: &'static str,
    pub bind_group_layouts: Vec<BindGroupLayoutId>,
    pub shader: ShaderStage,
    pub immediate_size: u32,
}
