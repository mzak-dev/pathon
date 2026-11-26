#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

// A renderer with `GiPack`, a box room built from quads and a feature that copies the atlases into buffers, so a test
// can read what the probes hold.

use std::f32::consts::FRAC_PI_2;
use std::sync::{Arc, Mutex};

use pathon_api::glam::{Affine3A, Quat, UVec3, Vec3};
use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BufferCopy, BufferDesc, BufferId, BufferUsage, DeviceApi, Extent3d, Origin3d, RhiError, TexelLayout,
    TextureAspect, TextureCopy,
};
use pathon_api::{Camera, RecordCtx, RenderFeature};
use pathon_core::{FrameInput, RenderError, Renderer, TargetId, ViewInput};
use pathon_gi::{GiFrame, GiPack, GiSettings, GiVolumeUniforms, IRRADIANCE_RES, IRRADIANCE_TILE, VISIBILITY_TILE};
use pathon_golden::{announce_adapter, gpu_or_skip};
use pathon_material::{MaterialId, MaterialTable, StandardMaterial};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_rhi::{Backend, DeviceRequest, Format, PhysicalSize};
use pathon_scene::{Aabb, InstanceDesc, Light, MaterialHandle, MeshHandle, Scene};

pub mod sampler;

use sampler::{Sampled, Sampler, SharedRequests};

const ROW_ALIGNMENT: u32 = 256;

pub fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_TEST_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        other => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
    }
}

/// The atlases as one frame left them, half floats decoded.
#[derive(Clone, Debug)]
pub struct Atlases {
    pub uniforms: GiVolumeUniforms,
    pub irradiance: Vec<[f32; 4]>,
    pub visibility: Vec<[f32; 4]>,
}

#[derive(Debug, Default)]
struct Readback {
    uniforms: Option<GiVolumeUniforms>,
    irradiance: Option<(BufferId, u32, u32)>,
    visibility: Option<(BufferId, u32, u32)>,
}

type SharedReadback = Arc<Mutex<Readback>>;

fn padded_row(width: u32) -> u32 {
    (width * 8).next_multiple_of(ROW_ALIGNMENT)
}

fn readback_buffer(device: &dyn DeviceApi, label: &'static str, width: u32, height: u32) -> Result<BufferId, RhiError> {
    device.create_buffer(&BufferDesc {
        label,
        size: u64::from(padded_row(width)) * u64::from(height),
        usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
    })
}

struct Readout {
    shared: SharedReadback,
}

impl RenderFeature for Readout {
    fn name(&self) -> &'static str {
        "gi readout"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(frame) = ctx.shared().get::<GiFrame>().copied() else {
            return Ok(());
        };
        let (irradiance_size, visibility_size) = (frame.uniforms.irradiance_size(), frame.uniforms.visibility_size());
        let mut guard = self.shared.lock().expect("mutex");
        let state = &mut *guard;
        let destinations = [
            (&mut state.irradiance, "gi readout irradiance", irradiance_size),
            (&mut state.visibility, "gi readout visibility", visibility_size),
        ];
        let mut buffers = Vec::new();
        for (slot, label, (width, height)) in destinations {
            if slot.is_none_or(|(_, known_width, known_height)| (known_width, known_height) != (width, height)) {
                *slot = Some((readback_buffer(ctx.device(), label, width, height)?, width, height));
            }
            buffers.push(slot.expect("set above"));
        }
        state.uniforms = Some(frame.uniforms);
        drop(guard);
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("gi readout", Stage::Overlay).side_effect());
        let irradiance = pass.read(frame.irradiance, Access::COPY_SRC);
        let visibility = pass.read(frame.visibility, Access::COPY_SRC);
        pass.record(move |pass| {
            for (handle, (buffer, width, height)) in [(irradiance, buffers[0]), (visibility, buffers[1])] {
                let texture = pass.texture(handle);
                pass.commands().copy_texture_to_buffer(
                    TextureCopy {
                        texture,
                        mip: 0,
                        origin: Origin3d::default(),
                        aspect: TextureAspect::All,
                    },
                    BufferCopy {
                        buffer,
                        layout: TexelLayout {
                            offset: 0,
                            bytes_per_row: padded_row(width),
                            rows_per_image: height,
                        },
                    },
                    Extent3d::new_2d(width, height),
                );
            }
            Ok(())
        });
        Ok(())
    }
}

pub fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * fraction * 2.0_f32.powi(-24),
        31 if fraction == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent - 15),
    }
}

fn decode(device: &dyn DeviceApi, buffer: BufferId, width: u32, height: u32) -> Vec<[f32; 4]> {
    let bytes = device.read_buffer(buffer).expect("read the atlas");
    let row = padded_row(width) as usize;
    let mut texels = Vec::with_capacity((width * height) as usize);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let start = y * row + x * 8;
            let channel = |index: usize| {
                half_to_f32(u16::from_le_bytes([
                    bytes[start + 2 * index],
                    bytes[start + 2 * index + 1],
                ]))
            };
            texels.push([channel(0), channel(1), channel(2), channel(3)]);
        }
    }
    texels
}

/// One wall of a room: its material.
#[derive(Clone, Copy, Debug)]
pub struct Wall {
    pub albedo: [f32; 3],
    pub emission: [f32; 3],
}

impl Wall {
    pub const fn grey(albedo: f32) -> Self {
        Self {
            albedo: [albedo; 3],
            emission: [0.0; 3],
        }
    }

    pub const fn glowing(albedo: f32, emission: f32) -> Self {
        Self {
            albedo: [albedo; 3],
            emission: [emission; 3],
        }
    }
}

pub struct Rig {
    pub renderer: Renderer,
    pub scene: Scene,
    pub target: TargetId,
    readback: SharedReadback,
    requests: SharedRequests,
    quad: MeshId,
    frame: u64,
}

fn quad(device: &dyn DeviceApi, pool: &mut GeometryPool) -> MeshId {
    let positions = [
        [-1.0_f32, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let surface: Vec<[f32; 9]> = positions
        .iter()
        .map(|p| [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, p[0] * 0.5 + 0.5, p[1] * 0.5 + 0.5])
        .collect();
    let indices = [0_u32, 1, 2, 0, 2, 3];
    pool.upload(
        device,
        &MeshData::new(
            "quad",
            vec![bytemuck::cast_slice(&positions), bytemuck::cast_slice(&surface)],
            IndexData::U32(&indices),
        ),
    )
    .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

fn builder(
    settings: GiSettings,
    backend: Option<Backend>,
    readback: &SharedReadback,
    requests: &SharedRequests,
) -> pathon_core::RendererBuilder {
    Renderer::builder()
        .with_device_request(DeviceRequest {
            label: "gi test",
            backend,
            ..DeviceRequest::default()
        })
        .with_pack(GiPack::with_settings(settings))
        .with_feature(Readout {
            shared: Arc::clone(readback),
        })
        .with_feature(Sampler::new(Arc::clone(requests)))
}

fn finish(renderer: Renderer, readback: SharedReadback, requests: SharedRequests, target: TargetId) -> Rig {
    let mut renderer = renderer;
    let device = Arc::clone(renderer.device());
    let mut pool = GeometryPool::new(&*device, "gi test", VertexLayout::standard(), PoolDesc::default());
    let quad = quad(&*device, &mut pool);
    let shared = renderer.shared_mut();
    shared.insert(pool);
    shared.insert(MaterialTable::new("gi test materials", 4));
    Rig {
        renderer,
        scene: Scene::new(),
        target,
        readback,
        requests,
        quad,
        frame: 0,
    }
}

/// A rig on a real device, or `None` (the test is skipped) without an adapter.
pub fn rig(test: &str, settings: GiSettings) -> Option<Rig> {
    let readback = SharedReadback::default();
    let requests = SharedRequests::default();
    let built = builder(settings, backend_from_environment(), &readback, &requests).build();
    let mut renderer = gpu_or_skip(test, built, |error| {
        matches!(error, RenderError::Rhi(RhiError::NoAdapter(_)))
    })?;
    announce_adapter(test, renderer.device().caps());
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(16, 16), Format::Rgba8Unorm)
        .expect("target");
    Some(finish(renderer, readback, requests, target))
}

/// The same on the null device, which validates every barrier, binding and command.
pub fn null_rig(settings: GiSettings) -> Rig {
    let readback = SharedReadback::default();
    let requests = SharedRequests::default();
    let mut renderer = builder(settings, Some(Backend::Null), &readback, &requests)
        .build()
        .unwrap_or_else(|error| panic!("{error}"));
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(16, 16), Format::Rgba8Unorm)
        .expect("target");
    finish(renderer, readback, requests, target)
}

impl Rig {
    pub fn add_material(&mut self, wall: Wall) -> MaterialId {
        let material = StandardMaterial {
            base_color: [wall.albedo[0], wall.albedo[1], wall.albedo[2], 1.0],
            emissive: wall.emission,
            metallic: 0.0,
            roughness: 1.0,
            ..StandardMaterial::default()
        };
        self.renderer
            .shared_mut()
            .get_mut::<MaterialTable>()
            .expect("the table")
            .add(&material)
            .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
    }

    /// A quad of half sizes `half` (x, y) placed by `rotation` and `center`; it faces +Z before the rotation.
    pub fn add_quad(&mut self, material: MaterialId, center: Vec3, rotation: Quat, half: (f32, f32)) {
        let transform = Affine3A::from_scale_rotation_translation(Vec3::new(half.0, half.1, 1.0), rotation, center);
        let desc = InstanceDesc::new(
            MeshHandle::from_id(self.quad),
            MaterialHandle::from_id(material),
            transform,
        );
        self.scene.extract(|writer| writer.spawn(&desc)).expect("instance");
    }

    /// The six walls of the box `[min, max]`, in the order +x, -x, +y, -y, +z, -z.
    pub fn add_room(&mut self, min: Vec3, max: Vec3, walls: [Wall; 6]) {
        let center = (min + max) * 0.5;
        let half = (max - min) * 0.5;
        let placements = [
            (
                Vec3::new(max.x, center.y, center.z),
                Quat::from_rotation_y(-FRAC_PI_2),
                (half.z, half.y),
            ),
            (
                Vec3::new(min.x, center.y, center.z),
                Quat::from_rotation_y(FRAC_PI_2),
                (half.z, half.y),
            ),
            (
                Vec3::new(center.x, max.y, center.z),
                Quat::from_rotation_x(FRAC_PI_2),
                (half.x, half.z),
            ),
            (
                Vec3::new(center.x, min.y, center.z),
                Quat::from_rotation_x(-FRAC_PI_2),
                (half.x, half.z),
            ),
            (
                Vec3::new(center.x, center.y, max.z),
                Quat::from_rotation_y(std::f32::consts::PI),
                (half.x, half.y),
            ),
            (Vec3::new(center.x, center.y, min.z), Quat::IDENTITY, (half.x, half.y)),
        ];
        for ((position, rotation, size), wall) in placements.into_iter().zip(walls) {
            let material = self.add_material(wall);
            self.add_quad(material, position, rotation, size);
        }
    }

    pub fn add_light(&mut self, light: &Light) {
        self.scene.extract(|writer| writer.spawn_light(light)).expect("light");
    }

    /// Publishes the scene and renders one frame; returns the diagnostics reported while recording.
    pub fn frame(&mut self) -> Vec<pathon_api::Diagnostic> {
        self.renderer.shared_mut().insert(self.scene.snapshot());
        self.frame += 1;
        let input = FrameInput {
            views: vec![ViewInput {
                target: self.target,
                camera: Camera::default(),
            }],
            delta_seconds: 1.0 / 60.0,
            ..FrameInput::default()
        };
        self.renderer
            .render_frame(&input)
            .unwrap_or_else(|error| panic!("{error}"))
            .diagnostics
    }

    pub fn frames(&mut self, count: u32) -> Vec<pathon_api::Diagnostic> {
        let mut all = Vec::new();
        for _ in 0..count {
            all.extend(self.frame());
        }
        all
    }

    /// Runs one frame that evaluates the sampling function at `(position, normal)` pairs.
    pub fn sample(&mut self, points: &[(Vec3, Vec3)]) -> Vec<Sampled> {
        self.requests.lock().expect("mutex").points = points.to_vec();
        let diagnostics = self.frame();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        self.requests.lock().expect("mutex").points.clear();
        sampler::decode(&**self.renderer.device(), &self.requests)
    }

    /// The atlases the last frame produced; panics when no frame published a volume.
    pub fn atlases(&self) -> Atlases {
        let state = self.readback.lock().expect("mutex");
        let device = &**self.renderer.device();
        let (irradiance, width, height) = state.irradiance.expect("a frame published the volume");
        let (visibility, visibility_width, visibility_height) = state.visibility.expect("a frame published the volume");
        Atlases {
            uniforms: state.uniforms.expect("a frame published the volume"),
            irradiance: decode(device, irradiance, width, height),
            visibility: decode(device, visibility, visibility_width, visibility_height),
        }
    }
}

impl Atlases {
    pub fn probe_count(&self) -> u32 {
        let counts = self.uniforms.counts();
        counts.x * counts.y * counts.z
    }

    pub fn probe_cell(&self, index: u32) -> UVec3 {
        let counts = self.uniforms.counts();
        UVec3::new(
            index % counts.x,
            (index / counts.x) % counts.y,
            index / (counts.x * counts.y),
        )
    }

    pub fn probe_position(&self, index: u32) -> Vec3 {
        self.uniforms.origin() + self.probe_cell(index).as_vec3() * self.uniforms.spacing()
    }

    fn tile_origin(&self, index: u32, tile: u32) -> (u32, u32) {
        let per_row = self.uniforms.tiles_per_row();
        ((index % per_row) * tile, (index / per_row) * tile)
    }

    /// The interior texels of the irradiance tile of probe `index` (rgb and sky), row by row.
    pub fn irradiance_texels(&self, index: u32) -> Vec<[f32; 4]> {
        let (x0, y0) = self.tile_origin(index, IRRADIANCE_TILE);
        let width = self.uniforms.irradiance_size().0;
        let mut texels = Vec::new();
        for y in 0..IRRADIANCE_RES {
            for x in 0..IRRADIANCE_RES {
                texels.push(self.irradiance[((y0 + 1 + y) * width + x0 + 1 + x) as usize]);
            }
        }
        texels
    }

    /// The mean of the interior texels of a probe: the uniform radiance of a probe in a uniform field.
    pub fn mean_irradiance(&self, index: u32) -> Vec3 {
        let texels = self.irradiance_texels(index);
        let sum: Vec3 = texels.iter().map(|t| Vec3::new(t[0], t[1], t[2])).sum();
        sum / texels.len() as f32
    }

    /// Whether the probe holds nothing (it sits inside a solid cell).
    pub fn is_blank(&self, index: u32) -> bool {
        self.irradiance_texels(index)
            .iter()
            .all(|t| t[0] == 0.0 && t[1] == 0.0 && t[2] == 0.0)
    }

    /// The interior texels of the irradiance tile in the direction `direction` (nearest texel).
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an octahedral coordinate in 0..=1 times 8"
    )]
    pub fn irradiance_towards(&self, index: u32, direction: Vec3) -> Vec3 {
        let encoded = pathon_gi::reference::oct_encode(direction.normalize());
        let (x0, y0) = self.tile_origin(index, IRRADIANCE_TILE);
        let width = self.uniforms.irradiance_size().0;
        let texel = |axis: usize| {
            (((encoded[axis] * 0.5 + 0.5) * IRRADIANCE_RES as f32).floor() as u32).min(IRRADIANCE_RES - 1)
        };
        let value = self.irradiance[((y0 + 1 + texel(1)) * width + x0 + 1 + texel(0)) as usize];
        Vec3::new(value[0], value[1], value[2])
    }

    pub fn visibility_tile(&self, index: u32) -> Vec<[f32; 4]> {
        let (x0, y0) = self.tile_origin(index, VISIBILITY_TILE);
        let width = self.uniforms.visibility_size().0;
        let mut texels = Vec::new();
        for y in 0..VISIBILITY_TILE {
            for x in 0..VISIBILITY_TILE {
                texels.push(self.visibility[((y0 + y) * width + x0 + x) as usize]);
            }
        }
        texels
    }

    pub fn bounds(&self) -> Aabb {
        let counts = self.uniforms.counts();
        let origin = self.uniforms.origin();
        Aabb::new(
            origin,
            origin + (counts - UVec3::ONE).as_vec3() * self.uniforms.spacing(),
        )
    }
}
