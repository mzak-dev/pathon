#![cfg(feature = "gpu-tests")]

use std::num::NonZeroU32;

use pathon_rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
    BufferDesc, BufferUsage, CommandList, ComputePassDesc, ComputePipelineDesc, Device, DeviceApi, DeviceRequest,
    Extent3d, Features, Format, ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess, SubresourceRange,
    TexelLayout, TextureCopy, TextureDesc, TextureSampleType, TextureUsage, TextureViewDesc, TextureViewDimension,
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
            },
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    for texture in &textures {
        list.texture_access(*texture, SubresourceRange::ALL, Access::SAMPLED);
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
