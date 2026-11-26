#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

// Shading for the shadows of point and spot lights: a feature that draws the `Host`'s world and, per pixel, either the
// visibility `local_shadow_factor` gives for one light (what the shadow tests measure) or N.L summed over the lights times
// their visibility (what a human looks at). It declares the bindings and the helper the way `pathon-pbr` does.

use pathon_api::glam::{Mat4, Vec3};
use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, BufferDesc,
    BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState, CullMode, DepthAttachment, DepthState, Format,
    IndexFormat, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError, ShaderStage,
};
use pathon_api::{Camera, RecordCtx, RenderFeature, SetupCtx, Slot};
use pathon_shader::{ShaderFeatures, ShaderKey, ShaderLibrary, reflect_bind_groups};
use pathon_shadow::{LocalShadowBindings, LocalShadowFrame, SHADOW_LOCAL_MAP, SHADOW_MAP_ACCESS, modules};

use super::{ALBEDO, AMBIENT, DrawableWorld, LocalLights};

pub const MAX_LIGHTS: usize = 8;

const SHADING: &str = "
import package::shadow::sample::{LocalShadows, shadow_local_tile_index, shadow_local_visibility};

struct Frame {
    view_projection: mat4x4<f32>,
    // xyz: position of light i.
    lights: array<vec4<f32>, 8>,
    // x: number of lights, y: mode (0: visibility of light `z`, 1: lambert times visibility), z: the light of mode 0.
    counts: vec4<u32>,
}

struct Instance {
    model: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
LOCAL_DECLARATIONS

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
}

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @builtin(instance_index) index: u32,
) -> Varying {
    let model = instances[index].model;
    let world = model * vec4<f32>(position, 1.0);
    return Varying(frame.view_projection * world, world.xyz, (model * vec4<f32>(normal, 0.0)).xyz);
}

@fragment
fn fs(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var normal = normalize(in.normal);
    if (!front) {
        normal = -normal;
    }
    if (frame.counts.y == 0u) {
        let visibility = local_shadow_factor(frame.counts.z, in.world_position, normal, in.clip.xy);
        return vec4<f32>(vec3<f32>(visibility), 1.0);
    }
    var lit = 0.0;
    for (var light = 0u; light < frame.counts.x; light += 1u) {
        let offset = frame.lights[light].xyz - in.world_position;
        let to_light = normalize(offset);
        let visibility = local_shadow_factor(light, in.world_position, normal, in.clip.xy);
        lit += max(dot(normal, to_light), 0.0) * visibility * 10.0 / (4.0 + dot(offset, offset));
    }
    let linear = vec3<f32>(ALBEDO * (AMBIENT + (1.0 - AMBIENT) * min(lit, 1.0)));
    return vec4<f32>(pow(linear, vec3<f32>(1.0 / 2.2)), 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_projection: [[f32; 4]; 4],
    lights: [[f32; 4]; MAX_LIGHTS],
    counts: [u32; 4],
}

/// What the shading draws: the visibility of one light, or the lit scene.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Grey level = the visibility of this light (0 in shadow, 1 lit).
    Visibility(u32),
    Lit,
}

/// The mode, set by the test in `Shared`.
#[derive(Clone, Copy, Debug)]
pub struct ShadingMode(pub Mode);

struct Gpu {
    layout: BindGroupLayoutId,
    pipeline: RenderPipelineId,
    frame: BufferId,
    instances: BufferId,
}

pub struct LocalShading {
    gpu: Option<Gpu>,
    bindings: LocalShadowBindings,
}

impl LocalShading {
    pub fn new() -> Self {
        Self {
            gpu: None,
            bindings: LocalShadowBindings::starting_at(2),
        }
    }
}

impl std::fmt::Debug for LocalShading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalShading").finish_non_exhaustive()
    }
}

impl RenderFeature for LocalShading {
    fn name(&self) -> &'static str {
        "local shadow test shading"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let mut library = ShaderLibrary::new();
        for (path, source) in modules() {
            library.add_module(path, source).expect("shadow module");
        }
        let source = SHADING
            .replace("LOCAL_DECLARATIONS", &self.bindings.wgsl_declarations(0))
            .replace("AMBIENT", &format!("{AMBIENT:?}"))
            .replace("ALBEDO", &format!("{ALBEDO:?}"));
        library.add_root("shading", &source);
        let key = ShaderKey {
            root: "shading".to_owned(),
            features: ShaderFeatures::default(),
        };
        let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));
        let module = library.module(device, &key).unwrap_or_else(|error| panic!("{error}"));
        let layout_desc = reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection").remove(0);
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "local shadow test shading",
            ..layout_desc
        })?;
        let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
            label: "local shadow test shading",
            bind_group_layouts: vec![layout],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: pathon_mesh::VertexLayout::standard().vertex_buffer_layouts(),
            fragment: Some(ShaderStage {
                module,
                entry_point: "fs",
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState {
                cull_mode: CullMode::None,
                ..PrimitiveState::default()
            },
            depth: Some(DepthState::reverse_z(Format::Depth32Float)),
            sample_count: 1,
            immediate_size: 0,
        })?;
        let frame = device.create_buffer(&BufferDesc {
            label: "local shading frame",
            size: size_of::<FrameUniform>() as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        })?;
        let instances = device.create_buffer(&BufferDesc {
            label: "local shading instances",
            size: 64 * 64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        })?;
        self.gpu = Some(Gpu {
            layout,
            pipeline,
            frame,
            instances,
        });
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(gpu) = &self.gpu else {
            return Err(RhiError::Unsupported("shading used before setup"));
        };
        let (layout, pipeline, frame_buffer, instance_buffer) = (gpu.layout, gpu.pipeline, gpu.frame, gpu.instances);
        let (Some(output), Some(atlas)) = (ctx.slot(Slot::OUTPUT), ctx.slot(SHADOW_LOCAL_MAP)) else {
            return Ok(());
        };
        let Some(frame) = ctx.shared().get::<LocalShadowFrame>().copied() else {
            return Ok(());
        };
        let world = ctx
            .shared()
            .get::<DrawableWorld>()
            .expect("host published the world")
            .clone();
        let lights = ctx.shared().get::<LocalLights>().cloned().unwrap_or_default();
        let mode = ctx.shared().get::<ShadingMode>().map_or(Mode::Lit, |mode| mode.0);
        let camera: Camera = ctx.view().camera;
        let mut uniform = FrameUniform {
            view_projection: camera.view_projection().to_cols_array_2d(),
            lights: [[0.0; 4]; MAX_LIGHTS],
            counts: [0; 4],
        };
        for (slot, light) in uniform.lights.iter_mut().zip(&lights.lights) {
            *slot = [light.position.x, light.position.y, light.position.z, 0.0];
        }
        uniform.counts = match mode {
            Mode::Visibility(light) => [lights.lights.len().min(MAX_LIGHTS) as u32, 0, light, 0],
            Mode::Lit => [lights.lights.len().min(MAX_LIGHTS) as u32, 1, 0, 0],
        };
        ctx.device()
            .write_buffer(frame_buffer, 0, bytemuck::bytes_of(&uniform))?;
        let models: Vec<[[f32; 4]; 4]> = world
            .draws
            .iter()
            .map(|draw| Mat4::from(draw.model).to_cols_array_2d())
            .collect();
        ctx.device()
            .write_buffer(instance_buffer, 0, bytemuck::cast_slice(&models))?;

        let depth = ctx
            .graph()
            .create_texture("local shading depth", GraphTextureDesc::output(Format::Depth32Float));
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("local shadow test shading", Stage::Opaque));
        let atlas = pass.read(atlas, SHADOW_MAP_ACCESS);
        let color = pass.write(output, Access::COLOR_ATTACHMENT);
        let depth = pass.write(depth, Access::DEPTH_WRITE);
        let bindings = self.bindings;
        pass.record(move |pass| {
            let color_view = pass.view(color)?;
            let depth_view = pass.view(depth)?;
            let atlas_view = pass.view_with(atlas, &LocalShadowBindings::atlas_view_desc())?;
            let mut entries = vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::whole_buffer(frame_buffer),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::whole_buffer(instance_buffer),
                },
            ];
            entries.extend(bindings.bind_group_entries(&frame, atlas_view));
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "local shadow test shading",
                layout,
                entries,
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "local shadow test shading",
                color: vec![ColorAttachment::clear(color_view, Color::BLACK)],
                depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.set_bind_group(0, group);
            for (slot, buffer) in (0..).zip(&world.vertex_buffers) {
                render.set_vertex_buffer(slot, *buffer, 0);
            }
            render.set_index_buffer(world.index_buffer, IndexFormat::Uint32, 0);
            for (index, draw) in (0_u32..).zip(&world.draws) {
                render.draw_indexed(
                    draw.first_index..draw.first_index + draw.index_count,
                    draw.base_vertex,
                    index..index + 1,
                );
            }
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, color);
        Ok(())
    }
}

// ---- geometry for the tests: rays and boxes ----

/// An axis-aligned box occluder as the tests know it (what a unit cube of the host becomes with a model matrix).
#[derive(Clone, Copy, Debug)]
pub struct Box3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Box3 {
    pub fn around(centre: Vec3, half: Vec3) -> Self {
        Self {
            min: centre - half,
            max: centre + half,
        }
    }

    #[must_use]
    pub fn grown(self, by: f32) -> Self {
        Self {
            min: self.min - Vec3::splat(by),
            max: self.max + Vec3::splat(by),
        }
    }

    /// Whether the open segment from `from` to `to` passes through the box.
    pub fn blocks(&self, from: Vec3, to: Vec3) -> bool {
        let direction = to - from;
        let (mut near, mut far) = (0.0_f32, 1.0_f32);
        for axis in 0..3 {
            let (origin, step) = (from[axis], direction[axis]);
            if step.abs() < 1.0e-9 {
                if origin < self.min[axis] || origin > self.max[axis] {
                    return false;
                }
            } else {
                let (a, b) = ((self.min[axis] - origin) / step, (self.max[axis] - origin) / step);
                near = near.max(a.min(b));
                far = far.min(a.max(b));
            }
        }
        near < far
    }
}
