// M4-12: an arm of two bones, skinned in compute by `SkinPack`. Four frames bend the elbow; the log shows the wrist's
// object-space motion vector (current pose minus the previous frame's, read from the other bank of the deformed
// buffer), which is what TAA reprojects with. A last frame poses three arms at once and draws them from the deformed
// buffers (normals as colour) into a PNG: `cargo run -p pathon-examples --bin skin_arm -- wynik.png`.

use std::fs::File;
use std::io::BufWriter;

use pathon::api::Camera;
use pathon::api::rhi::{
    Access, BufferCopy, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList,
    DepthAttachment, DepthState, DeviceApi, DeviceRequest, Extent3d, Format, IndexFormat, PhysicalSize, PrimitiveState,
    RenderPassDesc, RenderPipelineDesc, ShaderModuleDesc, ShaderStage, SubresourceRange, TexelLayout, TextureCopy,
    TextureDesc, TextureUsage, TextureViewDesc, Viewport,
};
use pathon::{FrameInput, Renderer, ViewInput};
use pathon_mesh::{DeformedPositions, GeometryPool, PoolDesc, VertexLayout};
use pathon_skin::reference::motion;
use pathon_skin::{
    DeformedSurface, SampleArm, SkinFrame, SkinInstance, SkinPack, SkinSource, SkinTargets, deformed_vertex_buffers,
};

const RINGS: u32 = 16;
const SIDES: u32 = 8;
const CAPACITY: u32 = 1024;
const TILE: u32 = 192;
const ROW_ALIGNMENT: u32 = 256;

const DRAW_SHADER: &str = "
struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
}
@vertex fn vs(@location(0) position: vec4<f32>, @location(1) normal: vec3<f32>) -> Varying {
    // Orthographic, looking down -Z: x in -0.3..2.3, y in -1.3..1.3, reverse-Z depth.
    return Varying(vec4<f32>((position.x - 1.0) / 1.3, position.y / 1.3, 0.5 + position.z, 1.0), normal);
}
@fragment fn fs(in: Varying) -> @location(0) vec4<f32> {
    return vec4<f32>(normalize(in.normal) * 0.5 + 0.5, 1.0);
}
";

fn read_vec4(device: &dyn DeviceApi, buffer: BufferId, offset: u64) -> [f32; 4] {
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: 16,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let mut list = CommandList::new();
    list.buffer_access(buffer, Access::COPY_SRC);
    list.copy_buffer_to_buffer(buffer, offset, readback, 0, 16);
    list.buffer_access(buffer, Access::VERTEX | Access::STORAGE_READ);
    device.submit(list).expect("submit");
    let bytes = device.read_buffer(readback).expect("read");
    bytemuck::pod_read_unaligned(&bytes[..16])
}

fn write_png(path: &str, width: u32, rgba: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("output file"));
    let mut encoder = png::Encoder::new(file, width, TILE);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn main() {
    pathon_examples::init_logging();
    let output_path = std::env::args().nth(1).unwrap_or_else(|| "skin_arm.png".to_owned());
    let mut renderer = Renderer::builder()
        .with_device_request(DeviceRequest {
            label: "skin arm",
            ..DeviceRequest::default()
        })
        .with_pack(SkinPack::new())
        .build()
        .expect("a renderer on a GPU adapter");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(16, 16), Format::Rgba8Unorm)
        .expect("target");
    let device = std::sync::Arc::clone(renderer.device());
    let device: &dyn DeviceApi = &*device;

    let arm = SampleArm::new(RINGS, SIDES);
    let mut pool = GeometryPool::new(device, "arm", VertexLayout::skinned(), PoolDesc::default());
    let mesh = pool.upload(device, &arm.mesh_data("arm")).expect("arm mesh");
    let source = SkinSource::from_pool(&pool, mesh).expect("skin source");
    let mut positions = DeformedPositions::new(device, "deformed positions", CAPACITY, 2).expect("positions");
    let surface = DeformedSurface::new(device, "deformed surface", CAPACITY).expect("surface");
    let targets = SkinTargets::new(&positions, &surface).expect("targets");
    let ranges: Vec<_> = (0..3)
        .map(|_| positions.allocate(arm.vertex_count()).expect("range"))
        .collect();
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    let wrist = (arm.vertex_count() - SIDES) as usize;

    for (step, angle) in [0.0_f32, 0.3, 0.6, 0.9].into_iter().enumerate() {
        let mut frame = SkinFrame::new(targets);
        frame.bone_matrices.extend(SampleArm::pose(angle));
        let mut instance = SkinInstance::new(source, ranges[0]).with_bones(0, 2);
        if step > 0 {
            instance = instance.continuing();
        }
        frame.instances.push(instance);
        renderer.shared_mut().insert(frame);
        renderer.render_frame(&input).expect("frame");
        let frame_number = renderer
            .shared()
            .get::<pathon_skin::SkinOutput>()
            .expect("skinned")
            .frame;
        let at = |previous| {
            let offset = if previous {
                positions.previous_offset(ranges[0], frame_number)
            } else {
                positions.current_offset(ranges[0], frame_number)
            } + wrist as u64 * pathon_mesh::DEFORMED_POSITION_STRIDE;
            let [x, y, z, _] = read_vec4(device, positions.buffer(), offset);
            [x, y, z]
        };
        let (current, previous) = (at(false), at(true));
        log::info!(
            "frame {frame_number}: elbow {angle:.1} rad, wrist {current:.3?}, motion {:.3?}",
            motion(current, previous)
        );
    }

    // Three arms in one frame, each bent differently, all new (no history).
    let mut frame = SkinFrame::new(targets);
    for (index, angle) in [0.0_f32, 0.7, 1.4].into_iter().enumerate() {
        let first = u32::try_from(frame.bone_matrices.len()).expect("few bones");
        frame.bone_matrices.extend(SampleArm::pose(angle));
        frame
            .instances
            .push(SkinInstance::new(source, ranges[index]).with_bones(first, 2));
    }
    renderer.shared_mut().insert(frame);
    renderer.render_frame(&input).expect("frame");
    let frame_number = renderer
        .shared()
        .get::<pathon_skin::SkinOutput>()
        .expect("skinned")
        .frame;

    let width = TILE * 3;
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "skin arm",
            wgsl: DRAW_SHADER,
        })
        .expect("module");
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "skin arm",
            bind_group_layouts: vec![],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: deformed_vertex_buffers(),
            fragment: Some(ShaderStage {
                module,
                entry_point: "fs",
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState::default(),
            depth: Some(DepthState::reverse_z(Format::Depth32Float)),
            sample_count: 1,
            immediate_size: 0,
        })
        .expect("pipeline");
    let color = device
        .create_texture(&TextureDesc::new_2d(
            "skin arm color",
            width,
            TILE,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
        ))
        .expect("color");
    let depth = device
        .create_texture(&TextureDesc::new_2d(
            "skin arm depth",
            width,
            TILE,
            Format::Depth32Float,
            TextureUsage::RENDER_TARGET,
        ))
        .expect("depth");
    let color_view = device
        .create_texture_view(color, &TextureViewDesc::default())
        .expect("view");
    let depth_view = device
        .create_texture_view(depth, &TextureViewDesc::default())
        .expect("view");
    let row_bytes = (width * 4).next_multiple_of(ROW_ALIGNMENT);
    let readback = device
        .create_buffer(&BufferDesc {
            label: "skin arm readback",
            size: u64::from(row_bytes * TILE),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let location = pool.mesh(mesh).expect("arm").location;
    let index_buffer = pool.index_buffer(location.index_chunk).expect("indices");

    let mut list = CommandList::new();
    list.buffer_access(positions.buffer(), Access::VERTEX);
    list.buffer_access(surface.buffer(), Access::VERTEX);
    list.texture_access(color, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    list.texture_access(depth, SubresourceRange::ALL, Access::empty(), Access::DEPTH_WRITE);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "skin arm",
            color: vec![ColorAttachment::clear(color_view, Color::BLACK)],
            depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_index_buffer(index_buffer, IndexFormat::Uint32, 0);
        for (tile, range) in ranges.iter().enumerate() {
            pass.set_viewport(Viewport {
                x: (u32::try_from(tile).expect("three tiles") * TILE) as f32,
                y: 0.0,
                width: TILE as f32,
                height: TILE as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            });
            pass.set_vertex_buffer(0, positions.buffer(), positions.current_offset(*range, frame_number));
            pass.set_vertex_buffer(1, surface.buffer(), DeformedSurface::offset(range.first_vertex));
            pass.draw_indexed(location.indices(), 0, 0..1);
        }
    }
    list.texture_access(color, SubresourceRange::ALL, Access::COLOR_ATTACHMENT, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(color),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: row_bytes,
                rows_per_image: TILE,
            },
        },
        Extent3d::new_2d(width, TILE),
    );
    device.submit(list).expect("submit");
    let padded = device.read_buffer(readback).expect("read");
    let pixels: Vec<u8> = padded
        .chunks(row_bytes as usize)
        .take(TILE as usize)
        .flat_map(|row| row[..(width * 4) as usize].iter().copied())
        .collect();
    write_png(&output_path, width, &pixels);
    log::info!("wrote {output_path}");
}
