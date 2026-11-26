use std::collections::HashMap;
use std::sync::Arc;

use pathon_api::rhi::{
    BindGroupLayoutDesc, BindGroupLayoutId, BlendState, ColorTargetState, CompareFunction, ComputePipelineDesc,
    ComputePipelineId, CullMode, DepthBias, DepthState, DeviceApi, Format, PrimitiveState, RenderPipelineDesc,
    RenderPipelineId, RhiError, ShaderStage, VertexBufferLayout,
};
use pathon_ibl::IblBindings;
use pathon_material::{AlphaKind, MaterialHooks, MaterialVariant};
use pathon_shader::{PipelineCache, ShaderError, ShaderKey, ShaderLibrary, reflect_bind_groups};
use pathon_shadow::ShadowBindings;

pub(crate) const COMMON_MODULE: &str = "pbr::common";
pub(crate) const LIGHTS_MODULE: &str = "pbr::lights";
pub(crate) const OCCLUSION_MODULE: &str = "pbr::occlusion";
pub(crate) const HOOKS_MODULE: &str = "pbr::hooks";

const COMMON_WESL: &str = include_str!("../wgsl/common.wesl");
pub(crate) const AREA_MODULE: &str = "pbr::area";

const AREA_WESL: &str = include_str!("../wgsl/area.wesl");
const LIGHTS_WESL: &str = include_str!("../wgsl/lights.wesl");
const OCCLUSION_WESL: &str = include_str!("../wgsl/occlusion.wesl");
const HOOKS_WESL: &str = include_str!("../wgsl/hooks.wesl");
const PREPASS_WESL: &str = include_str!("../wgsl/prepass.wesl");
const FORWARD_WESL: &str = include_str!("../wgsl/forward.wesl");
const CLUSTER_WESL: &str = include_str!("../wgsl/cluster.wesl");
const DEPTH_PYRAMID_WESL: &str = include_str!("../wgsl/depth_pyramid.wesl");

/// First binding of the environment maps in the colour pass (after frame, materials, instances, lights and the two
/// cluster buffers).
pub const IBL_FIRST_BINDING: u32 = 6;
/// First binding of the shadow cascades, the map and its sampler (right after the four environment bindings); they
/// exist only in the permutation with the `shadows` flag.
pub const SHADOW_FIRST_BINDING: u32 = IBL_FIRST_BINDING + IblBindings::COUNT;
/// WESL flag of the colour pass: the light that `pathon-shadow` shadows is attenuated by its cascaded map.
pub const SHADOWS_FLAG: &str = "shadows";
/// WESL flag of the colour pass: the scene has area lights, so the shader carries their code and reads the LTC table.
pub const AREA_LIGHTS_FLAG: &str = "area_lights";
/// Binding of the LTC table (`crate::ltc`), right after the shadow bindings; it exists only with the `area_lights`
/// flag.
pub const LTC_BINDING: u32 = SHADOW_FIRST_BINDING + ShadowBindings::COUNT;
/// The group of the material textures when a `TextureBinder` is present.
pub const TEXTURE_GROUP: u32 = 1;

pub const HDR_FORMAT: Format = Format::Rgba16Float;
pub(crate) const DEPTH_FORMAT: Format = Format::Depth32Float;

type ReflectionKey = (ShaderKey, Vec<String>, usize);

/// The WESL modules and roots of the pack, the pipelines built from them and the bind group layouts reflection found.
/// Pipelines are made on first use of a permutation: a material the scene never draws costs nothing.
#[derive(Debug)]
pub struct PbrShaders {
    library: ShaderLibrary,
    pipelines: PipelineCache,
    layouts: HashMap<BindGroupLayoutDesc, BindGroupLayoutId>,
    // Reflection parses the composed WGSL, so a permutation is reflected once, not on every frame that draws it.
    reflected: HashMap<ReflectionKey, BindGroupLayoutId>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PrepassKey {
    pub masked: bool,
    /// Also write `roughness` and `albedo`.
    pub full: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ForwardKey {
    pub variant: MaterialVariant,
    /// Also write the `diffuse` and `specular` slots.
    pub split: bool,
    /// Read the cascaded shadow map of `pathon-shadow` (the `shadows` flag): three more bindings in group 0.
    pub shadows: bool,
    /// Shade area lights (the `area_lights` flag): one more binding in group 0, the LTC table.
    pub area: bool,
}

impl ForwardKey {
    pub fn shader_key(self) -> ShaderKey {
        let mut key = self.variant.shader_key("forward");
        key.features
            .set(SHADOWS_FLAG, self.shadows)
            .expect("the shadows flag is a WGSL identifier");
        key.features
            .set(AREA_LIGHTS_FLAG, self.area)
            .expect("the area_lights flag is a WGSL identifier");
        key
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RenderPipeline {
    pub pipeline: RenderPipelineId,
    /// The layout of group 0 as the shader uses it; the bind group of the pass is made from it.
    pub frame_layout: BindGroupLayoutId,
}

impl PbrShaders {
    /// `hooks` carry the texture source of the `TextureBinder` and any surface hook of the application.
    pub fn new(hooks: &MaterialHooks) -> Result<Self, ShaderError> {
        let mut library = ShaderLibrary::new();
        for (path, source) in hooks.modules() {
            library.add_module(path, source)?;
        }
        for (path, source) in pathon_ibl::modules() {
            library.add_module(path, source)?;
        }
        for (path, source) in pathon_shadow::modules() {
            library.add_module(path, source)?;
        }
        for (path, source) in [
            (COMMON_MODULE, COMMON_WESL),
            (LIGHTS_MODULE, LIGHTS_WESL),
            (AREA_MODULE, AREA_WESL),
            (OCCLUSION_MODULE, OCCLUSION_WESL),
            (HOOKS_MODULE, HOOKS_WESL),
        ] {
            library.add_module(path, source)?;
        }
        let ibl = IblBindings::starting_at(IBL_FIRST_BINDING).wgsl_declarations(0);
        let forward = FORWARD_WESL
            .replace("// IBL_BINDINGS", &ibl)
            .replace("// SHADOW_BINDINGS", &shadow_declarations())
            .replace("// LTC_BINDING", &ltc_declaration());
        library.add_root("forward", &forward);
        library.add_root("prepass", PREPASS_WESL);
        library.add_root("cluster", CLUSTER_WESL);
        library.add_root("depth_pyramid", DEPTH_PYRAMID_WESL);
        Ok(Self {
            library,
            pipelines: PipelineCache::new(),
            layouts: HashMap::new(),
            reflected: HashMap::new(),
        })
    }

    pub fn group_layout(
        &mut self,
        device: &dyn DeviceApi,
        desc: &BindGroupLayoutDesc,
    ) -> Result<BindGroupLayoutId, RhiError> {
        if let Some(layout) = self.layouts.get(desc) {
            return Ok(*layout);
        }
        let layout = device.create_bind_group_layout(desc)?;
        self.layouts.insert(desc.clone(), layout);
        Ok(layout)
    }

    // The layout of group 0 for `entries` of `key`, as reflection finds it.
    fn reflected_group_zero(
        &mut self,
        device: &dyn DeviceApi,
        key: &ShaderKey,
        entries: &[&str],
        adjust: fn(&mut BindGroupLayoutDesc),
    ) -> Result<BindGroupLayoutId, RhiError> {
        let entry_names = entries.iter().map(|entry| (*entry).to_owned()).collect();
        let memo_key = (key.clone(), entry_names, adjust as usize);
        if let Some(layout) = self.reflected.get(&memo_key) {
            return Ok(*layout);
        }
        let wgsl = self.library.wgsl(key).map_err(|error| shader_error(&error))?;
        let mut groups = reflect_bind_groups(&wgsl, entries).map_err(|error| shader_error(&error))?;
        let mut group_zero = if groups.is_empty() {
            BindGroupLayoutDesc {
                label: "empty",
                entries: Vec::new(),
            }
        } else {
            groups.swap_remove(0)
        };
        adjust(&mut group_zero);
        let layout = self.group_layout(device, &group_zero)?;
        self.reflected.insert(memo_key, layout);
        Ok(layout)
    }

    pub fn prepass_pipeline(
        &mut self,
        device: &dyn DeviceApi,
        key: PrepassKey,
        vertex_buffers: &[VertexBufferLayout],
        texture_layout: Option<BindGroupLayoutId>,
    ) -> Result<RenderPipeline, RhiError> {
        let alpha = if key.masked {
            AlphaKind::Masked
        } else {
            AlphaKind::Opaque
        };
        let shader = MaterialVariant {
            alpha,
            ..MaterialVariant::default()
        }
        .shader_key("prepass");
        let fragment = if key.full { "fs_full" } else { "fs" };
        let frame_layout = self.reflected_group_zero(device, &shader, &["vs", fragment], |_| {})?;
        let module = self
            .library
            .module(device, &shader)
            .map_err(|error| shader_error(&error))?;
        let mut targets = vec![
            ColorTargetState::opaque(Format::Rg16Float),
            ColorTargetState::opaque(Format::Rg16Float),
            ColorTargetState::opaque(Format::R32Uint),
        ];
        if key.full {
            targets.push(ColorTargetState::opaque(Format::R8Unorm));
            targets.push(ColorTargetState::opaque(Format::Rgba8Unorm));
        }
        let pipeline = self.pipelines.render(
            device,
            &RenderPipelineDesc {
                label: "pbr prepass",
                bind_group_layouts: layouts_with_textures(frame_layout, texture_layout),
                vertex: ShaderStage {
                    module,
                    entry_point: "vs",
                    constants: vec![],
                },
                vertex_buffers: vertex_buffers.to_vec(),
                fragment: Some(ShaderStage {
                    module,
                    entry_point: fragment,
                    constants: vec![],
                }),
                color_targets: targets,
                primitive: PrimitiveState {
                    cull_mode: CullMode::None,
                    ..PrimitiveState::default()
                },
                depth: Some(DepthState::reverse_z(DEPTH_FORMAT)),
                sample_count: 1,
                immediate_size: 0,
            },
        )?;
        Ok(RenderPipeline { pipeline, frame_layout })
    }

    pub fn forward_pipeline(
        &mut self,
        device: &dyn DeviceApi,
        key: ForwardKey,
        vertex_buffers: &[VertexBufferLayout],
        texture_layout: Option<BindGroupLayoutId>,
    ) -> Result<RenderPipeline, RhiError> {
        let shader = key.shader_key();
        let fragment = if key.split { "fs_split" } else { "fs" };
        let frame_layout = self.reflected_group_zero(device, &shader, &["vs", fragment], |_| {})?;
        let module = self
            .library
            .module(device, &shader)
            .map_err(|error| shader_error(&error))?;
        let transparent = key.variant.alpha == AlphaKind::Transparent;
        let mut targets = vec![ColorTargetState {
            blend: transparent.then_some(BlendState::PREMULTIPLIED_ALPHA),
            ..ColorTargetState::opaque(HDR_FORMAT)
        }];
        if key.split {
            targets.push(ColorTargetState::opaque(HDR_FORMAT));
            targets.push(ColorTargetState::opaque(HDR_FORMAT));
        }
        // The prepass wrote the depth of everything opaque: the colour pass only keeps what is at that depth or nearer.
        let depth = DepthState {
            format: DEPTH_FORMAT,
            write_enabled: false,
            compare: CompareFunction::GreaterEqual,
            bias: DepthBias::NONE,
        };
        let pipeline = self.pipelines.render(
            device,
            &RenderPipelineDesc {
                label: if transparent { "pbr transparent" } else { "pbr forward" },
                bind_group_layouts: layouts_with_textures(frame_layout, texture_layout),
                vertex: ShaderStage {
                    module,
                    entry_point: "vs",
                    constants: vec![],
                },
                vertex_buffers: vertex_buffers.to_vec(),
                fragment: Some(ShaderStage {
                    module,
                    entry_point: fragment,
                    constants: vec![],
                }),
                color_targets: targets,
                primitive: PrimitiveState {
                    cull_mode: CullMode::None,
                    ..PrimitiveState::default()
                },
                depth: Some(depth),
                sample_count: 1,
                immediate_size: 0,
            },
        )?;
        Ok(RenderPipeline { pipeline, frame_layout })
    }

    /// A compute pipeline of the root `root` and its group 0 layout from reflection.
    pub fn compute_pipeline(
        &mut self,
        device: &dyn DeviceApi,
        root: &str,
        entry_point: &'static str,
        label: &'static str,
        adjust: fn(&mut BindGroupLayoutDesc),
    ) -> Result<(ComputePipelineId, BindGroupLayoutId), RhiError> {
        let key = ShaderKey {
            root: root.to_owned(),
            features: pathon_shader::ShaderFeatures::default(),
        };
        let layout = self.reflected_group_zero(device, &key, &[entry_point], adjust)?;
        let module = self
            .library
            .module(device, &key)
            .map_err(|error| shader_error(&error))?;
        let pipeline = self.pipelines.compute(
            device,
            &ComputePipelineDesc {
                label,
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point,
                    constants: vec![],
                },
                immediate_size: 0,
            },
        )?;
        Ok((pipeline, layout))
    }

    /// A compute pipeline with a layout the caller chose, for roots whose entry points use different bindings but
    /// share one bind group.
    pub fn compute_pipeline_with_layout(
        &mut self,
        device: &dyn DeviceApi,
        root: &str,
        entry_point: &'static str,
        label: &'static str,
        layout: BindGroupLayoutId,
    ) -> Result<ComputePipelineId, RhiError> {
        let key = ShaderKey {
            root: root.to_owned(),
            features: pathon_shader::ShaderFeatures::default(),
        };
        let module = self
            .library
            .module(device, &key)
            .map_err(|error| shader_error(&error))?;
        self.pipelines.compute(
            device,
            &ComputePipelineDesc {
                label,
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point,
                    constants: vec![],
                },
                immediate_size: 0,
            },
        )
    }

    /// The composed WGSL of `key`, for tests and reflection.
    pub fn wgsl(&mut self, key: &ShaderKey) -> Result<Arc<str>, ShaderError> {
        self.library.wgsl(key)
    }
}

// Every declaration carries `@if(shadows)`, so the permutation without shadows has the layout it had before.
fn shadow_declarations() -> String {
    ShadowBindings::starting_at(SHADOW_FIRST_BINDING)
        .wgsl_declarations(0)
        .lines()
        .map(|line| ["@if(", SHADOWS_FLAG, ") ", line, "\n"].concat())
        .collect()
}

// The LTC table exists only in the permutation with the `area_lights` flag.
fn ltc_declaration() -> String {
    format!(
        "@if({AREA_LIGHTS_FLAG}) @group(0) @binding({LTC_BINDING}) var<storage, read> ltc_table: array<vec4<f32>>;
"
    )
}

fn shader_error(error: &ShaderError) -> RhiError {
    RhiError::validation("L0006", error.to_string())
}

fn layouts_with_textures(frame: BindGroupLayoutId, textures: Option<BindGroupLayoutId>) -> Vec<BindGroupLayoutId> {
    std::iter::once(frame).chain(textures).collect()
}
