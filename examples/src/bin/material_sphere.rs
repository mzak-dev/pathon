// M4-03: a grid of spheres shaded by the standard material (glTF metallic-roughness plus clearcoat), one directional
// light and a constant ambient term standing in for IBL. Rows: dielectric, metal, clearcoated paint; columns: roughness
// from glossy to rough. The picture goes to a PNG file (first argument, `material_sphere.png` by default).

use std::fs::File;
use std::io::BufWriter;

use pathon::api::Camera;
use pathon::api::glam::Vec3;
use pathon::api::resources::DeferredRelease;
use pathon::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindingResource, BufferCopy, BufferDesc, BufferId, BufferUsage, Color,
    ColorAttachment, ColorTargetState, CommandList, CullMode, DepthAttachment, DepthState, Device, DeviceApi,
    DeviceRequest, Extent3d, Format, FrontFace, PrimitiveState, RenderPassDesc, RenderPipelineDesc, ShaderStage,
    SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureUsage, TextureViewDesc,
};
use pathon_material::{MaterialHooks, MaterialId, MaterialTable, MaterialVariant, StandardMaterial};
use pathon_mesh::{GeometryPool, IndexData, MeshData, PoolDesc, VertexLayout};
use pathon_shader::{ShaderLibrary, reflect_bind_groups};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 640;
const COLUMNS: usize = 5;
const ROWS: usize = 3;

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
    // The material never takes screen-space derivatives itself: the entry point hands them over.
    input.uv_dx = dpdx(in.uv);
    input.uv_dy = dpdy(in.uv);
    input.color = vec4<f32>(1.0);
    input.front_facing = front_facing;

    let surface = load_surface(materials[in.material], input);
    let view = normalize(frame.camera_position.xyz - in.world_position);
    let bsdf = evaluate(surface, view, LightSample(frame.light_direction.xyz, frame.light_radiance.xyz));
    let ambient = 0.05 * (surface.diffuse_color + 0.5 * surface.f0) * surface.occlusion;
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

struct SphereMesh {
    positions: Vec<[f32; 3]>,
    surface: Vec<Surface>,
    indices: Vec<u32>,
}

fn sphere(rings: u32, segments: u32) -> SphereMesh {
    let (mut positions, mut surface) = (Vec::new(), Vec::new());
    for ring in 0..=rings {
        let theta = std::f32::consts::PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let point = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
            positions.push(point);
            surface.push(Surface {
                normal: point,
                tangent: [-phi.sin(), 0.0, phi.cos(), 1.0],
                uv: [segment as f32 / segments as f32, ring as f32 / rings as f32],
            });
        }
    }
    let mut indices = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let first = ring * (segments + 1) + segment;
            let below = first + segments + 1;
            for triangle in [[first, below, first + 1], [first + 1, below, below + 1]] {
                let corner = |index: u32| Vec3::from(positions[index as usize]);
                let facing =
                    (corner(triangle[1]) - corner(triangle[0])).cross(corner(triangle[2]) - corner(triangle[0]));
                let outward = facing.dot(corner(triangle[0]) + corner(triangle[1]) + corner(triangle[2])) > 0.0;
                // The poles collapse a triangle to a line; keeping it costs nothing and draws nothing.
                indices.extend(if outward {
                    triangle
                } else {
                    [triangle[0], triangle[2], triangle[1]]
                });
            }
        }
    }
    SphereMesh {
        positions,
        surface,
        indices,
    }
}

fn material_for(row: usize, column: usize) -> StandardMaterial {
    let roughness = 0.08 + 0.23 * column as f32;
    match row {
        0 => StandardMaterial {
            base_color: [0.8, 0.12, 0.1, 1.0],
            metallic: 0.0,
            roughness,
            ..StandardMaterial::default()
        },
        1 => StandardMaterial {
            base_color: [1.0, 0.77, 0.34, 1.0],
            metallic: 1.0,
            roughness,
            ..StandardMaterial::default()
        },
        _ => StandardMaterial {
            base_color: [0.05, 0.2, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.55,
            clearcoat: 1.0,
            clearcoat_roughness: 0.03 + 0.2 * column as f32,
            ..StandardMaterial::default()
        },
    }
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

fn write_png(path: &str, rgba: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("output file"));
    let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn main() {
    pathon_examples::init_logging();
    let device = Device::build(&DeviceRequest {
        label: "material sphere",
        ..Default::default()
    })
    .expect("a GPU adapter");
    let output_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "material_sphere.png".to_owned());

    let mut table = MaterialTable::new("materials", 2);
    let mut ids: Vec<MaterialId> = Vec::new();
    let mut variant = MaterialVariant::default();
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let material = material_for(row, column);
            variant = variant.merge(MaterialVariant::of(&material)).expect("one alpha kind");
            ids.push(
                table
                    .add(&material)
                    .unwrap_or_else(|error| panic!("{}", error.to_diagnostic())),
            );
        }
    }
    let materials = table
        .sync(&device, &mut DeferredRelease::new(2), 0)
        .expect("material upload");

    let mut library = ShaderLibrary::new();
    for (path, source) in MaterialHooks::default().modules() {
        library.add_module(path, source).expect("material module");
    }
    library.add_root("forward", SHADER);
    let key = variant.shader_key("forward");
    let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));
    let module = library.module(&device, &key).unwrap_or_else(|error| panic!("{error}"));
    let layout = device
        .create_bind_group_layout(&reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection")[0])
        .expect("layout");

    let mut pool = GeometryPool::new(&device, "spheres", VertexLayout::standard(), PoolDesc::default());
    let mesh = sphere(48, 96);
    let sphere = pool
        .upload(
            &device,
            &MeshData::new(
                "sphere",
                vec![
                    bytemuck::cast_slice(&mesh.positions),
                    bytemuck::cast_slice(&mesh.surface),
                ],
                IndexData::U32(&mesh.indices),
            ),
        )
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));

    let spacing = 1.15;
    let instances: Vec<Instance> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let (row, column) = (index / COLUMNS, index % COLUMNS);
            Instance {
                offset_scale: [
                    (column as f32 - (COLUMNS - 1) as f32 / 2.0) * spacing,
                    ((ROWS - 1) as f32 / 2.0 - row as f32) * spacing,
                    0.0,
                    0.5,
                ],
                material: [id.index(), 0, 0, 0],
            }
        })
        .collect();
    let camera = Camera::look_at(
        Vec3::new(0.0, 0.0, 7.0),
        Vec3::ZERO,
        30.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
    );
    let eye = camera.position;
    let light = Vec3::new(-0.5, 0.6, 0.8).normalize();
    let frame = Frame {
        view_projection: camera.view_projection().to_cols_array_2d(),
        camera_position: [eye.x, eye.y, eye.z, 1.0],
        light_direction: [light.x, light.y, light.z, 0.0],
        light_radiance: [3.0, 2.9, 2.7, 0.0],
    };
    let frame_buffer = upload(&device, "frame", bytemuck::bytes_of(&frame), BufferUsage::UNIFORM);
    let instance_buffer = upload(
        &device,
        "instances",
        bytemuck::cast_slice(&instances),
        BufferUsage::STORAGE,
    );
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "forward",
            layout,
            entries: [frame_buffer, materials, instance_buffer]
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
            bind_group_layouts: vec![layout],
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
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8UnormSrgb)],
            primitive: PrimitiveState {
                front_face: FrontFace::Ccw,
                cull_mode: CullMode::Back,
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
            WIDTH,
            HEIGHT,
            Format::Rgba8UnormSrgb,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
        ))
        .expect("color");
    let depth = device
        .create_texture(&TextureDesc::new_2d(
            "depth",
            WIDTH,
            HEIGHT,
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
    let row_bytes = WIDTH * 4;
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: u64::from(row_bytes * HEIGHT),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");

    let mut list = CommandList::new();
    list.texture_access(color, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    list.texture_access(depth, SubresourceRange::ALL, Access::empty(), Access::DEPTH_WRITE);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "spheres",
            color: vec![ColorAttachment::clear(color_view, Color::BLACK)],
            depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        let instance_count = u32::try_from(instances.len()).expect("few instances");
        pool.draw(&mut pass, sphere, 0..instance_count)
            .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    }
    list.texture_access(color, SubresourceRange::ALL, Access::COLOR_ATTACHMENT, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(color),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: row_bytes,
                rows_per_image: HEIGHT,
            },
        },
        Extent3d::new_2d(WIDTH, HEIGHT),
    );
    device.submit(list).expect("submit");

    write_png(&output_path, &device.read_buffer(readback).expect("read"));
    log::info!("wrote {output_path}: {COLUMNS}x{ROWS} spheres, shader variant {variant:?}");
}
