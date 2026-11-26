#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]
#![expect(
    clippy::cast_possible_truncation,
    reason = "test geometry and images are small, their indices fit every integer type"
)]

// A scene for the shadow tests: a host feature that owns a `GeometryPool` and fills the `ShadowScene` every frame, a
// shading feature that lights the same geometry with N.L times `shadow_factor_directional` (what pathon-pbr will do),
// and a probe that copies one cascade's depth map back. All of it runs through a real `Renderer`, so the shadow pack
// is exercised the way an application uses it.

use std::sync::{Arc, Mutex};

use pathon_api::glam::{Affine3A, Mat4, Quat, Vec3};
use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, BufferCopy,
    BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState, CullMode, DepthAttachment, DepthState,
    DeviceApi, Extent3d, Format, IndexFormat, Origin3d, PrimitiveState, RenderPassDesc, RenderPipelineDesc,
    RenderPipelineId, RhiError, ShaderStage, TexelLayout, TextureAspect, TextureCopy,
};
use pathon_api::{Camera, RecordCtx, RenderFeature, SetupCtx, Slot};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_shader::{ShaderFeatures, ShaderKey, ShaderLibrary, reflect_bind_groups};
use pathon_shadow::{
    DirectionalShadowLight, LocalShadowLight, SHADOW_MAP, SHADOW_MAP_ACCESS, ShadowBindings, ShadowCaster, ShadowFrame,
    ShadowScene, modules,
};

pub mod local;

pub const AMBIENT: f32 = 0.15;
pub const ALBEDO: f32 = 0.8;

// ---- geometry ----

pub struct MeshSource {
    pub positions: Vec<[f32; 3]>,
    // Normal, tangent and UV, the second stream of `VertexLayout::standard()`.
    pub attributes: Vec<[f32; 9]>,
    pub indices: Vec<u32>,
}

fn vertex(position: [f32; 3], normal: [f32; 3]) -> ([f32; 3], [f32; 9]) {
    (
        position,
        [normal[0], normal[1], normal[2], 1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    )
}

fn assemble(vertices: Vec<([f32; 3], [f32; 9])>, indices: Vec<u32>) -> MeshSource {
    let (positions, attributes) = vertices.into_iter().unzip();
    MeshSource {
        positions,
        attributes,
        indices,
    }
}

/// Unit cube centred on the origin, one flat-shaded quad per face.
pub fn cube() -> MeshSource {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (normal, u, v) in faces {
        let base = vertices.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let position = std::array::from_fn(|axis| 0.5 * (normal[axis] + a * u[axis] + b * v[axis]));
            vertices.push(vertex(position, normal));
        }
        // Outward-facing whichever way the axes were permuted.
        let (u, v, n) = (Vec3::from(u), Vec3::from(v), Vec3::from(normal));
        let order: [u32; 6] = if u.cross(v).dot(n) > 0.0 {
            [0, 1, 2, 0, 2, 3]
        } else {
            [0, 2, 1, 0, 3, 2]
        };
        indices.extend(order.map(|index| base + index));
    }
    assemble(vertices, indices)
}

/// Unit square on the XZ plane facing +Y.
pub fn plane() -> MeshSource {
    let corners = [[-0.5, 0.0, -0.5], [0.5, 0.0, -0.5], [0.5, 0.0, 0.5], [-0.5, 0.0, 0.5]];
    assemble(
        corners.map(|corner| vertex(corner, [0.0, 1.0, 0.0])).to_vec(),
        vec![0, 2, 1, 0, 3, 2],
    )
}

/// Sphere of radius 0.5.
pub fn sphere() -> MeshSource {
    const RINGS: u32 = 24;
    const SEGMENTS: u32 = 48;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for ring in 0..=RINGS {
        let theta = std::f32::consts::PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let normal = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
            vertices.push(vertex(normal.map(|axis| axis * 0.5), normal));
        }
    }
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * (SEGMENTS + 1) + segment;
            let b = a + SEGMENTS + 1;
            indices.extend([a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    assemble(vertices, indices)
}

#[derive(Clone, Copy, Debug)]
pub struct Object {
    pub kind: Kind,
    pub model: Affine3A,
    pub casts: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Cube,
    Plane,
    Sphere,
}

impl Object {
    pub fn new(kind: Kind, centre: Vec3, scale: Vec3) -> Self {
        Self {
            kind,
            model: Affine3A::from_scale_rotation_translation(scale, Quat::IDENTITY, centre),
            casts: true,
        }
    }

    #[must_use]
    pub fn rotated(self, rotation: Quat) -> Self {
        let (scale, _, translation) = Mat4::from(self.model).to_scale_rotation_translation();
        Self {
            model: Affine3A::from_scale_rotation_translation(scale, rotation, translation),
            ..self
        }
    }

    #[must_use]
    pub fn not_casting(self) -> Self {
        Self { casts: false, ..self }
    }
}

// ---- host: the geometry pool and the shadow scene ----

/// What the shading pass needs to draw the objects, published in `Shared` by the host feature.
#[derive(Clone, Debug)]
pub struct DrawableWorld {
    pub vertex_buffers: Vec<BufferId>,
    pub index_buffer: BufferId,
    pub draws: Vec<Drawable>,
}

#[derive(Clone, Copy, Debug)]
pub struct Drawable {
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    pub model: Affine3A,
}

/// The sun, changed by tests between frames.
#[derive(Clone, Copy, Debug)]
pub struct Sun {
    pub direction: Vec3,
}

pub struct Host {
    objects: Vec<Object>,
    pool: Option<GeometryPool>,
    meshes: Vec<(Kind, MeshId)>,
    casters: Vec<ShadowCaster>,
}

impl Host {
    pub fn new(objects: Vec<Object>) -> Self {
        Self {
            objects,
            pool: None,
            meshes: Vec::new(),
            casters: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("objects", &self.objects.len())
            .finish_non_exhaustive()
    }
}

fn upload(pool: &mut GeometryPool, device: &dyn DeviceApi, label: &'static str, source: &MeshSource) -> MeshId {
    pool.upload(
        device,
        &MeshData::new(
            label,
            vec![
                bytemuck::cast_slice(&source.positions),
                bytemuck::cast_slice(&source.attributes),
            ],
            IndexData::U32(&source.indices),
        ),
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

impl RenderFeature for Host {
    fn name(&self) -> &'static str {
        "shadow test host"
    }

    fn stage(&self) -> Stage {
        Stage::Setup
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let mut pool = GeometryPool::new(device, "shadow test", VertexLayout::standard(), PoolDesc::default());
        self.meshes = vec![
            (Kind::Cube, upload(&mut pool, device, "cube", &cube())),
            (Kind::Plane, upload(&mut pool, device, "plane", &plane())),
            (Kind::Sphere, upload(&mut pool, device, "sphere", &sphere())),
        ];
        self.casters.clear();
        let mut draws = Vec::new();
        let mut buffers = None;
        for object in &self.objects {
            let (_, mesh) = *self.meshes.iter().find(|(kind, _)| *kind == object.kind).expect("mesh");
            let info = pool.mesh(mesh).expect("uploaded");
            let location = info.location;
            buffers = Some((
                pool.vertex_buffers(location.vertex_chunk).expect("chunk").to_vec(),
                pool.index_buffer(location.index_chunk).expect("chunk"),
            ));
            draws.push(Drawable {
                first_index: location.first_index,
                index_count: location.index_count,
                base_vertex: location.base_vertex_signed(),
                model: object.model,
            });
            if object.casts {
                self.casters
                    .push(ShadowCaster::from_mesh(&pool, mesh, object.model).expect("caster"));
            }
        }
        let (vertex_buffers, index_buffer) = buffers.expect("at least one object");
        ctx.shared().insert(DrawableWorld {
            vertex_buffers,
            index_buffer,
            draws,
        });
        self.pool = Some(pool);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let direction = ctx
            .shared()
            .get::<Sun>()
            .map_or(Vec3::new(-0.5, -0.45, -0.6), |sun| sun.direction);
        let scene = ctx
            .shared_mut()
            .get_mut::<ShadowScene>()
            .expect("ShadowPack inserts the scene");
        scene.clear_casters();
        for caster in &self.casters {
            scene.add_caster(*caster);
        }
        scene.light = Some(DirectionalShadowLight::new(direction, 0));
        if let Some(local) = ctx.shared().get::<LocalLights>().cloned() {
            let scene = ctx
                .shared_mut()
                .get_mut::<ShadowScene>()
                .expect("ShadowPack inserts the scene");
            scene.clear_local_lights();
            for light in local.lights {
                scene.add_local_light(light);
            }
            if !local.with_sun {
                scene.light = None;
            }
        }
        Ok(())
    }
}

/// The point and spot lights the host offers the shadow pack, set by the test in `Shared` (the sun is off unless
/// `with_sun`).
#[derive(Clone, Debug, Default)]
pub struct LocalLights {
    pub lights: Vec<LocalShadowLight>,
    pub with_sun: bool,
}

// ---- shading ----

const SHADING: &str = "
import package::shadow::sample::{ShadowCascades, shadow_factor_directional};

struct Frame {
    view_projection: mat4x4<f32>,
    view: mat4x4<f32>,
    light_direction: vec4<f32>,
}

struct Instance {
    model: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
SHADOW_DECLARATIONS

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
fn fs(in: Varying) -> @location(0) vec4<f32> {
    let normal = normalize(in.normal);
    let to_light = -frame.light_direction.xyz;
    let view_depth = -(frame.view * vec4<f32>(in.world_position, 1.0)).z;
    let visibility = shadow_factor_directional(
        shadow_cascades,
        shadow_map,
        shadow_sampler,
        0u,
        in.world_position,
        normal,
        view_depth,
        in.clip.xy,
    );
    let lit = max(dot(normal, to_light), 0.0) * visibility;
    let linear = vec3<f32>(ALBEDO * (AMBIENT + (1.0 - AMBIENT) * lit));
    return vec4<f32>(pow(linear, vec3<f32>(1.0 / 2.2)), 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_projection: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    light_direction: [f32; 4],
}

struct ShadingGpu {
    layout: BindGroupLayoutId,
    pipeline: RenderPipelineId,
    frame: BufferId,
    instances: BufferId,
}

/// Lights the world with N.L times the shadow factor; writes `Slot::OUTPUT`.
pub struct Shading {
    gpu: Option<ShadingGpu>,
    bindings: ShadowBindings,
}

impl Shading {
    pub fn new() -> Self {
        Self {
            gpu: None,
            bindings: ShadowBindings::starting_at(2),
        }
    }
}

impl std::fmt::Debug for Shading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shading").finish_non_exhaustive()
    }
}

impl RenderFeature for Shading {
    fn name(&self) -> &'static str {
        "shadow test shading"
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
            .replace("SHADOW_DECLARATIONS", &self.bindings.wgsl_declarations(0))
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
            label: "shadow test shading",
            ..layout_desc
        })?;
        let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
            label: "shadow test shading",
            bind_group_layouts: vec![layout],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: VertexLayout::standard().vertex_buffer_layouts(),
            fragment: Some(ShaderStage {
                module,
                entry_point: "fs",
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState {
                cull_mode: CullMode::Back,
                ..PrimitiveState::default()
            },
            depth: Some(DepthState::reverse_z(Format::Depth32Float)),
            sample_count: 1,
            immediate_size: 0,
        })?;
        let frame = device.create_buffer(&BufferDesc {
            label: "shading frame",
            size: size_of::<FrameUniform>() as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        })?;
        let instances = device.create_buffer(&BufferDesc {
            label: "shading instances",
            size: 64 * 64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        })?;
        self.gpu = Some(ShadingGpu {
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
        let (Some(output), Some(map)) = (ctx.slot(Slot::OUTPUT), ctx.slot(SHADOW_MAP)) else {
            return Ok(());
        };
        let world = ctx
            .shared()
            .get::<DrawableWorld>()
            .expect("host published the world")
            .clone();
        let shadows = *ctx
            .shared()
            .get::<ShadowFrame>()
            .expect("shadow pack published a frame");
        let camera: Camera = ctx.view().camera;
        let sun = ctx
            .shared()
            .get::<Sun>()
            .map_or(Vec3::new(-0.5, -0.45, -0.6), |sun| sun.direction)
            .normalize();
        let uniform = FrameUniform {
            view_projection: camera.view_projection().to_cols_array_2d(),
            view: camera.world_to_view.to_cols_array_2d(),
            light_direction: [sun.x, sun.y, sun.z, 0.0],
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
            .create_texture("shading depth", GraphTextureDesc::output(Format::Depth32Float));
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("shadow test shading", Stage::Opaque));
        let map = pass.read(map, SHADOW_MAP_ACCESS);
        let color = pass.write(output, Access::COLOR_ATTACHMENT);
        let depth = pass.write(depth, Access::DEPTH_WRITE);
        let bindings = self.bindings;
        pass.record(move |pass| {
            let color_view = pass.view(color)?;
            let depth_view = pass.view(depth)?;
            let map_view = pass.view_with(map, &ShadowBindings::map_view_desc())?;
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
            entries.extend(bindings.bind_group_entries(&shadows, map_view));
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "shadow test shading",
                layout,
                entries,
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "shadow test shading",
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

// ---- probe: one cascade's depth map on the CPU ----

#[derive(Clone, Debug, Default)]
pub struct MapReadback(Arc<Mutex<Option<BufferId>>>);

impl MapReadback {
    fn set(&self, buffer: BufferId) {
        *self.0.lock().expect("mutex") = Some(buffer);
    }

    pub fn get(&self) -> BufferId {
        self.0.lock().expect("mutex").expect("the probe ran setup")
    }
}

/// Copies one layer of the shadow map into a buffer the test reads after the frame.
#[derive(Debug)]
pub struct MapProbe {
    pub layer: u32,
    pub resolution: u32,
    pub readback: MapReadback,
}

impl RenderFeature for MapProbe {
    fn name(&self) -> &'static str {
        "shadow map probe"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let buffer = ctx.device().create_buffer(&BufferDesc {
            label: "shadow map probe",
            size: u64::from(self.resolution * self.resolution * 4),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })?;
        self.readback.set(buffer);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(map) = ctx.slot(SHADOW_MAP) else {
            return Ok(());
        };
        let (buffer, layer, resolution) = (self.readback.get(), self.layer, self.resolution);
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("shadow map probe", Stage::Overlay).side_effect());
        let map = pass.read(map, Access::COPY_SRC);
        pass.record(move |pass| {
            let texture = pass.texture(map);
            pass.commands().copy_texture_to_buffer(
                TextureCopy {
                    texture,
                    mip: 0,
                    origin: Origin3d { x: 0, y: 0, z: layer },
                    aspect: TextureAspect::DepthOnly,
                },
                BufferCopy {
                    buffer,
                    layout: TexelLayout {
                        offset: 0,
                        bytes_per_row: resolution * 4,
                        rows_per_image: resolution,
                    },
                },
                Extent3d::new_2d(resolution, resolution),
            );
            Ok(())
        });
        Ok(())
    }
}

/// Depth map layer as `resolution * resolution` floats, row by row.
pub fn read_map(device: &dyn DeviceApi, readback: &MapReadback) -> Vec<f32> {
    let bytes = device.read_buffer(readback.get()).expect("read");
    bytemuck::cast_slice(&bytes).to_vec()
}
