// M4-01: three meshes of different sizes share one set of mega-buffers. The scene is drawn offscreen and printed as
// text; then one mesh is removed and a new one reuses its range.

use pathon::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList, Device,
    DeviceApi, DeviceRequest, Extent3d, Format, PrimitiveState, RenderPassDesc, RenderPipelineDesc, ShaderModuleDesc,
    ShaderStage, SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureUsage, TextureViewDesc,
};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};

const SHADER: &str = "
struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
}
@vertex fn vs(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> Varying {
    return Varying(vec4<f32>(position.xy, 0.0, 1.0), normal);
}
@fragment fn fs(in: Varying) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
";

const WIDTH: u32 = 64;
const HEIGHT: u32 = 16;
const ROW_BYTES: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Surface {
    normal: [f32; 3],
    tangent: [f32; 4],
    uv: [f32; 2],
}

// The normal doubles as the colour, so each mesh shows up as its own letter.
fn shape(corners: &[[f32; 2]], color: [f32; 3]) -> (Vec<[f32; 3]>, Vec<Surface>) {
    let positions = corners.iter().map(|corner| [corner[0], corner[1], 0.0]).collect();
    let surface = corners
        .iter()
        .map(|_| Surface {
            normal: color,
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0; 2],
        })
        .collect();
    (positions, surface)
}

fn upload(
    pool: &mut GeometryPool,
    device: &Device,
    label: &'static str,
    corners: &[[f32; 2]],
    color: [f32; 3],
    indices: &[u16],
) -> MeshId {
    let (positions, surface) = shape(corners, color);
    let data = MeshData::new(
        label,
        vec![bytemuck::cast_slice(&positions), bytemuck::cast_slice(&surface)],
        IndexData::U16(indices),
    );
    pool.upload(device, &data)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

fn draw_to_text(device: &Device, pool: &GeometryPool, meshes: &[MeshId]) -> String {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "mesh pool",
            wgsl: SHADER,
        })
        .expect("module");
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "mesh pool",
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
        .expect("pipeline");
    let target = device
        .create_texture(&TextureDesc::new_2d(
            "target",
            WIDTH,
            HEIGHT,
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
            size: u64::from(ROW_BYTES * HEIGHT),
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
            pool.draw(&mut pass, *mesh, 0..1)
                .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
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
                rows_per_image: HEIGHT,
            },
        },
        Extent3d::new_2d(WIDTH, HEIGHT),
    );
    device.submit(list).expect("submit");
    let pixels = device.read_buffer(readback).expect("read");

    let mut text = String::new();
    for row in pixels.chunks(ROW_BYTES as usize).take(HEIGHT as usize) {
        for texel in row.as_chunks::<4>().0.iter().take(WIDTH as usize) {
            text.push(match (texel[0] > 127, texel[1] > 127, texel[2] > 127) {
                (true, false, false) => 'R',
                (false, true, false) => 'G',
                (false, false, true) => 'B',
                (true, true, false) => 'Y',
                _ => '.',
            });
        }
        text.push('\n');
    }
    text
}

fn log_stats(pool: &GeometryPool) {
    let stats = pool.stats();
    log::info!(
        "{} meshes, {} vertex chunk(s), {} of {} vertices used, fragmentation {:.2}",
        stats.meshes,
        stats.vertices.chunks,
        stats.vertices.used,
        stats.vertices.capacity,
        stats.vertices.fragmentation()
    );
}

fn main() {
    pathon_examples::init_logging();
    let device = Device::build(&DeviceRequest {
        label: "mesh pool",
        ..Default::default()
    })
    .expect("a GPU adapter");
    let mut pool = GeometryPool::new(
        &device,
        "scene",
        VertexLayout::standard(),
        PoolDesc::default().with_chunk_sizes(64, 64).with_budget(64, 64),
    );

    let left = upload(
        &mut pool,
        &device,
        "left",
        &[[-0.9, -0.8], [-0.3, -0.8], [-0.6, 0.8]],
        [1.0, 0.0, 0.0],
        &[0, 1, 2],
    );
    let middle = upload(
        &mut pool,
        &device,
        "middle",
        &[[-0.2, -0.8], [0.2, -0.8], [0.2, 0.8], [-0.2, 0.8]],
        [0.0, 1.0, 0.0],
        &[0, 1, 2, 0, 2, 3],
    );
    let right = upload(
        &mut pool,
        &device,
        "right",
        &[[0.3, -0.8], [0.9, -0.8], [0.6, 0.8]],
        [0.0, 0.0, 1.0],
        &[0, 1, 2],
    );
    log_stats(&pool);
    log::info!(
        "three meshes from one buffer:\n{}",
        draw_to_text(&device, &pool, &[left, middle, right])
    );

    pool.remove(middle, 1).expect("remove");
    pool.begin_frame(&device, 3);
    let wide = upload(
        &mut pool,
        &device,
        "wide",
        &[[-0.2, -0.2], [0.2, -0.2], [0.2, 0.2], [-0.2, 0.2]],
        [1.0, 1.0, 0.0],
        &[0, 1, 2, 0, 2, 3],
    );
    log_stats(&pool);
    log::info!(
        "middle removed, a new mesh reuses its range:\n{}",
        draw_to_text(&device, &pool, &[left, wide, right])
    );
}
