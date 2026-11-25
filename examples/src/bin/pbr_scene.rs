// M4-05: `PbrPack` (prepass, clustered lights, forward colour pass) with `IblPack` and `PostPack` on a small scene: a
// floor, a gold and a red plastic sphere, a sun, coloured point lights and a spot. Offscreen; the picture goes to a PNG
// file: `pbr_scene [file.png]`. The host side is what an own engine would write: meshes, materials and the scene
// snapshot go to `Shared`, the packs do the rest.

use std::f32::consts::{PI, TAU};
use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;

use pathon::api::Camera;
use pathon::api::glam::{Affine3A, Quat, Vec3};
use pathon::api::rhi::{Access, Format, PhysicalSize};
use pathon::{FrameInput, Renderer, ViewInput};
use pathon_golden::read_texture;
use pathon_ibl::{IblPack, IblSettings, IblSource, SyntheticSky};
use pathon_material::{MaterialTable, StandardMaterial};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_pbr::PbrPack;
use pathon_post::{PostPack, PostSettings};
use pathon_scene::{InstanceDesc, Light, MaterialHandle, MeshHandle, Scene};

const SIZE: u32 = 512;

struct Shape {
    positions: Vec<[f32; 3]>,
    surface: Vec<[f32; 9]>,
    indices: Vec<u32>,
}

// Position, then normal, tangent (xyz, w = handedness) and uv: the `standard` layout.
fn sphere(rings: u32, segments: u32) -> Shape {
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
                segment as f32 / segments as f32,
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

fn floor() -> Shape {
    let corners = [(-1.0_f32, 1.0_f32), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)];
    Shape {
        positions: corners.map(|(x, z)| [x, 0.0, z]).to_vec(),
        surface: corners
            .map(|(x, z)| [0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, x * 0.5 + 0.5, z * 0.5 + 0.5])
            .to_vec(),
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

fn upload(
    device: &dyn pathon::api::rhi::DeviceApi,
    pool: &mut GeometryPool,
    label: &'static str,
    shape: &Shape,
) -> MeshId {
    pool.upload(
        device,
        &MeshData::new(
            label,
            vec![
                bytemuck::cast_slice(&shape.positions),
                bytemuck::cast_slice(&shape.surface),
            ],
            IndexData::U32(&shape.indices),
        ),
    )
    .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

fn pointing(direction: Vec3) -> Quat {
    Quat::from_rotation_arc(Vec3::NEG_Z, direction.normalize())
}

fn main() {
    pathon_examples::init_logging();
    let output_path = std::env::args().nth(1).unwrap_or_else(|| "pbr_scene.png".to_owned());

    let sky = SyntheticSky::default().image(256, 128).expect("a valid image");
    let mut renderer = Renderer::builder()
        .with_pack(IblPack::new())
        .with_shared(IblSource::new(Arc::new(sky), IblSettings::COMPACT))
        .with_pack(PbrPack::new())
        .with_pack(PostPack::with_settings(PostSettings::default()))
        .build()
        .unwrap_or_else(|error| panic!("{error}"));
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba8Unorm)
        .expect("offscreen target");

    // Host data: meshes, materials, the scene.
    let device = Arc::clone(renderer.device());
    let mut pool = GeometryPool::new(&*device, "scene", VertexLayout::standard(), PoolDesc::default());
    let ground = upload(&*device, &mut pool, "floor", &floor());
    let ball = upload(&*device, &mut pool, "sphere", &sphere(64, 128));
    let mut materials = MaterialTable::new("materials", 4);
    let mut material = |base_color: [f32; 4], metallic: f32, roughness: f32| {
        materials
            .add(&StandardMaterial {
                base_color,
                metallic,
                roughness,
                ..StandardMaterial::default()
            })
            .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
    };
    let floor_material = material([0.6, 0.6, 0.55, 1.0], 0.0, 0.6);
    let gold = material([1.0, 0.76, 0.34, 1.0], 1.0, 0.25);
    let plastic = material([0.8, 0.1, 0.08, 1.0], 0.0, 0.35);

    let mut scene = Scene::new();
    scene.extract(|writer| {
        let spawn = |writer: &mut pathon_scene::SceneWriter<'_>, mesh, material, scale: f32, position: Vec3| {
            writer
                .spawn(&InstanceDesc::new(
                    MeshHandle::from_id(mesh),
                    MaterialHandle::from_id(material),
                    Affine3A::from_scale_rotation_translation(Vec3::splat(scale), Quat::IDENTITY, position),
                ))
                .expect("instance");
        };
        spawn(writer, ground, floor_material, 6.0, Vec3::new(0.0, -1.0, 0.0));
        spawn(writer, ball, gold, 0.9, Vec3::new(-1.3, -0.1, 0.0));
        spawn(writer, ball, plastic, 0.8, Vec3::new(1.4, -0.2, 0.3));
        writer
            .spawn_light(&Light::directional(
                Affine3A::from_quat(pointing(Vec3::new(-0.5, -0.8, -0.6))),
                1.6,
            ))
            .expect("sun");
        for (position, color) in [
            (Vec3::new(-3.0, 1.2, 1.5), Vec3::new(1.0, 0.3, 0.2)),
            (Vec3::new(3.2, 1.0, 1.0), Vec3::new(0.2, 0.5, 1.0)),
        ] {
            writer
                .spawn_light(
                    &Light::point(Affine3A::from_translation(position), 90.0)
                        .with_color(color)
                        .with_range(5.0),
                )
                .expect("point light");
        }
        writer
            .spawn_light(
                &Light::spot(
                    Affine3A::from_rotation_translation(
                        pointing(Vec3::new(-2.2, -3.0, -1.6)),
                        Vec3::new(2.5, 3.0, 1.8),
                    ),
                    500.0,
                    0.15,
                    0.35,
                )
                .with_range(9.0),
            )
            .expect("spot");
    });
    let shared = renderer.shared_mut();
    shared.insert(pool);
    shared.insert(materials);
    shared.insert(scene.snapshot());

    let camera = Camera::look_at(
        Vec3::new(0.0, 2.8, 7.6),
        Vec3::new(0.0, -0.4, 0.0),
        50.0_f32.to_radians(),
        1.0,
    );
    let report = renderer
        .render_frame(&FrameInput {
            views: vec![ViewInput { target, camera }],
            delta_seconds: 1.0 / 60.0,
            ..FrameInput::default()
        })
        .unwrap_or_else(|error| panic!("{error}"));
    for diagnostic in &report.diagnostics {
        log::warn!("{diagnostic:?}");
    }

    let texture = renderer.target_texture(target).expect("offscreen texture");
    let image = read_texture(&*device, texture, Access::SAMPLED).expect("readback");
    let file = BufWriter::new(File::create(&output_path).expect("create the png"));
    let mut encoder = png::Encoder::new(file, SIZE, SIZE);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(image.as_rgba8()).expect("png data");
    log::info!("wrote {output_path} ({SIZE}x{SIZE}); slots written: depth, normals, motion, material_id, hdr");
}
