// M4-14: every base slot through `pathon-debug`. `SyntheticSlots` stands in for the producer and writes each slot with
// a known pattern; the debug pack shows them one by one (`slot_<name>.png`), all at once in a grid (`grid.png`) and
// with lines, a gizmo, a box and a wireframe on top (`overlay.png`).
// `cargo run -p pathon-examples --bin debug_slots -- [output_dir]`

use std::fs::{File, create_dir_all};
use std::io::BufWriter;
use std::path::Path;

use pathon::api::Camera;
use pathon::api::glam::{Mat4, Vec3};
use pathon::api::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, CommandList, DeviceApi, Extent3d, Format, PhysicalSize,
    SubresourceRange, TexelLayout, TextureCopy,
};
use pathon::{FrameInput, Renderer, ViewInput};
use pathon_debug::{Corner, DebugColor, DebugDraw, DebugPack, DebugSettings, LineStyle, PreviewSlot, SlotPreview};
use pathon_test::SyntheticSlots;

const SIZE: u32 = 256;

fn write_png(path: &Path, rgba: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("create the png"));
    let mut encoder = png::Encoder::new(file, SIZE, SIZE);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn render_to_png(renderer: &mut Renderer, input: &FrameInput, path: &Path) {
    let report = renderer.render_frame(input).expect("frame");
    for diagnostic in &report.diagnostics {
        log::warn!("{diagnostic}");
    }
    let device = renderer.device();
    let texture = renderer.target_texture(input.views[0].target).expect("target texture");
    let row_bytes = SIZE * 4;
    let readback = device
        .create_buffer(&BufferDesc {
            label: "debug slots readback",
            size: u64::from(row_bytes * SIZE),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback buffer");
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(texture),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: row_bytes,
                rows_per_image: SIZE,
            },
        },
        Extent3d::new_2d(SIZE, SIZE),
    );
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_SRC, Access::SAMPLED);
    device.submit(list).expect("submit");
    write_png(path, &device.read_buffer(readback).expect("read"));
    device.destroy(readback.into());
    log::info!("wrote {}", path.display());
}

fn main() {
    pathon_examples::init_logging();
    let output_dir = std::env::args().nth(1).unwrap_or_else(|| "debug_slots_out".to_owned());
    let output_dir = Path::new(&output_dir);
    create_dir_all(output_dir).expect("create the output directory");

    let mut renderer = Renderer::builder()
        .with_pack(SyntheticSlots::new())
        .with_pack(DebugPack::new())
        .build()
        .expect("a GPU renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba8Unorm)
        .expect("offscreen target");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    let show = |renderer: &mut Renderer, preview: Option<SlotPreview>| {
        renderer
            .shared_mut()
            .get_mut::<DebugSettings>()
            .expect("debug pack")
            .preview = preview;
    };

    for slot in PreviewSlot::ALL {
        show(&mut renderer, Some(SlotPreview::fullscreen(slot)));
        let name = format!("slot_{}.png", slot.slot().name());
        render_to_png(&mut renderer, &input, &output_dir.join(name));
    }

    show(&mut renderer, Some(SlotPreview::grid(PreviewSlot::ALL.to_vec())));
    render_to_png(&mut renderer, &input, &output_dir.join("grid.png"));

    show(
        &mut renderer,
        Some(SlotPreview::corner(PreviewSlot::Hdr, Corner::TopRight)),
    );
    let shared = renderer.shared_mut();
    let draw = shared.get_mut::<DebugDraw>().expect("debug pack");
    draw.aabb(
        Vec3::splat(-0.6),
        Vec3::splat(0.6),
        LineStyle::new(DebugColor::YELLOW).thickness(2.0),
    );
    draw.sphere(Vec3::ZERO, 0.9, LineStyle::new(DebugColor::CYAN).thickness(1.5));
    draw.axes(Mat4::IDENTITY, 1.2, 3.0);
    draw.frustum(
        &Camera::look_at(Vec3::new(2.0, 1.5, 1.0), Vec3::ZERO, 0.6, 1.0),
        1.5,
        DebugColor::MAGENTA,
    );
    let positions = [
        Vec3::new(-1.4, -1.4, -0.5),
        Vec3::new(-0.6, -1.4, -0.5),
        Vec3::new(-1.0, -0.8, -0.5),
    ];
    draw.wireframe(
        &positions,
        &[0, 1, 2],
        Mat4::IDENTITY,
        LineStyle::new(DebugColor::ORANGE).thickness(2.0),
    );
    render_to_png(&mut renderer, &input, &output_dir.join("overlay.png"));
}
