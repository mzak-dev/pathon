// M4-08: the physical sky (Hillaire 2020) drawn into `hdr` and tonemapped by `PostPack`. The sun is given as elevation
// and azimuth, the cloud cover as 0..1; auto-exposure adapts over 90 frames of 1/60 s. With `IblPack` next to it the
// sky is also baked into an environment; the log shows the sun light the sky publishes and the environment builds.
// `sky [elevation_degrees] [azimuth_degrees] [cloud_coverage] [file.png]`, for example `sky 3 0 0 sunset.png`.

use std::fs::File;
use std::io::BufWriter;

use pathon::api::Camera;
use pathon::api::glam::Vec3;
use pathon::api::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, CommandList, DeviceApi, Extent3d, Format, PhysicalSize,
    SubresourceRange, TexelLayout, TextureCopy,
};
use pathon::{FrameInput, Renderer, ViewInput};
use pathon_ibl::{IblEnvironment, IblPack};
use pathon_post::PostPack;
use pathon_sky::{SkyPack, SkySettings, SunLight};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 320;
const FRAMES: u32 = 90;

fn write_png(path: &str, rgba: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("create the png"));
    let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn number(argument: Option<String>, default: f32) -> f32 {
    argument.and_then(|text| text.parse().ok()).unwrap_or(default)
}

fn main() {
    pathon_examples::init_logging();
    let mut arguments = std::env::args().skip(1);
    let elevation = number(arguments.next(), 25.0);
    let azimuth = number(arguments.next(), 0.0);
    let cloud_coverage = number(arguments.next(), 0.0);
    let output_path = arguments.next().unwrap_or_else(|| "sky.png".to_owned());

    let settings = SkySettings {
        cloud_coverage,
        ..SkySettings::default().with_sun(elevation, azimuth)
    };
    let mut renderer = Renderer::builder()
        .with_pack(IblPack::new())
        .with_pack(SkyPack::new().with_settings(settings))
        .with_pack(PostPack::new())
        .build()
        .expect("a GPU renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(WIDTH, HEIGHT), Format::Rgba8Unorm)
        .expect("offscreen target");

    // Looks at the horizon, a little above it, so both the sky and the ground are in the picture.
    let position = Vec3::new(0.0, 1.7, 0.0);
    let camera = Camera::look_at(
        position,
        position + Vec3::new(azimuth.to_radians().sin(), 0.22, -azimuth.to_radians().cos()),
        70.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
    );
    let input = FrameInput {
        views: vec![ViewInput { target, camera }],
        delta_seconds: 1.0 / 60.0,
        ..FrameInput::default()
    };
    for _ in 0..FRAMES {
        let report = renderer.render_frame(&input).expect("frame");
        for diagnostic in &report.diagnostics {
            log::warn!("{diagnostic}");
        }
    }
    if let Some(sun) = renderer.shared().get::<SunLight>() {
        log::info!(
            "sun light: {:.0} lux ({:?} per channel), towards {}",
            sun.luminous_illuminance(),
            sun.illuminance_lux,
            sun.direction_to_sun
        );
    }
    if let Some(environment) = renderer.shared().get::<IblEnvironment>() {
        log::info!(
            "environment for image-based lighting: generation {}, {} roughness levels",
            environment.generation,
            environment.prefiltered_levels
        );
    }

    let device = renderer.device();
    let texture = renderer.target_texture(target).expect("target texture");
    let row_bytes = WIDTH * 4;
    let readback = device
        .create_buffer(&BufferDesc {
            label: "sky readback",
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
    log::info!("wrote {output_path} (sun at {elevation} degrees, clouds {cloud_coverage})");
}
