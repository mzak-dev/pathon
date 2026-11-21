#![cfg(feature = "gpu-tests")]

use pathon_rhi::{
    BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType, BufferDesc,
    BufferUsage, CommandList, ComputePassDesc, ComputePipelineDesc, Device, DeviceApi, DeviceRequest, Features,
    ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess,
};

const WGSL: &str = r"
override scale: f32 = 1.0;
var<immediate> value: u32;
@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(1)
fn main() {
    out[0] = value;
    out[1] = bitcast<u32>(scale * 2.0);
}
";

#[test]
fn compute_reads_immediates_and_pipeline_overrides() {
    let device = Device::build(&DeviceRequest {
        label: "immediates",
        required_features: Features::IMMEDIATES,
        ..Default::default()
    })
    .expect("a GPU adapter with IMMEDIATES is required for gpu-tests");

    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "immediates",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::StorageBuffer {
                    access: StorageAccess::ReadWrite,
                },
                count: None,
            }],
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
            label: "immediates",
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::whole_buffer(output),
            }],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "immediates",
            wgsl: WGSL,
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "immediates",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![("scale", 3.0)],
            },
            immediate_size: 4,
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "immediates",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.set_immediates(0, &1234u32.to_le_bytes());
        pass.dispatch(1, 1, 1);
    }
    list.copy_buffer_to_buffer(output, 0, readback, 0, 8);
    device.submit(list).expect("submit");

    let values: Vec<u32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
    assert_eq!(values[0], 1234);
    assert_eq!(f32::from_bits(values[1]), 6.0);
}
