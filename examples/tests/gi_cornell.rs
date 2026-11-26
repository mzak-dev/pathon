#![cfg(feature = "gpu-tests")]

// K12: the Cornell box of `examples/src/cornell.rs` through the whole viewer stack with the probe volume, against a
// golden image, and the picture without it for what the probes add.

use pathon::api::glam::Vec3;
use pathon_examples::cornell::{ROOM, cornell_scene};
use pathon_examples::viewer::screenshot::{ScreenshotRequest, render_screenshot};
use pathon_examples::viewer::stack::ViewerConfig;
use pathon_gi::GiSettings;
use pathon_golden::{GoldenImage, Tolerance, assert_golden, gpu_or_skip};

const SIZE: u32 = 192;

fn render(gi: Option<GiSettings>) -> Option<GoldenImage> {
    let request = ScreenshotRequest {
        width: SIZE,
        height: SIZE,
        eye_and_target: Some((
            Vec3::new(0.0, ROOM * 0.5, ROOM * 1.45),
            Vec3::new(0.0, ROOM * 0.45, 0.0),
        )),
        vertical_fov_degrees: Some(45.0),
        bloom: false,
        environment_intensity: Some(0.0),
        sky_intensity: Some(0.0),
        ..ScreenshotRequest::default()
    };
    let config = ViewerConfig {
        gi,
        ..ViewerConfig::default()
    };
    gpu_or_skip(
        "gi_cornell",
        render_screenshot(&cornell_scene(), &config, &request),
        |message| message.contains("no GPU adapter"),
    )
}

fn mean_luma(image: &GoldenImage) -> f32 {
    let mut sum = 0.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = image.pixel(x, y);
            sum += 0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2]);
        }
    }
    sum / (SIZE * SIZE) as f32
}

#[test]
fn the_cornell_box_with_gi_matches_its_golden_image() {
    let Some(image) = render(Some(GiSettings::default())) else {
        return;
    };
    assert_golden("gi_cornell", &image, Tolerance::cross_backend());
}

#[test]
fn the_probes_light_what_the_lamp_does_not_reach() {
    let (Some(with), Some(without)) = (render(Some(GiSettings::default())), render(None)) else {
        return;
    };
    let (with, without) = (mean_luma(&with), mean_luma(&without));
    eprintln!("mean luma of the Cornell box: {without:.1} without GI, {with:.1} with");
    assert!(with > without * 1.3, "{with} vs {without}");
}
