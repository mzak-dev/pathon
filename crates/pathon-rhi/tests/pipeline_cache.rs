use pathon_rhi::{Device, DeviceApi, DeviceRequest, PipelineCacheData};

#[test]
fn null_device_has_no_pipeline_cache() {
    assert!(Device::null().pipeline_cache_data().is_none());
}

#[test]
fn pipeline_cache_data_exposes_bytes_and_debug_prints_only_the_length() {
    // SAFETY: the bytes never reach a driver; the test only inspects the newtype.
    let data = unsafe { PipelineCacheData::from_bytes(vec![1, 2, 3]) };
    assert_eq!(data.as_bytes(), &[1, 2, 3]);
    assert_eq!(format!("{data:?}"), "PipelineCacheData { len: 3 }");
    assert_eq!(data.clone().as_bytes(), data.as_bytes());
}

#[test]
fn null_device_ignores_a_blob_in_the_request() {
    // SAFETY: the null backend never hands the bytes to a driver.
    let blob = unsafe { PipelineCacheData::from_bytes(vec![7; 16]) };
    let device = Device::build(&DeviceRequest {
        backend: Some(pathon_rhi::Backend::Null),
        pipeline_cache: Some(blob),
        ..Default::default()
    })
    .expect("null device");
    assert!(device.pipeline_cache_data().is_none());
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_rhi::{
        Backend, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource,
        BindingType, BufferDesc, BufferUsage, CommandList, ComputePassDesc, ComputePipelineDesc, Device, DeviceApi,
        DeviceRequest, Features, PipelineCacheData, RhiError, ShaderModuleDesc, ShaderStage, ShaderStages,
        StorageAccess,
    };

    const WGSL: &str = r"
@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(1)
fn main() {
    out[0] = 42u;
}
";

    fn vulkan_request(pipeline_cache: Option<PipelineCacheData>) -> DeviceRequest {
        DeviceRequest {
            label: "pipeline cache",
            backend: Some(Backend::Vulkan),
            required_features: Features::PIPELINE_CACHE,
            pipeline_cache,
            ..Default::default()
        }
    }

    #[expect(clippy::print_stderr, reason = "the only way a skipped GPU test says why")]
    fn build_or_skip(request: &DeviceRequest) -> Option<Device> {
        match Device::build(request) {
            Ok(device) => Some(device),
            Err(error @ (RhiError::NoAdapter(_) | RhiError::MissingFeatures(_))) => {
                eprintln!("skipped: no Vulkan adapter with PIPELINE_CACHE ({error})");
                None
            }
            Err(error) => panic!("device creation failed: {error}"),
        }
    }

    fn dispatch_and_read(device: &Device) -> u32 {
        let layout = device
            .create_bind_group_layout(&BindGroupLayoutDesc {
                label: "cache",
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
                label: "cache",
                layout,
                entries: vec![BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::whole_buffer(output),
                }],
            })
            .expect("bind group");
        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "cache",
                wgsl: WGSL,
            })
            .expect("module");
        let pipeline = device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "cache",
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
        {
            let mut pass = list.compute_pass(ComputePassDesc {
                label: "cache",
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

    #[test]
    fn blob_round_trips_between_devices() {
        let Some(first) = build_or_skip(&vulkan_request(None)) else {
            return;
        };
        assert_eq!(dispatch_and_read(&first), 42);
        let blob = first
            .pipeline_cache_data()
            .expect("a cache blob after creating a pipeline");
        assert_ne!(blob.as_bytes().len(), 0);

        let Some(second) = build_or_skip(&vulkan_request(Some(blob))) else {
            return;
        };
        assert_eq!(dispatch_and_read(&second), 42);
        let again = second.pipeline_cache_data().expect("a cache blob on the seeded device");
        assert_ne!(again.as_bytes().len(), 0);
    }

    #[test]
    fn garbage_blob_starts_an_empty_cache() {
        // SAFETY: deliberately violates the contract to prove `fallback: true` makes the driver reject the data
        // instead of trusting it.
        let garbage = unsafe { PipelineCacheData::from_bytes((0..=255u8).cycle().take(4096).collect()) };
        let Some(device) = build_or_skip(&vulkan_request(Some(garbage))) else {
            return;
        };
        assert_eq!(dispatch_and_read(&device), 42);
    }

    #[test]
    fn blob_without_the_feature_is_ignored() {
        // SAFETY: the feature is off, so the backend must not pass the bytes to the driver at all.
        let blob = unsafe { PipelineCacheData::from_bytes(vec![9; 64]) };
        let device = Device::build(&DeviceRequest {
            label: "no cache",
            pipeline_cache: Some(blob),
            ..Default::default()
        })
        .expect("a GPU adapter is required for gpu-tests");
        assert!(!device.caps().has(Features::PIPELINE_CACHE));
        assert!(device.pipeline_cache_data().is_none());
    }
}
