#![cfg(feature = "gpu-tests")]

use std::num::NonZeroU32;

use pathon_rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, BufferCopy, BufferDesc, BufferUsage, Color, ColorAttachment, ColorTargetState,
    CommandList, ComputePassDesc, ComputePipelineDesc, Device, DeviceApi, DeviceRequest, Extent3d, Features, Format,
    PrimitiveState, RenderPassDesc, RenderPipelineDesc, SamplerBindingType, SamplerDesc, ShaderModuleDesc, ShaderStage,
    ShaderStages, StorageAccess, SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureId, TextureSampleType,
    TextureUsage, TextureViewDesc, TextureViewDimension, TextureViewId,
};

const TEXTURE_COUNT: u32 = 1024;

const WGSL: &str = r"
enable wgpu_binding_array;
@group(0) @binding(0) var textures: binding_array<texture_2d<u32>, 1024>;
@group(0) @binding(1) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(1)
fn main() {
    let first = textureLoad(textures[700], vec2<i32>(0, 0), 0);
    let second = textureLoad(textures[3], vec2<i32>(0, 0), 0);
    out[0] = first.r;
    out[1] = second.r;
}
";

#[test]
fn compute_reads_elements_of_a_texture_binding_array() {
    let device = Device::build(&DeviceRequest {
        label: "binding array",
        required_features: Features::TEXTURE_BINDING_ARRAY,
        ..Default::default()
    })
    .expect("a GPU adapter with TEXTURE_BINDING_ARRAY is required for gpu-tests");

    let textures: Vec<_> = (0..TEXTURE_COUNT)
        .map(|index| {
            let texture = device
                .create_texture(&TextureDesc::new_2d(
                    "element",
                    1,
                    1,
                    Format::R32Uint,
                    TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                ))
                .expect("texture");
            let texel = index.to_le_bytes();
            device
                .write_texture(
                    &TextureCopy::whole(texture),
                    &texel,
                    TexelLayout {
                        offset: 0,
                        bytes_per_row: 4,
                        rows_per_image: 1,
                    },
                    Extent3d::new_2d(1, 1),
                )
                .expect("upload");
            texture
        })
        .collect();
    let views: Vec<_> = textures
        .iter()
        .map(|texture| {
            device
                .create_texture_view(*texture, &TextureViewDesc::default())
                .expect("view")
        })
        .collect();

    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "binding array",
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::SampledTexture {
                        sample_type: TextureSampleType::Uint,
                        dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: NonZeroU32::new(TEXTURE_COUNT),
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageBuffer {
                        access: StorageAccess::ReadWrite,
                    },
                    count: None,
                },
            ],
        })
        .expect("layout");
    let output = device
        .create_buffer(&BufferDesc {
            label: "out",
            size: 8,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
        })
        .expect("output");
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: 8,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "binding array",
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureViewArray(views),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::whole_buffer(output),
                },
            ],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "binding array",
            wgsl: WGSL,
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "binding array",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    for texture in &textures {
        list.texture_access(*texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    }
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "binding array",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(1, 1, 1);
    }
    list.copy_buffer_to_buffer(output, 0, readback, 0, 8);
    device.submit(list).expect("submit");

    let values: Vec<u32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
    assert_eq!(values, [700, 3]);
}

const NON_UNIFORM_COUNT: u32 = 64;
const NON_UNIFORM_FEATURES: Features = Features::TEXTURE_BINDING_ARRAY.union(Features::NON_UNIFORM_INDEXING);

// Invocation `i` reads element `(i * 7) % 64`: the index differs per invocation and per pixel.
const NON_UNIFORM_COMPUTE_WGSL: &str = r"
enable wgpu_binding_array;
@group(0) @binding(0) var textures: binding_array<texture_2d<u32>, 64>;
@group(0) @binding(1) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    out[id.x] = textureLoad(textures[(id.x * 7u) % 64u], vec2<i32>(0, 0), 0).r;
}
";

const NON_UNIFORM_RENDER_WGSL: &str = r"
enable wgpu_binding_array;
@group(0) @binding(0) var textures: binding_array<texture_2d<u32>, 64>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<u32> {
    let element = (u32(position.x) * 7u) % 64u;
    return textureLoad(textures[element], vec2<i32>(0, 0), 0);
}
";

fn non_uniform_device(label: &'static str) -> Device {
    Device::build(&DeviceRequest {
        label,
        required_features: NON_UNIFORM_FEATURES,
        ..Default::default()
    })
    .expect("a GPU adapter with TEXTURE_BINDING_ARRAY and NON_UNIFORM_INDEXING is required for gpu-tests")
}

// Texture `k` holds the single texel `k + 100`, already in the `SAMPLED` state once the returned barrier ran.
fn numbered_textures(device: &Device) -> (Vec<TextureId>, Vec<TextureViewId>) {
    let textures: Vec<_> = (0..NON_UNIFORM_COUNT)
        .map(|index| {
            let texture = device
                .create_texture(&TextureDesc::new_2d(
                    "element",
                    1,
                    1,
                    Format::R32Uint,
                    TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                ))
                .expect("texture");
            device
                .write_texture(
                    &TextureCopy::whole(texture),
                    &(index + 100).to_le_bytes(),
                    TexelLayout {
                        offset: 0,
                        bytes_per_row: 4,
                        rows_per_image: 1,
                    },
                    Extent3d::new_2d(1, 1),
                )
                .expect("upload");
            texture
        })
        .collect();
    let views = textures
        .iter()
        .map(|texture| {
            device
                .create_texture_view(*texture, &TextureViewDesc::default())
                .expect("view")
        })
        .collect();
    (textures, views)
}

fn numbered_layout(device: &Device, visibility: ShaderStages, extra: Vec<BindGroupLayoutEntry>) -> BindGroupLayoutId {
    let mut entries = vec![BindGroupLayoutEntry {
        binding: 0,
        visibility,
        ty: BindingType::SampledTexture {
            sample_type: TextureSampleType::Uint,
            dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: NonZeroU32::new(NON_UNIFORM_COUNT),
    }];
    entries.extend(extra);
    device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "non-uniform",
            entries,
        })
        .expect("layout")
}

fn expected_element(invocation: u32) -> u32 {
    (invocation * 7) % NON_UNIFORM_COUNT + 100
}

#[test]
fn compute_invocations_index_a_texture_binding_array_non_uniformly() {
    let device = non_uniform_device("non-uniform compute");
    let (textures, views) = numbered_textures(&device);
    let layout = numbered_layout(
        &device,
        ShaderStages::COMPUTE,
        vec![BindGroupLayoutEntry {
            binding: 1,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::StorageBuffer {
                access: StorageAccess::ReadWrite,
            },
            count: None,
        }],
    );
    let size = u64::from(NON_UNIFORM_COUNT) * 4;
    let output = device
        .create_buffer(&BufferDesc {
            label: "out",
            size,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
        })
        .expect("output");
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "non-uniform",
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureViewArray(views),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::whole_buffer(output),
                },
            ],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "non-uniform",
            wgsl: NON_UNIFORM_COMPUTE_WGSL,
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "non-uniform",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    for texture in &textures {
        list.texture_access(*texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    }
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "non-uniform",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(1, 1, 1);
    }
    list.copy_buffer_to_buffer(output, 0, readback, 0, size);
    device.submit(list).expect("submit");

    let values: Vec<u32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
    let expected: Vec<u32> = (0..NON_UNIFORM_COUNT).map(expected_element).collect();
    assert_eq!(values, expected);
}

#[test]
fn fragments_index_a_texture_binding_array_non_uniformly() {
    let device = non_uniform_device("non-uniform fragment");
    let (textures, views) = numbered_textures(&device);
    let layout = numbered_layout(&device, ShaderStages::FRAGMENT, vec![]);
    let row_bytes = NON_UNIFORM_COUNT * 4;
    let target = device
        .create_texture(&TextureDesc::new_2d(
            "target",
            NON_UNIFORM_COUNT,
            1,
            Format::R32Uint,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
        ))
        .expect("target");
    let target_view = device
        .create_texture_view(target, &TextureViewDesc::default())
        .expect("target view");
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: u64::from(row_bytes),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "non-uniform",
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureViewArray(views),
            }],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "non-uniform",
            wgsl: NON_UNIFORM_RENDER_WGSL,
        })
        .expect("module");
    let stage = |entry_point| ShaderStage {
        module,
        entry_point,
        constants: vec![],
    };
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "non-uniform",
            bind_group_layouts: vec![layout],
            vertex: stage("vs_main"),
            vertex_buffers: vec![],
            fragment: Some(stage("fs_main")),
            color_targets: vec![ColorTargetState::opaque(Format::R32Uint)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    for texture in &textures {
        list.texture_access(*texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    }
    list.texture_access(target, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "non-uniform",
            color: vec![ColorAttachment::clear(target_view, Color::BLACK)],
            depth: None,
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.draw(0..3, 0..1);
    }
    list.texture_access(
        target,
        SubresourceRange::ALL,
        Access::COLOR_ATTACHMENT,
        Access::COPY_SRC,
    );
    list.copy_texture_to_buffer(
        TextureCopy::whole(target),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: row_bytes,
                rows_per_image: 1,
            },
        },
        Extent3d::new_2d(NON_UNIFORM_COUNT, 1),
    );
    device.submit(list).expect("submit");

    let values: Vec<u32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
    let expected: Vec<u32> = (0..NON_UNIFORM_COUNT).map(expected_element).collect();
    assert_eq!(values, expected);
}

const SAMPLER_WGSL: &str = r"
enable wgpu_binding_array;
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var samplers: binding_array<sampler, 2>;
@group(0) @binding(2) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(1)
fn main() {
    out[0] = textureSampleLevel(source, samplers[0], vec2<f32>(0.375, 0.5), 0.0).r;
    out[1] = textureSampleLevel(source, samplers[1], vec2<f32>(0.375, 0.5), 0.0).r;
}
";

#[test]
fn compute_reads_elements_of_a_sampler_binding_array() {
    let device = Device::build(&DeviceRequest {
        label: "sampler array",
        required_features: Features::TEXTURE_BINDING_ARRAY,
        ..Default::default()
    })
    .expect("a GPU adapter with TEXTURE_BINDING_ARRAY is required for gpu-tests");

    let source = device
        .create_texture(&TextureDesc::new_2d(
            "source",
            2,
            1,
            Format::Rgba8Unorm,
            TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        ))
        .expect("source");
    device
        .write_texture(
            &TextureCopy::whole(source),
            &[0, 0, 0, 255, 255, 0, 0, 255],
            TexelLayout {
                offset: 0,
                bytes_per_row: 8,
                rows_per_image: 1,
            },
            Extent3d::new_2d(2, 1),
        )
        .expect("upload");
    let view = device
        .create_texture_view(source, &TextureViewDesc::default())
        .expect("view");
    let nearest = device.create_sampler(&SamplerDesc::NEAREST_CLAMP).expect("nearest");
    let linear = device.create_sampler(&SamplerDesc::LINEAR_CLAMP).expect("linear");

    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "sampler array",
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::texture_2d(),
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: NonZeroU32::new(2),
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageBuffer {
                        access: StorageAccess::ReadWrite,
                    },
                    count: None,
                },
            ],
        })
        .expect("layout");
    let output = device
        .create_buffer(&BufferDesc {
            label: "out",
            size: 8,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
        })
        .expect("output");
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: 8,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "sampler array",
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::SamplerArray(vec![nearest, linear]),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::whole_buffer(output),
                },
            ],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "sampler array",
            wgsl: SAMPLER_WGSL,
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "sampler array",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    list.texture_access(source, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "sampler array",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(1, 1, 1);
    }
    list.copy_buffer_to_buffer(output, 0, readback, 0, 8);
    device.submit(list).expect("submit");

    let values: Vec<f32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
    assert_eq!(values[0], 0.0, "nearest sampler picks the first texel");
    assert!((values[1] - 0.25).abs() < 0.01, "linear sampler blends: {}", values[1]);
}
