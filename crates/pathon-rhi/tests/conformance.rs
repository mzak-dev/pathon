use pathon_rhi::{
    Access, Backend, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource,
    BindingType, BufferCopy, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList,
    ComputePassDesc, ComputePipelineDesc, Device, DeviceApi, Extent3d, Features, Format, IndexFormat, PassTimestamps,
    PrimitiveState, QuerySetDesc, RenderPassDesc, RenderPipelineDesc, SamplerBindingType, SamplerDesc,
    ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess, SubresourceRange, TexelLayout, TextureCopy,
    TextureDesc, TextureUsage, TextureViewDesc, VertexAttribute, VertexBufferLayout, VertexFormat, VertexStepMode,
};

const COMPUTE_WGSL: &str = r"
@group(0) @binding(0) var<storage, read_write> values: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&values)) {
        values[id.x] = values[id.x] * 2u;
    }
}
";

const RASTER_WGSL: &str = r"
@group(0) @binding(0) var color_texture: texture_2d<f32>;
@group(0) @binding(1) var color_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = position * 0.5 + 0.5;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(color_texture, color_sampler, in.uv);
}
";

const ROW_BYTES: u32 = 256;

fn readback_buffer(device: &Device, size: u64) -> BufferId {
    device
        .create_buffer(&BufferDesc {
            label: "readback",
            size,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback buffer")
}

fn executes(device: &Device) -> bool {
    device.caps().backend != Backend::Null
}

fn buffer_texture_round_trip(device: &Device) {
    let width = ROW_BYTES / 4;
    let pixels: Vec<u8> = (0..ROW_BYTES * 4)
        .map(|index| u8::try_from(index % 251).expect("fits"))
        .collect();
    let upload = device
        .create_buffer(&BufferDesc {
            label: "upload",
            size: pixels.len() as u64,
            usage: BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
        })
        .expect("upload buffer");
    device.write_buffer(upload, 0, &pixels).expect("write");
    let texture = device
        .create_texture(&TextureDesc::new_2d(
            "copy target",
            width,
            4,
            Format::Rgba8Unorm,
            TextureUsage::COPY_DST | TextureUsage::COPY_SRC,
        ))
        .expect("texture");
    let readback = readback_buffer(device, pixels.len() as u64);
    let layout = TexelLayout {
        offset: 0,
        bytes_per_row: ROW_BYTES,
        rows_per_image: 4,
    };
    let size = Extent3d::new_2d(width, 4);

    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST);
    list.copy_buffer_to_texture(BufferCopy { buffer: upload, layout }, TextureCopy::whole(texture), size);
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(texture),
        BufferCopy {
            buffer: readback,
            layout,
        },
        size,
    );
    device.submit(list).expect("copy round trip");

    if executes(device) {
        assert_eq!(device.read_buffer(readback).expect("read"), pixels);
    }
}

fn compute_doubles_values(device: &Device) {
    let values: Vec<u32> = (0..256).collect();
    let bytes: &[u8] = bytemuck::cast_slice(&values);
    let storage = device
        .create_buffer(&BufferDesc {
            label: "values",
            size: bytes.len() as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
        })
        .expect("storage");
    device.write_buffer(storage, 0, bytes).expect("write");
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "values",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::StorageBuffer {
                    access: StorageAccess::ReadWrite,
                },
            }],
        })
        .expect("layout");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "values",
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::whole_buffer(storage),
            }],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "double",
            wgsl: COMPUTE_WGSL,
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "double",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
            },
        })
        .expect("pipeline");
    let readback = readback_buffer(device, bytes.len() as u64);

    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "double",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(4, 1, 1);
    }
    list.copy_buffer_to_buffer(storage, 0, readback, 0, bytes.len() as u64);
    device.submit(list).expect("compute");

    if executes(device) {
        let result: Vec<u32> = bytemuck::cast_slice(&device.read_buffer(readback).expect("read")).to_vec();
        let expected: Vec<u32> = values.iter().map(|value| value * 2).collect();
        assert_eq!(result, expected);
    }
}

fn indexed_quad_samples_texture(device: &Device) {
    let target_size = ROW_BYTES / 4;
    let green = [0_u8, 255, 0, 255];
    let source = device
        .create_texture(&TextureDesc::new_2d(
            "source",
            2,
            2,
            Format::Rgba8Unorm,
            TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        ))
        .expect("source");
    device
        .write_texture(
            &TextureCopy::whole(source),
            &green.repeat(4),
            TexelLayout {
                offset: 0,
                bytes_per_row: 8,
                rows_per_image: 2,
            },
            Extent3d::new_2d(2, 2),
        )
        .expect("upload source");
    let target = device
        .create_texture(&TextureDesc::new_2d(
            "target",
            target_size,
            target_size,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
        ))
        .expect("target");
    let source_view = device
        .create_texture_view(source, &TextureViewDesc::default())
        .expect("view");
    let target_view = device
        .create_texture_view(target, &TextureViewDesc::default())
        .expect("view");
    let sampler = device.create_sampler(&SamplerDesc::NEAREST_CLAMP).expect("sampler");

    let positions: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0];
    let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
    let vertex_buffer = device
        .create_buffer(&BufferDesc {
            label: "quad vertices",
            size: 32,
            usage: BufferUsage::VERTEX | BufferUsage::COPY_DST,
        })
        .expect("vertices");
    device
        .write_buffer(vertex_buffer, 0, bytemuck::cast_slice(&positions))
        .expect("write");
    let index_buffer = device
        .create_buffer(&BufferDesc {
            label: "quad indices",
            size: 12,
            usage: BufferUsage::INDEX | BufferUsage::COPY_DST,
        })
        .expect("indices");
    device
        .write_buffer(index_buffer, 0, bytemuck::cast_slice(&indices))
        .expect("write");

    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "sampled",
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::texture_2d(),
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                },
            ],
        })
        .expect("layout");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "sampled",
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(source_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(sampler),
                },
            ],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "quad",
            wgsl: RASTER_WGSL,
        })
        .expect("module");
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "quad",
            bind_group_layouts: vec![layout],
            vertex: ShaderStage {
                module,
                entry_point: "vs_main",
            },
            vertex_buffers: vec![VertexBufferLayout {
                stride: 8,
                step_mode: VertexStepMode::Vertex,
                attributes: vec![VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 0,
                    location: 0,
                }],
            }],
            fragment: Some(ShaderStage {
                module,
                entry_point: "fs_main",
            }),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
        })
        .expect("pipeline");

    let timestamps = if device.caps().has(Features::TIMESTAMP_QUERY) {
        let query_set = device
            .create_query_set(&QuerySetDesc {
                label: "quad",
                count: 2,
            })
            .expect("query set");
        Some(PassTimestamps {
            query_set,
            begin_index: 0,
            end_index: 1,
        })
    } else {
        None
    };
    let readback = readback_buffer(device, u64::from(ROW_BYTES * target_size));

    let mut list = CommandList::new();
    list.texture_access(source, SubresourceRange::ALL, Access::SHADER_READ);
    list.texture_access(target, SubresourceRange::ALL, Access::COLOR_ATTACHMENT);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "quad",
            color: vec![ColorAttachment::clear(target_view, Color::BLACK)],
            depth: None,
            timestamps,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.set_vertex_buffer(0, vertex_buffer, 0);
        pass.set_index_buffer(index_buffer, IndexFormat::Uint16, 0);
        pass.draw_indexed(0..6, 0, 0..1);
    }
    list.texture_access(target, SubresourceRange::ALL, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(target),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: ROW_BYTES,
                rows_per_image: target_size,
            },
        },
        Extent3d::new_2d(target_size, target_size),
    );
    let timestamp_buffers = timestamps.map(|timestamps| {
        let resolve = device
            .create_buffer(&BufferDesc {
                label: "timestamps",
                size: 16,
                usage: BufferUsage::QUERY_RESOLVE | BufferUsage::COPY_SRC,
            })
            .expect("resolve buffer");
        let read = readback_buffer(device, 16);
        list.resolve_queries(timestamps.query_set, 0..2, resolve, 0);
        list.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
        read
    });
    device.submit(list).expect("raster");

    if executes(device) {
        let pixels = device.read_buffer(readback).expect("read");
        let center = usize::try_from((target_size / 2) * ROW_BYTES + (target_size / 2) * 4).expect("fits");
        assert_eq!(&pixels[center..center + 4], &green);
        if let Some(read) = timestamp_buffers {
            let ticks: Vec<u64> = bytemuck::cast_slice(&device.read_buffer(read).expect("read")).to_vec();
            assert!(
                ticks[1] >= ticks[0],
                "end timestamp {} precedes begin {}",
                ticks[1],
                ticks[0]
            );
        }
    }
}

fn run_conformance(device: &Device) {
    buffer_texture_round_trip(device);
    compute_doubles_values(device);
    indexed_quad_samples_texture(device);
}

#[test]
fn conformance_on_null() {
    run_conformance(&Device::null());
}

#[cfg(feature = "gpu-tests")]
#[test]
fn conformance_on_wgpu() {
    let device = Device::build(&pathon_rhi::DeviceRequest {
        label: "conformance",
        optional_features: Features::TIMESTAMP_QUERY,
        ..Default::default()
    })
    .expect("a GPU adapter is required for gpu-tests");
    run_conformance(&device);
}
