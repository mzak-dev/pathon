// The headless mode: one frame rendered offscreen from a chosen camera and written as a PNG. The reference images of
// docs/scenes/ and the golden image of the test scene (M4-05, M4-20) come from here.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use pathon::api::glam::Vec3;
use pathon::rhi::{Access, DeviceApi, Format, PhysicalSize};
use pathon_golden::{GoldenImage, read_texture};

use super::camera::FlyCamera;
use super::session::{Session, viewer_builder};
use crate::gltf_loader::LoadedScene;

const SCREENSHOT_FORMAT: Format = Format::Rgba8UnormSrgb;

#[derive(Clone, Debug)]
pub struct ScreenshotRequest {
    pub width: u32,
    pub height: u32,
    /// Renders at `supersample` times the size and averages down; 1 is off.
    pub supersample: u32,
    pub camera: Option<usize>,
    /// Overrides the camera: where the free camera stands and what it looks at.
    pub eye_and_target: Option<(Vec3, Vec3)>,
    pub vertical_fov_degrees: Option<f32>,
    pub exposure_stops: f32,
    pub sun_shadows: bool,
}

impl Default for ScreenshotRequest {
    fn default() -> Self {
        Self {
            width: 960,
            height: 540,
            supersample: 1,
            camera: None,
            eye_and_target: None,
            vertical_fov_degrees: None,
            exposure_stops: 0.0,
            sun_shadows: true,
        }
    }
}

fn srgb_to_linear(value: u8) -> f32 {
    let encoded = f32::from(value) / 255.0;
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 before the cast"
    )]
    {
        (encoded * 255.0 + 0.5).clamp(0.0, 255.0) as u8
    }
}

// Averages `factor` x `factor` blocks in linear light, so anti-aliased edges keep their weight.
fn downsample(image: &GoldenImage, factor: u32) -> Result<GoldenImage, String> {
    if factor <= 1 {
        return Ok(image.clone());
    }
    let (width, height) = (image.width() / factor, image.height() / factor);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0.0_f32; 3];
            for dy in 0..factor {
                for dx in 0..factor {
                    let pixel = image.pixel(x * factor + dx, y * factor + dy);
                    for channel in 0..3 {
                        sum[channel] += srgb_to_linear(pixel[channel]);
                    }
                }
            }
            let samples = (factor * factor) as f32;
            rgba.extend(sum.map(|channel| linear_to_srgb(channel / samples)));
            rgba.push(u8::MAX);
        }
    }
    GoldenImage::from_rgba8(width, height, rgba).map_err(|error| error.to_string())
}

pub fn write_png(path: &Path, image: &GoldenImage) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(image.as_rgba8())
        .and_then(|()| writer.finish())
        .map_err(|error| error.to_string())
}

/// Renders `scene` once and returns the picture (already averaged down when supersampling).
pub fn render_screenshot(scene: &LoadedScene, request: &ScreenshotRequest) -> Result<GoldenImage, String> {
    let renderer = viewer_builder().build().map_err(|error| error.to_string())?;
    log::info!("adapter: {}", renderer.device().caps().adapter.name);
    let mut session = Session::new(renderer, scene).map_err(|error| error.to_string())?;
    if let Some(index) = request.camera
        && !session.use_scene_camera(index)
    {
        return Err(format!("the file has no camera {index}"));
    }
    if let Some((eye, target)) = request.eye_and_target {
        session.camera = FlyCamera::new(eye, target - eye, session.camera.vertical_fov, session.camera.speed);
    }
    if let Some(degrees) = request.vertical_fov_degrees {
        session.camera.vertical_fov = degrees.to_radians();
    }
    session.settings.adjust_exposure(request.exposure_stops * 2.0);
    session.settings.sun_shadows = request.sun_shadows;

    let factor = request.supersample.max(1);
    let size = PhysicalSize::new(request.width * factor, request.height * factor);
    let target = session
        .renderer
        .add_offscreen_target(size, SCREENSHOT_FORMAT)
        .map_err(|error| error.to_string())?;
    let aspect = request.width as f32 / request.height.max(1) as f32;
    let report = session.render(target, aspect).map_err(|error| error.to_string())?;
    for diagnostic in &report.diagnostics {
        log::warn!("{diagnostic}");
    }
    let texture = session
        .renderer
        .target_texture(target)
        .ok_or("the offscreen target has no texture")?;
    let picture = read_texture(session.renderer.device().as_ref(), texture, Access::SAMPLED)
        .map_err(|error| error.to_string())?;
    log::info!("{}", session.describe());
    downsample(&picture, factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averaging_black_and_white_in_linear_light_gives_the_srgb_of_one_half() {
        let image = GoldenImage::from_rgba8(
            2,
            2,
            vec![0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255],
        )
        .expect("image");
        let averaged = downsample(&image, 2).expect("2x2 to 1x1");
        assert_eq!(averaged.pixel(0, 0)[0], 188);
    }
}
