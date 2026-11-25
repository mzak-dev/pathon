// The viewer as a pack (M4-10): the one `RenderFeature` that draws the test scene. It plays the part of the packs that
// come later (pbr, shadows, post) in the simplest form that makes the scene readable: a sun shadow map, a forward
// pass with the standard material, and an ACES tonemap into the target. It reads scene data from `Shared`:
// `SharedAssets` (host, once) and `ViewerSettings` (host, every frame).

use std::collections::HashMap;
use std::sync::Arc;

use pathon::api::glam::{Mat4, Vec3};
use pathon::api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon::api::resources::UniformRing;
use pathon::api::{RecordCtx, RenderFeature, SetupCtx, Slot};
use pathon::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, BlendState, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment,
    ColorTargetState, CompareFunction, CullMode, DepthAttachment, DepthState, DeviceApi, Format, PrimitiveState,
    RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError, SamplerBindingType, SamplerDesc, SamplerId,
    ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages, StorageAccess, TextureSampleType,
    TextureViewDimension,
};
use pathon_material::{AlphaKind, MaterialHooks, MaterialVariant};
use pathon_shader::ShaderLibrary;

use super::gpu_data::{
    DrawCall, FrameUniform, InstanceData, LightData, PipelineKey, SHADOW_MAP_SIZE, SceneDraws, build_draws,
    build_lights, sun_view_projection,
};
use super::settings::{DEFAULT_EXPOSURE, ViewerSettings};
use super::upload::{SharedAssets, ViewerAssets};

const FORWARD_ROOT: &str = "forward";
const FORWARD_SOURCE: &str = include_str!("forward.wesl");
const TEXTURE_SOURCE: &str = include_str!("texture_source.wesl");
const TONEMAP_SOURCE: &str = include_str!("tonemap.wgsl");
const HDR_FORMAT: Format = Format::Rgba16Float;
const DEPTH_FORMAT: Format = Format::Depth32Float;
const UNIFORM_RING_BYTES: u64 = 64 << 10;
const SKY_COLOR: [f64; 3] = [0.35, 0.5, 0.8];
// Pre-tonemap radiance of the backdrop at the default exposure.
const SKY_RADIANCE: f32 = 2.0;

fn shader_error(error: impl std::fmt::Display) -> RhiError {
    RhiError::validation("S0000", format!("viewer shader: {error}"))
}

fn buffer_with(
    device: &dyn DeviceApi,
    label: &'static str,
    usage: BufferUsage,
    bytes: &[u8],
) -> Result<BufferId, RhiError> {
    // A zero-sized binding is invalid: an empty scene still gets one element.
    let size = bytes.len().max(16) as u64;
    let buffer = device.create_buffer(&BufferDesc {
        label,
        size,
        usage: usage | BufferUsage::COPY_DST,
    })?;
    if !bytes.is_empty() {
        device.write_buffer(buffer, 0, bytes)?;
    }
    Ok(buffer)
}

fn layout_entry(binding: u32, visibility: ShaderStages, ty: BindingType) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

fn read_only_storage() -> BindingType {
    BindingType::StorageBuffer {
        access: StorageAccess::ReadOnly,
    }
}

#[derive(Debug)]
struct Gpu {
    assets: Arc<ViewerAssets>,
    forward_layout: BindGroupLayoutId,
    shadow_layout: BindGroupLayoutId,
    tonemap_layout: BindGroupLayoutId,
    instance_buffer: BufferId,
    light_buffer: BufferId,
    shadow_sampler: SamplerId,
    forward_modules: HashMap<AlphaKind, ShaderModuleId>,
    shadow_module: ShaderModuleId,
    tonemap_module: ShaderModuleId,
    forward_pipelines: HashMap<PipelineKey, RenderPipelineId>,
    shadow_pipelines: HashMap<bool, RenderPipelineId>,
    tonemap_pipelines: HashMap<Format, RenderPipelineId>,
    draws: SceneDraws,
    solid: Arc<[DrawCall]>,
    light_count: u32,
    sun: Option<usize>,
    sun_direction: Vec3,
    ring: UniformRing,
    ring_epoch: Option<u64>,
}

impl Gpu {
    fn build(device: &dyn DeviceApi, assets: Arc<ViewerAssets>) -> Result<Self, RhiError> {
        let world = Arc::clone(&assets.snapshot);
        let draws = build_draws(&world, &assets.materials, &assets.material_ids);
        let (lights, sun) = build_lights(&world);

        let mut library = ShaderLibrary::new();
        let mut hooks = MaterialHooks::default();
        hooks.set_texture_source(TEXTURE_SOURCE);
        for (path, source) in hooks.modules() {
            library.add_module(path, source).map_err(shader_error)?;
        }
        library.add_root(FORWARD_ROOT, FORWARD_SOURCE);
        let mut forward_modules = HashMap::new();
        for (alpha, variant) in &draws.variants {
            let module = library
                .module(device, &variant.shader_key(FORWARD_ROOT))
                .map_err(shader_error)?;
            forward_modules.insert(*alpha, module);
        }
        // The shadow pass reads only `vs_shadow`, which every permutation carries unchanged.
        let shadow_module = match forward_modules.values().next() {
            Some(module) => *module,
            None => library
                .module(device, &MaterialVariant::default().shader_key(FORWARD_ROOT))
                .map_err(shader_error)?,
        };
        let tonemap_module = device.create_shader_module(&ShaderModuleDesc {
            label: "viewer tonemap",
            wgsl: TONEMAP_SOURCE,
        })?;

        let both = ShaderStages::VERTEX_FRAGMENT;
        let forward_layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "viewer frame",
            entries: vec![
                layout_entry(0, both, BindingType::UniformBuffer),
                layout_entry(1, ShaderStages::FRAGMENT, read_only_storage()),
                layout_entry(2, ShaderStages::VERTEX, read_only_storage()),
                layout_entry(3, ShaderStages::FRAGMENT, read_only_storage()),
                layout_entry(
                    4,
                    ShaderStages::FRAGMENT,
                    BindingType::SampledTexture {
                        sample_type: TextureSampleType::Depth,
                        dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    5,
                    ShaderStages::FRAGMENT,
                    BindingType::Sampler(SamplerBindingType::Comparison),
                ),
            ],
        })?;
        let shadow_layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "viewer shadow frame",
            entries: vec![
                layout_entry(0, ShaderStages::VERTEX, BindingType::UniformBuffer),
                layout_entry(2, ShaderStages::VERTEX, read_only_storage()),
            ],
        })?;
        let tonemap_layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "viewer tonemap",
            entries: vec![layout_entry(0, ShaderStages::FRAGMENT, BindingType::texture_2d())],
        })?;
        let shadow_sampler = device.create_sampler(&SamplerDesc {
            label: "sun shadow",
            compare: Some(CompareFunction::GreaterEqual),
            mag_filter: pathon::rhi::FilterMode::Linear,
            min_filter: pathon::rhi::FilterMode::Linear,
            ..SamplerDesc::NEAREST_CLAMP
        })?;

        let sun_direction = sun
            .and_then(|index| world.lights().iter().nth(index))
            .map_or(Vec3::NEG_Y, |(_, light)| light.direction());
        let solid: Arc<[DrawCall]> = draws.solid.clone().into();
        let mut gpu = Self {
            instance_buffer: buffer_with(
                device,
                "viewer instances",
                BufferUsage::STORAGE,
                bytemuck::cast_slice::<InstanceData, u8>(&draws.instances),
            )?,
            light_buffer: buffer_with(
                device,
                "viewer lights",
                BufferUsage::STORAGE,
                bytemuck::cast_slice::<LightData, u8>(&lights),
            )?,
            light_count: u32::try_from(lights.len()).expect("at most MAX_LIGHTS"),
            ring: UniformRing::new(device, "viewer frame uniforms", UNIFORM_RING_BYTES)?,
            assets,
            forward_layout,
            shadow_layout,
            tonemap_layout,
            shadow_sampler,
            forward_modules,
            shadow_module,
            tonemap_module,
            forward_pipelines: HashMap::new(),
            shadow_pipelines: HashMap::new(),
            tonemap_pipelines: HashMap::new(),
            draws,
            solid,
            sun,
            sun_direction,
            ring_epoch: None,
        };
        gpu.create_scene_pipelines(device)?;
        Ok(gpu)
    }

    fn create_scene_pipelines(&mut self, device: &dyn DeviceApi) -> Result<(), RhiError> {
        let standard = self.assets.pool.layout().vertex_buffer_layouts();
        let keys: Vec<PipelineKey> = self
            .draws
            .solid
            .iter()
            .chain(&self.draws.blended)
            .map(|call| call.key)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        for key in keys {
            let module = self.forward_modules[&key.alpha];
            let transparent = key.alpha == AlphaKind::Transparent;
            let mut target = ColorTargetState::opaque(HDR_FORMAT);
            if transparent {
                target.blend = Some(BlendState::PREMULTIPLIED_ALPHA);
            }
            let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
                label: "viewer forward",
                bind_group_layouts: vec![self.forward_layout, self.assets.material_layout],
                vertex: ShaderStage {
                    module,
                    entry_point: "vs",
                    constants: vec![],
                },
                vertex_buffers: standard.clone(),
                fragment: Some(ShaderStage {
                    module,
                    entry_point: "fs",
                    constants: vec![],
                }),
                color_targets: vec![target],
                primitive: PrimitiveState {
                    cull_mode: if key.double_sided {
                        CullMode::None
                    } else {
                        CullMode::Back
                    },
                    ..PrimitiveState::default()
                },
                depth: Some(DepthState {
                    write_enabled: !transparent,
                    ..DepthState::reverse_z(DEPTH_FORMAT)
                }),
                sample_count: 1,
                immediate_size: 0,
            })?;
            self.forward_pipelines.insert(key, pipeline);
        }
        for double_sided in [false, true] {
            let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
                label: "viewer sun shadow",
                bind_group_layouts: vec![self.shadow_layout],
                vertex: ShaderStage {
                    module: self.shadow_module,
                    entry_point: "vs_shadow",
                    constants: vec![],
                },
                vertex_buffers: standard[..1].to_vec(),
                fragment: None,
                color_targets: vec![],
                primitive: PrimitiveState {
                    cull_mode: if double_sided { CullMode::None } else { CullMode::Back },
                    ..PrimitiveState::default()
                },
                depth: Some(DepthState::reverse_z(DEPTH_FORMAT)),
                sample_count: 1,
                immediate_size: 0,
            })?;
            self.shadow_pipelines.insert(double_sided, pipeline);
        }
        Ok(())
    }

    fn tonemap_pipeline(&mut self, device: &dyn DeviceApi, format: Format) -> Result<RenderPipelineId, RhiError> {
        if let Some(pipeline) = self.tonemap_pipelines.get(&format) {
            return Ok(*pipeline);
        }
        let entry_point = if format.is_srgb() {
            "fs_srgb_target"
        } else {
            "fs_linear_target"
        };
        let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
            label: "viewer tonemap",
            bind_group_layouts: vec![self.tonemap_layout],
            vertex: ShaderStage {
                module: self.tonemap_module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: vec![],
            fragment: Some(ShaderStage {
                module: self.tonemap_module,
                entry_point,
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(format)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.tonemap_pipelines.insert(format, pipeline);
        Ok(pipeline)
    }

    // Back to front by the distance of the instance centre; good enough for windows and a wire bin.
    fn blended_back_to_front(&self, eye: Vec3) -> Vec<DrawCall> {
        let mut blended = self.draws.blended.clone();
        blended.sort_by(|a, b| {
            let (da, db) = (a.center.distance_squared(eye), b.center.distance_squared(eye));
            db.total_cmp(&da)
        });
        blended
    }
}

/// The pack of the test-scene viewer; the host puts a [`SharedAssets`] into `Shared` after the upload.
#[derive(Debug, Default)]
pub struct ViewerPack {
    gpu: Option<Gpu>,
}

impl ViewerPack {
    pub fn new() -> Self {
        Self::default()
    }
}

fn draw_calls(
    pass: &mut pathon::rhi::RenderPassEncoder<'_>,
    assets: &ViewerAssets,
    calls: impl Iterator<Item = DrawCall>,
    pipelines: &dyn Fn(PipelineKey) -> RenderPipelineId,
    with_materials: bool,
) -> Result<(), RhiError> {
    let (mut current_key, mut current_material) = (None, None);
    for call in calls {
        if current_key != Some(call.key) {
            pass.set_pipeline(pipelines(call.key));
            current_key = Some(call.key);
        }
        if with_materials && current_material != Some(call.material) {
            pass.set_bind_group(1, assets.material_groups[call.material as usize]);
            current_material = Some(call.material);
        }
        assets
            .pool
            .draw(pass, call.mesh, call.instance..call.instance + 1)
            .map_err(|error| RhiError::validation("M0006", error.to_string()))?;
    }
    Ok(())
}

impl RenderFeature for ViewerPack {
    fn name(&self) -> &'static str {
        "gltf viewer"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn on_device_reset(&mut self, _ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        // The assets belong to the host and are dead with the device; the host uploads them again.
        self.gpu = None;
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(output) = ctx.slot(Slot::OUTPUT) else {
            return Ok(());
        };
        let Some(assets) = ctx.shared().get::<SharedAssets>().map(|shared| Arc::clone(&shared.0)) else {
            return Ok(());
        };
        let settings = ctx.shared().get::<ViewerSettings>().copied().unwrap_or_default();
        let output_format = ctx
            .graph()
            .texture_format(output)
            .ok_or(RhiError::InvalidHandle("output"))?;

        if self.gpu.as_ref().is_none_or(|gpu| !Arc::ptr_eq(&gpu.assets, &assets)) {
            self.gpu = Some(Gpu::build(ctx.device(), assets)?);
        }
        let gpu = self.gpu.as_mut().ok_or(RhiError::InvalidHandle("viewer gpu state"))?;
        let tonemap_pipeline = gpu.tonemap_pipeline(ctx.device(), output_format)?;

        let view = ctx.view();
        let camera = view.camera;
        let record_epoch = view.record_epoch;
        let has_sun = settings.sun_shadows && gpu.sun.is_some();
        let sun_matrix = if has_sun {
            sun_view_projection(gpu.sun_direction, gpu.assets.bounds)
        } else {
            Mat4::IDENTITY
        };
        let frame_uniform = FrameUniform {
            view_projection: camera.view_projection().to_cols_array_2d(),
            sun_view_projection: sun_matrix.to_cols_array_2d(),
            camera_position: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            params: [
                settings.exposure,
                settings.emissive_scale,
                settings.ambient_sky_lux,
                settings.ambient_ground_lux,
            ],
            counts: [
                gpu.light_count,
                u32::from(has_sun),
                u32::try_from(gpu.sun.unwrap_or(0)).unwrap_or(0),
                0,
            ],
        };
        if gpu.ring_epoch != Some(record_epoch) {
            gpu.ring.begin_frame();
            gpu.ring_epoch = Some(record_epoch);
        }
        let uniform = gpu.ring.write(ctx.device(), bytemuck::bytes_of(&frame_uniform))?;

        let blended = gpu.blended_back_to_front(camera.position);
        let solid = Arc::clone(&gpu.solid);
        let forward_pipelines = gpu.forward_pipelines.clone();
        let shadow_pipelines = gpu.shadow_pipelines.clone();
        let (forward_layout, shadow_layout, tonemap_layout) =
            (gpu.forward_layout, gpu.shadow_layout, gpu.tonemap_layout);
        let (instance_buffer, light_buffer, shadow_sampler) =
            (gpu.instance_buffer, gpu.light_buffer, gpu.shadow_sampler);
        let material_buffer = gpu.assets.material_buffer;
        let assets = Arc::clone(&gpu.assets);

        let graph = ctx.graph();
        let shadow_map = graph.create_texture(
            "sun shadow map",
            GraphTextureDesc::fixed(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE, DEPTH_FORMAT),
        );
        let hdr = graph.create_texture("viewer hdr", GraphTextureDesc::render(HDR_FORMAT));
        let depth = graph.create_texture("viewer depth", GraphTextureDesc::render(DEPTH_FORMAT));

        let mut pass = graph.add_pass(PassDesc::new("viewer sun shadow", Stage::Opaque));
        let shadow_written = pass.write(shadow_map, Access::DEPTH_WRITE);
        let shadow_assets = Arc::clone(&assets);
        let shadow_solid = Arc::clone(&solid);
        pass.record(move |pass| {
            let shadow_view = pass.view(shadow_written)?;
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "viewer shadow frame",
                layout: shadow_layout,
                entries: vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: uniform.binding(),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::whole_buffer(instance_buffer),
                    },
                ],
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "viewer sun shadow",
                color: vec![],
                depth: Some(DepthAttachment::clear_reverse_z(shadow_view)),
                timestamps: None,
            });
            if has_sun {
                render.set_bind_group(0, group);
                let casters = shadow_solid
                    .iter()
                    .copied()
                    .filter(|call| call.key.alpha != AlphaKind::Transparent);
                draw_calls(
                    &mut render,
                    &shadow_assets,
                    casters,
                    &|key| shadow_pipelines[&key.double_sided],
                    false,
                )?;
            }
            Ok(())
        });

        let mut pass = graph.add_pass(PassDesc::new("viewer forward", Stage::Opaque));
        let shadow_read = pass.read(shadow_written, Access::SAMPLED);
        let hdr_written = pass.write(hdr, Access::COLOR_ATTACHMENT);
        let depth_written = pass.write(depth, Access::DEPTH_WRITE);
        // The backdrop of the windows follows the exposure, so it keeps its look when the scene is brightened.
        let sky_scale = f64::from(SKY_RADIANCE * settings.exposure / DEFAULT_EXPOSURE);
        let sky = SKY_COLOR.map(|channel| channel * sky_scale);
        pass.record(move |pass| {
            let (hdr_view, depth_view) = (pass.view(hdr_written)?, pass.view(depth_written)?);
            let shadow_view = pass.view(shadow_read)?;
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "viewer frame",
                layout: forward_layout,
                entries: vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: uniform.binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::whole_buffer(material_buffer),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::whole_buffer(instance_buffer),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::whole_buffer(light_buffer),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::TextureView(shadow_view),
                    },
                    BindGroupEntry {
                        binding: 5,
                        resource: BindingResource::Sampler(shadow_sampler),
                    },
                ],
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "viewer forward",
                color: vec![ColorAttachment::clear(
                    hdr_view,
                    Color {
                        r: sky[0],
                        g: sky[1],
                        b: sky[2],
                        a: 1.0,
                    },
                )],
                depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
                timestamps: None,
            });
            render.set_bind_group(0, group);
            draw_calls(
                &mut render,
                &assets,
                solid.iter().copied().chain(blended),
                &|key| forward_pipelines[&key],
                true,
            )
        });

        let mut pass = graph.add_pass(PassDesc::new("viewer tonemap", Stage::Tonemap));
        let hdr_read = pass.read(hdr_written, Access::SAMPLED);
        let color = pass.modify(output, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let (hdr_view, color_view) = (pass.view(hdr_read)?, pass.view(color)?);
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "viewer tonemap",
                layout: tonemap_layout,
                entries: vec![BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(hdr_view),
                }],
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "viewer tonemap",
                color: vec![ColorAttachment::load(color_view)],
                depth: None,
                timestamps: None,
            });
            render.set_pipeline(tonemap_pipeline);
            render.set_bind_group(0, group);
            render.draw(0..3, 0..1);
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, color);
        Ok(())
    }
}
