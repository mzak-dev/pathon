#![cfg(feature = "gpu-tests")]
// The sky on a real device: the tables against the CPU model, the picture in several lights against golden images,
// energy of the environment that image-based lighting makes of it (ADR-0045).

mod support;

use std::f64::consts::PI;

use pathon_api::{Camera, Slot};
use pathon_golden::{Tolerance, assert_golden};
use pathon_ibl::IblEnvironment;
use pathon_sky::reference::{self, Planet, SkyConditions, SkyModel};
use pathon_sky::{Atmosphere, MULTISCATTER_LUT, SKY_VIEW_LUT, SkySettings, TRANSMITTANCE_LUT};
use support::{Harness, Options, all_finite, camera_looking, golden_image, pixel_direction, radiance, read_rgba16f};

const SIZE: (u32, u32) = (128, 96);

fn harness(test: &str, settings: SkySettings) -> Option<Harness> {
    Harness::new(
        test,
        Options {
            settings,
            ..Options::default()
        },
    )
}

fn conditions(settings: &SkySettings) -> SkyConditions {
    let sun = settings.sun_direction.normalize();
    SkyConditions {
        camera_radius_km: f64::from(settings.camera_radius_km(1.7)),
        sun_direction: [f64::from(sun.x), f64::from(sun.y), f64::from(sun.z)],
        sun_illuminance: std::array::from_fn(|channel| {
            f64::from(settings.sun_illuminance_lux) * f64::from(settings.sun_tint[channel])
        }),
        cloud_coverage: f64::from(settings.cloud_coverage),
    }
}

fn model(settings: &SkySettings) -> SkyModel {
    SkyModel::new(&settings.atmosphere, settings.ground_albedo, None)
}

fn sun_at(elevation: f32) -> SkySettings {
    SkySettings::default().with_sun(elevation, 0.0)
}

fn centre(texels: &[[f32; 4]]) -> [f64; 3] {
    radiance(texels, SIZE, SIZE.0 / 2, SIZE.1 / 2)
}

#[test]
fn the_transmittance_table_matches_the_cpu_integral() {
    let Some(mut harness) = harness(
        "the_transmittance_table_matches_the_cpu_integral",
        SkySettings::default(),
    ) else {
        return;
    };
    let camera = harness.camera();
    harness.render(camera);
    let (texels, (width, height)) = harness.texels(TRANSMITTANCE_LUT);
    let planet = Planet::new(&Atmosphere::EARTH);
    let mut worst: f64 = 0.0;
    for y in 0..height {
        for x in 0..width {
            let (radius, mu) = planet.transmittance_params(
                (f64::from(x) + 0.5) / f64::from(width),
                (f64::from(y) + 0.5) / f64::from(height),
            );
            let expected = planet.transmittance_integral(radius, mu, 256);
            let texel = texels[(y * width + x) as usize];
            for channel in 0..3 {
                let error = (f64::from(texel[channel]) - expected[channel]).abs();
                let allowed = 0.02 * expected[channel] + 3e-4;
                worst = worst.max(error / allowed);
                assert!(
                    error <= allowed,
                    "texel ({x}, {y}) channel {channel}: {} against {} (radius {radius}, mu {mu})",
                    texel[channel],
                    expected[channel]
                );
            }
        }
    }
    eprintln!("transmittance table: worst error is {worst:.2} of the tolerance");
}

#[test]
fn nothing_is_nan_or_infinite_in_any_light() {
    for (elevation, cloud) in [
        (80.0, 0.0),
        (20.0, 0.0),
        (2.0, 0.0),
        (0.0, 0.0),
        (-8.0, 0.0),
        (35.0, 0.6),
        (35.0, 1.0),
    ] {
        let settings = SkySettings {
            cloud_coverage: cloud,
            ..sun_at(elevation)
        };
        let Some(mut harness) = harness("nothing_is_nan_or_infinite_in_any_light", settings) else {
            return;
        };
        for pitch in [-30.0, 0.0, 90.0 - 1.0] {
            let camera = camera_looking(0.0, pitch, SIZE);
            harness.render(camera);
            for slot in [Slot::HDR, TRANSMITTANCE_LUT, MULTISCATTER_LUT, SKY_VIEW_LUT] {
                let (texels, _) = harness.texels(slot);
                assert!(
                    all_finite(&texels),
                    "{} is not finite for sun {elevation}, cloud {cloud}, pitch {pitch}",
                    slot.name()
                );
                assert!(
                    texels.iter().all(|texel| texel[..3].iter().all(|value| *value >= 0.0)),
                    "{} is negative",
                    slot.name()
                );
            }
        }
    }
}

#[test]
fn the_zenith_is_blue_and_the_horizon_at_sunset_is_redder_than_at_noon() {
    let looking_up = camera_looking(0.0, 85.0, SIZE);
    let horizon = camera_looking(0.0, 2.0, SIZE);

    let Some(mut noon) = harness("zenith_and_horizon", sun_at(55.0)) else {
        return;
    };
    noon.render(looking_up);
    let zenith = centre(&noon.hdr());
    eprintln!("noon zenith radiance {zenith:?} nits");
    assert!(
        zenith[2] > 1.3 * zenith[0] && zenith[2] > zenith[1],
        "the zenith is blue: {zenith:?}"
    );
    assert!(
        (1_000.0..20_000.0).contains(&zenith[1]),
        "a clear zenith is some thousand nits: {zenith:?}"
    );

    noon.render(horizon);
    let noon_horizon = centre(&noon.hdr());

    let Some(mut sunset) = harness("zenith_and_horizon", sun_at(1.0)) else {
        return;
    };
    sunset.render(horizon);
    let sunset_horizon = centre(&sunset.hdr());
    eprintln!("horizon at noon {noon_horizon:?}, at sunset {sunset_horizon:?}");
    let redness = |color: [f64; 3]| color[0] / color[2];
    assert!(
        redness(sunset_horizon) > 1.0,
        "the sunset horizon is red: {sunset_horizon:?}"
    );
    assert!(
        redness(sunset_horizon) > 1.5 * redness(noon_horizon),
        "redder than at noon: {sunset_horizon:?} against {noon_horizon:?}"
    );
}

#[test]
fn the_picture_follows_the_cpu_model() {
    let mut worst: f64 = 0.0;
    for elevation in [60.0, 15.0, 2.0] {
        let settings = sun_at(elevation);
        let Some(mut harness) = harness("the_picture_follows_the_cpu_model", settings.clone()) else {
            return;
        };
        let (model, conditions) = (model(&settings), conditions(&settings));
        for (yaw, pitch) in [
            (0.0, 80.0),
            (0.0, 40.0),
            (0.0, 8.0),
            (180.0, 30.0),
            (180.0, 3.0),
            (90.0, 15.0),
            (0.0, -20.0),
        ] {
            let camera = camera_looking(yaw, pitch, SIZE);
            harness.render(camera);
            let texels = harness.hdr();
            for (x, y) in [(64, 48), (20, 20), (110, 70), (64, 10)] {
                let direction = pixel_direction(&camera, SIZE, x, y);
                let toward_sun = reference::dot(direction, conditions.sun_direction)
                    .clamp(-1.0, 1.0)
                    .acos();
                if toward_sun < 8.0_f64.to_radians() {
                    continue;
                }
                let expected = model.radiance(&conditions, direction, 96);
                let actual = radiance(&texels, SIZE, x, y);
                let scale = expected.iter().copied().fold(0.0, f64::max).max(1.0);
                for channel in 0..3 {
                    let error = (actual[channel] - expected[channel]).abs() / (expected[channel] + 0.02 * scale);
                    worst = worst.max(error);
                    assert!(
                        error < 0.16,
                        "sun {elevation}, yaw {yaw}, pitch {pitch}, pixel ({x}, {y}), channel {channel}: {actual:?} against {expected:?}"
                    );
                }
            }
        }
    }
    eprintln!("picture against the CPU model: worst relative error {worst:.3}");
}

#[test]
fn clouds_blend_to_the_overcast_sky() {
    let settings = SkySettings {
        cloud_coverage: 1.0,
        ..sun_at(50.0)
    };
    let Some(mut harness) = harness("clouds_blend_to_the_overcast_sky", settings.clone()) else {
        return;
    };
    harness.render(camera_looking(0.0, 85.0, SIZE));
    let zenith = centre(&harness.hdr());
    let expected = model(&settings).overcast_zenith_radiance(&conditions(&settings));
    for channel in 0..3 {
        assert!(
            (zenith[channel] - expected[channel]).abs() < 0.03 * expected[channel],
            "{zenith:?} against {expected:?}"
        );
    }
    // The sun is gone: nothing in the picture is brighter than the overcast zenith.
    harness.render(camera_looking(0.0, 50.0, SIZE));
    let brightest = harness.hdr().iter().map(|texel| texel[1]).fold(0.0, f32::max);
    assert!(
        f64::from(brightest) < 1.1 * expected[1],
        "{brightest} against {expected:?}"
    );
}

#[test]
fn the_sun_disc_is_there_and_capped() {
    // At 128 x 96 the disc is smaller than a pixel, so the sun is put exactly behind the centre of one.
    let camera = camera_looking(0.0, 30.0, SIZE);
    let [x, y, z] = pixel_direction(&camera, SIZE, SIZE.0 / 2, SIZE.1 / 2);
    #[expect(clippy::cast_possible_truncation, reason = "a unit direction fits f32")]
    let settings = SkySettings {
        sun_direction: pathon_api::glam::Vec3::new(x as f32, y as f32, z as f32),
        ..SkySettings::default()
    };
    let Some(mut harness) = harness("the_sun_disc_is_there_and_capped", settings.clone()) else {
        return;
    };
    harness.render(camera);
    let texels = harness.hdr();
    let disc = centre(&texels);
    assert!(disc[1] >= f64::from(settings.sun_disc_max_radiance) * 0.9, "{disc:?}");
    assert!(disc.iter().all(|value| *value <= 65_000.0));
    let beside = radiance(&texels, SIZE, SIZE.0 / 2 + 3, SIZE.1 / 2);
    assert!(beside[1] < 0.3 * disc[1], "the disc is small: {beside:?}");

    let mut settings = settings;
    settings.draw_sun_disc = false;
    harness.set_settings(settings);
    harness.render(camera);
    assert!(
        centre(&harness.hdr())[1] < 0.2 * disc[1],
        "no disc when it is switched off"
    );
}

#[test]
fn the_sky_is_drawn_only_where_the_depth_is_empty() {
    let settings = sun_at(40.0);
    let options = |with_wall| Options {
        settings: settings.clone(),
        with_wall,
        ..Options::default()
    };
    let (Some(mut with_wall), Some(mut plain)) = (
        Harness::new("the_sky_is_drawn_only_where_the_depth_is_empty", options(true)),
        Harness::new("the_sky_is_drawn_only_where_the_depth_is_empty", options(false)),
    ) else {
        return;
    };
    let camera = camera_looking(0.0, 20.0, SIZE);
    with_wall.render(camera);
    plain.render(camera);
    let (walled, open) = (with_wall.hdr(), plain.hdr());
    for y in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let (walled, open) = (radiance(&walled, SIZE, x, y), radiance(&open, SIZE, x, y));
            if x < SIZE.0 / 2 {
                assert!(
                    walled.iter().all(|value| (value - 0.25).abs() < 1e-3),
                    "the wall keeps its colour at ({x}, {y}): {walled:?}"
                );
            } else {
                assert_eq!(walled, open, "the sky fills the rest at ({x}, {y})");
            }
        }
    }
}

fn golden(name: &str, test: &str, settings: SkySettings, camera: Camera, ev100: f32) {
    let Some(mut harness) = harness(test, settings) else {
        return;
    };
    harness.render(camera);
    let texels = harness.hdr();
    assert!(all_finite(&texels));
    assert_golden(name, &golden_image(&texels, SIZE, ev100), Tolerance::cross_backend());
}

#[test]
fn sky_noon_matches_its_golden_image() {
    golden(
        "sky_noon",
        "sky_noon_matches_its_golden_image",
        sun_at(62.0),
        camera_looking(0.0, 30.0, SIZE),
        14.5,
    );
}

#[test]
fn sky_low_sun_matches_its_golden_image() {
    golden(
        "sky_low_sun",
        "sky_low_sun_matches_its_golden_image",
        sun_at(12.0),
        camera_looking(0.0, 15.0, SIZE),
        13.0,
    );
}

#[test]
fn sky_sunset_matches_its_golden_image() {
    golden(
        "sky_sunset",
        "sky_sunset_matches_its_golden_image",
        sun_at(1.0),
        camera_looking(0.0, 8.0, SIZE),
        10.0,
    );
}

#[test]
fn sky_overcast_matches_its_golden_image() {
    golden(
        "sky_overcast",
        "sky_overcast_matches_its_golden_image",
        SkySettings {
            cloud_coverage: 0.85,
            ..sun_at(40.0)
        },
        camera_looking(40.0, 25.0, SIZE),
        13.0,
    );
}

// Irradiance the environment cube gives a normal, against the cosine integral of the CPU sky over the sphere.
#[test]
fn the_irradiance_of_the_sky_environment_matches_the_cpu_integral() {
    for elevation in [50.0, 8.0] {
        let settings = sun_at(elevation);
        let Some(mut harness) = Harness::new(
            "the_irradiance_of_the_sky_environment_matches_the_cpu_integral",
            Options {
                settings: settings.clone(),
                with_ibl: true,
                ..Options::default()
            },
        ) else {
            return;
        };
        let camera = harness.camera();
        harness.render(camera);
        let environment = *harness
            .renderer
            .shared()
            .get::<IblEnvironment>()
            .expect("the environment was built from the sky");

        let (model, conditions) = (model(&settings), conditions(&settings));
        let (width, height) = (128_usize, 64_usize);
        let mut samples = Vec::with_capacity(width * height);
        for y in 0..height {
            let polar = PI * (y as f64 + 0.5) / height as f64;
            let solid_angle = 2.0 * PI / width as f64 * PI / height as f64 * polar.sin();
            for x in 0..width {
                let direction = pathon_ibl::reference::equirect_direction(
                    (x as f64 + 0.5) / width as f64,
                    (y as f64 + 0.5) / height as f64,
                );
                samples.push((direction, model.radiance(&conditions, direction, 48), solid_angle));
            }
        }
        let normals = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        for (face, normal) in normals.into_iter().enumerate() {
            let mut expected = [0.0; 3];
            for (direction, light, solid_angle) in &samples {
                let cosine = reference::dot(*direction, normal).max(0.0);
                for channel in 0..3 {
                    expected[channel] += light[channel] * cosine * solid_angle / PI;
                }
            }
            let ((size, _), texels) = read_rgba16f(
                &**harness.renderer.device(),
                environment.irradiance.texture,
                IblEnvironment::STATE,
                0,
                u32::try_from(face).expect("six faces"),
            );
            let middle = (size / 2) as usize;
            let at = |x: usize, y: usize| texels[y * size as usize + x];
            let mut actual = [0.0_f64; 3];
            for (x, y) in [
                (middle - 1, middle - 1),
                (middle, middle - 1),
                (middle - 1, middle),
                (middle, middle),
            ] {
                for (sum, value) in actual.iter_mut().zip(at(x, y)) {
                    *sum += f64::from(value) / 4.0;
                }
            }
            eprintln!("sun {elevation}, face {face}: {actual:?} against {expected:?}");
            for channel in 0..3 {
                let error = (actual[channel] - expected[channel]).abs();
                assert!(
                    error <= 0.05 * expected[channel] + 1.0,
                    "sun {elevation}, face {face}, channel {channel}: {actual:?} against {expected:?}"
                );
            }
        }
    }
}
