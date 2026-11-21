use pathon_rhi_types::{
    AddressMode, Backend, BindingType, BlendComponent, BlendFactor, BlendOperation, BlendState, BufferUsage,
    ColorWrites, CompareFunction, CullMode, DeviceType, Features, FilterMode, Format, FormatCaps, FrontFace,
    IndexFormat, Limits, PresentMode, PrimitiveTopology, RhiError, SamplerBindingType, ShaderStages, StorageAccess,
    TextureAspect, TextureDimension, TextureSampleType, TextureUsage, TextureViewDimension, VertexFormat,
    VertexStepMode,
};
use wgpu::{AstcBlock, AstcChannel};

// One table per enum: the same row drives both directions, so the mappings cannot drift apart.
macro_rules! two_way {
    ($to:ident, $from:ident, $ours:ty => $theirs:ty { $($variant:ident => { $($pattern:tt)* }),* $(,)? }) => {
        pub(crate) fn $to(value: $ours) -> Result<$theirs, RhiError> {
            Ok(match value {
                $(<$ours>::$variant => $($pattern)*,)*
                _ => return Err(RhiError::Unsupported(concat!("no wgpu mapping for this ", stringify!($ours), " variant"))),
            })
        }

        pub(crate) fn $from(value: $theirs) -> Option<$ours> {
            match value {
                $($($pattern)* => Some(<$ours>::$variant),)*
                _ => None,
            }
        }
    };
}

two_way!(format_to_wgpu, format_from_wgpu, Format => wgpu::TextureFormat {
    R8Unorm => { wgpu::TextureFormat::R8Unorm },
    R8Uint => { wgpu::TextureFormat::R8Uint },
    Rg8Unorm => { wgpu::TextureFormat::Rg8Unorm },
    Rgba8Unorm => { wgpu::TextureFormat::Rgba8Unorm },
    Rgba8UnormSrgb => { wgpu::TextureFormat::Rgba8UnormSrgb },
    Bgra8Unorm => { wgpu::TextureFormat::Bgra8Unorm },
    Bgra8UnormSrgb => { wgpu::TextureFormat::Bgra8UnormSrgb },
    R16Float => { wgpu::TextureFormat::R16Float },
    R16Unorm => { wgpu::TextureFormat::R16Unorm },
    Rg16Float => { wgpu::TextureFormat::Rg16Float },
    Rgba16Float => { wgpu::TextureFormat::Rgba16Float },
    Rgba16Unorm => { wgpu::TextureFormat::Rgba16Unorm },
    R32Float => { wgpu::TextureFormat::R32Float },
    R32Uint => { wgpu::TextureFormat::R32Uint },
    Rg32Float => { wgpu::TextureFormat::Rg32Float },
    Rg32Uint => { wgpu::TextureFormat::Rg32Uint },
    Rgba32Float => { wgpu::TextureFormat::Rgba32Float },
    Rgba32Uint => { wgpu::TextureFormat::Rgba32Uint },
    Rgb10a2Unorm => { wgpu::TextureFormat::Rgb10a2Unorm },
    Rg11b10Ufloat => { wgpu::TextureFormat::Rg11b10Ufloat },
    Depth16Unorm => { wgpu::TextureFormat::Depth16Unorm },
    Depth32Float => { wgpu::TextureFormat::Depth32Float },
    Depth24PlusStencil8 => { wgpu::TextureFormat::Depth24PlusStencil8 },
    Depth32FloatStencil8 => { wgpu::TextureFormat::Depth32FloatStencil8 },
    Bc1RgbaUnorm => { wgpu::TextureFormat::Bc1RgbaUnorm },
    Bc1RgbaUnormSrgb => { wgpu::TextureFormat::Bc1RgbaUnormSrgb },
    Bc3RgbaUnorm => { wgpu::TextureFormat::Bc3RgbaUnorm },
    Bc3RgbaUnormSrgb => { wgpu::TextureFormat::Bc3RgbaUnormSrgb },
    Bc4RUnorm => { wgpu::TextureFormat::Bc4RUnorm },
    Bc5RgUnorm => { wgpu::TextureFormat::Bc5RgUnorm },
    Bc6hRgbUfloat => { wgpu::TextureFormat::Bc6hRgbUfloat },
    Bc7RgbaUnorm => { wgpu::TextureFormat::Bc7RgbaUnorm },
    Bc7RgbaUnormSrgb => { wgpu::TextureFormat::Bc7RgbaUnormSrgb },
    Etc2Rgb8Unorm => { wgpu::TextureFormat::Etc2Rgb8Unorm },
    Etc2Rgba8Unorm => { wgpu::TextureFormat::Etc2Rgba8Unorm },
    Astc4x4Unorm => { wgpu::TextureFormat::Astc { block: AstcBlock::B4x4, channel: AstcChannel::Unorm } },
    Astc4x4UnormSrgb => { wgpu::TextureFormat::Astc { block: AstcBlock::B4x4, channel: AstcChannel::UnormSrgb } },
});

const FEATURE_TABLE: &[(Features, wgpu::Features)] = &[
    (Features::TIMESTAMP_QUERY, wgpu::Features::TIMESTAMP_QUERY),
    (
        Features::TIMESTAMP_QUERY_INSIDE_PASSES,
        wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
    ),
    (Features::TEXTURE_BINDING_ARRAY, wgpu::Features::TEXTURE_BINDING_ARRAY),
    (Features::BUFFER_BINDING_ARRAY, wgpu::Features::BUFFER_BINDING_ARRAY),
    (
        Features::NON_UNIFORM_INDEXING,
        wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
    ),
    (
        Features::PARTIALLY_BOUND_BINDING_ARRAY,
        wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY,
    ),
    (
        Features::MULTI_DRAW_INDIRECT_COUNT,
        wgpu::Features::MULTI_DRAW_INDIRECT_COUNT,
    ),
    (
        Features::INDIRECT_FIRST_INSTANCE,
        wgpu::Features::INDIRECT_FIRST_INSTANCE,
    ),
    (Features::SHADER_INT64, wgpu::Features::SHADER_INT64),
    (
        Features::SHADER_INT64_ATOMIC_MIN_MAX,
        wgpu::Features::SHADER_INT64_ATOMIC_MIN_MAX,
    ),
    (Features::SHADER_F16, wgpu::Features::SHADER_F16),
    (Features::SUBGROUP, wgpu::Features::SUBGROUP),
    (Features::TEXTURE_ATOMIC, wgpu::Features::TEXTURE_ATOMIC),
    (
        Features::TEXTURE_FORMAT_16BIT_NORM,
        wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
    ),
    (Features::FLOAT32_FILTERABLE, wgpu::Features::FLOAT32_FILTERABLE),
    (Features::TEXTURE_COMPRESSION_BC, wgpu::Features::TEXTURE_COMPRESSION_BC),
    (
        Features::TEXTURE_COMPRESSION_ETC2,
        wgpu::Features::TEXTURE_COMPRESSION_ETC2,
    ),
    (
        Features::TEXTURE_COMPRESSION_ASTC,
        wgpu::Features::TEXTURE_COMPRESSION_ASTC,
    ),
    (Features::PIPELINE_CACHE, wgpu::Features::PIPELINE_CACHE),
    (Features::DEPTH_CLIP_CONTROL, wgpu::Features::DEPTH_CLIP_CONTROL),
    (Features::DEPTH32FLOAT_STENCIL8, wgpu::Features::DEPTH32FLOAT_STENCIL8),
    (Features::RAY_QUERY, wgpu::Features::EXPERIMENTAL_RAY_QUERY),
    (Features::MESH_SHADER, wgpu::Features::EXPERIMENTAL_MESH_SHADER),
    (Features::POLYGON_MODE_LINE, wgpu::Features::POLYGON_MODE_LINE),
];

// Experimental wgpu features need an explicit unsafe opt-in; pathon enables them once M8 packs need them.
pub(crate) const EXPERIMENTAL: Features = Features::RAY_QUERY.union(Features::MESH_SHADER);

pub(crate) fn features_to_wgpu(features: Features) -> wgpu::Features {
    let mut result = wgpu::Features::empty();
    for (ours, theirs) in FEATURE_TABLE {
        if features.contains(*ours) {
            result |= *theirs;
        }
    }
    result
}

pub(crate) fn features_from_wgpu(features: wgpu::Features) -> Features {
    let mut result = Features::empty();
    for (ours, theirs) in FEATURE_TABLE {
        if features.contains(*theirs) {
            result |= *ours;
        }
    }
    result
}

fn clamp_u32<T: TryInto<u32>>(value: T) -> u32 {
    value.try_into().unwrap_or(u32::MAX)
}

pub(crate) fn limits_from_wgpu(limits: &wgpu::Limits) -> Limits {
    Limits {
        max_texture_dimension_2d: limits.max_texture_dimension_2d,
        max_texture_array_layers: limits.max_texture_array_layers,
        max_bind_groups: limits.max_bind_groups,
        max_bindings_per_bind_group: limits.max_bindings_per_bind_group,
        max_uniform_buffer_binding_size: limits.max_uniform_buffer_binding_size,
        max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
        max_buffer_size: limits.max_buffer_size,
        max_color_attachments: limits.max_color_attachments,
        max_compute_workgroup_size: [
            limits.max_compute_workgroup_size_x,
            limits.max_compute_workgroup_size_y,
            limits.max_compute_workgroup_size_z,
        ],
        max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
        max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
        max_binding_array_elements_per_shader_stage: limits.max_binding_array_elements_per_shader_stage,
        min_uniform_buffer_offset_alignment: clamp_u32(limits.min_uniform_buffer_offset_alignment),
        min_storage_buffer_offset_alignment: clamp_u32(limits.min_storage_buffer_offset_alignment),
    }
}

pub(crate) fn format_caps_from_wgpu(features: wgpu::TextureFormatFeatures) -> FormatCaps {
    let usages = features.allowed_usages;
    let flags = features.flags;
    let mut caps = FormatCaps::empty();
    let pairs = [
        (
            usages.contains(wgpu::TextureUsages::TEXTURE_BINDING),
            FormatCaps::SAMPLED,
        ),
        (
            flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE),
            FormatCaps::FILTERABLE,
        ),
        (
            usages.contains(wgpu::TextureUsages::STORAGE_BINDING),
            FormatCaps::STORAGE,
        ),
        (
            usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
            FormatCaps::RENDER_TARGET,
        ),
        (
            flags.contains(wgpu::TextureFormatFeatureFlags::BLENDABLE),
            FormatCaps::BLENDABLE,
        ),
        (
            flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4),
            FormatCaps::MULTISAMPLE,
        ),
        (
            flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE),
            FormatCaps::RESOLVE,
        ),
    ];
    for (supported, cap) in pairs {
        if supported {
            caps |= cap;
        }
    }
    caps
}

pub(crate) fn backend_from_wgpu(backend: wgpu::Backend) -> Backend {
    match backend {
        wgpu::Backend::Vulkan => Backend::Vulkan,
        wgpu::Backend::Dx12 => Backend::Dx12,
        wgpu::Backend::Metal => Backend::Metal,
        wgpu::Backend::BrowserWebGpu => Backend::WebGpu,
        _ => Backend::Null,
    }
}

pub(crate) fn backends_to_wgpu(backend: Option<Backend>) -> wgpu::Backends {
    match backend {
        None => wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::METAL | wgpu::Backends::BROWSER_WEBGPU,
        Some(Backend::Vulkan) => wgpu::Backends::VULKAN,
        Some(Backend::Dx12) => wgpu::Backends::DX12,
        Some(Backend::Metal) => wgpu::Backends::METAL,
        Some(Backend::WebGpu) => wgpu::Backends::BROWSER_WEBGPU,
        // `Null` and any backend wgpu cannot select: device creation fails with a missing adapter instead of picking
        // another one.
        Some(_) => wgpu::Backends::empty(),
    }
}

pub(crate) fn device_type_from_wgpu(device_type: wgpu::DeviceType) -> DeviceType {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => DeviceType::DiscreteGpu,
        wgpu::DeviceType::IntegratedGpu => DeviceType::IntegratedGpu,
        wgpu::DeviceType::VirtualGpu => DeviceType::VirtualGpu,
        wgpu::DeviceType::Cpu => DeviceType::Cpu,
        wgpu::DeviceType::Other => DeviceType::Other,
    }
}

pub(crate) fn texture_usage_to_wgpu(usage: TextureUsage) -> wgpu::TextureUsages {
    let mut result = wgpu::TextureUsages::empty();
    let pairs = [
        (TextureUsage::COPY_SRC, wgpu::TextureUsages::COPY_SRC),
        (TextureUsage::COPY_DST, wgpu::TextureUsages::COPY_DST),
        (TextureUsage::SAMPLED, wgpu::TextureUsages::TEXTURE_BINDING),
        (TextureUsage::STORAGE, wgpu::TextureUsages::STORAGE_BINDING),
        (TextureUsage::RENDER_TARGET, wgpu::TextureUsages::RENDER_ATTACHMENT),
    ];
    for (ours, theirs) in pairs {
        if usage.contains(ours) {
            result |= theirs;
        }
    }
    result
}

pub(crate) fn texture_usage_from_wgpu(usage: wgpu::TextureUsages) -> TextureUsage {
    let mut result = TextureUsage::empty();
    let pairs = [
        (TextureUsage::COPY_SRC, wgpu::TextureUsages::COPY_SRC),
        (TextureUsage::COPY_DST, wgpu::TextureUsages::COPY_DST),
        (TextureUsage::SAMPLED, wgpu::TextureUsages::TEXTURE_BINDING),
        (TextureUsage::STORAGE, wgpu::TextureUsages::STORAGE_BINDING),
        (TextureUsage::RENDER_TARGET, wgpu::TextureUsages::RENDER_ATTACHMENT),
    ];
    for (ours, theirs) in pairs {
        if usage.contains(theirs) {
            result |= ours;
        }
    }
    result
}

pub(crate) fn buffer_usage_to_wgpu(usage: BufferUsage) -> wgpu::BufferUsages {
    let mut result = wgpu::BufferUsages::empty();
    let pairs = [
        (BufferUsage::COPY_SRC, wgpu::BufferUsages::COPY_SRC),
        (BufferUsage::COPY_DST, wgpu::BufferUsages::COPY_DST),
        (BufferUsage::VERTEX, wgpu::BufferUsages::VERTEX),
        (BufferUsage::INDEX, wgpu::BufferUsages::INDEX),
        (BufferUsage::UNIFORM, wgpu::BufferUsages::UNIFORM),
        (BufferUsage::STORAGE, wgpu::BufferUsages::STORAGE),
        (BufferUsage::INDIRECT, wgpu::BufferUsages::INDIRECT),
        (BufferUsage::MAP_READ, wgpu::BufferUsages::MAP_READ),
        (BufferUsage::QUERY_RESOLVE, wgpu::BufferUsages::QUERY_RESOLVE),
    ];
    for (ours, theirs) in pairs {
        if usage.contains(ours) {
            result |= theirs;
        }
    }
    result
}

pub(crate) fn texture_dimension_to_wgpu(dimension: TextureDimension) -> wgpu::TextureDimension {
    match dimension {
        TextureDimension::D1 => wgpu::TextureDimension::D1,
        TextureDimension::D2 => wgpu::TextureDimension::D2,
        TextureDimension::D3 => wgpu::TextureDimension::D3,
    }
}

pub(crate) fn view_dimension_to_wgpu(dimension: TextureViewDimension) -> wgpu::TextureViewDimension {
    match dimension {
        TextureViewDimension::D1 => wgpu::TextureViewDimension::D1,
        TextureViewDimension::D2 => wgpu::TextureViewDimension::D2,
        TextureViewDimension::D2Array => wgpu::TextureViewDimension::D2Array,
        TextureViewDimension::Cube => wgpu::TextureViewDimension::Cube,
        TextureViewDimension::CubeArray => wgpu::TextureViewDimension::CubeArray,
        TextureViewDimension::D3 => wgpu::TextureViewDimension::D3,
    }
}

pub(crate) fn aspect_to_wgpu(aspect: TextureAspect) -> wgpu::TextureAspect {
    match aspect {
        TextureAspect::All => wgpu::TextureAspect::All,
        TextureAspect::DepthOnly => wgpu::TextureAspect::DepthOnly,
        TextureAspect::StencilOnly => wgpu::TextureAspect::StencilOnly,
    }
}

pub(crate) fn address_mode_to_wgpu(mode: AddressMode) -> wgpu::AddressMode {
    match mode {
        AddressMode::ClampToEdge => wgpu::AddressMode::ClampToEdge,
        AddressMode::Repeat => wgpu::AddressMode::Repeat,
        AddressMode::MirrorRepeat => wgpu::AddressMode::MirrorRepeat,
    }
}

pub(crate) fn filter_to_wgpu(filter: FilterMode) -> wgpu::FilterMode {
    match filter {
        FilterMode::Nearest => wgpu::FilterMode::Nearest,
        FilterMode::Linear => wgpu::FilterMode::Linear,
    }
}

pub(crate) fn mipmap_filter_to_wgpu(filter: FilterMode) -> wgpu::MipmapFilterMode {
    match filter {
        FilterMode::Nearest => wgpu::MipmapFilterMode::Nearest,
        FilterMode::Linear => wgpu::MipmapFilterMode::Linear,
    }
}

pub(crate) fn compare_to_wgpu(compare: CompareFunction) -> wgpu::CompareFunction {
    match compare {
        CompareFunction::Never => wgpu::CompareFunction::Never,
        CompareFunction::Less => wgpu::CompareFunction::Less,
        CompareFunction::Equal => wgpu::CompareFunction::Equal,
        CompareFunction::LessEqual => wgpu::CompareFunction::LessEqual,
        CompareFunction::Greater => wgpu::CompareFunction::Greater,
        CompareFunction::NotEqual => wgpu::CompareFunction::NotEqual,
        CompareFunction::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        CompareFunction::Always => wgpu::CompareFunction::Always,
    }
}

pub(crate) fn shader_stages_to_wgpu(stages: ShaderStages) -> wgpu::ShaderStages {
    let mut result = wgpu::ShaderStages::empty();
    if stages.contains(ShaderStages::VERTEX) {
        result |= wgpu::ShaderStages::VERTEX;
    }
    if stages.contains(ShaderStages::FRAGMENT) {
        result |= wgpu::ShaderStages::FRAGMENT;
    }
    if stages.contains(ShaderStages::COMPUTE) {
        result |= wgpu::ShaderStages::COMPUTE;
    }
    result
}

fn storage_access_to_wgpu(access: StorageAccess) -> wgpu::StorageTextureAccess {
    match access {
        StorageAccess::ReadOnly => wgpu::StorageTextureAccess::ReadOnly,
        StorageAccess::WriteOnly => wgpu::StorageTextureAccess::WriteOnly,
        StorageAccess::ReadWrite => wgpu::StorageTextureAccess::ReadWrite,
    }
}

pub(crate) fn binding_type_to_wgpu(ty: BindingType) -> Result<wgpu::BindingType, RhiError> {
    Ok(match ty {
        BindingType::UniformBuffer => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        BindingType::StorageBuffer { access } => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage {
                read_only: access == StorageAccess::ReadOnly,
            },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        BindingType::SampledTexture {
            sample_type,
            dimension,
            multisampled,
        } => wgpu::BindingType::Texture {
            sample_type: match sample_type {
                TextureSampleType::Float => wgpu::TextureSampleType::Float { filterable: true },
                TextureSampleType::UnfilterableFloat => wgpu::TextureSampleType::Float { filterable: false },
                TextureSampleType::Depth => wgpu::TextureSampleType::Depth,
                TextureSampleType::Uint => wgpu::TextureSampleType::Uint,
                TextureSampleType::Sint => wgpu::TextureSampleType::Sint,
            },
            view_dimension: view_dimension_to_wgpu(dimension),
            multisampled,
        },
        BindingType::StorageTexture {
            access,
            format,
            dimension,
        } => wgpu::BindingType::StorageTexture {
            access: storage_access_to_wgpu(access),
            format: format_to_wgpu(format)?,
            view_dimension: view_dimension_to_wgpu(dimension),
        },
        BindingType::Sampler(kind) => wgpu::BindingType::Sampler(match kind {
            SamplerBindingType::Filtering => wgpu::SamplerBindingType::Filtering,
            SamplerBindingType::NonFiltering => wgpu::SamplerBindingType::NonFiltering,
            SamplerBindingType::Comparison => wgpu::SamplerBindingType::Comparison,
        }),
        _ => return Err(RhiError::Unsupported("no wgpu mapping for this binding type")),
    })
}

pub(crate) fn vertex_format_to_wgpu(format: VertexFormat) -> Result<wgpu::VertexFormat, RhiError> {
    Ok(match format {
        VertexFormat::Float32 => wgpu::VertexFormat::Float32,
        VertexFormat::Float32x2 => wgpu::VertexFormat::Float32x2,
        VertexFormat::Float32x3 => wgpu::VertexFormat::Float32x3,
        VertexFormat::Float32x4 => wgpu::VertexFormat::Float32x4,
        VertexFormat::Uint32 => wgpu::VertexFormat::Uint32,
        VertexFormat::Uint32x2 => wgpu::VertexFormat::Uint32x2,
        VertexFormat::Uint32x3 => wgpu::VertexFormat::Uint32x3,
        VertexFormat::Uint32x4 => wgpu::VertexFormat::Uint32x4,
        VertexFormat::Sint32 => wgpu::VertexFormat::Sint32,
        VertexFormat::Unorm8x4 => wgpu::VertexFormat::Unorm8x4,
        VertexFormat::Snorm8x4 => wgpu::VertexFormat::Snorm8x4,
        VertexFormat::Float16x2 => wgpu::VertexFormat::Float16x2,
        VertexFormat::Float16x4 => wgpu::VertexFormat::Float16x4,
        _ => return Err(RhiError::Unsupported("no wgpu mapping for this vertex format")),
    })
}

pub(crate) fn step_mode_to_wgpu(mode: VertexStepMode) -> wgpu::VertexStepMode {
    match mode {
        VertexStepMode::Vertex => wgpu::VertexStepMode::Vertex,
        VertexStepMode::Instance => wgpu::VertexStepMode::Instance,
    }
}

pub(crate) fn topology_to_wgpu(topology: PrimitiveTopology) -> wgpu::PrimitiveTopology {
    match topology {
        PrimitiveTopology::PointList => wgpu::PrimitiveTopology::PointList,
        PrimitiveTopology::LineList => wgpu::PrimitiveTopology::LineList,
        PrimitiveTopology::LineStrip => wgpu::PrimitiveTopology::LineStrip,
        PrimitiveTopology::TriangleList => wgpu::PrimitiveTopology::TriangleList,
        PrimitiveTopology::TriangleStrip => wgpu::PrimitiveTopology::TriangleStrip,
    }
}

pub(crate) fn front_face_to_wgpu(face: FrontFace) -> wgpu::FrontFace {
    match face {
        FrontFace::Ccw => wgpu::FrontFace::Ccw,
        FrontFace::Cw => wgpu::FrontFace::Cw,
    }
}

pub(crate) fn cull_mode_to_wgpu(mode: CullMode) -> Option<wgpu::Face> {
    match mode {
        CullMode::None => None,
        CullMode::Front => Some(wgpu::Face::Front),
        CullMode::Back => Some(wgpu::Face::Back),
    }
}

pub(crate) fn index_format_to_wgpu(format: IndexFormat) -> wgpu::IndexFormat {
    match format {
        IndexFormat::Uint16 => wgpu::IndexFormat::Uint16,
        IndexFormat::Uint32 => wgpu::IndexFormat::Uint32,
    }
}

fn blend_factor_to_wgpu(factor: BlendFactor) -> wgpu::BlendFactor {
    match factor {
        BlendFactor::Zero => wgpu::BlendFactor::Zero,
        BlendFactor::One => wgpu::BlendFactor::One,
        BlendFactor::Src => wgpu::BlendFactor::Src,
        BlendFactor::OneMinusSrc => wgpu::BlendFactor::OneMinusSrc,
        BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        BlendFactor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        BlendFactor::Dst => wgpu::BlendFactor::Dst,
        BlendFactor::OneMinusDst => wgpu::BlendFactor::OneMinusDst,
        BlendFactor::DstAlpha => wgpu::BlendFactor::DstAlpha,
        BlendFactor::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
    }
}

fn blend_component_to_wgpu(component: BlendComponent) -> wgpu::BlendComponent {
    wgpu::BlendComponent {
        src_factor: blend_factor_to_wgpu(component.src_factor),
        dst_factor: blend_factor_to_wgpu(component.dst_factor),
        operation: match component.operation {
            BlendOperation::Add => wgpu::BlendOperation::Add,
            BlendOperation::Subtract => wgpu::BlendOperation::Subtract,
            BlendOperation::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
            BlendOperation::Min => wgpu::BlendOperation::Min,
            BlendOperation::Max => wgpu::BlendOperation::Max,
        },
    }
}

pub(crate) fn blend_to_wgpu(blend: BlendState) -> wgpu::BlendState {
    wgpu::BlendState {
        color: blend_component_to_wgpu(blend.color),
        alpha: blend_component_to_wgpu(blend.alpha),
    }
}

pub(crate) fn color_writes_to_wgpu(writes: ColorWrites) -> wgpu::ColorWrites {
    wgpu::ColorWrites::from_bits_truncate(writes.bits())
}

pub(crate) fn present_mode_to_wgpu(mode: PresentMode, supported: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    let wanted = match mode {
        PresentMode::Vsync => wgpu::PresentMode::Fifo,
        PresentMode::LowLatency => wgpu::PresentMode::Mailbox,
        PresentMode::Immediate => wgpu::PresentMode::Immediate,
    };
    if supported.contains(&wanted) {
        wanted
    } else {
        wgpu::PresentMode::Fifo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_round_trips_through_wgpu() {
        for format in Format::ALL {
            assert_eq!(
                format_from_wgpu(format_to_wgpu(*format).unwrap()),
                Some(*format),
                "{format:?}"
            );
        }
    }

    #[test]
    fn every_feature_bit_round_trips_through_wgpu() {
        for (feature, _) in FEATURE_TABLE {
            assert_eq!(features_from_wgpu(features_to_wgpu(*feature)), *feature);
        }
        assert_eq!(
            FEATURE_TABLE
                .iter()
                .fold(Features::empty(), |all, (feature, _)| all | *feature),
            Features::all(),
            "every pathon feature bit needs a wgpu mapping"
        );
    }
}
