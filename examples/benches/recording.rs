// M2-16: CPU cost of recording the same draws through pathon's command list (built, then translated into a wgpu
// encoder) versus calling wgpu directly. Both stop at `CommandEncoder::finish`; nothing is submitted, so only the
// recording overhead is measured. Needs a GPU. Results: docs/bench/README.md.

use std::sync::OnceLock;

use divan::Bencher;
use pathon::rhi::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource,
    BindingType, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList, DeviceApi,
    Format, IndexFormat, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, ShaderModuleDesc,
    ShaderStage, ShaderStages, TextureDesc, TextureUsage, TextureViewDesc, TextureViewId, VertexAttribute,
    VertexBufferLayout, VertexFormat, VertexStepMode,
};
use pathon_examples::block_on_ready;
use pathon_rhi_wgpu::{WgpuDevice, interop};

const SHADER: &str = r"
@group(0) @binding(0) var<uniform> offset: vec4<f32>;

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position * 0.1, 0.0, 1.0) + offset;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.5, 0.0, 1.0);
}
";

#[expect(clippy::print_stderr, reason = "the benchmark reports which GPU it measured")]
fn main() {
    let caps = scene().pathon.caps();
    eprintln!(
        "adapter: {} ({:?}, {:?})",
        caps.adapter.name, caps.backend, caps.adapter.device_type
    );
    divan::main();
}

const QUAD: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0];
const INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

struct Direct {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    view: wgpu::TextureView,
}

struct ViaPathon {
    pipeline: RenderPipelineId,
    group: BindGroupId,
    vertices: BufferId,
    indices: BufferId,
    view: TextureViewId,
}

struct Scene {
    device: wgpu::Device,
    pathon: WgpuDevice,
    direct: Direct,
    via_pathon: ViaPathon,
}

fn wgpu_device() -> (wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = block_on_ready(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .expect("a GPU adapter is required for this benchmark");
    let (device, queue) = block_on_ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
    (instance, adapter, device, queue)
}

fn direct_objects(device: &wgpu::Device, queue: &wgpu::Queue) -> Direct {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bench"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bench"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bench"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("bench"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                }],
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState::from(wgpu::TextureFormat::Rgba8Unorm))],
        }),
        multiview_mask: None,
        cache: None,
    });
    let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages, data: &[u8]| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, data);
        buffer
    };
    let uniform = buffer("offset", 16, wgpu::BufferUsages::UNIFORM, &[0; 16]);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bench"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    let vertices = buffer("quad", 32, wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&QUAD));
    let indices = buffer(
        "quad indices",
        12,
        wgpu::BufferUsages::INDEX,
        bytemuck::cast_slice(&INDICES),
    );
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: 256,
            height: 256,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    Direct {
        pipeline,
        group,
        vertices,
        indices,
        view,
    }
}

fn pathon_objects(device: &WgpuDevice) -> ViaPathon {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "bench",
            wgsl: SHADER,
        })
        .expect("module");
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "bench",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::UniformBuffer,
            }],
        })
        .expect("layout");
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "bench",
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
    let buffer = |label: &'static str, size: u64, usage: BufferUsage, data: &[u8]| {
        let buffer = device
            .create_buffer(&BufferDesc {
                label,
                size,
                usage: usage | BufferUsage::COPY_DST,
            })
            .expect("buffer");
        device.write_buffer(buffer, 0, data).expect("write");
        buffer
    };
    let uniform = buffer("offset", 16, BufferUsage::UNIFORM, &[0; 16]);
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "bench",
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::whole_buffer(uniform),
            }],
        })
        .expect("bind group");
    let vertices = buffer("quad", 32, BufferUsage::VERTEX, bytemuck::cast_slice(&QUAD));
    let indices = buffer("quad indices", 12, BufferUsage::INDEX, bytemuck::cast_slice(&INDICES));
    let target = device
        .create_texture(&TextureDesc::new_2d(
            "target",
            256,
            256,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET,
        ))
        .expect("target");
    let view = device
        .create_texture_view(target, &TextureViewDesc::default())
        .expect("view");
    ViaPathon {
        pipeline,
        group,
        vertices,
        indices,
        view,
    }
}

fn scene() -> &'static Scene {
    static SCENE: OnceLock<Scene> = OnceLock::new();
    SCENE.get_or_init(|| {
        let (instance, adapter, device, queue) = wgpu_device();
        let direct = direct_objects(&device, &queue);
        let pathon = interop::adopt(instance, adapter, device.clone(), queue);
        let via_pathon = pathon_objects(&pathon);
        Scene {
            device,
            pathon,
            direct,
            via_pathon,
        }
    })
}

fn new_encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bench") })
}

#[divan::bench(args = [100, 1_000, 10_000])]
fn direct_wgpu(bencher: Bencher<'_, '_>, draws: u32) {
    let scene = scene();
    let objects = &scene.direct;
    bencher.bench_local(|| {
        let mut encoder = new_encoder(&scene.device);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &objects.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&objects.pipeline);
            for _ in 0..draws {
                pass.set_bind_group(0, &objects.group, &[]);
                pass.set_vertex_buffer(0, objects.vertices.slice(..));
                pass.set_index_buffer(objects.indices.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..6, 0, 0..1);
            }
        }
        encoder.finish()
    });
}

#[divan::bench(args = [100, 1_000, 10_000])]
fn via_pathon_command_list(bencher: Bencher<'_, '_>, draws: u32) {
    let scene = scene();
    let objects = &scene.via_pathon;
    bencher.bench_local(|| {
        let mut list = CommandList::new();
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "bench",
                color: vec![ColorAttachment::clear(objects.view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            pass.set_pipeline(objects.pipeline);
            for _ in 0..draws {
                pass.set_bind_group(0, objects.group);
                pass.set_vertex_buffer(0, objects.vertices, 0);
                pass.set_index_buffer(objects.indices, IndexFormat::Uint16, 0);
                pass.draw_indexed(0..6, 0, 0..1);
            }
        }
        let mut encoder = new_encoder(&scene.device);
        scene.pathon.encode_into(list, &mut encoder).expect("encode");
        encoder.finish()
    });
}
