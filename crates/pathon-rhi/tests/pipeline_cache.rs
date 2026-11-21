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
        Access, Backend, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource,
        BindingType, BufferCopy, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState,
        CommandList, ComputePassDesc, ComputePipelineDesc, ComputePipelineId, Device, DeviceApi, DeviceRequest,
        Extent3d, Features, Format, PipelineCacheData, PrimitiveState, RenderPassDesc, RenderPipelineDesc,
        RenderPipelineId, RhiError, ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess, SubresourceRange,
        TexelLayout, TextureCopy, TextureDesc, TextureId, TextureUsage, TextureViewDesc, TextureViewId,
    };

    // Offsets in the wgpu cache header: backend byte at 16, adapter key from 17 (wgpu-core `pipeline_cache`).
    const BACKEND_BYTE: usize = 16;
    const ADAPTER_KEY_BYTE: usize = 20;
    const ROW_BYTES: u32 = 256;
    const TARGET_SIZE: u32 = 8;

    const COMPUTE_WGSL: &str = r"
@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(1)
fn main() {
    out[0] = 42u;
}
";

    const RENDER_WGSL: &str = r"
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
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

    fn blob_len(device: &Device) -> usize {
        device
            .pipeline_cache_data()
            .expect("a device with PIPELINE_CACHE has a blob")
            .as_bytes()
            .len()
    }

    fn readback_buffer(device: &Device, size: u64) -> BufferId {
        device
            .create_buffer(&BufferDesc {
                label: "readback",
                size,
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback")
    }

    struct ComputeCase {
        pipeline: ComputePipelineId,
        group: pathon_rhi::BindGroupId,
        output: BufferId,
        readback: BufferId,
    }

    fn create_compute(device: &Device) -> ComputeCase {
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
                label: "cache compute",
                wgsl: COMPUTE_WGSL,
            })
            .expect("module");
        let pipeline = device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "cache compute",
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point: "main",
                    constants: vec![],
                },
                immediate_size: 0,
            })
            .expect("compute pipeline");
        ComputeCase {
            pipeline,
            group,
            output,
            readback: readback_buffer(device, 4),
        }
    }

    fn run_compute(device: &Device, case: &ComputeCase) -> u32 {
        let mut list = CommandList::new();
        {
            let mut pass = list.compute_pass(ComputePassDesc {
                label: "cache",
                timestamps: None,
            });
            pass.set_pipeline(case.pipeline);
            pass.set_bind_group(0, case.group);
            pass.dispatch(1, 1, 1);
        }
        list.copy_buffer_to_buffer(case.output, 0, case.readback, 0, 4);
        device.submit(list).expect("submit compute");
        let bytes = device.read_buffer(case.readback).expect("read");
        u32::from_le_bytes(bytes[..4].try_into().expect("four bytes"))
    }

    struct RenderCase {
        pipeline: RenderPipelineId,
        target: TextureId,
        view: TextureViewId,
        readback: BufferId,
    }

    fn create_render(device: &Device) -> RenderCase {
        let target = device
            .create_texture(&TextureDesc::new_2d(
                "cache target",
                TARGET_SIZE,
                TARGET_SIZE,
                Format::Rgba8Unorm,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            ))
            .expect("target");
        let view = device
            .create_texture_view(target, &TextureViewDesc::default())
            .expect("view");
        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "cache render",
                wgsl: RENDER_WGSL,
            })
            .expect("module");
        let pipeline = device
            .create_render_pipeline(&RenderPipelineDesc {
                label: "cache render",
                bind_group_layouts: vec![],
                vertex: ShaderStage {
                    module,
                    entry_point: "vs_main",
                    constants: vec![],
                },
                vertex_buffers: vec![],
                fragment: Some(ShaderStage {
                    module,
                    entry_point: "fs_main",
                    constants: vec![],
                }),
                color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
                primitive: PrimitiveState::default(),
                depth: None,
                sample_count: 1,
                immediate_size: 0,
            })
            .expect("render pipeline");
        RenderCase {
            pipeline,
            target,
            view,
            readback: readback_buffer(device, u64::from(ROW_BYTES * TARGET_SIZE)),
        }
    }

    fn run_render(device: &Device, case: &RenderCase) -> [u8; 4] {
        let mut list = CommandList::new();
        list.texture_access(case.target, SubresourceRange::ALL, Access::COLOR_ATTACHMENT);
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "cache",
                color: vec![ColorAttachment::clear(case.view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            pass.set_pipeline(case.pipeline);
            pass.draw(0..3, 0..1);
        }
        list.texture_access(case.target, SubresourceRange::ALL, Access::COPY_SRC);
        list.copy_texture_to_buffer(
            TextureCopy::whole(case.target),
            BufferCopy {
                buffer: case.readback,
                layout: TexelLayout {
                    offset: 0,
                    bytes_per_row: ROW_BYTES,
                    rows_per_image: TARGET_SIZE,
                },
            },
            Extent3d::new_2d(TARGET_SIZE, TARGET_SIZE),
        );
        device.submit(list).expect("submit render");
        let pixels = device.read_buffer(case.readback).expect("read");
        let center = usize::try_from((TARGET_SIZE / 2) * ROW_BYTES + (TARGET_SIZE / 2) * 4).expect("fits");
        pixels[center..center + 4].try_into().expect("four bytes")
    }

    fn assert_both_pipelines_work(device: &Device) {
        let compute = create_compute(device);
        assert_eq!(run_compute(device, &compute), 42);
        let render = create_render(device);
        assert_eq!(run_render(device, &render), [0, 255, 0, 255]);
    }

    fn blob_after_both_pipelines() -> Option<PipelineCacheData> {
        let device = build_or_skip(&vulkan_request(None))?;
        assert_both_pipelines_work(&device);
        Some(device.pipeline_cache_data().expect("a blob after creating pipelines"))
    }

    fn with_flipped_byte(blob: &PipelineCacheData, offset: usize) -> PipelineCacheData {
        let mut bytes = blob.as_bytes().to_vec();
        bytes[offset] ^= 0xff;
        // SAFETY: the flipped byte sits in the wgpu header, which wgpu-core validates before the data reaches the
        // driver; a mismatch is rejected there, so the driver never sees these bytes.
        unsafe { PipelineCacheData::from_bytes(bytes) }
    }

    #[test]
    fn blob_round_trips_between_devices() {
        let Some(first) = build_or_skip(&vulkan_request(None)) else {
            return;
        };
        let empty_len = blob_len(&first);
        let compute = create_compute(&first);
        assert_eq!(run_compute(&first, &compute), 42);
        let after_compute = blob_len(&first);
        assert!(
            after_compute > empty_len,
            "the compute pipeline did not write into the cache"
        );
        let render = create_render(&first);
        assert_eq!(run_render(&first, &render), [0, 255, 0, 255]);
        let after_render = blob_len(&first);
        assert!(
            after_render > after_compute,
            "the render pipeline did not write into the cache"
        );
        let blob = first.pipeline_cache_data().expect("blob");

        let Some(second) = build_or_skip(&vulkan_request(Some(blob))) else {
            return;
        };
        assert!(
            blob_len(&second) >= after_render,
            "the seed was dropped: a fresh device does not hold the first one's pipelines"
        );
        assert_both_pipelines_work(&second);
    }

    #[test]
    fn garbage_blob_starts_an_empty_cache() {
        // SAFETY: wgpu-core rejects the bytes on the header magic check (`Corrupted`), so they never reach the driver.
        let garbage = unsafe { PipelineCacheData::from_bytes((0..=255u8).cycle().take(4096).collect()) };
        let Some(device) = build_or_skip(&vulkan_request(Some(garbage))) else {
            return;
        };
        assert_both_pipelines_work(&device);
    }

    #[test]
    fn blob_from_another_adapter_starts_an_empty_cache() {
        let Some(blob) = blob_after_both_pipelines() else {
            return;
        };
        let Some(device) = build_or_skip(&vulkan_request(Some(with_flipped_byte(&blob, ADAPTER_KEY_BYTE)))) else {
            return;
        };
        assert_both_pipelines_work(&device);
    }

    #[test]
    fn blob_from_another_backend_starts_an_empty_cache() {
        let Some(blob) = blob_after_both_pipelines() else {
            return;
        };
        let Some(device) = build_or_skip(&vulkan_request(Some(with_flipped_byte(&blob, BACKEND_BYTE)))) else {
            return;
        };
        assert_both_pipelines_work(&device);
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
