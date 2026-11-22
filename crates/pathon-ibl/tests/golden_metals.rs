#![cfg(feature = "gpu-tests")]
// M4-06: spheres in `material::standard` lit by nothing but the environment, a synthetic sky with a bright sun.
// Rows are a dielectric and two metals, columns a roughness sweep. The scene is rendered in linear HDR, which the
// assertions read (metals are not black, reflections blur monotonically), and tone mapped on the CPU into the golden
// image `ibl_metals`. `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

mod support;

use pathon_api::Camera;
use pathon_api::glam::{Vec3, Vec4};
use pathon_api::resources::DeferredRelease;
use pathon_golden::{GoldenImage, Tolerance, assert_golden};
use pathon_ibl::{IblBindings, IblBuilder, IblEnvironment, IblSettings, SyntheticSky};
use pathon_material::{MaterialHooks, MaterialTable, MaterialVariant, StandardMaterial};
use pathon_rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindingResource, BufferDesc, BufferUsage, Color, ColorAttachment,
    ColorTargetState, CommandList, CullMode, DepthAttachment, DepthState, Device, DeviceApi, Format, PrimitiveState,
    RenderPassDesc, RenderPipelineDesc, ShaderStage, SubresourceRange, TextureDesc, TextureUsage, TextureViewDesc,
};
use pathon_shader::{ShaderLibrary, reflect_bind_groups};
use support::{Mip, gpu, read_rgba16f};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 384;
const COLUMNS: usize = 5;
const ROWS: usize = 3;
const SPACING: f32 = 1.15;
const RADIUS: f32 = 0.5;
const ROUGHNESS: [f32; COLUMNS] = [0.08, 0.28, 0.48, 0.68, 0.88];

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

const RINGS: u32 = 32u;
const SEGMENTS: u32 = 64u;
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
    return vec4<f32>(surface.emissive + lighting.diffuse * surface.occlusion + lighting.specular, 1.0);
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
        roughness: ROUGHNESS[column],
        ..StandardMaterial::default()
    }
}

fn sphere_centre(row: usize, column: usize) -> Vec3 {
    Vec3::new(
        (column as f32 - (COLUMNS - 1) as f32 / 2.0) * SPACING,
        ((ROWS - 1) as f32 / 2.0 - row as f32) * SPACING,
        0.0,
    )
}

fn camera() -> Camera {
    Camera::look_at(
        Vec3::new(0.0, 0.0, 7.0),
        Vec3::ZERO,
        30.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
    )
}

fn upload(device: &Device, label: &'static str, bytes: &[u8], usage: BufferUsage) -> pathon_rhi::BufferId {
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

// Linear HDR radiance of the whole grid, read back from an `Rgba16Float` target.
fn render(device: &Device, environment: &IblEnvironment) -> Mip {
    let mut table = MaterialTable::new("materials", 2);
    let mut variant = MaterialVariant::default();
    let mut instances = Vec::new();
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let material = material_for(row, column);
            variant = variant.merge(MaterialVariant::of(&material)).expect("one alpha kind");
            let id = table.add(&material).expect("valid material");
            let centre = sphere_centre(row, column);
            instances.push(Instance {
                offset_scale: [centre.x, centre.y, centre.z, RADIUS],
                material: [id.index(), 0, 0, 0],
            });
        }
    }
    let materials = table
        .sync(device, &mut DeferredRelease::new(2), 0)
        .expect("material upload");

    let mut library = ShaderLibrary::new();
    for (path, source) in MaterialHooks::default().modules() {
        library.add_module(path, source).expect("material module");
    }
    for (path, source) in pathon_ibl::modules() {
        library.add_module(path, source).expect("ibl module");
    }
    let ibl = IblBindings::starting_at(3);
    library.add_root(
        "ibl_metals",
        &SHADER.replace("IBL_DECLARATIONS", &ibl.wgsl_declarations(0)),
    );
    let key = variant.shader_key("ibl_metals");
    let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));
    let module = library.module(device, &key).unwrap_or_else(|error| panic!("{error}"));
    let layout = device
        .create_bind_group_layout(&reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection")[0])
        .expect("layout");

    let camera = camera();
    let eye = camera.position;
    let frame = Frame {
        view_projection: camera.view_projection().to_cols_array_2d(),
        camera_position: [eye.x, eye.y, eye.z, 1.0],
    };
    let mut entries: Vec<BindGroupEntry> = [
        upload(device, "frame", bytemuck::bytes_of(&frame), BufferUsage::UNIFORM),
        materials,
        upload(
            device,
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
    entries.extend(ibl.bind_group_entries(environment));
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "ibl metals",
            layout,
            entries,
        })
        .expect("bind group");

    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "ibl metals",
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
            color_targets: vec![ColorTargetState::opaque(Format::Rgba16Float)],
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
            "ibl metals color",
            WIDTH,
            HEIGHT,
            Format::Rgba16Float,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
        ))
        .expect("color");
    let depth = device
        .create_texture(&TextureDesc::new_2d(
            "ibl metals depth",
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

    let mut list = CommandList::new();
    list.texture_access(color, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    list.texture_access(depth, SubresourceRange::ALL, Access::empty(), Access::DEPTH_WRITE);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "ibl metals",
            color: vec![ColorAttachment::clear(color_view, Color::BLACK)],
            depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        let instance_count = u32::try_from(instances.len()).expect("few instances");
        pass.draw(0..32 * 64 * 6, 0..instance_count);
    }
    list.texture_access(color, SubresourceRange::ALL, Access::COLOR_ATTACHMENT, Access::COPY_SRC);
    device.submit(list).expect("submit");
    read_rgba16f(device, color, Access::COPY_SRC, 0, 0)
}

fn luminance(texel: [f32; 4]) -> f32 {
    0.2126 * texel[0] + 0.7152 * texel[1] + 0.0722 * texel[2]
}

// Pixels well inside the disc of one sphere: its centre and radius in pixels come from the camera.
fn sphere_pixels(row: usize, column: usize) -> Vec<(u32, u32)> {
    let camera = camera();
    let to_pixel = |point: Vec3| -> (f32, f32) {
        let clip = camera.view_projection() * Vec4::new(point.x, point.y, point.z, 1.0);
        let ndc = clip / clip.w;
        ((ndc.x * 0.5 + 0.5) * WIDTH as f32, (0.5 - ndc.y * 0.5) * HEIGHT as f32)
    };
    let centre = sphere_centre(row, column);
    let (cx, cy) = to_pixel(centre);
    let (edge_x, _) = to_pixel(centre + Vec3::new(RADIUS, 0.0, 0.0));
    let radius = (edge_x - cx) * 0.8;
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            dx * dx + dy * dy <= radius * radius
        })
        .collect()
}

fn statistics(image: &Mip, row: usize, column: usize) -> (f32, f32) {
    let values: Vec<f32> = sphere_pixels(row, column)
        .into_iter()
        .map(|(x, y)| luminance(image.at(x, y)))
        .collect();
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let variance = values.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / values.len() as f32;
    (mean, variance.sqrt())
}

fn srgb_byte(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 before the cast"
    )]
    let byte = (encoded.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    byte
}

// Reinhard on every channel: enough to show the sun without a tone mapper pack.
fn tone_mapped(image: &Mip) -> GoldenImage {
    let bytes: Vec<u8> = image
        .texels
        .iter()
        .flat_map(|texel| {
            let [r, g, b] = [texel[0], texel[1], texel[2]].map(|channel| srgb_byte(channel / (1.0 + channel)));
            [r, g, b, 255]
        })
        .collect();
    GoldenImage::from_rgba8(image.width, image.height, bytes).expect("image")
}

#[test]
fn metals_lit_by_the_environment_are_bright_and_blur_with_roughness() {
    let test = "metals_lit_by_the_environment_are_bright_and_blur_with_roughness";
    let Some(device) = gpu(test) else {
        return;
    };
    let image = SyntheticSky::default().image(1024, 512).expect("sky");
    let mut builder = IblBuilder::new();
    let environment = builder
        .build_environment(&device, &image, &IblSettings::DESKTOP)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    let hdr = render(&device, &environment);

    let mut means = [[0.0_f32; COLUMNS]; ROWS];
    let mut contrast = [[0.0_f32; COLUMNS]; ROWS];
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let (mean, deviation) = statistics(&hdr, row, column);
            (means[row][column], contrast[row][column]) = (mean, deviation);
        }
    }
    eprintln!("mean luminance per sphere: {means:.3?}");
    eprintln!("luminance deviation per sphere: {contrast:.3?}");

    // Without IBL a metal has no diffuse light and nothing to reflect: black. Under the environment it is as bright as
    // what it reflects.
    for column in 0..COLUMNS {
        assert!(
            means[1][column] > 0.15,
            "gold, roughness {}: {}",
            ROUGHNESS[column],
            means[1][column]
        );
        assert!(
            means[2][column] > 0.25,
            "silver, roughness {}: {}",
            ROUGHNESS[column],
            means[2][column]
        );
        assert!(
            means[0][column] > 0.05,
            "dielectric, roughness {}: {}",
            ROUGHNESS[column],
            means[0][column]
        );
    }
    // Reflections blur as roughness grows, so the contrast inside a metal sphere falls monotonically.
    for row in [1, 2] {
        for column in 1..COLUMNS {
            assert!(
                contrast[row][column] < contrast[row][column - 1],
                "row {row}: contrast {} at roughness {} is not below {} at {}",
                contrast[row][column],
                ROUGHNESS[column],
                contrast[row][column - 1],
                ROUGHNESS[column - 1]
            );
        }
        assert!(
            contrast[row][COLUMNS - 1] < 0.5 * contrast[row][0],
            "row {row}: {:?}",
            contrast[row]
        );
    }
    // A rough metal still reflects about the same energy as a smooth one (single plus multiple scattering).
    for column in 1..COLUMNS {
        let ratio = means[2][column] / means[2][0];
        assert!(
            (0.6..1.4).contains(&ratio),
            "silver roughness {}: {ratio}",
            ROUGHNESS[column]
        );
    }

    assert_golden("ibl_metals", &tone_mapped(&hdr), Tolerance::cross_backend());
    environment.release(&device);
    builder.release(&device);
}
