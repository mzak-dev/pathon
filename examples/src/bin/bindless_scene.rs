// M4-04: one scene, rendered through whichever texture path the device allows: a floor, two spheres and a cube from
// `pathon-mesh`, three standard materials with different maps from `pathon-texture`, the shader of `pathon-material`.
// Only `TextureBinder::texture_source_wgsl` and `bind_group` know which path runs (ADR-0036).
//
//   cargo run -p pathon-examples --bin bindless_scene [--bindless | --per-material] [plik.png]
//
// Without a flag the device decides (bindless where it has binding arrays, a bind group per material otherwise); the
// two flags force a path, and the two pictures are identical. The picture goes to a PNG file (`bindless_scene.png`).

use std::f32::consts::{PI, TAU};
use std::fs::File;
use std::io::BufWriter;

use pathon::api::Camera;
use pathon::api::glam::Vec3;
use pathon::api::resources::DeferredRelease;
use pathon::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindingResource, BufferCopy, BufferDesc, BufferId, BufferUsage, Color,
    ColorAttachment, ColorTargetState, CommandList, CullMode, DepthAttachment, DepthState, Device, DeviceApi,
    DeviceRequest, Extent3d, Format, PrimitiveState, RenderPassDesc, RenderPipelineDesc, ShaderStage, SubresourceRange,
    TexelLayout, TextureCopy, TextureDesc, TextureUsage, TextureViewDesc,
};
use pathon_bindless::{
    BINDLESS_FEATURES, BinderDesc, PathPreference, SamplerPreset, TextureBinder, TextureHandle, TexturePath,
};
use pathon_material::{
    MaterialHooks, MaterialId, MaterialTable, MaterialTextures, MaterialVariant, StandardMaterial, TextureSlot,
};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_shader::{ShaderLibrary, reflect_bind_groups};
use pathon_texture::{ColorSpace, TextureLoader, TextureSource, Variant};

const SIZE: u32 = 512;
const TEXTURE_SIZE: u32 = 64;
const TEXTURE_GROUP: u32 = 1;

const SHADER: &str = "
import package::material::standard::{MaterialData, evaluate, load_surface};
import package::material::surface::{LightSample, SurfaceInput};

struct Frame {
    view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    light_direction: vec4<f32>,
    light_radiance: vec4<f32>,
}

struct Instance {
    offset_scale: vec4<f32>,
    material: vec4<u32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> materials: array<MaterialData>;
@group(0) @binding(2) var<storage, read> instances: array<Instance>;

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) material: u32,
}

@vertex
fn vs(
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
) -> Varying {
    let instance = instances[instance_index];
    let world = position * instance.offset_scale.w + instance.offset_scale.xyz;
    return Varying(frame.view_projection * vec4<f32>(world, 1.0), world, normal, tangent, uv, instance.material.x);
}

@fragment
fn fs(in: Varying, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    var input: SurfaceInput;
    input.position = in.world_position;
    input.normal = in.normal;
    input.tangent = in.tangent;
    input.uv = in.uv;
    input.uv_dx = dpdx(in.uv);
    input.uv_dy = dpdy(in.uv);
    input.color = vec4<f32>(1.0);
    input.front_facing = front_facing;

    let surface = load_surface(materials[in.material], input);
    let view = normalize(frame.camera_position.xyz - in.world_position);
    let bsdf = evaluate(surface, view, LightSample(frame.light_direction.xyz, frame.light_radiance.xyz));
    let ambient = 0.3 * (surface.diffuse_color + 0.5 * surface.f0) * surface.occlusion;
    let radiance = surface.emissive + bsdf.diffuse + bsdf.specular + ambient;
    return vec4<f32>(radiance / (vec3<f32>(1.0) + radiance), 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Frame {
    view_projection: [[f32; 4]; 4],
    camera_position: [f32; 4],
    light_direction: [f32; 4],
    light_radiance: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    offset_scale: [f32; 4],
    material: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Surface {
    normal: [f32; 3],
    tangent: [f32; 4],
    uv: [f32; 2],
}

struct Shape {
    positions: Vec<[f32; 3]>,
    surface: Vec<Surface>,
    indices: Vec<u32>,
}

impl Shape {
    fn upload(&self, device: &Device, pool: &mut GeometryPool, label: &'static str) -> MeshId {
        pool.upload(
            device,
            &MeshData::new(
                label,
                vec![
                    bytemuck::cast_slice(&self.positions),
                    bytemuck::cast_slice(&self.surface),
                ],
                IndexData::U32(&self.indices),
            ),
        )
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
    }
}

fn sphere(rings: u32, segments: u32) -> Shape {
    let (mut positions, mut surface) = (Vec::new(), Vec::new());
    for ring in 0..=rings {
        let theta = PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = TAU * segment as f32 / segments as f32;
            let point = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
            positions.push(point);
            surface.push(Surface {
                normal: point,
                tangent: [-phi.sin(), 0.0, phi.cos(), 1.0],
                uv: [2.0 * segment as f32 / segments as f32, ring as f32 / rings as f32],
            });
        }
    }
    let mut indices = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let first = ring * (segments + 1) + segment;
            let below = first + segments + 1;
            indices.extend([first, below, first + 1, first + 1, below, below + 1]);
        }
    }
    Shape {
        positions,
        surface,
        indices,
    }
}

// Faces are two triangles each with their own corners, so normals and uv stay flat per face.
fn quad_faces(faces: &[([f32; 3], [f32; 3], [f32; 3])], face_offset: f32, uv_scale: f32) -> Shape {
    let (mut positions, mut surface, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for (normal, tangent, bitangent) in faces {
        let base = u32::try_from(positions.len()).expect("a few vertices");
        for (u, v) in [(-1.0_f32, -1.0_f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let point = [0, 1, 2].map(|axis| normal[axis] * face_offset + tangent[axis] * u + bitangent[axis] * v);
            positions.push(point);
            surface.push(Surface {
                normal: *normal,
                tangent: [tangent[0], tangent[1], tangent[2], 1.0],
                uv: [f32::midpoint(u, 1.0) * uv_scale, (1.0 - v) * 0.5 * uv_scale],
            });
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Shape {
        positions,
        surface,
        indices,
    }
}

fn cube() -> Shape {
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

fn floor() -> Shape {
    quad_faces(&[([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0])], 0.0, 3.0)
}

struct Image {
    label: &'static str,
    color_space: ColorSpace,
    texel: fn(u32, u32) -> [u8; 3],
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=255 before the cast"
)]
fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn checker(x: u32, y: u32) -> [u8; 3] {
    if ((x / 8) + (y / 8)).is_multiple_of(2) {
        [214, 52, 46]
    } else {
        [232, 228, 220]
    }
}

fn stripes(x: u32, y: u32) -> [u8; 3] {
    if ((x + y) / 6).is_multiple_of(2) {
        [48, 84, 214]
    } else {
        [236, 204, 56]
    }
}

fn radial(x: u32, y: u32) -> [u8; 3] {
    let (dx, dy) = (x as f32 - 31.5, y as f32 - 31.5);
    let falloff = 1.0 - ((dx * dx + dy * dy).sqrt() / 40.0).min(1.0);
    [
        to_byte(0.1 + 0.2 * falloff),
        to_byte(0.25 + 0.7 * falloff),
        to_byte(0.1 + 0.3 * falloff),
    ]
}

fn bumps(x: u32, y: u32) -> [u8; 3] {
    let (u, v) = (x as f32 * TAU / 16.0, y as f32 * TAU / 16.0);
    let (dx, dy) = (0.8 * u.cos() * v.sin(), 0.8 * u.sin() * v.cos());
    let length = (dx * dx + dy * dy + 1.0).sqrt();
    [
        to_byte(0.5 - 0.5 * dx / length),
        to_byte(0.5 - 0.5 * dy / length),
        to_byte(0.5 + 0.5 / length),
    ]
}

// glTF packing: occlusion in red, roughness in green, metalness in blue.
fn packed(x: u32, y: u32) -> [u8; 3] {
    let dots = if ((x % 16) as f32 - 8.0).hypot((y % 16) as f32 - 8.0) < 3.0 {
        0.45
    } else {
        1.0
    };
    let metallic = if (y / 16).is_multiple_of(2) { 1.0 } else { 0.0 };
    [to_byte(dots), to_byte(0.2 + 0.7 * x as f32 / 63.0), to_byte(metallic)]
}

fn glow(x: u32, y: u32) -> [u8; 3] {
    if ((x % 16) as f32 - 8.0).hypot((y % 16) as f32 - 8.0) < 3.5 {
        [255, 120, 30]
    } else {
        [0, 0, 0]
    }
}

const IMAGES: [Image; 6] = [
    Image {
        label: "checker",
        color_space: ColorSpace::Srgb,
        texel: checker,
    },
    Image {
        label: "stripes",
        color_space: ColorSpace::Srgb,
        texel: stripes,
    },
    Image {
        label: "radial",
        color_space: ColorSpace::Srgb,
        texel: radial,
    },
    Image {
        label: "bumps",
        color_space: ColorSpace::Normal,
        texel: bumps,
    },
    Image {
        label: "packed",
        color_space: ColorSpace::Linear,
        texel: packed,
    },
    Image {
        label: "glow",
        color_space: ColorSpace::Srgb,
        texel: glow,
    },
];

fn pixels(image: &Image) -> Vec<u8> {
    (0..TEXTURE_SIZE * TEXTURE_SIZE)
        .flat_map(|index| {
            let [r, g, b] = (image.texel)(index % TEXTURE_SIZE, index / TEXTURE_SIZE);
            [r, g, b, 255]
        })
        .collect()
}

struct Object {
    mesh: MeshId,
    material: MaterialId,
    textures: Vec<TextureHandle>,
    offset_scale: [f32; 4],
}

struct Rendered {
    path: TexturePath,
    image: Vec<u8>,
    group_builds: u64,
    material_groups: usize,
}

fn upload(device: &Device, label: &'static str, bytes: &[u8], usage: BufferUsage) -> BufferId {
    let buffer = device
        .create_buffer(&BufferDesc {
            label,
            size: bytes.len() as u64,
            usage: usage | BufferUsage::COPY_DST,
        })
        .expect("buffer");
    device.write_buffer(buffer, 0, bytes).expect("upload");
    buffer
}

fn render(device: &Device, preference: PathPreference) -> Rendered {
    let mut binder = TextureBinder::new(
        device,
        &BinderDesc {
            preference,
            ..BinderDesc::default()
        },
    )
    .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    if let Some(diagnostic) = binder.fallback_diagnostic() {
        log::warn!("[{}] {}", diagnostic.code, diagnostic.message);
    }
    binder.begin_frame(device, 0);

    let mut loader = TextureLoader::new();
    let handles: Vec<TextureHandle> = IMAGES
        .iter()
        .map(|image| {
            let data = pixels(image);
            let source = TextureSource::new(image.label, TEXTURE_SIZE, TEXTURE_SIZE, image.color_space)
                .with_variant(Variant::rgba8(vec![&data]));
            let texture = loader
                .load(device, &source)
                .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
            let view = texture.create_view(device).expect("view");
            binder
                .register(view, SamplerPreset::LinearRepeat)
                .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
        })
        .collect();
    let [checker, stripes, radial, bumps, packed, glow] = [0, 1, 2, 3, 4, 5].map(|index| handles[index]);
    let slot = |handle: TextureHandle| Some(TextureSlot(binder.shader_value(handle).expect("live")));

    let materials = [
        (
            StandardMaterial {
                base_color: [1.0; 4],
                metallic: 0.0,
                roughness: 0.55,
                textures: MaterialTextures {
                    base_color: slot(checker),
                    normal: slot(bumps),
                    ..MaterialTextures::default()
                },
                ..StandardMaterial::default()
            },
            vec![checker, bumps],
        ),
        (
            StandardMaterial {
                base_color: [1.0; 4],
                metallic: 0.6,
                roughness: 1.0,
                emissive: [1.0, 1.0, 1.0],
                emissive_strength: 1.5,
                textures: MaterialTextures {
                    base_color: slot(stripes),
                    metallic_roughness: slot(packed),
                    emissive: slot(glow),
                    ..MaterialTextures::default()
                },
                ..StandardMaterial::default()
            },
            vec![stripes, packed, glow],
        ),
        (
            StandardMaterial {
                base_color: [1.0; 4],
                metallic: 0.0,
                roughness: 0.4,
                textures: MaterialTextures {
                    base_color: slot(radial),
                    occlusion: slot(packed),
                    ..MaterialTextures::default()
                },
                ..StandardMaterial::default()
            },
            vec![radial, packed],
        ),
    ];

    let mut table = MaterialTable::new("materials", 2);
    let mut variant = MaterialVariant::default();
    let mut ids = Vec::new();
    for (material, _) in &materials {
        variant = variant.merge(MaterialVariant::of(material)).expect("one alpha kind");
        ids.push(
            table
                .add(material)
                .unwrap_or_else(|error| panic!("{}", error.to_diagnostic())),
        );
    }
    let material_buffer = table
        .sync(device, &mut DeferredRelease::new(2), 0)
        .expect("material upload");

    let mut hooks = MaterialHooks::default();
    hooks.set_texture_source(binder.texture_source_wgsl(TEXTURE_GROUP));
    hooks
        .validate()
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    let mut library = ShaderLibrary::new();
    for (path, source) in hooks.modules() {
        library.add_module(path, source).expect("material module");
    }
    library.add_root("forward", SHADER);
    let key = variant.shader_key("forward");
    let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));
    let module = library.module(device, &key).unwrap_or_else(|error| panic!("{error}"));
    let frame_layout = device
        .create_bind_group_layout(&reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection")[0])
        .expect("layout");

    let mut pool = GeometryPool::new(device, "scene", VertexLayout::standard(), PoolDesc::default());
    let ball = sphere(32, 64).upload(device, &mut pool, "sphere");
    let block = cube().upload(device, &mut pool, "cube");
    let ground = floor().upload(device, &mut pool, "floor");
    let object = |mesh, index: usize, offset_scale| Object {
        mesh,
        material: ids[index],
        textures: materials[index].1.clone(),
        offset_scale,
    };
    let objects = [
        object(ground, 0, [0.0, -1.0, 0.0, 4.0]),
        object(ball, 1, [-1.7, -0.1, 0.0, 0.9]),
        object(block, 2, [0.0, -0.25, -0.3, 0.75]),
        object(ball, 1, [1.7, -0.2, 0.3, 0.8]),
    ];

    let camera = Camera::look_at(
        Vec3::new(0.0, 2.2, 6.2),
        Vec3::new(0.0, -0.4, 0.0),
        40.0_f32.to_radians(),
        1.0,
    );
    let eye = camera.position;
    let light = Vec3::new(-0.5, 0.8, 0.6).normalize();
    let frame = Frame {
        view_projection: camera.view_projection().to_cols_array_2d(),
        camera_position: [eye.x, eye.y, eye.z, 1.0],
        light_direction: [light.x, light.y, light.z, 0.0],
        light_radiance: [4.0, 3.8, 3.5, 0.0],
    };
    let instances: Vec<Instance> = objects
        .iter()
        .map(|object| Instance {
            offset_scale: object.offset_scale,
            material: [object.material.index(), 0, 0, 0],
        })
        .collect();
    let frame_buffer = upload(device, "frame", bytemuck::bytes_of(&frame), BufferUsage::UNIFORM);
    let instance_buffer = upload(
        device,
        "instances",
        bytemuck::cast_slice(&instances),
        BufferUsage::STORAGE,
    );
    let frame_group = device
        .create_bind_group(&BindGroupDesc {
            label: "frame",
            layout: frame_layout,
            entries: [frame_buffer, material_buffer, instance_buffer]
                .iter()
                .enumerate()
                .map(|(binding, buffer)| BindGroupEntry {
                    binding: u32::try_from(binding).expect("three bindings"),
                    resource: BindingResource::whole_buffer(*buffer),
                })
                .collect(),
        })
        .expect("bind group");
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "forward",
            bind_group_layouts: vec![frame_layout, binder.layout()],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: pool.layout().vertex_buffer_layouts(),
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
        })
        .unwrap_or_else(|error| panic!("{error}"));

    let color = device
        .create_texture(&TextureDesc::new_2d(
            "color",
            SIZE,
            SIZE,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
        ))
        .expect("color");
    let depth = device
        .create_texture(&TextureDesc::new_2d(
            "depth",
            SIZE,
            SIZE,
            Format::Depth32Float,
            TextureUsage::RENDER_TARGET,
        ))
        .expect("depth");
    let color_view = device
        .create_texture_view(color, &TextureViewDesc::default())
        .expect("view");
    let depth_view = device
        .create_texture_view(depth, &TextureViewDesc::default())
        .expect("view");
    let row_bytes = SIZE * 4;
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: u64::from(row_bytes * SIZE),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");

    binder
        .prepare(device)
        .expect("one rebuild for every texture added above");
    let mut list = CommandList::new();
    list.texture_access(color, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    list.texture_access(depth, SubresourceRange::ALL, Access::empty(), Access::DEPTH_WRITE);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "scene",
            color: vec![ColorAttachment::clear(color_view, Color::BLACK)],
            depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, frame_group);
        for (index, object) in objects.iter().enumerate() {
            let textures = binder
                .bind_group(device, object.material.index(), &object.textures)
                .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
            pass.set_bind_group(TEXTURE_GROUP, textures);
            let instance = u32::try_from(index).expect("few objects");
            pool.draw(&mut pass, object.mesh, instance..instance + 1)
                .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
        }
    }
    list.texture_access(color, SubresourceRange::ALL, Access::COLOR_ATTACHMENT, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(color),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: row_bytes,
                rows_per_image: SIZE,
            },
        },
        Extent3d::new_2d(SIZE, SIZE),
    );
    device.submit(list).expect("submit");
    let image = device.read_buffer(readback).expect("read");

    let rendered = Rendered {
        path: binder.path(),
        image,
        group_builds: binder.group_builds(),
        material_groups: binder.material_group_count(),
    };
    device.wait_idle().expect("idle");
    binder.release(device);
    loader.release(device);
    rendered
}

fn write_png(path: &str, rgba: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("output file"));
    let mut encoder = png::Encoder::new(file, SIZE, SIZE);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn main() {
    pathon_examples::init_logging();
    let mut preference = PathPreference::Auto;
    let mut output_path = "bindless_scene.png".to_owned();
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--bindless" => preference = PathPreference::Bindless,
            "--per-material" => preference = PathPreference::PerMaterial,
            _ => output_path = argument,
        }
    }
    let device = Device::build(&DeviceRequest {
        label: "bindless scene",
        optional_features: BINDLESS_FEATURES,
        ..Default::default()
    })
    .expect("a GPU adapter");

    let rendered = render(&device, preference);
    write_png(&output_path, &rendered.image);
    log::info!(
        "wrote {output_path}: textures through {:?}, {} bind groups built, {} of them per material",
        rendered.path,
        rendered.group_builds,
        rendered.material_groups
    );
}
