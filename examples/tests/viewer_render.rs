#![cfg(feature = "gpu-tests")]

// M4-10: the whole viewer path on a real GPU with a scene built in the test: loader types -> upload (pool, textures,
// material table, scene) -> the viewer pack (sun shadow, forward, tonemap) -> offscreen target -> PNG-ready pixels.

use pathon::api::glam::{Affine3A, Vec3};
use pathon_examples::gltf_loader::{CpuInstance, CpuMaterial, CpuMesh, LoadedScene, SurfaceVertex};
use pathon_examples::viewer::screenshot::{ScreenshotRequest, render_screenshot};
use pathon_golden::{GoldenImage, gpu_or_skip};
use pathon_material::StandardMaterial;
use pathon_scene::Light;

const SIZE: u32 = 64;

// A 2 x 2 m quad in the XY plane facing +Z, lit by a sun that shines along -Z.
fn red_quad_scene() -> LoadedScene {
    let corner = |x: f32, y: f32| {
        (
            [x, y, 0.0],
            SurfaceVertex {
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [x * 0.5 + 0.5, 0.5 - y * 0.5],
            },
        )
    };
    let corners = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    LoadedScene {
        meshes: vec![CpuMesh {
            name: "quad".into(),
            positions: corners.iter().map(|(position, _)| *position).collect(),
            surface: corners.iter().map(|(_, surface)| *surface).collect(),
            indices: vec![0, 1, 2, 0, 2, 3],
            material: 0,
        }],
        materials: vec![CpuMaterial {
            name: "red".into(),
            standard: StandardMaterial {
                base_color: [0.9, 0.02, 0.02, 1.0],
                metallic: 0.0,
                roughness: 0.7,
                ..StandardMaterial::default()
            },
            textures: Default::default(),
        }],
        instances: vec![CpuInstance {
            mesh: 0,
            transform: Affine3A::IDENTITY,
        }],
        lights: vec![Light::directional(Affine3A::IDENTITY, 600.0)],
        ..LoadedScene::default()
    }
}

fn render(sun_shadows: bool) -> Option<GoldenImage> {
    let request = ScreenshotRequest {
        width: SIZE,
        height: SIZE,
        eye_and_target: Some((Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO)),
        vertical_fov_degrees: Some(60.0),
        sun_shadows,
        ..ScreenshotRequest::default()
    };
    gpu_or_skip(
        "viewer_render",
        render_screenshot(&red_quad_scene(), &request),
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

#[test]
fn a_lit_quad_is_drawn_red_in_front_of_the_sky() {
    let Some(image) = render(false) else {
        return;
    };
    assert_red_quad_on_a_sky(&image);
}

#[test]
fn the_sun_shadow_pass_does_not_darken_a_quad_that_faces_the_sun() {
    let (Some(shadowed), Some(unshadowed)) = (render(true), render(false)) else {
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
