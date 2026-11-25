use bitflags::bitflags;

use crate::desc::CompareFunction;
use crate::error::RhiError;
use crate::format::Format;
use crate::id::{BindGroupLayoutId, ShaderModuleId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ShaderModuleDesc<'src> {
    pub label: &'static str,
    pub wgsl: &'src str,
}

// Stored as bits so descriptions stay `Eq + Hash` (pipeline keys); a NaN equals itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OverrideValue(u64);

impl OverrideValue {
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl From<f64> for OverrideValue {
    fn from(value: f64) -> Self {
        Self(value.to_bits())
    }
}

impl From<f32> for OverrideValue {
    fn from(value: f32) -> Self {
        f64::from(value).into()
    }
}

impl From<u32> for OverrideValue {
    fn from(value: u32) -> Self {
        f64::from(value).into()
    }
}

impl From<i32> for OverrideValue {
    fn from(value: i32) -> Self {
        f64::from(value).into()
    }
}

impl From<bool> for OverrideValue {
    fn from(value: bool) -> Self {
        f64::from(u8::from(value)).into()
    }
}

// Names must be unique; their order does not matter to the backends (pipeline keys sort them).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShaderStage {
    pub module: ShaderModuleId,
    pub entry_point: &'static str,
    pub constants: Vec<(&'static str, OverrideValue)>,
}

impl ShaderStage {
    pub fn check_constants(&self, label: &str) -> Result<(), RhiError> {
        let duplicate = self
            .constants
            .iter()
            .enumerate()
            .find(|(index, (name, _))| self.constants[..*index].iter().any(|(earlier, _)| earlier == name));
        match duplicate {
            Some((_, (name, _))) => Err(RhiError::validation(
                "R0107",
                format!("pipeline '{label}': override '{name}' is given more than once"),
            )),
            None => Ok(()),
        }
    }
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

// `Line` needs `Features::POLYGON_MODE_LINE`; without it pipeline creation fails with `MissingFeatures` (ADR-0046).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum PolygonMode {
    #[default]
    Fill,
    Line,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct PrimitiveState {
    pub topology: PrimitiveTopology,
    pub front_face: FrontFace,
    pub cull_mode: CullMode,
    pub polygon_mode: PolygonMode,
}

impl PrimitiveState {
    // Backends call this before creating a pipeline, so every one of them refuses the same way.
    pub fn check_features(&self, caps: &crate::caps::Caps) -> Result<(), RhiError> {
        match self.polygon_mode {
            PolygonMode::Fill => Ok(()),
            PolygonMode::Line if caps.has(crate::caps::Features::POLYGON_MODE_LINE) => Ok(()),
            PolygonMode::Line => Err(RhiError::MissingFeatures(crate::caps::Features::POLYGON_MODE_LINE)),
        }
    }
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

// Added to the depth of every fragment of a primitive, in the sign of the depth buffer: with reverse-Z a bias that
// moves geometry away from the viewer is negative. `constant` counts smallest depth steps, `slope_scale` multiplies the
// primitive's depth slope, `clamp` limits the sum (0 = no limit). The floats are stored as bits, like `OverrideValue`,
// so a description stays `Eq + Hash`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct DepthBias {
    pub constant: i32,
    slope_scale_bits: u32,
    clamp_bits: u32,
}

impl DepthBias {
    pub const NONE: Self = Self {
        constant: 0,
        slope_scale_bits: 0,
        clamp_bits: 0,
    };

    pub const fn new(constant: i32, slope_scale: f32, clamp: f32) -> Self {
        Self {
            constant,
            slope_scale_bits: slope_scale.to_bits(),
            clamp_bits: clamp.to_bits(),
        }
    }

    pub const fn slope_scale(self) -> f32 {
        f32::from_bits(self.slope_scale_bits)
    }

    pub const fn clamp(self) -> f32 {
        f32::from_bits(self.clamp_bits)
    }

    pub const fn is_finite(self) -> bool {
        self.slope_scale().is_finite() && self.clamp().is_finite()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DepthState {
    pub format: Format,
    pub write_enabled: bool,
    pub compare: CompareFunction,
    pub bias: DepthBias,
}

impl DepthState {
    // Reverse-Z (slot contract, M2-10): nearer fragments have larger depth.
    pub const fn reverse_z(format: Format) -> Self {
        Self {
            format,
            write_enabled: true,
            compare: CompareFunction::GreaterEqual,
            bias: DepthBias::NONE,
        }
    }

    #[must_use]
    pub const fn with_bias(self, bias: DepthBias) -> Self {
        Self { bias, ..self }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
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

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ComputePipelineDesc {
    pub label: &'static str,
    pub bind_group_layouts: Vec<BindGroupLayoutId>,
    pub shader: ShaderStage,
    pub immediate_size: u32,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::id::{Id, RawHandle};

    #[test]
    fn override_values_round_trip_and_compare_by_bits() {
        assert_eq!(OverrideValue::from(3.5f64).get(), 3.5);
        assert_eq!(OverrideValue::from(1.25f32).get(), 1.25);
        assert_eq!(OverrideValue::from(7u32).get(), 7.0);
        assert_eq!(OverrideValue::from(-2i32).get(), -2.0);
        assert_eq!(OverrideValue::from(true).get(), 1.0);
        assert_eq!(OverrideValue::from(false).get(), 0.0);
        assert_eq!(OverrideValue::from(f64::NAN), OverrideValue::from(f64::NAN));
    }

    #[test]
    fn depth_bias_round_trips_and_hashes_by_bits() {
        let bias = DepthBias::new(-2, -1.5, 0.25);
        assert_eq!((bias.constant, bias.slope_scale(), bias.clamp()), (-2, -1.5, 0.25));
        assert!(bias.is_finite());
        assert!(!DepthBias::new(0, f32::NAN, 0.0).is_finite());
        assert_eq!(DepthBias::default(), DepthBias::NONE);
        let states: HashSet<DepthState> = [
            DepthState::reverse_z(Format::Depth32Float).with_bias(bias),
            DepthState::reverse_z(Format::Depth32Float).with_bias(bias),
            DepthState::reverse_z(Format::Depth32Float),
        ]
        .into_iter()
        .collect();
        assert_eq!(states.len(), 2);
    }

    #[test]
    fn a_compute_pipeline_description_is_hashable() {
        let desc = ComputePipelineDesc {
            label: "key",
            bind_group_layouts: vec![],
            shader: ShaderStage {
                module: ShaderModuleId::from_raw(RawHandle::new(0, 0)),
                entry_point: "main",
                constants: vec![("scale", 2.0.into())],
            },
            immediate_size: 4,
        };
        let keys: HashSet<ComputePipelineDesc> = [desc.clone(), desc].into_iter().collect();
        assert_eq!(keys.len(), 1);
    }
}
