// Several meshes of different sizes live in one mega-buffer; each draws from its own `base_vertex` / `first_index`.
// Runs on the null device (command validation) always and on a real GPU with `gpu-tests` (pixel readback).

use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_rhi::{
    Access, Backend, BufferCopy, BufferDesc, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList,
    Device, DeviceApi, Extent3d, Format, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId,
    ShaderModuleDesc, ShaderStage, SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureUsage,
    TextureViewDesc,
};

const SHADER: &str = "
struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
}
struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
}
@vertex fn vs(vertex: Vertex) -> Varying {
    var out: Varying;
    out.clip = vec4<f32>(vertex.position.xy, 0.0, 1.0);
    out.color = vertex.normal;
    return out;
}
@fragment fn fs(in: Varying) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
";

const SIZE: u32 = 64;
const ROW_BYTES: u32 = SIZE * 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Surface {
    normal: [f32; 3],
    tangent: [f32; 4],
    uv: [f32; 2],
}

// The normal doubles as the mesh's colour, so every mesh is recognizable in the image.
struct Shape {
    positions: Vec<[f32; 3]>,
    surface: Vec<Surface>,
    indices: Vec<u16>,
}

impl Shape {
    fn new(corners: &[[f32; 2]], color: [f32; 3], indices: &[u16]) -> Self {
        Self {
            positions: corners.iter().map(|corner| [corner[0], corner[1], 0.0]).collect(),
            surface: corners
                .iter()
                .map(|_| Surface {
                    normal: color,
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0; 2],
                })
                .collect(),
            indices: indices.to_vec(),
        }
    }

    fn data(&self, label: &'static str) -> MeshData<'_> {
        MeshData::new(
            label,
            vec![
                bytemuck::cast_slice(&self.positions),
                bytemuck::cast_slice(&self.surface),
            ],
            IndexData::U16(&self.indices),
        )
    }
}

fn pipeline(device: &Device, pool: &GeometryPool) -> RenderPipelineId {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "mesh pool test",
            wgsl: SHADER,
        })
        .expect("module");
    device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "mesh pool test",
            bind_group_layouts: vec![],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: pool.layout().vertex_buffer_layouts(),
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
        .expect("pipeline")
}

fn render(device: &Device, pool: &GeometryPool, pipeline: RenderPipelineId, meshes: &[MeshId]) -> Vec<u8> {
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
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: u64::from(ROW_BYTES * SIZE),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let mut list = CommandList::new();
    list.texture_access(target, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "meshes",
            color: vec![ColorAttachment::clear(view, Color::BLACK)],
            depth: None,
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        for mesh in meshes {
            pool.draw(&mut pass, *mesh, 0..1).expect("draw");
        }
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

fn pixel(image: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = (y * ROW_BYTES + x * 4) as usize;
    [image[at], image[at + 1], image[at + 2], image[at + 3]]
}

const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

fn scene(device: &Device) {
    let executes = device.caps().backend != Backend::Null;
    let triangle_left = Shape::new(&[[-0.9, -0.5], [-0.3, -0.5], [-0.6, 0.5]], [1.0, 0.0, 0.0], &[0, 1, 2]);
    let quad_middle = Shape::new(
        &[[-0.2, -0.5], [0.2, -0.5], [0.2, 0.5], [-0.2, 0.5]],
        [0.0, 1.0, 0.0],
        &[0, 1, 2, 0, 2, 3],
    );
    let triangle_right = Shape::new(&[[0.3, -0.5], [0.9, -0.5], [0.6, 0.5]], [0.0, 0.0, 1.0], &[0, 1, 2]);
    let quad_top = Shape::new(
        &[[-0.1, 0.6], [0.1, 0.6], [0.1, 0.9], [-0.1, 0.9]],
        [1.0, 1.0, 0.0],
        &[0, 1, 2, 0, 2, 3],
    );
    let mut pool = GeometryPool::new(
        device,
        "scene",
        VertexLayout::standard(),
        PoolDesc::default().with_chunk_sizes(64, 64).with_budget(64, 64),
    );
    let left = pool.upload(device, &triangle_left.data("left")).expect("left");
    let middle = pool.upload(device, &quad_middle.data("middle")).expect("middle");
    let right = pool.upload(device, &triangle_right.data("right")).expect("right");

    let locations: Vec<_> = [left, middle, right]
        .iter()
        .map(|mesh| pool.mesh(*mesh).expect("live").location)
        .collect();
    assert!(
        locations
            .iter()
            .all(|location| location.vertex_chunk == 0 && location.index_chunk == 0)
    );
    assert_eq!(
        locations
            .iter()
            .map(|location| location.base_vertex)
            .collect::<Vec<_>>(),
        [0, 3, 7]
    );
    assert_eq!(pool.stats().vertices.chunks, 1, "three meshes, one set of buffers");

    let pipeline = pipeline(device, &pool);
    let image = render(device, &pool, pipeline, &[left, middle, right]);
    if executes {
        assert_eq!(pixel(&image, 13, 37), RED, "left triangle");
        assert_eq!(pixel(&image, 32, 32), GREEN, "middle quad");
        assert_eq!(pixel(&image, 51, 37), BLUE, "right triangle");
        assert_eq!(pixel(&image, 32, 8), BLACK, "nothing drawn at the top yet");
        assert_eq!(pixel(&image, 2, 2), BLACK, "background");
    }

    pool.remove(middle, 1).expect("remove");
    pool.begin_frame(device, 3);
    let top = pool.upload(device, &quad_top.data("top")).expect("top");
    assert_eq!(
        pool.mesh(top).expect("live").location.base_vertex,
        3,
        "the new mesh reuses the removed mesh's vertex range"
    );

    let image = render(device, &pool, pipeline, &[left, top, right]);
    if executes {
        assert_eq!(pixel(&image, 13, 37), RED, "left triangle after reuse");
        assert_eq!(pixel(&image, 32, 8), YELLOW, "new quad in the reused range");
        assert_eq!(pixel(&image, 32, 32), BLACK, "the removed quad is gone");
        assert_eq!(pixel(&image, 51, 37), BLUE, "right triangle after reuse");
    }
}

#[test]
fn three_meshes_draw_from_one_buffer_on_null() {
    scene(&Device::null());
}

#[cfg(feature = "gpu-tests")]
#[test]
fn three_meshes_draw_from_one_buffer_on_wgpu() {
    let device = Device::build(&pathon_rhi::DeviceRequest {
        label: "mesh pool",
        ..Default::default()
    })
    .expect("a GPU adapter is required for gpu-tests");
    scene(&device);
}
