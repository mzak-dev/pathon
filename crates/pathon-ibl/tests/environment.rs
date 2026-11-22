#![cfg(feature = "gpu-tests")]
// The compute passes against CPU references written as plain numerical integration (`pathon_ibl::reference`), and
// the white furnace: a constant environment of 1 must stay 1 in every map. `PATHON_TEST_BACKEND=vulkan|dx12|metal`
// picks the backend.

mod support;

use pathon_ibl::reference::{brdf_lut_texel, irradiance_over_pi, prefiltered_radiance, texel_direction};
use pathon_ibl::{IblBuilder, IblEnvironment, IblSettings, SyntheticSky};
use pathon_rhi::{Access, Device};
use support::{gpu, read_rgba16f, sky_radiance};

const SKY_WIDTH: u32 = 1024;
const SKY_HEIGHT: u32 = 512;

fn build(device: &Device, builder: &mut IblBuilder, sky: &SyntheticSky, settings: &IblSettings) -> IblEnvironment {
    let image = sky.image(SKY_WIDTH, SKY_HEIGHT).expect("image");
    builder
        .build_environment(device, &image, settings)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

fn angle_to_sun(sky: &SyntheticSky, direction: [f64; 3]) -> f64 {
    let cosine: f64 = direction
        .iter()
        .zip(sky.sun_direction)
        .map(|(axis, sun)| axis * f64::from(sun))
        .sum();
    cosine.clamp(-1.0, 1.0).acos().to_degrees()
}

// Texels spread over all six faces, a few per face, at positions that never sit on a face edge.
fn probe_texels(size: u32) -> Vec<(u32, u32, u32)> {
    let at = |per_mille: u32| (size * per_mille / 1000).min(size - 1);
    (0..6)
        .flat_map(|face| [(200, 300), (500, 500), (800, 650), (350, 850)].map(|(x, y)| (face, at(x), at(y))))
        .collect()
}

fn within(actual: f32, expected: f64, relative: f64, absolute: f64) -> bool {
    (f64::from(actual) - expected).abs() <= relative * expected.abs() + absolute
}

#[test]
fn the_brdf_table_matches_the_cpu_integral() {
    let Some(device) = gpu("the_brdf_table_matches_the_cpu_integral") else {
        return;
    };
    let settings = IblSettings::DESKTOP;
    let lut = IblBuilder::new().brdf_lut(&device, &settings).expect("lut");
    let mip = read_rgba16f(&device, lut.texture, Access::SAMPLED, 0, 0);
    assert_eq!((mip.width, mip.height), (settings.lut_size, settings.lut_size));

    let size = settings.lut_size;
    let mut worst: f64 = 0.0;
    for y in (3..size).step_by(17) {
        for x in (3..size).step_by(19) {
            let n_dot_v = (f64::from(x) + 0.5) / f64::from(size);
            let roughness = (f64::from(y) + 0.5) / f64::from(size);
            let expected = brdf_lut_texel(n_dot_v, roughness);
            let actual = mip.at(x, y);
            for channel in 0..2 {
                let error = (f64::from(actual[channel]) - expected[channel]).abs();
                worst = worst.max(error);
                assert!(
                    error < 0.01,
                    "n.v {n_dot_v:.3} roughness {roughness:.3} channel {channel}: gpu {} cpu {}",
                    actual[channel],
                    expected[channel]
                );
            }
        }
    }
    eprintln!("brdf lut: worst absolute error {worst:.5}");
}

#[test]
fn the_brdf_table_is_generated_once_and_cached() {
    let Some(device) = gpu("the_brdf_table_is_generated_once_and_cached") else {
        return;
    };
    let mut builder = IblBuilder::new();
    let first = builder.brdf_lut(&device, &IblSettings::COMPACT).expect("lut");
    let second = builder.brdf_lut(&device, &IblSettings::COMPACT).expect("lut");
    assert_eq!(first, second);
    let other = builder.brdf_lut(&device, &IblSettings::DESKTOP).expect("lut");
    assert_ne!(first.texture, other.texture);
    builder.release(&device);
}

#[test]
fn the_prefiltered_cube_matches_the_cpu_integral_at_every_roughness() {
    let test = "the_prefiltered_cube_matches_the_cpu_integral_at_every_roughness";
    let Some(device) = gpu(test) else {
        return;
    };
    let sky = SyntheticSky::default();
    let settings = IblSettings::DESKTOP;
    let environment = build(&device, &mut IblBuilder::new(), &sky, &settings);
    let levels = environment.prefiltered_levels;
    assert_eq!(levels, 6);

    let reference = sky_radiance(sky);
    for mip in [0, 1, 2, 4, 5] {
        let roughness = f64::from(mip) / f64::from(levels - 1);
        let size = settings.cube_size >> mip;
        let mut worst: f64 = 0.0;
        let mut checked = 0;
        for face in 0..6 {
            let data = read_rgba16f(&device, environment.prefiltered.texture, Access::SAMPLED, mip, face);
            for (probe_face, x, y) in probe_texels(size) {
                if probe_face != face {
                    continue;
                }
                let direction = texel_direction(face, x, y, size);
                // The sharp mip is the picture itself: next to the sun's edge a texel is an average of both sides.
                if mip == 0 && angle_to_sun(&sky, direction) < 12.0 {
                    continue;
                }
                let expected = prefiltered_radiance(&reference, direction, roughness);
                let actual = data.at(x, y);
                for channel in 0..3 {
                    let error = (f64::from(actual[channel]) - expected[channel]).abs() / (expected[channel] + 0.05);
                    worst = worst.max(error);
                    assert!(
                        within(actual[channel], expected[channel], 0.06, 0.02),
                        "mip {mip} (roughness {roughness:.2}) face {face} texel ({x},{y}) channel {channel}: gpu {} cpu {}",
                        actual[channel],
                        expected[channel]
                    );
                }
                checked += 1;
            }
        }
        assert!(checked >= 3, "mip {mip}: only {checked} texels checked");
        eprintln!("prefiltered mip {mip}: {checked} texels, worst relative error {worst:.4}");
    }
}

#[test]
fn the_irradiance_cube_matches_the_cpu_integral() {
    let Some(device) = gpu("the_irradiance_cube_matches_the_cpu_integral") else {
        return;
    };
    let sky = SyntheticSky::default();
    let settings = IblSettings::DESKTOP;
    let environment = build(&device, &mut IblBuilder::new(), &sky, &settings);
    let reference = sky_radiance(sky);

    let size = settings.irradiance_size;
    let mut worst: f64 = 0.0;
    for face in 0..6 {
        let data = read_rgba16f(&device, environment.irradiance.texture, Access::SAMPLED, 0, face);
        for (probe_face, x, y) in probe_texels(size) {
            if probe_face != face {
                continue;
            }
            let expected = irradiance_over_pi(&reference, texel_direction(face, x, y, size));
            let actual = data.at(x, y);
            for channel in 0..3 {
                worst = worst.max((f64::from(actual[channel]) - expected[channel]).abs() / expected[channel]);
                assert!(
                    within(actual[channel], expected[channel], 0.03, 0.01),
                    "face {face} texel ({x},{y}) channel {channel}: gpu {} cpu {}",
                    actual[channel],
                    expected[channel]
                );
            }
        }
    }
    eprintln!("irradiance: worst relative error {worst:.4}");
}

#[test]
fn a_constant_environment_stays_constant_in_every_map() {
    let Some(device) = gpu("a_constant_environment_stays_constant_in_every_map") else {
        return;
    };
    let settings = IblSettings::COMPACT;
    let environment = build(&device, &mut IblBuilder::new(), &SyntheticSky::furnace(1.0), &settings);

    // The white furnace: diffuse albedo 1 under an environment of 1 returns 1 (within 2 %), and so does every
    // roughness of the specular chain.
    for face in 0..6 {
        let data = read_rgba16f(&device, environment.irradiance.texture, Access::SAMPLED, 0, face);
        for texel in &data.texels {
            for channel in 0..3 {
                assert!((texel[channel] - 1.0).abs() < 0.02, "irradiance {texel:?}");
            }
        }
        for mip in 0..environment.prefiltered_levels {
            let data = read_rgba16f(&device, environment.prefiltered.texture, Access::SAMPLED, mip, face);
            for texel in &data.texels {
                for channel in 0..3 {
                    assert!((texel[channel] - 1.0).abs() < 0.02, "mip {mip}: {texel:?}");
                }
            }
        }
    }
}

#[test]
fn a_second_build_gets_a_new_generation_and_the_same_table() {
    let Some(device) = gpu("a_second_build_gets_a_new_generation_and_the_same_table") else {
        return;
    };
    let mut builder = IblBuilder::new();
    let settings = IblSettings::COMPACT;
    let first = build(&device, &mut builder, &SyntheticSky::default(), &settings);
    let second = build(&device, &mut builder, &SyntheticSky::furnace(0.5), &settings);
    assert_eq!(second.generation, first.generation + 1);
    assert_eq!(second.brdf_lut, first.brdf_lut);
    assert_ne!(second.prefiltered.texture, first.prefiltered.texture);
    first.release(&device);
    second.release(&device);
    builder.release(&device);
}
