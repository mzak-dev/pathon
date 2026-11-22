#![cfg(feature = "gpu-tests")]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{ThreadedPool, p99};
use pathon_rhi::{
    BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId, BindingResource,
    BindingType, BufferDesc, BufferUsage, CommandList, ComputePassDesc, ComputePipelineDesc, ComputePipelineId, Device,
    DeviceRequest, ShaderModuleId, ShaderStage, ShaderStages, StorageAccess,
};
use pathon_rhi_types::DeviceApi;
use pathon_shader::{BackgroundCompiler, BackgroundConfig, Lookup, ShaderFeatures, ShaderKey};

const COMPUTE: &str = "
@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@if(wide) const FACTOR: u32 = 4u;
@if(!wide) const FACTOR: u32 = 1u;

@compute @workgroup_size(1)
fn main() {
    @if(shadows) { out[0] = 10u * FACTOR; }
    @else { out[0] = FACTOR; }
}
";

// Same frame budget as the null-device test (tests/background.rs): the compiler may take 10 ms of a 16.6 ms frame.
const FRAME_BUDGET: Duration = Duration::from_millis(10);
const FILLER_FLAGS: usize = 4;

fn key(flags: &[String]) -> ShaderKey {
    ShaderKey {
        root: "compute".to_owned(),
        features: flags.iter().fold(ShaderFeatures::default(), |features, flag| {
            features.with(flag).expect("flag")
        }),
    }
}

// The four meaningful permutations, each repeated with a different unused flag, so sixteen distinct modules reach the
// driver.
fn sixteen_permutations() -> Vec<(ShaderKey, u32)> {
    let mut keys = Vec::new();
    for filler in 0..FILLER_FLAGS {
        for (wide, shadows, expected) in [(false, false, 1), (true, false, 4), (false, true, 10), (true, true, 40)] {
            let mut flags = vec![format!("filler{filler}")];
            if wide {
                flags.push("wide".to_owned());
            }
            if shadows {
                flags.push("shadows".to_owned());
            }
            keys.push((key(&flags), expected));
        }
    }
    keys
}

fn layout(device: &Device) -> BindGroupLayoutId {
    device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "background",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::StorageBuffer {
                    access: StorageAccess::ReadWrite,
                },
                count: None,
            }],
        })
        .expect("layout")
}

fn compute_desc(layout: BindGroupLayoutId, module: ShaderModuleId) -> ComputePipelineDesc {
    ComputePipelineDesc {
        label: "background",
        bind_group_layouts: vec![layout],
        shader: ShaderStage {
            module,
            entry_point: "main",
            constants: vec![],
        },
        immediate_size: 0,
    }
}

fn run(device: &Device, layout: BindGroupLayoutId, pipeline: ComputePipelineId) -> u32 {
    let output = device
        .create_buffer(&BufferDesc {
            label: "out",
            size: 4,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
        })
        .expect("output");
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: 4,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "background",
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::whole_buffer(output),
            }],
        })
        .expect("group");
    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "background",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(1, 1, 1);
    }
    list.copy_buffer_to_buffer(output, 0, readback, 0, 4);
    device.submit(list).expect("submit");
    let bytes = device.read_buffer(readback).expect("read");
    u32::from_le_bytes(bytes[..4].try_into().expect("four bytes"))
}

// Real wgpu compilation on pool threads: composing, module creation and pipeline creation all happen off the frame
// thread, the frame only pumps and looks up, and what comes out computes the right answers.
#[test]
fn sixteen_new_permutations_compile_on_real_wgpu_without_stalling_frames() {
    let device = Arc::new(
        Device::build(&DeviceRequest {
            label: "background compile",
            ..Default::default()
        })
        .expect("a GPU adapter is required for gpu-tests"),
    );
    let handle: Arc<dyn DeviceApi> = Arc::<Device>::clone(&device);
    let mut compiler = BackgroundCompiler::new(handle, Arc::new(ThreadedPool::new(4)), BackgroundConfig::new());
    compiler.add_root("compute", COMPUTE);
    let layout = layout(&device);
    let permutations = sixteen_permutations();
    let mut pipelines: Vec<Option<ComputePipelineId>> = vec![None; permutations.len()];
    let mut frames = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let loop_started = Instant::now();

    while pipelines.iter().any(Option::is_none) || frames.len() < 100 {
        assert!(Instant::now() < deadline, "the permutations never finished");
        let started = Instant::now();
        compiler.pump();
        for (index, (key, _)) in permutations.iter().enumerate() {
            if pipelines[index].is_some() {
                continue;
            }
            if let Some(module) = compiler.shader_module(key).ready()
                && let Lookup::Ready(pipeline) = compiler.compute_pipeline(&compute_desc(layout, module))
            {
                pipelines[index] = Some(pipeline);
            }
        }
        frames.push(started.elapsed());
        std::thread::sleep(Duration::from_millis(1));
    }

    println!("compiled in {:?}", loop_started.elapsed());
    let p99 = p99(&mut frames);
    println!(
        "p99 frame cost {p99:?} over {} frames (budget {FRAME_BUDGET:?})",
        frames.len()
    );
    assert!(
        p99 < FRAME_BUDGET,
        "p99 frame cost {p99:?} over {} frames",
        frames.len()
    );
    assert!(compiler.take_failures().is_empty());
    for ((_, expected), pipeline) in permutations.iter().zip(&pipelines) {
        let pipeline = pipeline.expect("every pipeline is ready");
        assert_eq!(run(&device, layout, pipeline), *expected);
    }
}
