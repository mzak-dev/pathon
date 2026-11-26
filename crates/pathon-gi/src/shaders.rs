use std::collections::HashMap;
use std::num::NonZeroU32;

use pathon_api::rhi::{
    BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId, BindingType, ComputePipelineDesc, ComputePipelineId,
    DeviceApi, Format, RhiError, SamplerBindingType, ShaderStage, ShaderStages, StorageAccess, TextureSampleType,
    TextureViewDimension,
};
use pathon_material::MaterialHooks;
use pathon_shader::{PipelineCache, ShaderError, ShaderFeatures, ShaderKey, ShaderLibrary};

/// Path of the sampling module as `ShaderLibrary::add_module` takes it; shaders import it as `package::gi::sample`.
pub const SAMPLE_MODULE: &str = "gi::sample";
/// Octahedral mapping, the volume uniform and the packing of colours, imported as `package::gi::common`.
pub const COMMON_MODULE: &str = "gi::common";

const COMMON_WESL: &str = include_str!("../wgsl/common.wesl");
const SAMPLE_WESL: &str = include_str!("../wgsl/sample.wesl");
const BINDINGS_WGSL: &str = include_str!("../wgsl/bindings.wgsl");
const VOXELS_WGSL: &str = include_str!("../wgsl/voxels.wgsl");
const CLEAR_WESL: &str = include_str!("../wgsl/clear.wesl");
const VOXELIZE_WESL: &str = include_str!("../wgsl/voxelize.wesl");
const COMPACT_WESL: &str = include_str!("../wgsl/compact.wesl");
const LIGHT_WESL: &str = include_str!("../wgsl/light.wesl");
const TRACE_WESL: &str = include_str!("../wgsl/trace.wesl");
const UPDATE_WESL: &str = include_str!("../wgsl/update.wesl");

/// The group of the material textures in the voxelizer, when the scene has a `TextureBinder`.
pub(crate) const TEXTURE_GROUP: u32 = 2;

/// The WESL modules a consumer of the probe volume needs, as `(path, source)` for `ShaderLibrary::add_module` or
/// `BackgroundCompiler::add_module`: the sampling function and what it imports.
pub fn modules() -> [(&'static str, &'static str); 2] {
    [(COMMON_MODULE, COMMON_WESL), (SAMPLE_MODULE, SAMPLE_WESL)]
}

/// The compute passes of the pack.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Pass {
    Clear,
    Voxelize,
    Compact,
    Light,
    Trace,
    UpdateIrradiance,
    UpdateVisibility,
}

impl Pass {
    fn root(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Voxelize => "voxelize",
            Self::Compact => "compact",
            Self::Light => "light",
            Self::Trace => "trace",
            Self::UpdateIrradiance | Self::UpdateVisibility => "update",
        }
    }

    fn entry_point(self) -> &'static str {
        match self {
            Self::Clear => "clear_voxels",
            Self::Voxelize => "voxelize",
            Self::Compact => "compact_solid",
            Self::Light => "light_voxels",
            Self::Trace => "trace_probes",
            Self::UpdateIrradiance => "update_irradiance",
            Self::UpdateVisibility => "update_visibility",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Clear => "gi clear voxels",
            Self::Voxelize => "gi voxelize",
            Self::Compact => "gi compact solid cells",
            Self::Light => "gi light voxels",
            Self::Trace => "gi trace probes",
            Self::UpdateIrradiance => "gi update irradiance",
            Self::UpdateVisibility => "gi update visibility",
        }
    }
}

fn entry(binding: u32, ty: BindingType) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty,
        count: None::<NonZeroU32>,
    }
}

const UNIFORM: BindingType = BindingType::UniformBuffer;
const STORAGE: BindingType = BindingType::StorageBuffer {
    access: StorageAccess::ReadWrite,
};
const STORAGE_READ: BindingType = BindingType::StorageBuffer {
    access: StorageAccess::ReadOnly,
};
const SAMPLED_2D: BindingType = BindingType::SampledTexture {
    sample_type: TextureSampleType::Float,
    dimension: TextureViewDimension::D2,
    multisampled: false,
};
const SAMPLED_CUBE: BindingType = BindingType::SampledTexture {
    sample_type: TextureSampleType::Float,
    dimension: TextureViewDimension::Cube,
    multisampled: false,
};
const SAMPLER: BindingType = BindingType::Sampler(SamplerBindingType::Filtering);
const STORAGE_TEXTURE: BindingType = BindingType::StorageTexture {
    access: StorageAccess::WriteOnly,
    format: Format::Rgba16Float,
    dimension: TextureViewDimension::D2,
};

/// Group 0 of a pass: the subset of the bindings of `bindings.wgsl` the pass reads or writes.
pub(crate) fn group_zero_bindings(pass: Pass) -> Vec<BindGroupLayoutEntry> {
    let bindings: &[(u32, BindingType)] = match pass {
        Pass::Clear => &[(1, UNIFORM), (3, STORAGE), (4, STORAGE), (5, STORAGE), (15, STORAGE)],
        Pass::Compact => &[(1, UNIFORM), (3, STORAGE), (15, STORAGE)],
        Pass::Voxelize => &[(1, UNIFORM), (3, STORAGE), (4, STORAGE)],
        Pass::Light => &[
            (0, UNIFORM),
            (1, UNIFORM),
            (2, UNIFORM),
            (3, STORAGE),
            (4, STORAGE),
            (5, STORAGE),
            (6, STORAGE_READ),
            (15, STORAGE),
            (8, SAMPLED_2D),
            (9, SAMPLED_2D),
            (10, SAMPLER),
        ],
        Pass::Trace => &[
            (0, UNIFORM),
            (1, UNIFORM),
            (2, UNIFORM),
            (3, STORAGE),
            (5, STORAGE),
            (7, STORAGE),
            (13, SAMPLED_CUBE),
            (14, SAMPLER),
        ],
        Pass::UpdateIrradiance => &[
            (0, UNIFORM),
            (2, UNIFORM),
            (7, STORAGE),
            (8, SAMPLED_2D),
            (11, STORAGE_TEXTURE),
        ],
        Pass::UpdateVisibility => &[
            (0, UNIFORM),
            (2, UNIFORM),
            (7, STORAGE),
            (9, SAMPLED_2D),
            (12, STORAGE_TEXTURE),
        ],
    };
    bindings.iter().map(|(binding, ty)| entry(*binding, *ty)).collect()
}

/// Group 1 of the voxelizer: the job, the two vertex streams and the indices of its mesh.
fn job_bindings() -> Vec<BindGroupLayoutEntry> {
    vec![
        entry(0, UNIFORM),
        entry(1, STORAGE_READ),
        entry(2, STORAGE_READ),
        entry(3, STORAGE_READ),
    ]
}

/// The shader library of the pack and the pipelines made from it.
#[derive(Debug)]
pub(crate) struct GiShaders {
    library: ShaderLibrary,
    pipelines: PipelineCache,
    group_zero: HashMap<Pass, BindGroupLayoutId>,
    job_layout: BindGroupLayoutId,
    texture_layout: Option<BindGroupLayoutId>,
}

fn shader_error(error: &ShaderError) -> RhiError {
    RhiError::validation("J0006", error.to_string())
}

impl GiShaders {
    /// `hooks` carry the texture source of the scene's `TextureBinder`; `texture_layout` is its layout, the voxelizer's
    /// group [`TEXTURE_GROUP`].
    pub(crate) fn new(
        device: &dyn DeviceApi,
        hooks: &MaterialHooks,
        texture_layout: Option<BindGroupLayoutId>,
    ) -> Result<Self, RhiError> {
        let mut library = ShaderLibrary::new();
        for (path, source) in hooks.modules().into_iter().chain(modules()) {
            library.add_module(path, source).map_err(|error| shader_error(&error))?;
        }
        for (name, source) in [
            ("clear", CLEAR_WESL),
            ("voxelize", VOXELIZE_WESL),
            ("compact", COMPACT_WESL),
            ("light", LIGHT_WESL),
            ("trace", TRACE_WESL),
            ("update", UPDATE_WESL),
        ] {
            let text = source
                .replace("// GI_BINDINGS", BINDINGS_WGSL)
                .replace("// GI_VOXELS", VOXELS_WGSL);
            library.add_root(name, &text);
        }
        let mut group_zero = HashMap::new();
        for pass in [
            Pass::Clear,
            Pass::Voxelize,
            Pass::Compact,
            Pass::Light,
            Pass::Trace,
            Pass::UpdateIrradiance,
            Pass::UpdateVisibility,
        ] {
            group_zero.insert(
                pass,
                device.create_bind_group_layout(&BindGroupLayoutDesc {
                    label: pass.label(),
                    entries: group_zero_bindings(pass),
                })?,
            );
        }
        let job_layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "gi voxelize job",
            entries: job_bindings(),
        })?;
        Ok(Self {
            library,
            pipelines: PipelineCache::new(),
            group_zero,
            job_layout,
            texture_layout,
        })
    }

    pub(crate) fn group_zero_layout(&self, pass: Pass) -> BindGroupLayoutId {
        self.group_zero[&pass]
    }

    pub(crate) fn job_layout(&self) -> BindGroupLayoutId {
        self.job_layout
    }

    pub(crate) fn pipeline(&mut self, device: &dyn DeviceApi, pass: Pass) -> Result<ComputePipelineId, RhiError> {
        let key = ShaderKey {
            root: pass.root().to_owned(),
            features: ShaderFeatures::default(),
        };
        let module = self
            .library
            .module(device, &key)
            .map_err(|error| shader_error(&error))?;
        let mut layouts = vec![self.group_zero[&pass]];
        if pass == Pass::Voxelize {
            layouts.push(self.job_layout);
            layouts.extend(self.texture_layout);
        }
        self.pipelines.compute(
            device,
            &ComputePipelineDesc {
                label: pass.label(),
                bind_group_layouts: layouts,
                shader: ShaderStage {
                    module,
                    entry_point: pass.entry_point(),
                    constants: vec![],
                },
                immediate_size: 0,
            },
        )
    }
}
