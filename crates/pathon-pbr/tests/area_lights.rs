// K05 (ADR-0048): rectangular area lights. The null half validates the permutation with the LTC table everywhere; the
// GPU half (`--features gpu-tests`) renders a floor under a rectangle and compares the colour pass with numeric
// integration of the same light on the CPU: the diffuse term exactly (it is analytic), the specular one within what
// the LTC fit allows.

#![cfg_attr(
    not(feature = "gpu-tests"),
    allow(unused_imports, dead_code, reason = "the GPU half is compiled only with the feature")
)]
#![allow(
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    reason = "the numbers behind the tolerances are worth seeing in the test log; the CPU model is f64, the scene f32"
)]

mod support;

use pathon_api::glam::{Affine3A, DVec3, Quat, Vec3};
use pathon_api::{Camera, Severity, Slot};
use pathon_material::{MaterialTable, StandardMaterial};
use pathon_pbr::PbrPack;
use pathon_pbr::area::{
    RectLight, diffuse_cosine_integral, diffuse_cosine_integral_numeric, ggx_specular_cosine, specular_ltc,
    specular_numeric,
};
use pathon_rhi::Format;
use pathon_scene::Light;
use support::probe::{Probe, ProbeReader};
use support::scene::{Harness, add_content, instance, null_harness, scale_translate};

const SIZE: u32 = 192;
const FOV_DEGREES: f32 = 50.0;
const FLOOR_HALF_SIZE: f32 = 6.0;
const EYE: Vec3 = Vec3::new(0.0, 1.2, 3.2);
const TARGET: Vec3 = Vec3::new(0.0, 0.0, 0.0);
const BASE_COLOR: [f32; 3] = [0.6, 0.5, 0.4];
const DIELECTRIC_F0: f64 = 0.04;

fn facing_down(position: Vec3) -> Affine3A {
    Affine3A::from_rotation_translation(Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::NEG_Y), position)
}

fn camera() -> Camera {
    Camera::look_at(EYE, TARGET, FOV_DEGREES.to_radians(), 1.0)
}

fn add_floor(harness: &mut Harness, roughness: f32) {
    let content = add_content(&mut harness.renderer, false);
    let material = harness
        .renderer
        .shared_mut()
        .get_mut::<MaterialTable>()
        .expect("the content made a table")
        .add(&StandardMaterial {
            base_color: [BASE_COLOR[0], BASE_COLOR[1], BASE_COLOR[2], 1.0],
            metallic: 0.0,
            roughness,
            ..StandardMaterial::default()
        })
        .expect("a valid material");
    instance(
        &mut harness.scene,
        content.ground,
        material,
        scale_translate(FLOOR_HALF_SIZE, Vec3::ZERO),
    );
}

fn add_lights(harness: &mut Harness, lights: &[Light]) {
    harness
        .scene
        .extract(|writer| {
            for light in lights {
                writer.spawn_light(light)?;
            }
            Ok::<_, pathon_scene::SceneError>(())
        })
        .expect("lights");
}

fn errors_of(diagnostics: &[pathon_api::Diagnostic]) -> Vec<&pathon_api::Diagnostic> {
    diagnostics.iter().filter(|d| d.severity == Severity::Error).collect()
}

#[test]
fn a_floor_under_an_area_light_validates_on_the_null_device() {
    let mut harness = null_harness(SIZE, Format::Rgba8Unorm, |builder| builder.with_pack(PbrPack::new()));
    add_floor(&mut harness, 0.4);
    add_lights(
        &mut harness,
        &[
            Light::area(facing_down(Vec3::new(0.0, 1.6, -0.5)), 900.0, 2.4, 0.8),
            Light::area(facing_down(Vec3::new(2.0, 1.0, 0.5)), 300.0, 0.5, 0.5).with_two_sided(),
            Light::point(Affine3A::from_translation(Vec3::new(-2.0, 1.0, 0.0)), 500.0),
        ],
    );

    let diagnostics = harness.render(camera());

    assert!(errors_of(&diagnostics).is_empty(), "{:?}", errors_of(&diagnostics));
}

#[test]
fn an_explicit_range_shorter_than_half_the_diagonal_is_reported_once() {
    let mut harness = null_harness(SIZE, Format::Rgba8Unorm, |builder| builder.with_pack(PbrPack::new()));
    add_floor(&mut harness, 0.4);
    add_lights(
        &mut harness,
        &[Light::area(facing_down(Vec3::new(0.0, 1.6, 0.0)), 900.0, 3.0, 4.0).with_range(2.0)],
    );

    let first = harness.render(camera());
    let second = harness.render(camera());

    let count = |diagnostics: &[pathon_api::Diagnostic]| diagnostics.iter().filter(|d| d.code == "E0001").count();
    assert_eq!(count(&first), 1);
    assert_eq!(count(&second), 0, "reported once");
}

// A rectangle of side `w` seen from ten times its size is a point source whose intensity is the Lambertian lobe:
// I(theta) = P / pi cos(theta) (ADR-0048). The analytic diffuse integral must converge to the point light.
#[test]
fn a_distant_rectangle_is_the_point_light_of_the_same_power() {
    let power = 800.0_f32;
    for (width, height) in [(0.5_f32, 0.5_f32), (1.0, 0.25), (2.0, 2.0)] {
        let size = f64::from(width.max(height));
        for (offset, tilt) in [(0.0, 0.0), (0.3, 0.2), (1.5, 0.5)] {
            let center = Vec3::new(0.0, (10.0 * size) as f32, 0.0);
            let rotation = Quat::from_rotation_x(tilt) * Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::NEG_Y);
            let light = Light::area(
                Affine3A::from_rotation_translation(rotation, center),
                power,
                width,
                height,
            );
            let rect = RectLight::from_light(&light).expect("an area light");
            let point = DVec3::new(offset * size, 0.0, 0.0);
            let normal = DVec3::Y;

            let integral = diffuse_cosine_integral(&rect, point, normal);

            let to_light = rect.center - point;
            let distance = to_light.length();
            let direction = to_light / distance;
            let emission_cosine = (-direction).dot(rect.normal);
            // Point source: intensity P/pi * cos(emission) over the squared distance, times the cosine at the receiver.
            let luminance = f64::from(light.luminance_nits().expect("an area light"));
            let point_irradiance =
                luminance * rect.area() * emission_cosine * normal.dot(direction) / (distance * distance);
            let area_irradiance = luminance * integral;

            let error = (area_irradiance / point_irradiance - 1.0).abs();
            assert!(
                error < 0.02,
                "{width}x{height} at offset {offset} tilt {tilt}: area {area_irradiance} vs point {point_irradiance} ({error})"
            );
        }
    }
}

// The same limit for the specular term. The lobe is narrow, so the point source is the BRDF at the centre direction;
// the light sits at the mirror direction and a few degrees off it (the tail of the lobe is not what the image shows).
#[test]
fn a_distant_rectangle_has_the_specular_of_the_point_light() {
    let normal = DVec3::Y;
    for roughness in [0.35, 0.6, 0.9] {
        for view_angle in [0.1_f64, 0.5, 0.9] {
            for off_mirror in [0.0_f64, 0.1] {
                let view = DVec3::new(view_angle.sin(), view_angle.cos(), 0.0);
                let light_angle = view_angle + off_mirror;
                let direction = DVec3::new(-light_angle.sin(), light_angle.cos(), 0.0);
                let center = direction * 12.0;
                let facing =
                    Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::new(-direction.x as f32, -direction.y as f32, 0.0));
                let light = Light::area(
                    Affine3A::from_rotation_translation(facing, Vec3::new(center.x as f32, center.y as f32, 0.0)),
                    800.0,
                    1.0,
                    1.0,
                );
                let rect = RectLight::from_light(&light).expect("an area light");
                let to_light = rect.center;
                let point_specular = ggx_specular_cosine(normal, view, direction, roughness, DIELECTRIC_F0, 1.0)
                    * rect.area()
                    * (-direction).dot(rect.normal)
                    / to_light.length_squared();

                let ltc = specular_ltc(&rect, DVec3::ZERO, normal, view, roughness, (DIELECTRIC_F0, 1.0));

                let error = (ltc / point_specular - 1.0).abs();
                eprintln!(
                    "roughness {roughness} view {view_angle} off mirror {off_mirror}: ltc {ltc:.6e} point {point_specular:.6e} error {error:.4}"
                );
                assert!(
                    error < 0.16,
                    "roughness {roughness} view {view_angle} off mirror {off_mirror}: {error}"
                );
            }
        }
    }
}

#[test]
fn the_diffuse_integral_agrees_with_quadrature_to_a_hundredth_of_a_percent() {
    let light = Light::area(facing_down(Vec3::new(0.3, 1.0, -0.2)), 500.0, 3.0, 0.4);
    let rect = RectLight::from_light(&light).expect("an area light");
    for (point, normal) in [
        (DVec3::ZERO, DVec3::Y),
        (DVec3::new(1.2, 0.0, 0.7), DVec3::new(0.2, 1.0, 0.1).normalize()),
        (DVec3::new(-2.5, 0.3, 1.0), DVec3::new(1.0, 0.5, 0.0).normalize()),
        (DVec3::new(0.0, 0.5, 0.0), DVec3::new(0.0, 0.0, 1.0)),
    ] {
        let analytic = diffuse_cosine_integral(&rect, point, normal);
        let numeric = diffuse_cosine_integral_numeric(&rect, point, normal, 700);

        assert!(
            (analytic - numeric).abs() <= 1.0e-4 * numeric.abs().max(1.0e-4),
            "{point} {normal}: {analytic} vs {numeric}"
        );
    }
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_golden::{Tolerance, assert_golden, read_texture};
    use pathon_pbr::{LightAssignment, PbrSettings};
    use pathon_post::{BloomSettings, ExposureMode, ExposureSettings, PostPack, PostSettings};
    use pathon_rhi::Access;
    use support::probe::half_to_f32;
    use support::scene::harness;

    use super::*;

    struct Images {
        hdr: Vec<[f32; 4]>,
        diffuse: Vec<[f32; 4]>,
        specular: Vec<[f32; 4]>,
    }

    fn halves(reader: &ProbeReader, device: &dyn pathon_rhi::DeviceApi) -> Vec<[f32; 4]> {
        reader
            .texels(device, 0)
            .chunks(8)
            .map(|texel| {
                [0, 1, 2, 3]
                    .map(|channel| half_to_f32(u16::from_le_bytes([texel[channel * 2], texel[channel * 2 + 1]])))
            })
            .collect()
    }

    fn render_floor(test: &str, roughness: f32, lights: &[Light]) -> Option<Images> {
        let mut readers = None;
        let mut harness = harness(test, SIZE, Format::Rgba8Unorm, |builder| {
            let size = (SIZE, SIZE);
            let (hdr, hdr_reader) = Probe::new(Slot::HDR, size, 1, 8);
            let (diffuse, diffuse_reader) = Probe::new(Slot::DIFFUSE, size, 1, 8);
            let (specular, specular_reader) = Probe::new(Slot::SPECULAR, size, 1, 8);
            readers = Some([hdr_reader, diffuse_reader, specular_reader]);
            builder
                .with_pack(PbrPack::new())
                .with_feature(hdr)
                .with_feature(diffuse)
                .with_feature(specular)
        })?;
        add_floor(&mut harness, roughness);
        add_lights(&mut harness, lights);
        let diagnostics = harness.render(camera());
        assert!(errors_of(&diagnostics).is_empty(), "{:?}", errors_of(&diagnostics));
        let device = std::sync::Arc::clone(harness.renderer.device());
        let [hdr, diffuse, specular] = readers?;
        Some(Images {
            hdr: halves(&hdr, &*device),
            diffuse: halves(&diffuse, &*device),
            specular: halves(&specular, &*device),
        })
    }

    // The world point, the unit vector to the camera and whether the pixel shows the floor.
    fn floor_point(x: u32, y: u32) -> Option<(DVec3, DVec3)> {
        let camera = camera();
        let tan_half = f64::from((FOV_DEGREES.to_radians() * 0.5).tan());
        let ndc_x = 2.0 * (f64::from(x) + 0.5) / f64::from(SIZE) - 1.0;
        let ndc_y = 1.0 - 2.0 * (f64::from(y) + 0.5) / f64::from(SIZE);
        let view_to_world = camera.world_to_view.inverse();
        let direction_view = Vec3::new((ndc_x * tan_half) as f32, (ndc_y * tan_half) as f32, -1.0);
        let direction = view_to_world.transform_vector3(direction_view).normalize();
        let eye = DVec3::new(f64::from(EYE.x), f64::from(EYE.y), f64::from(EYE.z));
        let direction = DVec3::new(f64::from(direction.x), f64::from(direction.y), f64::from(direction.z));
        if direction.y >= -1.0e-3 {
            return None;
        }
        let point = eye - direction * (eye.y / direction.y);
        (point.x.abs() < f64::from(FLOOR_HALF_SIZE) - 0.05 && point.z.abs() < f64::from(FLOOR_HALF_SIZE) - 0.05)
            .then(|| (point, (eye - point).normalize()))
    }

    struct Comparison {
        diffuse_worst: f64,
        specular_mean: f64,
        specular_worst: f64,
        // The same two against the CPU evaluation of the LTC table: the shader against its model.
        shader_vs_table_mean: f64,
        table_vs_numeric_mean: f64,
        checked: usize,
    }

    // The colour pass against the CPU. `roughness` is the material's; the floor is a dielectric with `BASE_COLOR`.
    fn compare(images: &Images, light: &Light, roughness: f32) -> Comparison {
        let rect = RectLight::from_light(light).expect("an area light");
        let luminance = f64::from(light.luminance_nits().expect("an area light"));
        let radius = f64::from(light.influence_radius().expect("an area light"));
        let roughness = f64::from(roughness).max(0.045);
        let mut comparison = Comparison {
            diffuse_worst: 0.0,
            specular_mean: 0.0,
            specular_worst: 0.0,
            shader_vs_table_mean: 0.0,
            table_vs_numeric_mean: 0.0,
            checked: 0,
        };
        let (mut specular_error, mut specular_total) = (0.0, 0.0);
        let (mut shader_error, mut table_error) = (0.0, 0.0);
        for y in (0..SIZE).step_by(8) {
            for x in (0..SIZE).step_by(8) {
                let Some((point, view)) = floor_point(x, y) else {
                    continue;
                };
                let index = (y * SIZE + x) as usize;
                let to_center = rect.center - point;
                let window = {
                    let factor = to_center.length_squared() / (radius * radius);
                    (1.0 - factor * factor).clamp(0.0, 1.0).powi(2)
                };
                let half = (to_center.normalize() + view).normalize();
                let fresnel = DIELECTRIC_F0 + (1.0 - DIELECTRIC_F0) * (1.0 - view.dot(half).clamp(0.0, 1.0)).powi(5);
                let irradiance = diffuse_cosine_integral_numeric(&rect, point, DVec3::Y, 400);
                for (channel, base) in BASE_COLOR.iter().enumerate() {
                    let expected =
                        luminance * window * f64::from(*base) * (1.0 - fresnel) / std::f64::consts::PI * irradiance;
                    let got = f64::from(images.diffuse[index][channel]);
                    if expected > 0.02 {
                        comparison.diffuse_worst = comparison.diffuse_worst.max((got / expected - 1.0).abs());
                    } else {
                        assert!(
                            (got - expected).abs() < 0.002,
                            "pixel ({x}, {y}) channel {channel}: diffuse {got} vs {expected}"
                        );
                    }
                }
                let expected_specular = luminance
                    * window
                    * specular_numeric(&rect, point, DVec3::Y, view, roughness, (DIELECTRIC_F0, 1.0), 500);
                let got_specular = f64::from(images.specular[index][1]);
                let table_specular =
                    luminance * window * specular_ltc(&rect, point, DVec3::Y, view, roughness, (DIELECTRIC_F0, 1.0));
                // The error counts against what the pixel shows, diffuse and specular together: a relative error of
                // a specular tail 100 times dimmer than the diffuse light is not visible.
                let shown = f64::from(images.diffuse[index][1]) + expected_specular;
                shader_error += (got_specular - table_specular).abs();
                table_error += (table_specular - expected_specular).abs();
                specular_error += (got_specular - expected_specular).abs();
                specular_total += shown;
                if shown > 0.02 {
                    comparison.specular_worst = comparison
                        .specular_worst
                        .max((got_specular - expected_specular).abs() / shown);
                }
                comparison.checked += 1;
            }
        }
        comparison.specular_mean = specular_error / specular_total.max(1.0e-9);
        comparison.shader_vs_table_mean = shader_error / specular_total.max(1.0e-9);
        comparison.table_vs_numeric_mean = table_error / specular_total.max(1.0e-9);
        comparison
    }

    #[test]
    fn the_floor_under_a_rectangle_matches_numeric_integration() {
        let test = "the_floor_under_a_rectangle_matches_numeric_integration";
        let tilt = Quat::from_rotation_x(0.5) * Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::NEG_Y);
        let lights = [
            Light::area(
                facing_down(Vec3::new(0.0, 1.6, -0.5)),
                3.0 * std::f32::consts::PI * 1.92,
                2.4,
                0.8,
            ),
            Light::area(
                Affine3A::from_rotation_translation(tilt, Vec3::new(-0.5, 1.0, -1.5)),
                3.0 * std::f32::consts::PI * 4.5,
                6.0,
                0.75,
            ),
        ];
        for (light, roughness) in lights
            .iter()
            .flat_map(|light| [0.9, 0.5, 0.25].map(|roughness| (light, roughness)))
        {
            let Some(images) = render_floor(test, roughness, std::slice::from_ref(light)) else {
                return;
            };
            assert!(
                images
                    .hdr
                    .iter()
                    .all(|texel| texel.iter().all(|value| value.is_finite())),
                "NaN or infinity in hdr"
            );

            let comparison = compare(&images, light, roughness);

            eprintln!(
                "roughness {roughness}: {} pixels, diffuse worst {:.4}, specular mean {:.4} worst {:.4}, shader vs table {:.4}, table vs numeric {:.4}",
                comparison.checked,
                comparison.diffuse_worst,
                comparison.specular_mean,
                comparison.specular_worst,
                comparison.shader_vs_table_mean,
                comparison.table_vs_numeric_mean
            );
            assert!(comparison.checked > 250, "the floor fills the lower image");
            assert!(
                comparison.diffuse_worst < 0.01,
                "diffuse is analytic: {}",
                comparison.diffuse_worst
            );
            assert!(
                comparison.specular_mean < SPECULAR_MEAN_TOLERANCE,
                "specular {}",
                comparison.specular_mean
            );
            assert!(
                comparison.specular_worst < SPECULAR_WORST_TOLERANCE,
                "specular {}",
                comparison.specular_worst
            );
        }
    }

    // What the LTC fit costs (ADR-0048), as a share of what the pixel shows (diffuse + specular). Measured on the
    // cases above: mean 1.4 to 7.4%, worst 5 to 53% (the worst is a narrow highlight at a view 80 degrees off the
    // normal, where the fitted lobe is 10 to 15% off in total variation). Diffuse has no such error.
    const SPECULAR_MEAN_TOLERANCE: f64 = 0.10;
    const SPECULAR_WORST_TOLERANCE: f64 = 0.60;

    #[test]
    fn a_light_behind_the_floor_or_facing_away_adds_nothing_and_two_sides_keep_the_power() {
        let test = "a_light_behind_the_floor_or_facing_away_adds_nothing_and_two_sides_keep_the_power";
        let power = 600.0;
        let above = facing_down(Vec3::new(0.0, 1.5, -0.5));
        let facing_away = Affine3A::from_rotation_translation(
            Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::Y),
            Vec3::new(0.0, 1.5, -0.5),
        );
        let below = facing_down(Vec3::new(0.0, -1.0, -0.5));
        let (Some(one_sided), Some(away), Some(under), Some(two_sided)) = (
            render_floor(test, 0.5, &[Light::area(above, power, 2.0, 1.0)]),
            render_floor(test, 0.5, &[Light::area(facing_away, power, 2.0, 1.0)]),
            render_floor(test, 0.5, &[Light::area(below, power, 2.0, 1.0).with_two_sided()]),
            render_floor(test, 0.5, &[Light::area(above, 2.0 * power, 2.0, 1.0).with_two_sided()]),
        ) else {
            return;
        };

        let brightest = |images: &Images| images.hdr.iter().map(|texel| texel[1]).fold(0.0_f32, f32::max);
        assert!(
            brightest(&one_sided) > 0.05,
            "the lit floor is visible: {}",
            brightest(&one_sided)
        );
        assert_eq!(brightest(&away), 0.0, "a light facing away lights nothing");
        assert_eq!(brightest(&under), 0.0, "a light under the floor lights nothing");
        let worst = one_sided
            .hdr
            .iter()
            .zip(&two_sided.hdr)
            .map(|(a, b)| (a[1] - b[1]).abs() / a[1].abs().max(0.02))
            .fold(0.0_f32, f32::max);
        assert!(
            worst < 0.01,
            "two sides with twice the power light the floor like one side: {worst}"
        );
    }

    #[test]
    fn rectangles_through_the_surface_and_grazing_it_leave_no_nan() {
        let test = "rectangles_through_the_surface_and_grazing_it_leave_no_nan";
        let vertical =
            |position: Vec3, yaw: f32| Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), position);
        let lights = [
            // Standing on the floor and cutting it, facing +Z.
            Light::area(vertical(Vec3::new(0.0, 0.0, -0.5), 0.0), 400.0, 2.0, 1.0),
            // Lying in the floor plane.
            Light::area(
                Affine3A::from_rotation_translation(
                    Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::Y),
                    Vec3::new(1.5, 0.0, 0.5),
                ),
                400.0,
                1.0,
                1.0,
            )
            .with_two_sided(),
            // A hair above the floor, facing down, and very small.
            Light::area(facing_down(Vec3::new(-1.5, 0.001, 0.5)), 400.0, 0.01, 0.01),
            // Huge and close.
            Light::area(facing_down(Vec3::new(0.0, 0.3, 0.0)), 4000.0, 50.0, 50.0),
        ];
        for roughness in [0.0, 0.3, 1.0] {
            let Some(images) = render_floor(test, roughness, &lights) else {
                return;
            };
            for (name, image) in [
                ("hdr", &images.hdr),
                ("diffuse", &images.diffuse),
                ("specular", &images.specular),
            ] {
                assert!(
                    image
                        .iter()
                        .all(|texel| texel.iter().all(|value| value.is_finite() && *value >= 0.0)),
                    "roughness {roughness}: {name} has NaN, infinity or negative values"
                );
            }
        }
    }

    fn post() -> PostPack {
        PostPack::with_settings(PostSettings {
            exposure: ExposureSettings {
                mode: ExposureMode::Manual { ev: 0.0 },
                compensation_ev: 0.0,
            },
            bloom: BloomSettings {
                enabled: false,
                ..BloomSettings::default()
            },
            ..PostSettings::default()
        })
    }

    // A floor, a metal and a dielectric sphere under a rectangle that is bright enough to show its reflection.
    #[test]
    fn the_area_light_scene_matches_its_golden_image() {
        let test = "the_area_light_scene_matches_its_golden_image";
        let Some(mut harness) = harness(test, 256, Format::Rgba8Unorm, |builder| {
            builder.with_pack(PbrPack::new()).with_pack(post())
        }) else {
            return;
        };
        let content = add_content(&mut harness.renderer, false);
        instance(
            &mut harness.scene,
            content.ground,
            content.floor_material,
            scale_translate(5.0, Vec3::new(0.0, -1.0, 0.0)),
        );
        instance(
            &mut harness.scene,
            content.ball,
            content.metal_material,
            scale_translate(0.9, Vec3::new(-1.3, -0.1, 0.0)),
        );
        instance(
            &mut harness.scene,
            content.ball,
            content.plastic_material,
            scale_translate(0.8, Vec3::new(1.3, -0.2, 0.2)),
        );
        add_lights(
            &mut harness,
            &[Light::area(
                facing_down(Vec3::new(0.0, 2.2, -0.3)),
                5.0 * std::f32::consts::PI * 4.0,
                4.0,
                1.0,
            )],
        );

        let diagnostics = harness.render(Camera::look_at(
            Vec3::new(0.0, 1.6, 6.0),
            Vec3::new(0.0, -0.4, 0.0),
            45.0_f32.to_radians(),
            1.0,
        ));

        assert!(errors_of(&diagnostics).is_empty(), "{:?}", errors_of(&diagnostics));
        let device = std::sync::Arc::clone(harness.renderer.device());
        let texture = harness
            .renderer
            .target_texture(harness.target)
            .expect("offscreen texture");
        let image = read_texture(&*device, texture, Access::SAMPLED).expect("readback");
        assert_golden("pbr_area_light", &image, Tolerance::cross_backend());
    }

    fn many_area_lights(harness: &mut Harness) {
        let mut random = support::Lcg(8);
        let lights: Vec<Light> = (0..100)
            .map(|_| {
                let position = Vec3::new(
                    random.between(-5.0, 5.0),
                    random.between(0.4, 2.0),
                    random.between(-4.0, 4.0),
                );
                let rotation = Quat::from_euler(
                    pathon_api::glam::EulerRot::XYZ,
                    random.between(0.0, std::f32::consts::TAU),
                    random.between(0.0, std::f32::consts::TAU),
                    random.between(0.0, std::f32::consts::TAU),
                );
                Light::area(
                    Affine3A::from_rotation_translation(rotation, position),
                    random.between(20.0, 120.0),
                    random.between(0.3, 2.5),
                    random.between(0.1, 1.5),
                )
                .with_range(random.between(1.0, 3.0))
                .with_color(Vec3::new(
                    random.between(0.3, 1.0),
                    random.between(0.3, 1.0),
                    random.between(0.3, 1.0),
                ))
            })
            .collect();
        add_lights(harness, &lights);
    }

    #[test]
    fn a_hundred_clustered_area_lights_equal_testing_every_light() {
        let test = "a_hundred_clustered_area_lights_equal_testing_every_light";
        let mut readers = None;
        let Some(mut harness) = harness(test, SIZE, Format::Rgba8Unorm, |builder| {
            let (hdr, reader) = Probe::new(Slot::HDR, (SIZE, SIZE), 1, 8);
            readers = Some(reader);
            builder.with_pack(PbrPack::new()).with_feature(hdr)
        }) else {
            return;
        };
        let Some(reader) = readers else { return };
        add_floor(&mut harness, 0.5);
        many_area_lights(&mut harness);
        let device = std::sync::Arc::clone(harness.renderer.device());

        assert_eq!(errors_of(&harness.render(camera())).len(), 0);
        let clustered = halves(&reader, &*device);
        harness.renderer.shared_mut().insert(PbrSettings {
            light_assignment: LightAssignment::BruteForce,
            ..PbrSettings::default()
        });
        assert_eq!(errors_of(&harness.render(camera())).len(), 0);
        let brute_force = halves(&reader, &*device);

        let differing = clustered
            .iter()
            .zip(&brute_force)
            .filter(|(a, b)| {
                (0..3).any(|channel| (a[channel] - b[channel]).abs() > 1.0e-3 * a[channel].abs().max(0.05))
            })
            .count();
        assert_eq!(
            differing, 0,
            "{differing} pixels differ between clustered and brute-force lights"
        );
        assert!(
            clustered.iter().filter(|texel| texel[0] > 0.05).count() > 1000,
            "the lights light the floor"
        );
    }
}
