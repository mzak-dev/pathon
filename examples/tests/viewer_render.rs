#![cfg(feature = "gpu-tests")]

// M4-10: the whole viewer path on a real GPU with a scene built in the test: loader types -> upload (pool, textures,
// material table, scene) -> the lighting (the pathon packs, or `--simple`) -> offscreen target -> PNG-ready pixels.

mod viewer_scene;

use pathon_examples::viewer::screenshot::{ScreenshotRequest, render_screenshot};
use pathon_examples::viewer::settings::LightingMode;
use pathon_examples::viewer::stack::ViewerConfig;
use pathon_golden::{GoldenImage, gpu_or_skip};
use viewer_scene::{SIZE, red_quad_scene};

// The synthetic sky is about 1 in these units; this makes it a mid-bright backdrop at the default exposure.
const TEST_SKY_INTENSITY: f32 = 150.0;

fn render(lighting: LightingMode, sun_shadows: bool) -> Option<GoldenImage> {
    let request = ScreenshotRequest {
        width: SIZE,
        height: SIZE,
        eye_and_target: Some((pathon::api::glam::Vec3::new(0.0, 0.0, 3.0), pathon::api::glam::Vec3::ZERO)),
        vertical_fov_degrees: Some(60.0),
        sun_shadows,
        sky_intensity: Some(TEST_SKY_INTENSITY),
        bloom: false,
        ..ScreenshotRequest::default()
    };
    let config = ViewerConfig {
        lighting,
        ..ViewerConfig::default()
    };
    gpu_or_skip(
        "viewer_render",
        render_screenshot(&red_quad_scene(), &config, &request),
        |message| message.contains("no GPU adapter"),
    )
}

fn assert_red_quad_on_a_sky(image: &GoldenImage) {
    let centre = image.pixel(SIZE / 2, SIZE / 2).map(u32::from);
    let corner = image.pixel(1, 1).map(u32::from);
    assert!(
        centre[0] > 100 && centre[0] > centre[1] * 2 && centre[0] > centre[2] * 2,
        "centre {centre:?} is not red"
    );
    assert!(corner[2] > corner[0], "corner {corner:?} is not the bluish sky");
}

fn a_lit_quad_is_red_in_front_of_the_sky(lighting: LightingMode) {
    let Some(image) = render(lighting, false) else {
        return;
    };
    assert_red_quad_on_a_sky(&image);
}

fn the_sun_shadow_does_not_darken_a_quad_that_faces_the_sun(lighting: LightingMode) {
    let (Some(shadowed), Some(unshadowed)) = (render(lighting, true), render(lighting, false)) else {
        return;
    };
    assert_red_quad_on_a_sky(&shadowed);
    let (with, without) = (shadowed.pixel(SIZE / 2, SIZE / 2), unshadowed.pixel(SIZE / 2, SIZE / 2));
    let difference = with
        .iter()
        .zip(without)
        .map(|(a, b)| i32::from(*a).abs_diff(i32::from(b)))
        .max();
    assert!(difference <= Some(4), "{with:?} against {without:?}");
}

#[test]
fn a_lit_quad_is_drawn_red_in_front_of_the_sky() {
    a_lit_quad_is_red_in_front_of_the_sky(LightingMode::Simple);
}

#[test]
fn the_sun_shadow_pass_does_not_darken_a_quad_that_faces_the_sun() {
    the_sun_shadow_does_not_darken_a_quad_that_faces_the_sun(LightingMode::Simple);
}

#[test]
fn the_pbr_stack_draws_a_lit_quad_red_in_front_of_the_environment() {
    a_lit_quad_is_red_in_front_of_the_sky(LightingMode::Pbr);
}

#[test]
fn the_pbr_stack_does_not_shadow_a_quad_that_faces_the_sun() {
    the_sun_shadow_does_not_darken_a_quad_that_faces_the_sun(LightingMode::Pbr);
}
