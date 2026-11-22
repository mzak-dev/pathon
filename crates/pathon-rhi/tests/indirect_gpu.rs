#![cfg(feature = "gpu-tests")]

// A compute pass writes the draw and dispatch arguments, and the indirect commands then run them. `PATHON_GPU_BACKEND`
// (`vulkan`, `dx12`, `metal`) pins the backend; otherwise the default adapter is used.

use pathon_rhi::{
    Access, Backend, BindGroupDesc, BindGroupEntry, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutEntry,
    BindGroupLayoutId, BindingResource, BindingType, BufferCopy, BufferDesc, BufferId, BufferUsage, Color,
    ColorAttachment, ColorTargetState, CommandList, ComputePassDesc, ComputePipelineDesc, Device, DeviceApi,
    DeviceRequest, DrawIndexedIndirectArgs, Extent3d, Features, Format, IndexFormat, MultiDrawCount, PrimitiveState,
    RenderPassDesc, RenderPassEncoder, RenderPipelineDesc, RenderPipelineId, RhiError, ShaderModuleDesc, ShaderStage,
    ShaderStages, StorageAccess, SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureUsage,
    TextureViewDesc, VertexAttribute, VertexBufferLayout, VertexFormat, VertexStepMode,
};

const SIZE: u32 = 64;
const ROW_BYTES: u32 = SIZE * 4;
const MESHES: u32 = 3;
const ELEMENT_SIZE: usize = 20;

// Mesh 1 is "culled": its instance count is 0. Indexed arguments start at byte 0 (20 B each), non-indexed ones at
// byte 64 (16 B each). `counts[0]` is 3 (everything) and `counts[1]` is 1 (only the first mesh).
const WRITE_DRAW_ARGUMENTS: &str = "
@group(0) @binding(0) var<storage, read_write> args: array<u32>;
@group(0) @binding(1) var<storage, read_write> counts: array<u32>;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let mesh = id.x;
    let instances = select(1u, 0u, mesh == 1u);
    let indexed = mesh * 5u;
    args[indexed] = 3u;
    args[indexed + 1u] = instances;
    args[indexed + 2u] = 0u;
    args[indexed + 3u] = mesh * 3u;
    args[indexed + 4u] = 0u;
    let direct = 16u + mesh * 4u;
    args[direct] = 3u;
    args[direct + 1u] = instances;
    args[direct + 2u] = mesh * 3u;
    args[direct + 3u] = 0u;
    if (mesh == 0u) {
        counts[0] = 3u;
        counts[1] = 1u;
    }
}
";

const DRAW_SHADER: &str = "
struct Vertex {
    @location(0) position: vec2<f32>,
    @location(1) color: vec3<f32>,
}
struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
}
@vertex fn vs(vertex: Vertex) -> Varying {
    var out: Varying;
    out.clip = vec4<f32>(vertex.position, 0.0, 1.0);
    out.color = vertex.color;
    return out;
}
@fragment fn fs(in: Varying) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
";

const WRITE_DISPATCH_ARGUMENTS: &str = "
@group(0) @binding(0) var<storage, read_write> args: array<u32>;

@compute @workgroup_size(1)
fn main() {
    args[0] = 4u;
    args[1] = 1u;
    args[2] = 1u;
}
";

const FILL: &str = "
@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(1)
fn main(@builtin(workgroup_id) id: vec3<u32>) {
    out[id.x] = id.x + 1u;
}
";

const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_GPU_BACKEND").ok()?.as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        other => panic!("PATHON_GPU_BACKEND must be vulkan, dx12 or metal, not '{other}'"),
    }
}

fn build_device(required_features: Features) -> Result<Device, RhiError> {
    Device::build(&DeviceRequest {
        label: "indirect",
        backend: backend_from_environment(),
        required_features,
        ..Default::default()
    })
}

fn storage_layout(device: &Device, bindings: u32) -> BindGroupLayoutId {
    device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "indirect",
            entries: (0..bindings)
                .map(|binding| BindGroupLayoutEntry {
                    binding,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageBuffer {
                        access: StorageAccess::ReadWrite,
                    },
                    count: None,
                })
                .collect(),
        })
        .expect("layout")
}

fn storage_group(device: &Device, layout: BindGroupLayoutId, buffers: &[BufferId]) -> BindGroupId {
    device
        .create_bind_group(&BindGroupDesc {
            label: "indirect",
            layout,
            entries: buffers
                .iter()
                .zip(0..)
                .map(|(buffer, binding)| BindGroupEntry {
                    binding,
                    resource: BindingResource::whole_buffer(*buffer),
                })
                .collect(),
        })
        .expect("bind group")
}

fn compute_pipeline(device: &Device, layout: BindGroupLayoutId, wgsl: &'static str) -> pathon_rhi::ComputePipelineId {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "indirect",
            wgsl,
        })
        .expect("module");
    device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "indirect",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("compute pipeline")
}

fn buffer(device: &Device, label: &'static str, size: u64, usage: BufferUsage) -> BufferId {
    device
        .create_buffer(&BufferDesc { label, size, usage })
        .expect("buffer")
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect()
}

// Three triangles side by side (red, green, blue) in one vertex and one index buffer; every mesh indexes 0..3 and the
// draw arguments pick it with `base_vertex` (indexed) or `first_vertex` (non-indexed).
struct Scene {
    device: Device,
    pipeline: RenderPipelineId,
    vertices: BufferId,
    indices: BufferId,
    args: BufferId,
    counts: BufferId,
    write_arguments: pathon_rhi::ComputePipelineId,
    write_group: BindGroupId,
}

impl Scene {
    fn new(device: Device) -> Self {
        let corners: [[f32; 5]; 9] = [
            [-0.9, -0.5, 1.0, 0.0, 0.0],
            [-0.3, -0.5, 1.0, 0.0, 0.0],
            [-0.6, 0.5, 1.0, 0.0, 0.0],
            [-0.3, -0.5, 0.0, 1.0, 0.0],
            [0.3, -0.5, 0.0, 1.0, 0.0],
            [0.0, 0.5, 0.0, 1.0, 0.0],
            [0.3, -0.5, 0.0, 0.0, 1.0],
            [0.9, -0.5, 0.0, 0.0, 1.0],
            [0.6, 0.5, 0.0, 0.0, 1.0],
        ];
        let vertex_bytes: Vec<u8> = corners.iter().flatten().flat_map(|value| value.to_le_bytes()).collect();
        let index_bytes: Vec<u8> = [0_u16, 1, 2, 0].iter().flat_map(|index| index.to_le_bytes()).collect();
        let vertices = buffer(
            &device,
            "vertices",
            vertex_bytes.len() as u64,
            BufferUsage::VERTEX | BufferUsage::COPY_DST,
        );
        device.write_buffer(vertices, 0, &vertex_bytes).expect("vertices");
        let indices = buffer(&device, "indices", 8, BufferUsage::INDEX | BufferUsage::COPY_DST);
        device.write_buffer(indices, 0, &index_bytes).expect("indices");
        let args = buffer(
            &device,
            "args",
            128,
            BufferUsage::STORAGE | BufferUsage::INDIRECT | BufferUsage::COPY_SRC,
        );
        let counts = buffer(&device, "counts", 16, BufferUsage::STORAGE | BufferUsage::INDIRECT);

        let layout = storage_layout(&device, 2);
        let write_arguments = compute_pipeline(&device, layout, WRITE_DRAW_ARGUMENTS);
        let write_group = storage_group(&device, layout, &[args, counts]);

        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "indirect draw",
                wgsl: DRAW_SHADER,
            })
            .expect("module");
        let pipeline = device
            .create_render_pipeline(&RenderPipelineDesc {
                label: "indirect draw",
                bind_group_layouts: vec![],
                vertex: ShaderStage {
                    module,
                    entry_point: "vs",
                    constants: vec![],
                },
                vertex_buffers: vec![VertexBufferLayout {
                    stride: 20,
                    step_mode: VertexStepMode::Vertex,
                    attributes: vec![
                        VertexAttribute {
                            format: VertexFormat::Float32x2,
                            offset: 0,
                            location: 0,
                        },
                        VertexAttribute {
                            format: VertexFormat::Float32x3,
                            offset: 8,
                            location: 1,
                        },
                    ],
                }],
                fragment: Some(ShaderStage {
                    module,
                    entry_point: "fs",
                    constants: vec![],
                }),
                color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
                primitive: PrimitiveState::default(),
                depth: None,
                sample_count: 1,
                immediate_size: 0,
            })
            .expect("pipeline");
        Self {
            device,
            pipeline,
            vertices,
            indices,
            args,
            counts,
            write_arguments,
            write_group,
        }
    }

    // The compute pass fills `args` and `counts`, the barriers hand them to the indirect reads, then `draw` runs.
    fn render(&self, draw: impl FnOnce(&mut RenderPassEncoder<'_>)) -> Vec<u8> {
        let device = &self.device;
        let target = device
            .create_texture(&TextureDesc::new_2d(
                "target",
                SIZE,
                SIZE,
                Format::Rgba8Unorm,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            ))
            .expect("target");
        let view = device
            .create_texture_view(target, &TextureViewDesc::default())
            .expect("view");
        let readback = buffer(
            device,
            "readback",
            u64::from(ROW_BYTES * SIZE),
            BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        );
        let mut list = CommandList::new();
        {
            let mut pass = list.compute_pass(ComputePassDesc {
                label: "write draw arguments",
                timestamps: None,
            });
            pass.set_pipeline(self.write_arguments);
            pass.set_bind_group(0, self.write_group);
            pass.dispatch(MESHES, 1, 1);
        }
        list.buffer_access(self.args, Access::INDIRECT);
        list.buffer_access(self.counts, Access::INDIRECT);
        list.texture_access(target, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "indirect draws",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            pass.set_pipeline(self.pipeline);
            pass.set_vertex_buffer(0, self.vertices, 0);
            pass.set_index_buffer(self.indices, IndexFormat::Uint16, 0);
            draw(&mut pass);
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
                    bytes_per_row: ROW_BYTES,
                    rows_per_image: SIZE,
                },
            },
            Extent3d::new_2d(SIZE, SIZE),
        );
        device.submit(list).expect("submit");
        device.read_buffer(readback).expect("read")
    }

    // The centroids of the left, middle and right triangle.
    fn meshes_drawn(image: &[u8]) -> [[u8; 4]; 3] {
        [13, 32, 51].map(|x| {
            let at = (37 * ROW_BYTES + x * 4) as usize;
            [image[at], image[at + 1], image[at + 2], image[at + 3]]
        })
    }
}

#[test]
fn the_gpu_writes_the_arguments_the_cpu_layout_describes() {
    assert_eq!(u64::try_from(ELEMENT_SIZE), Ok(DrawIndexedIndirectArgs::SIZE));
    let scene = Scene::new(build_device(Features::empty()).expect("a GPU adapter is required for gpu-tests"));
    scene.render(|_| {});
    let readback = buffer(
        &scene.device,
        "args readback",
        128,
        BufferUsage::MAP_READ | BufferUsage::COPY_DST,
    );
    let mut list = CommandList::new();
    list.copy_buffer_to_buffer(scene.args, 0, readback, 0, 128);
    scene.device.submit(list).expect("copy");
    let bytes = scene.device.read_buffer(readback).expect("read");
    for mesh in 0..MESHES as usize {
        let at = mesh * ELEMENT_SIZE;
        let mut element = [0; ELEMENT_SIZE];
        element.copy_from_slice(&bytes[at..at + ELEMENT_SIZE]);
        assert_eq!(
            DrawIndexedIndirectArgs::from_bytes(element),
            DrawIndexedIndirectArgs {
                index_count: 3,
                instance_count: u32::from(mesh != 1),
                first_index: 0,
                base_vertex: i32::try_from(mesh * 3).expect("small"),
                first_instance: 0,
            },
            "mesh {mesh}"
        );
    }
}

#[test]
fn indirect_draws_render_what_the_gpu_wrote() {
    let scene = Scene::new(build_device(Features::empty()).expect("a GPU adapter is required for gpu-tests"));
    let (args, direct) = (scene.args, 64);

    let image = scene.render(|pass| pass.draw_indexed_indirect(args, 2 * DrawIndexedIndirectArgs::SIZE));
    assert_eq!(
        Scene::meshes_drawn(&image),
        [BLACK, BLACK, BLUE],
        "one indexed draw: the last mesh"
    );

    let image = scene.render(|pass| pass.draw_indirect(args, direct));
    assert_eq!(
        Scene::meshes_drawn(&image),
        [RED, BLACK, BLACK],
        "one non-indexed draw: the first mesh"
    );

    let image = scene.render(|pass| pass.multi_draw_indexed_indirect(args, 0, MESHES));
    assert_eq!(
        Scene::meshes_drawn(&image),
        [RED, BLACK, BLUE],
        "N indexed meshes from one buffer, the culled one skipped"
    );

    let image = scene.render(|pass| pass.multi_draw_indirect(args, direct, MESHES));
    assert_eq!(Scene::meshes_drawn(&image), [RED, BLACK, BLUE], "N non-indexed meshes");

    let image = scene.render(|pass| pass.multi_draw_indexed_indirect(args, 0, 1));
    assert_eq!(
        Scene::meshes_drawn(&image),
        [RED, BLACK, BLACK],
        "count bounds the draws"
    );
}

#[test]
fn counted_multi_draws_follow_the_count_the_gpu_wrote() {
    let device = match build_device(Features::MULTI_DRAW_INDIRECT_COUNT) {
        Ok(device) => device,
        Err(RhiError::MissingFeatures(missing)) => {
            skip(&format!("the adapter lacks {missing:?}"));
            return;
        }
        Err(error) => panic!("a GPU adapter is required for gpu-tests: {error}"),
    };
    let scene = Scene::new(device);
    let (args, counts) = (scene.args, scene.counts);
    let draws = |args_offset, count_offset| MultiDrawCount {
        args,
        args_offset,
        count_buffer: counts,
        count_offset,
        max_count: MESHES,
    };

    let image = scene.render(|pass| pass.multi_draw_indexed_indirect_count(draws(0, 0)));
    assert_eq!(Scene::meshes_drawn(&image), [RED, BLACK, BLUE], "count 3: every mesh");
    let image = scene.render(|pass| pass.multi_draw_indexed_indirect_count(draws(0, 4)));
    assert_eq!(
        Scene::meshes_drawn(&image),
        [RED, BLACK, BLACK],
        "count 1: the GPU-written count wins"
    );
    let image = scene.render(|pass| pass.multi_draw_indirect_count(draws(64, 0)));
    assert_eq!(Scene::meshes_drawn(&image), [RED, BLACK, BLUE], "non-indexed, count 3");
    let image = scene.render(|pass| pass.multi_draw_indirect_count(draws(64, 4)));
    assert_eq!(Scene::meshes_drawn(&image), [RED, BLACK, BLACK], "non-indexed, count 1");
}

#[test]
fn counted_draws_are_refused_on_a_device_without_the_feature() {
    let device = build_device(Features::empty()).expect("a GPU adapter is required for gpu-tests");
    if device.caps().has(Features::MULTI_DRAW_INDIRECT_COUNT) {
        skip("the default device already has MULTI_DRAW_INDIRECT_COUNT, so there is nothing to refuse");
        return;
    }
    let scene = Scene::new(device);
    let draws = MultiDrawCount {
        args: scene.args,
        args_offset: 0,
        count_buffer: scene.counts,
        count_offset: 0,
        max_count: 1,
    };
    let mut list = CommandList::new();
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "refused",
            color: vec![],
            depth: None,
            timestamps: None,
        });
        pass.multi_draw_indirect_count(draws);
    }
    assert_eq!(scene.device.submit(list).expect_err("refused").code(), "R0002");
}

#[test]
fn an_indirect_dispatch_runs_the_workgroups_the_gpu_asked_for() {
    let device = build_device(Features::empty()).expect("a GPU adapter is required for gpu-tests");
    let args = buffer(
        &device,
        "dispatch args",
        12,
        BufferUsage::STORAGE | BufferUsage::INDIRECT,
    );
    let out = buffer(&device, "out", 32, BufferUsage::STORAGE | BufferUsage::COPY_SRC);
    let readback = buffer(&device, "readback", 32, BufferUsage::MAP_READ | BufferUsage::COPY_DST);
    let layout = storage_layout(&device, 1);
    let write = compute_pipeline(&device, layout, WRITE_DISPATCH_ARGUMENTS);
    let fill = compute_pipeline(&device, layout, FILL);
    let args_group = storage_group(&device, layout, &[args]);
    let out_group = storage_group(&device, layout, &[out]);

    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "write dispatch arguments",
            timestamps: None,
        });
        pass.set_pipeline(write);
        pass.set_bind_group(0, args_group);
        pass.dispatch(1, 1, 1);
    }
    list.buffer_access(args, Access::INDIRECT);
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "indirect dispatch",
            timestamps: None,
        });
        pass.set_pipeline(fill);
        pass.set_bind_group(0, out_group);
        pass.dispatch_indirect(args, 0);
    }
    list.copy_buffer_to_buffer(out, 0, readback, 0, 32);
    device.submit(list).expect("submit");

    assert_eq!(
        words(&device.read_buffer(readback).expect("read")),
        [1, 2, 3, 4, 0, 0, 0, 0],
        "exactly the four workgroups the arguments asked for"
    );
}

#[expect(clippy::print_stderr, reason = "the only way a skipped GPU test says why")]
fn skip(reason: &str) {
    eprintln!("IGNORED (not a pass): {reason}");
}
