#![cfg(feature = "gpu-tests")]

use pathon_rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
    BufferCopy, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, CommandList, ComputePassDesc,
    ComputePipelineDesc, Device, DeviceApi, DeviceRequest, Extent3d, Format, RenderPassDesc, SamplerBindingType,
    SamplerDesc, ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess, SubresourceRange, TexelLayout,
    TextureCopy, TextureDesc, TextureSampleType, TextureUsage, TextureViewDesc, TextureViewDimension,
};

const FACE_SIZE: u32 = 4;

// +X, -X, +Y, -Y, +Z, -Z: the layer order of a cube texture.
const FACE_COLORS: [[f32; 4]; 6] = [
    [1.0, 0.0, 0.0, 1.0],
    [0.0, 1.0, 0.0, 1.0],
    [0.0, 0.0, 1.0, 1.0],
    [1.0, 1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0, 1.0],
    [1.0, 0.0, 1.0, 1.0],
];

const CUBE_WGSL: &str = r"
@group(0) @binding(0) var environment: texture_cube<f32>;
@group(0) @binding(1) var environment_sampler: sampler;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<f32>>;

@compute @workgroup_size(6)
fn main(@builtin(local_invocation_id) id: vec3<u32>) {
    var directions = array<vec3<f32>, 6>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, -1.0),
    );
    out[id.x] = textureSampleLevel(environment, environment_sampler, directions[id.x], 0.0);
}
";

const SRGB_WGSL: &str = r"
@group(0) @binding(0) var raw: texture_2d<f32>;
@group(0) @binding(1) var decoded: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(1)
fn main() {
    out[0] = textureLoad(raw, vec2<i32>(0, 0), 0).r;
    out[1] = textureLoad(decoded, vec2<i32>(0, 0), 0).r;
}
";

const SRGB_HALF_GREY_BYTE: u8 = 188;

fn device(label: &'static str) -> Device {
    Device::build(&DeviceRequest {
        label,
        ..Default::default()
    })
    .expect("a GPU adapter is required for gpu-tests")
}

fn readback(device: &Device, size: u64) -> BufferId {
    device
        .create_buffer(&BufferDesc {
            label: "readback",
            size,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback")
}

fn storage_output(device: &Device, size: u64) -> BufferId {
    device
        .create_buffer(&BufferDesc {
            label: "out",
            size,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
        })
        .expect("output")
}

fn sampled_entry(binding: u32, dimension: TextureViewDimension) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::SampledTexture {
            sample_type: TextureSampleType::Float,
            dimension,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::StorageBuffer {
            access: StorageAccess::ReadWrite,
        },
        count: None,
    }
}

fn dispatch_once(
    device: &Device,
    wgsl: &'static str,
    layout_entries: Vec<BindGroupLayoutEntry>,
    group_entries: Vec<BindGroupEntry>,
    list: &mut CommandList,
) {
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "views",
            entries: layout_entries,
        })
        .expect("layout");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "views",
            layout,
            entries: group_entries,
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc { label: "views", wgsl })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "views",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("pipeline");
    let mut pass = list.compute_pass(ComputePassDesc {
        label: "views",
        timestamps: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group);
    pass.dispatch(1, 1, 1);
}

#[test]
fn each_cube_face_is_rendered_through_its_own_view_and_sampled_through_a_cube_view() {
    let device = device("cube");
    let cube = device
        .create_texture(&TextureDesc::new_cube(
            "environment",
            FACE_SIZE,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED,
        ))
        .expect("cube texture");
    let cube_view = device
        .create_texture_view(cube, &TextureViewDesc::cube())
        .expect("cube view");
    let sampler = device.create_sampler(&SamplerDesc::NEAREST_CLAMP).expect("sampler");
    let output = storage_output(&device, 6 * 16);
    let readback = readback(&device, 6 * 16);

    let mut list = CommandList::new();
    list.texture_access(cube, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    for (face, [r, g, b, a]) in (0..).zip(FACE_COLORS) {
        let face_view = device
            .create_texture_view(cube, &TextureViewDesc::layer(face))
            .expect("face view");
        let _pass = list.render_pass(RenderPassDesc {
            label: "face",
            color: vec![ColorAttachment::clear(
                face_view,
                Color {
                    r: f64::from(r),
                    g: f64::from(g),
                    b: f64::from(b),
                    a: f64::from(a),
                },
            )],
            depth: None,
            timestamps: None,
        });
    }
    list.texture_access(cube, SubresourceRange::ALL, Access::COLOR_ATTACHMENT, Access::SAMPLED);
    dispatch_once(
        &device,
        CUBE_WGSL,
        vec![
            sampled_entry(0, TextureViewDimension::Cube),
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            storage_entry(2),
        ],
        vec![
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(cube_view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(sampler),
            },
            BindGroupEntry {
                binding: 2,
                resource: BindingResource::whole_buffer(output),
            },
        ],
        &mut list,
    );
    list.copy_buffer_to_buffer(output, 0, readback, 0, 6 * 16);
    device.submit(list).expect("submit");

    let sampled: Vec<f32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
    let expected: Vec<f32> = FACE_COLORS.iter().flatten().copied().collect();
    assert_eq!(sampled, expected, "direction i must land on face i");
}

#[test]
fn rendering_through_an_srgb_view_encodes_into_a_unorm_texture() {
    let device = device("srgb render");
    let texture = device
        .create_texture(
            &TextureDesc::new_2d(
                "albedo",
                1,
                1,
                Format::Rgba8Unorm,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            )
            .with_view_formats(&[Format::Rgba8UnormSrgb]),
        )
        .expect("texture");
    let srgb_view = device
        .create_texture_view(texture, &TextureViewDesc::with_format(Format::Rgba8UnormSrgb))
        .expect("sRGB view");
    let readback = readback(&device, 256);

    let mut list = CommandList::new();
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::COLOR_ATTACHMENT,
    );
    {
        let _pass = list.render_pass(RenderPassDesc {
            label: "clear",
            color: vec![ColorAttachment::clear(
                srgb_view,
                Color {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 1.0,
                },
            )],
            depth: None,
            timestamps: None,
        });
    }
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::COLOR_ATTACHMENT,
        Access::COPY_SRC,
    );
    list.copy_texture_to_buffer(
        TextureCopy::whole(texture),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: 256,
                rows_per_image: 1,
            },
        },
        Extent3d::new_2d(1, 1),
    );
    device.submit(list).expect("submit");

    let bytes = device.read_buffer(readback).expect("read");
    assert!(
        bytes[0].abs_diff(SRGB_HALF_GREY_BYTE) <= 1,
        "linear 0.5 must be stored sRGB-encoded as ~{SRGB_HALF_GREY_BYTE}, got {}",
        bytes[0]
    );
    assert_eq!(bytes[3], 255);
}

#[test]
fn sampling_through_an_srgb_view_decodes_a_unorm_texture() {
    let device = device("srgb sample");
    let texture = device
        .create_texture(
            &TextureDesc::new_2d(
                "albedo",
                1,
                1,
                Format::Rgba8Unorm,
                TextureUsage::SAMPLED | TextureUsage::COPY_DST,
            )
            .with_view_formats(&[Format::Rgba8UnormSrgb]),
        )
        .expect("texture");
    device
        .write_texture(
            &TextureCopy::whole(texture),
            &[SRGB_HALF_GREY_BYTE; 4],
            TexelLayout {
                offset: 0,
                bytes_per_row: 4,
                rows_per_image: 1,
            },
            Extent3d::new_2d(1, 1),
        )
        .expect("upload");
    let raw_view = device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("raw view");
    let decoded_view = device
        .create_texture_view(texture, &TextureViewDesc::with_format(Format::Rgba8UnormSrgb))
        .expect("sRGB view");
    let output = storage_output(&device, 8);
    let readback = readback(&device, 8);

    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    dispatch_once(
        &device,
        SRGB_WGSL,
        vec![
            sampled_entry(0, TextureViewDimension::D2),
            sampled_entry(1, TextureViewDimension::D2),
            storage_entry(2),
        ],
        vec![
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(raw_view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(decoded_view),
            },
            BindGroupEntry {
                binding: 2,
                resource: BindingResource::whole_buffer(output),
            },
        ],
        &mut list,
    );
    list.copy_buffer_to_buffer(output, 0, readback, 0, 8);
    device.submit(list).expect("submit");

    let values: Vec<f32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
    let encoded = f32::from(SRGB_HALF_GREY_BYTE) / 255.0;
    assert!(
        (values[0] - encoded).abs() < 0.005,
        "raw view returns the stored value: {}",
        values[0]
    );
    assert!(
        (values[1] - 0.5).abs() < 0.01,
        "sRGB view decodes to linear 0.5: {}",
        values[1]
    );
}

#[test]
fn a_view_format_missing_from_the_texture_is_an_error_not_a_panic() {
    let device = device("undeclared view format");
    let texture = device
        .create_texture(&TextureDesc::new_2d(
            "plain",
            1,
            1,
            Format::Rgba8Unorm,
            TextureUsage::SAMPLED,
        ))
        .expect("texture");

    let error = device
        .create_texture_view(texture, &TextureViewDesc::with_format(Format::Rgba8UnormSrgb))
        .expect_err("the sRGB twin was not declared");
    assert_eq!(
        error.code(),
        "R0009",
        "wgpu reports it as a backend validation error: {error}"
    );
}
