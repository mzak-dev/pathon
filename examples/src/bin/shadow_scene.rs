// M4-07: cascaded shadows of one directional light. A floor, a stack of boxes, a sphere, a plane facing the sun and a
// box far away are lit by N.L times `shadow_factor_directional`; `ShadowPack` renders four cascades into a depth array
// every frame. The scene and the shading are the ones of the golden test (crates/pathon-shadow/tests/support).
// `shadow_scene [file.png] [hard|pcf3|pcf5] [stable|focused] [cascades 1-4]` writes the picture and logs where the
// cascades split.

#[path = "../../../crates/pathon-shadow/tests/support/mod.rs"]
mod support;

use std::fs::File;
use std::io::BufWriter;

use pathon::api::Camera;
use pathon::api::glam::{Quat, Vec3};
use pathon::api::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, CommandList, DeviceApi, Extent3d, Format, PhysicalSize,
    SubresourceRange, TexelLayout, TextureCopy,
};
use pathon::{FrameInput, Renderer, ViewInput};
use pathon_shadow::{CascadeFit, ShadowFilter, ShadowFrame, ShadowPack, ShadowSettings};
use support::{Host, Kind, Object, Shading};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 384;

fn scene() -> Vec<Object> {
    let sun = Vec3::new(-0.5, -0.45, -0.6);
    vec![
        Object::new(Kind::Plane, Vec3::ZERO, Vec3::new(400.0, 1.0, 400.0)),
        Object::new(Kind::Cube, Vec3::new(0.0, 1.0, -6.0), Vec3::splat(2.0)),
        Object::new(Kind::Cube, Vec3::new(0.0, 2.5, -6.0), Vec3::splat(1.0)),
        Object::new(Kind::Sphere, Vec3::new(-5.0, 1.0, -8.0), Vec3::splat(2.0)),
        Object::new(Kind::Plane, Vec3::new(9.0, 3.0, -9.0), Vec3::new(3.0, 1.0, 3.0))
            .rotated(Quat::from_rotation_arc(Vec3::Y, -sun.normalize())),
        Object::new(Kind::Cube, Vec3::new(3.0, 2.0, -38.0), Vec3::splat(4.0)),
    ]
}

fn write_png(path: &str, rgba: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("create the png"));
    let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn main() {
    pathon_examples::init_logging();
    let mut arguments = std::env::args().skip(1);
    let output_path = arguments.next().unwrap_or_else(|| "shadow_scene.png".to_owned());
    let filter = match arguments.next().as_deref() {
        Some("hard") => ShadowFilter::Hard,
        Some("pcf5") => ShadowFilter::Pcf5,
        _ => ShadowFilter::Pcf3,
    };
    let fit = match arguments.next().as_deref() {
        Some("focused") => CascadeFit::Focused,
        _ => CascadeFit::Stable,
    };
    let cascade_count = arguments.next().and_then(|count| count.parse().ok()).unwrap_or(4);
    let settings = ShadowSettings {
        cascade_count,
        filter,
        fit,
        max_distance: 80.0,
        resolution: 1024,
        ..ShadowSettings::DESKTOP
    };

    let mut renderer = Renderer::builder()
        .with_feature(Host::new(scene()))
        .with_pack(ShadowPack::with_settings(settings))
        .with_feature(Shading::new())
        .build()
        .expect("a GPU renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(WIDTH, HEIGHT), Format::Rgba8Unorm)
        .expect("offscreen target");
    let camera = Camera::look_at(
        Vec3::new(-2.0, 6.0, 6.0),
        Vec3::new(0.0, 0.0, -20.0),
        60.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
    );
    let input = FrameInput {
        views: vec![ViewInput { target, camera }],
        ..FrameInput::default()
    };
    let report = renderer.render_frame(&input).expect("frame");
    for diagnostic in &report.diagnostics {
        log::warn!("{diagnostic}");
    }
    if let Some(frame) = renderer.shared().get::<ShadowFrame>() {
        for cascade in 0..frame.uniforms.cascade_count() as usize {
            log::info!(
                "cascade {cascade}: up to {:.1} m, {:.1} cm per texel",
                frame.uniforms.split(cascade),
                frame.uniforms.texel_world_size(cascade) * 100.0
            );
        }
    }

    let device = renderer.device();
    let texture = renderer.target_texture(target).expect("target texture");
    let row_bytes = WIDTH * 4;
    let readback = device
        .create_buffer(&BufferDesc {
            label: "shadow scene readback",
            size: u64::from(row_bytes * HEIGHT),
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
                rows_per_image: HEIGHT,
            },
        },
        Extent3d::new_2d(WIDTH, HEIGHT),
    );
    device.submit(list).expect("submit");
    write_png(&output_path, &device.read_buffer(readback).expect("read"));
    log::info!("wrote {output_path} ({filter:?}, {fit:?}, {cascade_count} cascades)");
}
