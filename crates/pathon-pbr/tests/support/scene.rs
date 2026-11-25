#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

// The test scene of M4-05: a floor, spheres of metal and dielectric, a cube and a masked fence, four materials (three
// with textures), a directional light, point lights and a spot light, rendered by `Renderer` with `PbrPack`.

use std::f32::consts::{PI, TAU};

use pathon_api::glam::{Affine3A, Quat, Vec3};
use pathon_api::{Camera, RenderFeature};
use pathon_bindless::{BINDLESS_FEATURES, BinderDesc, SamplerPreset, TextureBinder, TextureHandle};
use pathon_core::{FrameInput, RenderError, Renderer, RendererBuilder, TargetId, ViewInput};
use pathon_golden::{announce_adapter, gpu_or_skip};
use pathon_material::{AlphaMode, MaterialId, MaterialTable, MaterialTextures, StandardMaterial, TextureSlot};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_rhi::{Backend, DeviceApi, DeviceRequest, Format, PhysicalSize, RhiError};
use pathon_scene::{CameraDesc, InstanceDesc, InstanceId, Light, MaterialHandle, MeshHandle, Projection, Scene};
use pathon_texture::{ColorSpace, TextureLoader, TextureSource, Variant};

use super::backend_from_environment;

const TEXTURE_SIZE: u32 = 64;

pub struct Surface {
    normal: [f32; 3],
    tangent: [f32; 4],
    uv: [f32; 2],
}

pub struct Shape {
    positions: Vec<[f32; 3]>,
    surface: Vec<[f32; 9]>,
    indices: Vec<u32>,
}

impl Shape {
    fn upload(&self, device: &dyn DeviceApi, pool: &mut GeometryPool, label: &'static str) -> MeshId {
        pool.upload(
            device,
            &MeshData::new(
                label,
                vec![bytemuck::cast_slice(&self.positions), bytemuck::cast_slice(&self.surface)],
                IndexData::U32(&self.indices),
            ),
        )
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
    }
}

pub fn sphere(rings: u32, segments: u32) -> Shape {
    let (mut positions, mut surface) = (Vec::new(), Vec::new());
    for ring in 0..=rings {
        let theta = PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = TAU * segment as f32 / segments as f32;
            let point = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
            positions.push(point);
            surface.push([
                point[0],
                point[1],
                point[2],
                -phi.sin(),
                0.0,
                phi.cos(),
                1.0,
                2.0 * segment as f32 / segments as f32,
                ring as f32 / rings as f32,
            ]);
        }
    }
    let mut indices = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let first = ring * (segments + 1) + segment;
            let below = first + segments + 1;
            indices.extend([first, first + 1, below, first + 1, below + 1, below]);
        }
    }
    Shape {
        positions,
        surface,
        indices,
    }
}

fn quad_faces(faces: &[([f32; 3], [f32; 3], [f32; 3])], face_offset: f32, uv_scale: f32) -> Shape {
    let (mut positions, mut surface, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for (normal, tangent, bitangent) in faces {
        let base = u32::try_from(positions.len()).expect("a few vertices");
        for (u, v) in [(-1.0_f32, -1.0_f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            positions.push([0, 1, 2].map(|axis| normal[axis] * face_offset + tangent[axis] * u + bitangent[axis] * v));
            surface.push([
                normal[0],
                normal[1],
                normal[2],
                tangent[0],
                tangent[1],
                tangent[2],
                1.0,
                f32::midpoint(u, 1.0) * uv_scale,
                (1.0 - v) * 0.5 * uv_scale,
            ]);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Shape {
        positions,
        surface,
        indices,
    }
}

pub fn cube() -> Shape {
    quad_faces(
        &[
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ],
        1.0,
        1.0,
    )
}

/// A quad in the XY plane facing +Z, `[-1, 1]` on both axes.
pub fn wall() -> Shape {
    quad_faces(&[([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])], 0.0, 1.0)
}

/// A quad in the XZ plane facing +Y, `[-1, 1]` on both axes, uv repeating three times.
pub fn floor() -> Shape {
    quad_faces(&[([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0])], 0.0, 3.0)
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn checker(x: u32, y: u32) -> [u8; 4] {
    if ((x / 8) + (y / 8)).is_multiple_of(2) {
        [214, 52, 46, 255]
    } else {
        [232, 228, 220, 255]
    }
}

fn stripes(x: u32, y: u32) -> [u8; 4] {
    if ((x + y) / 6).is_multiple_of(2) {
        [48, 84, 214, 255]
    } else {
        [236, 204, 56, 255]
    }
}

fn bumps(x: u32, y: u32) -> [u8; 4] {
    let (u, v) = (x as f32 * TAU / 16.0, y as f32 * TAU / 16.0);
    let (dx, dy) = (0.8 * u.cos() * v.sin(), 0.8 * u.sin() * v.cos());
    let length = (dx * dx + dy * dy + 1.0).sqrt();
    [to_byte(0.5 - 0.5 * dx / length), to_byte(0.5 - 0.5 * dy / length), to_byte(0.5 + 0.5 / length), 255]
}

// glTF packing: occlusion in red, roughness in green, metalness in blue.
fn packed(x: u32, y: u32) -> [u8; 4] {
    let dots = if ((x % 16) as f32 - 8.0).hypot((y % 16) as f32 - 8.0) < 3.0 { 0.45 } else { 1.0 };
    let metallic = if (y / 16).is_multiple_of(2) { 1.0 } else { 0.0 };
    [to_byte(dots), to_byte(0.2 + 0.7 * x as f32 / 63.0), to_byte(metallic), 255]
}

// A fence: green with round holes.
fn fence(x: u32, y: u32) -> [u8; 4] {
    let inside = ((x % 16) as f32 - 8.0).hypot((y % 16) as f32 - 8.0) < 5.0;
    [40, 160, 70, if inside { 0 } else { 255 }]
}

fn upload_texture(
    device: &dyn DeviceApi,
    loader: &mut TextureLoader,
    binder: &mut TextureBinder,
    label: &'static str,
    color_space: ColorSpace,
    texel: fn(u32, u32) -> [u8; 4],
) -> TextureHandle {
    let data: Vec<u8> = (0..TEXTURE_SIZE * TEXTURE_SIZE)
        .flat_map(|index| texel(index % TEXTURE_SIZE, index / TEXTURE_SIZE))
        .collect();
    let source = TextureSource::new(label, TEXTURE_SIZE, TEXTURE_SIZE, color_space).with_variant(Variant::rgba8(vec![&data]));
    let texture = loader
        .load(device, &source)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    let view = texture.create_view(device).expect("view");
    binder
        .register(view, SamplerPreset::LinearRepeat)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

/// What the scene draws with.
#[derive(Clone, Copy, Debug)]
pub struct Content {
    pub ground: MeshId,
    pub ball: MeshId,
    pub block: MeshId,
    pub fence: MeshId,
    pub floor_material: MaterialId,
    pub metal_material: MaterialId,
    pub plastic_material: MaterialId,
    pub fence_material: MaterialId,
}

/// Meshes, materials and textures, inserted into `Shared` of the renderer.
pub fn add_content(renderer: &mut Renderer, textured: bool) -> Content {
    let device = renderer.device().clone();
    let device: &dyn DeviceApi = &*device;
    let mut pool = GeometryPool::new(device, "scene", VertexLayout::standard(), PoolDesc::default());
    let ground = floor().upload(device, &mut pool, "floor");
    let ball = sphere(64, 128).upload(device, &mut pool, "sphere");
    let block = cube().upload(device, &mut pool, "cube");
    let fence_mesh = wall().upload(device, &mut pool, "fence");

    let mut table = MaterialTable::new("materials", 4);
    let mut binder = TextureBinder::new(device, &BinderDesc::default()).unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    let mut loader = TextureLoader::new();
    let mut slots = MaterialTextures::default;
    let mut texture = |label, color_space, texel| upload_texture(device, &mut loader, &mut binder, label, color_space, texel);
    let (floor_textures, metal_textures, plastic_textures, fence_textures);
    if textured {
        let (checker, bumps, stripes, packed, fence_color) = (
            texture("checker", ColorSpace::Srgb, checker),
            texture("bumps", ColorSpace::Normal, bumps),
            texture("stripes", ColorSpace::Srgb, stripes),
            texture("packed", ColorSpace::Linear, packed),
            texture("fence", ColorSpace::Srgb, fence),
        );
        let slot = |handle| Some(TextureSlot(binder.shader_value(handle).expect("live")));
        floor_textures = MaterialTextures {
            base_color: slot(checker),
            normal: slot(bumps),
            ..slots()
        };
        metal_textures = MaterialTextures {
            base_color: slot(stripes),
            metallic_roughness: slot(packed),
            ..slots()
        };
        plastic_textures = MaterialTextures {
            occlusion: slot(packed),
            ..slots()
        };
        fence_textures = MaterialTextures {
            base_color: slot(fence_color),
            ..slots()
        };
    } else {
        (floor_textures, metal_textures, plastic_textures, fence_textures) = (slots(), slots(), slots(), slots());
    }
    let mut add = |material: StandardMaterial| table.add(&material).unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    let floor_material = add(StandardMaterial {
        base_color: if textured { [1.0; 4] } else { [0.55, 0.55, 0.5, 1.0] },
        metallic: 0.0,
        roughness: 0.55,
        textures: floor_textures,
        ..StandardMaterial::default()
    });
    let metal_material = add(StandardMaterial {
        base_color: if textured { [1.0; 4] } else { [0.95, 0.75, 0.3, 1.0] },
        metallic: 1.0,
        roughness: if textured { 1.0 } else { 0.3 },
        textures: metal_textures,
        ..StandardMaterial::default()
    });
    let plastic_material = add(StandardMaterial {
        base_color: [0.8, 0.12, 0.1, 1.0],
        metallic: 0.0,
        roughness: 0.35,
        textures: plastic_textures,
        ..StandardMaterial::default()
    });
    let fence_material = add(StandardMaterial {
        base_color: [1.0; 4],
        metallic: 0.0,
        roughness: 0.8,
        alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
        double_sided: true,
        textures: fence_textures,
        ..StandardMaterial::default()
    });
    let shared = renderer.shared_mut();
    shared.insert(pool);
    shared.insert(table);
    if textured {
        shared.insert(binder);
    }
    Content {
        ground,
        ball,
        block,
        fence: fence_mesh,
        floor_material,
        metal_material,
        plastic_material,
        fence_material,
    }
}

pub fn handles(mesh: MeshId, material: MaterialId) -> (MeshHandle, MaterialHandle) {
    (MeshHandle::from_id(mesh), MaterialHandle::from_id(material))
}

pub fn instance(scene: &mut Scene, mesh: MeshId, material: MaterialId, transform: Affine3A) -> InstanceId {
    let (mesh, material) = handles(mesh, material);
    scene
        .extract(|writer| writer.spawn(&InstanceDesc::new(mesh, material, transform)))
        .expect("instance")
}

pub fn scale_translate(scale: f32, position: Vec3) -> Affine3A {
    Affine3A::from_scale_rotation_translation(Vec3::splat(scale), Quat::IDENTITY, position)
}

pub struct Harness {
    pub renderer: Renderer,
    pub target: TargetId,
    pub scene: Scene,
    pub size: u32,
}

/// A renderer on a real device (or `None`, skipped, without an adapter) with an offscreen target of `size` squared.
pub fn harness(
    test: &str,
    size: u32,
    format: Format,
    configure: impl FnOnce(RendererBuilder) -> RendererBuilder,
) -> Option<Harness> {
    let request = DeviceRequest {
        label: "pbr test",
        backend: backend_from_environment(),
        optional_features: BINDLESS_FEATURES,
        ..Default::default()
    };
    let built = configure(Renderer::builder().with_device_request(request)).build();
    let mut renderer = gpu_or_skip(test, built, |error| matches!(error, RenderError::Rhi(RhiError::NoAdapter(_))))?;
    announce_adapter(test, renderer.device().caps());
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(size, size), format)
        .expect("target");
    Some(Harness {
        renderer,
        target,
        scene: Scene::new(),
        size,
    })
}

/// The same on the null device, which validates every barrier, binding and command.
pub fn null_harness(size: u32, format: Format, configure: impl FnOnce(RendererBuilder) -> RendererBuilder) -> Harness {
    let request = DeviceRequest {
        backend: Some(Backend::Null),
        optional_features: BINDLESS_FEATURES,
        ..Default::default()
    };
    let mut renderer = configure(Renderer::builder().with_device_request(request))
        .build()
        .unwrap_or_else(|error| panic!("{error}"));
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(size, size), format)
        .expect("target");
    Harness {
        renderer,
        target,
        scene: Scene::new(),
        size,
    }
}

impl Harness {
    /// Publishes the scene and renders one frame; returns the errors reported while recording.
    pub fn render(&mut self, camera: Camera) -> Vec<pathon_api::Diagnostic> {
        self.renderer.shared_mut().insert(self.scene.snapshot());
        let input = FrameInput {
            views: vec![ViewInput {
                target: self.target,
                camera,
            }],
            delta_seconds: 1.0 / 60.0,
            ..FrameInput::default()
        };
        let report = self.renderer.render_frame(&input).unwrap_or_else(|error| panic!("{error}"));
        report.diagnostics
    }
}

pub fn camera(size: u32) -> Camera {
    let _ = size;
    Camera::look_at(
        Vec3::new(0.0, 2.2, 6.2),
        Vec3::new(0.0, -0.4, 0.0),
        40.0_f32.to_radians(),
        1.0,
    )
}

pub fn no_op(_: &dyn RenderFeature) {}

pub fn unused(_: CameraDesc, _: Light, _: Projection) {}
