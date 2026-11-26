// The headless mode: one frame rendered offscreen from a chosen camera and written as a PNG. The reference images of
// docs/scenes/ and the golden image of the test scene (M4-05, M4-20) come from here.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use pathon::api::glam::Vec3;
use pathon::rhi::{Access, DeviceApi, Format, PhysicalSize};
use pathon_golden::{GoldenImage, read_texture};

use super::camera::FlyCamera;
use super::session::Session;
use super::settings::{ReferenceLighting, ViewerSettings};
use super::stack::ViewerConfig;
use crate::gltf_loader::LoadedScene;

pub(super) const SCREENSHOT_FORMAT: Format = Format::Rgba8UnormSrgb;

#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one command line switch; an enum per switch would only rename it"
)]
pub struct ScreenshotRequest {
    pub width: u32,
    pub height: u32,
    /// Renders at `supersample` times the size and averages down; 1 is off.
    pub supersample: u32,
    pub camera: Option<usize>,
    /// Picks the camera of the file by its node name instead of its index (`--camera` wins when both are given).
    pub camera_name: Option<String>,
    /// Overrides the camera: where the free camera stands and what it looks at.
    pub eye_and_target: Option<(Vec3, Vec3)>,
    pub vertical_fov_degrees: Option<f32>,
    pub exposure_stops: f32,
    pub sun_shadows: bool,
    /// Shadows of the point and spot lights; `area_shadows` adds the area lights (from their centre).
    pub local_shadows: bool,
    pub area_shadows: bool,
    pub local_shadow_lights: Option<u32>,
    pub redraw_local_shadows: bool,
    pub auto_exposure: bool,
    pub bloom: bool,
    /// Replace the defaults of `ViewerSettings` when set.
    pub environment_intensity: Option<f32>,
    pub sky_intensity: Option<f32>,
    pub brute_force_lights: bool,
    /// Lights the frame like the Cycles reference (docs/porownanie-obrazu.md), whatever the other switches say.
    pub reference_like: Option<ReferenceLighting>,
}

impl Default for ScreenshotRequest {
    fn default() -> Self {
        Self {
            width: 960,
            height: 540,
            supersample: 1,
            camera: None,
            camera_name: None,
            eye_and_target: None,
            vertical_fov_degrees: None,
            exposure_stops: 0.0,
            sun_shadows: true,
            local_shadows: true,
            area_shadows: false,
            local_shadow_lights: None,
            redraw_local_shadows: false,
            auto_exposure: false,
            bloom: true,
            environment_intensity: None,
            sky_intensity: None,
            brute_force_lights: false,
            reference_like: None,
        }
    }
}

impl ScreenshotRequest {
    /// The settings a session starts from, with this request's changes.
    pub fn settings(&self) -> ViewerSettings {
        let mut settings = self
            .reference_like
            .map_or_else(ViewerSettings::default, ViewerSettings::reference_like);
        settings.adjust_exposure(self.exposure_stops * 2.0);
        settings.sun_shadows = self.sun_shadows;
        settings.local_shadows = self.local_shadows;
        settings.area_shadows = self.area_shadows;
        settings.local_shadow_lights = self.local_shadow_lights.unwrap_or(settings.local_shadow_lights);
        settings.redraw_local_shadows = self.redraw_local_shadows;
        settings.auto_exposure |= self.auto_exposure;
        settings.bloom &= self.bloom;
        settings.brute_force_lights = self.brute_force_lights;
        settings.environment_intensity = self.environment_intensity.unwrap_or(settings.environment_intensity);
        settings.sky_intensity = self.sky_intensity.unwrap_or(settings.sky_intensity);
        settings
    }

    /// Puts the free camera of `session` where the request wants it.
    pub fn place_camera(&self, session: &mut Session) -> Result<(), String> {
        let by_name = self.camera_name.as_deref().map(|name| {
            session
                .assets
                .cameras
                .iter()
                .position(|entry| entry.name == name)
                .ok_or_else(|| format!("the file has no camera named {name}"))
        });
        if let Some(index) = self.camera.map(Ok).or(by_name).transpose()?
            && !session.use_scene_camera(index)
        {
            return Err(format!("the file has no camera {index}"));
        }
        if let Some((eye, target)) = self.eye_and_target {
            session.camera = FlyCamera::new(eye, target - eye, session.camera.vertical_fov, session.camera.speed);
        }
        if let Some(degrees) = self.vertical_fov_degrees {
            session.camera.vertical_fov = degrees.to_radians();
        }
        Ok(())
    }

    pub fn render_size(&self) -> PhysicalSize {
        let factor = self.supersample.max(1);
        PhysicalSize::new(self.width * factor, self.height * factor)
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
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
pub fn render_screenshot(
    scene: &LoadedScene,
    config: &ViewerConfig,
    request: &ScreenshotRequest,
) -> Result<GoldenImage, String> {
    let renderer = config.builder().build().map_err(|error| error.to_string())?;
    log::info!("adapter: {}", renderer.device().caps().adapter.name);
    let mut session = Session::new(renderer, scene, config).map_err(|error| error.to_string())?;
    request.place_camera(&mut session)?;
    session.settings = request.settings();

    let target = session
        .renderer
        .add_offscreen_target(request.render_size(), SCREENSHOT_FORMAT)
        .map_err(|error| error.to_string())?;
    let report = session
        .render_settled(target, request.aspect_ratio())
        .map_err(|error| error.to_string())?;
    for diagnostic in &report.diagnostics {
        log::warn!("{diagnostic}");
    }
    // The first frame of a target jumps straight to the automatic exposure, so one frame is the settled picture.
    let texture = session
        .renderer
        .target_texture(target)
        .ok_or("the offscreen target has no texture")?;
    let picture = read_texture(session.renderer.device().as_ref(), texture, Access::SAMPLED)
        .map_err(|error| error.to_string())?;
    log::info!("{}", session.describe());
    downsample(&picture, request.supersample.max(1))
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
