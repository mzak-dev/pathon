#![cfg(feature = "gpu-tests")]

use pathon_rhi::{
    BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType, BufferDesc,
    BufferUsage, CommandList, ComputePassDesc, ComputePipelineDesc, Device, DeviceApi, DeviceRequest, Features,
    OverrideValue, ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess,
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
                constants: vec![("scale", 3.0.into())],
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

fn device_without_immediates() -> Device {
    Device::build(&DeviceRequest {
        label: "no immediates",
        ..Default::default()
    })
    .expect("a GPU adapter is required for gpu-tests")
}

fn compute_desc(
    device: &Device,
    wgsl: &'static str,
    constants: Vec<(&'static str, OverrideValue)>,
    immediate_size: u32,
) -> ComputePipelineDesc {
    let module = device
        .create_shader_module(&ShaderModuleDesc { label: "probe", wgsl })
        .expect("module");
    ComputePipelineDesc {
        label: "probe",
        bind_group_layouts: vec![],
        shader: ShaderStage {
            module,
            entry_point: "main",
            constants,
        },
        immediate_size,
    }
}

const EMPTY_WGSL: &str = "override scale: f32 = 1.0; @compute @workgroup_size(1) fn main() { _ = scale; }";

#[test]
fn immediates_need_the_feature_and_report_it() {
    let device = device_without_immediates();
    assert_eq!(device.caps().limits.max_immediate_size, 0);
    let desc = compute_desc(&device, EMPTY_WGSL, vec![], 4);
    let error = device.create_compute_pipeline(&desc).expect_err("no feature");
    assert_eq!(error.code(), "R0002");
}

#[test]
fn a_misaligned_immediate_size_is_rejected_by_the_backend() {
    let device = Device::build(&DeviceRequest {
        label: "immediates",
        required_features: Features::IMMEDIATES,
        ..Default::default()
    })
    .expect("a GPU adapter with IMMEDIATES is required for gpu-tests");
    let desc = compute_desc(&device, EMPTY_WGSL, vec![], 6);
    assert_eq!(
        device.create_compute_pipeline(&desc).expect_err("misaligned").code(),
        "R0107"
    );
}

#[test]
fn set_immediates_without_the_feature_is_an_error_not_a_panic() {
    let device = device_without_immediates();
    let desc = compute_desc(&device, EMPTY_WGSL, vec![], 0);
    let pipeline = device.create_compute_pipeline(&desc).expect("pipeline");
    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "probe",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_immediates(0, &[0; 4]);
        pass.dispatch(1, 1, 1);
    }
    assert!(device.submit(list).is_err());
}

#[test]
fn an_unknown_or_duplicate_override_is_an_error_not_a_panic() {
    let device = device_without_immediates();
    let unknown = compute_desc(&device, EMPTY_WGSL, vec![("missing", 1.0.into())], 0);
    assert!(device.create_compute_pipeline(&unknown).is_err());
    let duplicate = compute_desc(
        &device,
        EMPTY_WGSL,
        vec![("scale", 1.0.into()), ("scale", 2.0.into())],
        0,
    );
    assert!(device.create_compute_pipeline(&duplicate).is_err());
}
