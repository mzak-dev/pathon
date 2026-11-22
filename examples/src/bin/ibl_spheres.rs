// M4-06: spheres of the standard material lit by an environment only, no analytic light. Rows: a dielectric, gold and
// silver; columns: roughness from glossy to rough. The environment is a procedural sky with a sun, or a Radiance
// `.hdr` file given as the second argument. The picture goes to a PNG file (first argument, `ibl_spheres.png` by
// default): `cargo run -p pathon-examples --bin ibl_spheres -- out.png environment.hdr`.

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
use pathon_examples::radiance;
use pathon_ibl::{EquirectImage, IblBindings, IblBuilder, IblSettings, SyntheticSky};
use pathon_material::{MaterialHooks, MaterialTable, MaterialVariant, StandardMaterial};
use pathon_shader::{ShaderLibrary, reflect_bind_groups};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 640;
const COLUMNS: usize = 5;
const ROWS: usize = 3;
const RINGS: u32 = 48;
const SEGMENTS: u32 = 96;

// The sphere is generated from the vertex index: nothing to upload but the instances.
const SHADER: &str = "
import package::ibl::shading::{IblInput, ibl_evaluate};
import package::material::standard::{MaterialData, load_surface};
import package::material::surface::SurfaceInput;

struct Frame {
    view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
}

struct Instance {
    offset_scale: vec4<f32>,
    material: vec4<u32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> materials: array<MaterialData>;
@group(0) @binding(2) var<storage, read> instances: array<Instance>;
IBL_DECLARATIONS

const RINGS: u32 = RING_COUNT;
const SEGMENTS: u32 = SEGMENT_COUNT;
const PI: f32 = 3.14159265358979;

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) @interpolate(flat) material: u32,
}

@vertex
fn vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance_index: u32) -> Varying {
    var ring_offsets = array<u32, 6>(0u, 0u, 1u, 1u, 0u, 1u);
    var segment_offsets = array<u32, 6>(0u, 1u, 0u, 0u, 1u, 1u);
    let quad = vertex / 6u;
    let corner = vertex % 6u;
    let theta = PI * f32((quad / SEGMENTS) + ring_offsets[corner]) / f32(RINGS);
    let phi = 2.0 * PI * f32((quad % SEGMENTS) + segment_offsets[corner]) / f32(SEGMENTS);
    let normal = vec3<f32>(sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi));
    let instance = instances[instance_index];
    let world = normal * instance.offset_scale.w + instance.offset_scale.xyz;
    return Varying(
        frame.view_projection * vec4<f32>(world, 1.0),
        world,
        normal,
        vec4<f32>(-sin(phi), 0.0, cos(phi), 1.0),
        instance.material.x,
    );
}

@fragment
fn fs(in: Varying) -> @location(0) vec4<f32> {
    var input: SurfaceInput;
    input.position = in.world_position;
    input.normal = normalize(in.normal);
    input.tangent = in.tangent;
    input.color = vec4<f32>(1.0);
    input.front_facing = true;
    let surface = load_surface(materials[in.material], input);
    let view = normalize(frame.camera_position.xyz - in.world_position);
    let lighting = ibl_evaluate(
        ibl_irradiance,
        ibl_prefiltered,
        ibl_brdf_lut,
        ibl_sampler,
        IblInput(surface.normal, view, surface.diffuse_color, surface.f0, surface.f90, surface.roughness),
    );
    let radiance = surface.emissive + lighting.diffuse * surface.occlusion + lighting.specular;
    return vec4<f32>(radiance / (vec3<f32>(1.0) + radiance), 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Frame {
    view_projection: [[f32; 4]; 4],
    camera_position: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    offset_scale: [f32; 4],
    material: [u32; 4],
}

fn material_for(row: usize, column: usize) -> StandardMaterial {
    let (base_color, metallic) = match row {
        0 => ([0.8, 0.12, 0.1, 1.0], 0.0),
        1 => ([1.0, 0.77, 0.34, 1.0], 1.0),
        _ => ([0.95, 0.95, 0.95, 1.0], 1.0),
    };
    StandardMaterial {
        base_color,
        metallic,
        roughness: 0.08 + 0.2 * column as f32,
        ..StandardMaterial::default()
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

fn environment_image(path: Option<&str>) -> EquirectImage {
    let Some(path) = path else {
        return SyntheticSky::default().image(1024, 512).expect("synthetic sky");
    };
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let picture = radiance::read(&bytes).unwrap_or_else(|error| panic!("{path}: {error}"));
    log::info!("read {path}: {}x{}", picture.width, picture.height);
    EquirectImage::from_rgb_f32(picture.width, picture.height, &picture.rgb)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

fn main() {
    pathon_examples::init_logging();
    let device = Device::build(&DeviceRequest {
        label: "ibl spheres",
        ..Default::default()
    })
    .expect("a GPU adapter");
    let mut arguments = std::env::args().skip(1);
    let output_path = arguments.next().unwrap_or_else(|| "ibl_spheres.png".to_owned());
    let environment_path = arguments.next();

    // The prefilter, the irradiance map and the BRDF table are computed once here, not per frame.
    let mut builder = IblBuilder::new();
    let environment = builder
        .build_environment(
            &device,
            &environment_image(environment_path.as_deref()),
            &IblSettings::DESKTOP,
        )
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));

    let mut table = MaterialTable::new("materials", 2);
    let mut variant = MaterialVariant::default();
    let mut instances = Vec::new();
    let spacing = 1.15;
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let material = material_for(row, column);
            variant = variant.merge(MaterialVariant::of(&material)).expect("one alpha kind");
            let id = table
                .add(&material)
                .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
            instances.push(Instance {
                offset_scale: [
                    (column as f32 - (COLUMNS - 1) as f32 / 2.0) * spacing,
                    ((ROWS - 1) as f32 / 2.0 - row as f32) * spacing,
                    0.0,
                    0.5,
                ],
                material: [id.index(), 0, 0, 0],
            });
        }
    }
    let materials = table
        .sync(&device, &mut DeferredRelease::new(2), 0)
        .expect("material upload");

    let mut library = ShaderLibrary::new();
    for (path, source) in MaterialHooks::default()
        .modules()
        .into_iter()
        .chain(pathon_ibl::modules())
    {
        library.add_module(path, source).expect("shader module");
    }
    let ibl = IblBindings::starting_at(3);
    let source = SHADER
        .replace("IBL_DECLARATIONS", &ibl.wgsl_declarations(0))
        .replace("RING_COUNT", &format!("{RINGS}u"))
        .replace("SEGMENT_COUNT", &format!("{SEGMENTS}u"));
    library.add_root("ibl_spheres", &source);
    let key = variant.shader_key("ibl_spheres");
    let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));
    let module = library.module(&device, &key).unwrap_or_else(|error| panic!("{error}"));
    let layout = device
        .create_bind_group_layout(&reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection")[0])
        .expect("layout");

    let camera = Camera::look_at(
        Vec3::new(0.0, 0.0, 7.0),
        Vec3::ZERO,
        30.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
    );
    let eye = camera.position;
    let frame = Frame {
        view_projection: camera.view_projection().to_cols_array_2d(),
        camera_position: [eye.x, eye.y, eye.z, 1.0],
    };
    let mut entries: Vec<BindGroupEntry> = [
        upload(&device, "frame", bytemuck::bytes_of(&frame), BufferUsage::UNIFORM),
        materials,
        upload(
            &device,
            "instances",
            bytemuck::cast_slice(&instances),
            BufferUsage::STORAGE,
        ),
    ]
    .iter()
    .enumerate()
    .map(|(binding, buffer)| BindGroupEntry {
        binding: u32::try_from(binding).expect("three bindings"),
        resource: BindingResource::whole_buffer(*buffer),
    })
    .collect();
    entries.extend(ibl.bind_group_entries(&environment));
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "ibl spheres",
            layout,
            entries,
        })
        .expect("bind group");

    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "ibl spheres",
            bind_group_layouts: vec![layout],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: vec![],
            fragment: Some(ShaderStage {
                module,
                entry_point: "fs",
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8UnormSrgb)],
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
        pass.draw(0..RINGS * SEGMENTS * 6, 0..instance_count);
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
    log::info!(
        "wrote {output_path}: {COLUMNS}x{ROWS} spheres lit by the environment alone, {} roughness levels",
        environment.prefiltered_levels
    );
    environment.release(&device);
    builder.release(&device);
}
