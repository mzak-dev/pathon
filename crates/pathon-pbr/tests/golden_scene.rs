// M4-05: the test scene through the whole chain (IblPack, PbrPack, PostPack) against its golden image, plus the
// invariants of the colour pass: no NaN, diffuse + specular = hdr, clustered lights = every light tested.
// `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

mod support;

use std::sync::Arc;

use pathon_api::glam::{Affine3A, Quat, Vec3};
use pathon_api::{RenderFeature, Severity, Slot};
use pathon_ibl::{IblPack, IblSettings, IblSource, SyntheticSky};
use pathon_pbr::{LightAssignment, PbrPack, PbrSettings};
use pathon_post::{BloomSettings, ExposureMode, ExposureSettings, PostPack, PostSettings};
use pathon_rhi::{Format, NullDevice};
use pathon_scene::Light;
use support::probe::{Probe, ProbeReader};
use support::scene::{Content, Harness, add_content, instance, null_harness, scale_translate};
use support::Lcg;

const SIZE: u32 = 256;

fn post() -> PostPack {
    PostPack::with_settings(PostSettings {
        exposure: ExposureSettings {
            mode: ExposureMode::Manual { ev: 0.4 },
            compensation_ev: 0.0,
        },
        bloom: BloomSettings {
            enabled: false,
            ..BloomSettings::default()
        },
        ..PostSettings::default()
    })
}

fn sky() -> IblSource {
    let image = SyntheticSky::default().image(128, 64).expect("a valid image");
    IblSource::new(Arc::new(image), IblSettings::COMPACT)
}

fn pointing(direction: Vec3) -> Quat {
    Quat::from_rotation_arc(Vec3::NEG_Z, direction.normalize())
}

fn lights(harness: &mut Harness) {
    let sun = Light::directional(Affine3A::from_quat(pointing(Vec3::new(-0.5, -0.8, -0.6))), 1.6);
    let points = [
        (Vec3::new(-3.0, 1.2, 1.5), Vec3::new(1.0, 0.3, 0.2)),
        (Vec3::new(3.2, 1.0, 1.0), Vec3::new(0.2, 0.5, 1.0)),
        (Vec3::new(0.0, 0.6, 2.4), Vec3::new(0.3, 1.0, 0.4)),
    ]
    .map(|(position, color)| {
        Light::point(Affine3A::from_translation(position), 90.0)
            .with_color(color)
            .with_range(5.0)
    });
    let spot = Light::spot(
        Affine3A::from_rotation_translation(
            pointing(Vec3::new(-2.2, -3.0, -1.6)),
            Vec3::new(2.5, 3.0, 1.8),
        ),
        500.0,
        0.15,
        0.35,
    )
    .with_range(9.0);
    harness
        .scene
        .extract(|writer| {
            writer.spawn_light(&sun)?;
            for point in &points {
                writer.spawn_light(point)?;
            }
            writer.spawn_light(&spot)
        })
        .expect("lights");
}

fn objects(harness: &mut Harness) -> Content {
    let content = add_content(&mut harness.renderer, true);
    let scene = &mut harness.scene;
    instance(scene, content.ground, content.floor_material, scale_translate(5.0, Vec3::new(0.0, -1.0, 0.0)));
    instance(scene, content.ball, content.metal_material, scale_translate(0.9, Vec3::new(-1.7, -0.1, 0.0)));
    instance(scene, content.ball, content.plastic_material, scale_translate(0.8, Vec3::new(1.6, -0.2, 0.3)));
    instance(
        scene,
        content.block,
        content.floor_material,
        Affine3A::from_scale_rotation_translation(
            Vec3::splat(0.6),
            Quat::from_rotation_y(0.45),
            Vec3::new(0.1, -0.4, -1.4),
        ),
    );
    instance(
        scene,
        content.fence,
        content.fence_material,
        Affine3A::from_scale_rotation_translation(Vec3::splat(0.55), Quat::from_rotation_y(-0.35), Vec3::new(-0.2, -0.45, 1.7)),
    );
    content
}

fn configure_split(
    builder: pathon_core::RendererBuilder,
) -> (pathon_core::RendererBuilder, [ProbeReader; 3]) {
    let size = (SIZE, SIZE);
    let (hdr, hdr_reader) = Probe::new(Slot::HDR, size, 1, 8);
    let (diffuse, diffuse_reader) = Probe::new(Slot::DIFFUSE, size, 1, 8);
    let (specular, specular_reader) = Probe::new(Slot::SPECULAR, size, 1, 8);
    let builder = builder
        .with_feature(hdr)
        .with_feature(diffuse)
        .with_feature(specular);
    (builder, [hdr_reader, diffuse_reader, specular_reader])
}

fn base_builder(builder: pathon_core::RendererBuilder) -> pathon_core::RendererBuilder {
    builder
        .with_pack(IblPack::new())
        .with_shared(sky())
        .with_pack(PbrPack::new())
        .with_pack(post())
}

fn golden_camera() -> pathon_api::Camera {
    pathon_api::Camera::look_at(Vec3::new(0.0, 2.8, 7.6), Vec3::new(0.0, -0.4, 0.0), 50.0_f32.to_radians(), 1.0)
}

fn assert_no_errors(diagnostics: &[pathon_api::Diagnostic]) {
    let errors: Vec<_> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn the_scene_validates_on_the_null_device_with_every_slot_requested() {
    let _ = NullDevice::new();
    let mut readers = None;
    let mut harness = null_harness(SIZE, Format::Rgba8Unorm, |builder| {
        let (builder, split) = configure_split(base_builder(builder));
        readers = Some(split);
        builder
    });
    objects(&mut harness);
    lights(&mut harness);

    let diagnostics = harness.render(golden_camera());

    assert_no_errors(&diagnostics);
    assert!(readers.is_some());
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_golden::{Tolerance, assert_golden, read_texture};
    use pathon_rhi::Access;
    use support::scene::harness;

    use super::*;

    fn halves(reader: &ProbeReader, device: &dyn pathon_rhi::DeviceApi) -> Vec<[f32; 4]> {
        reader
            .texels(device, 0)
            .chunks(8)
            .map(|texel| {
                [0, 1, 2, 3].map(|channel| {
                    support::probe::half_to_f32(u16::from_le_bytes([texel[channel * 2], texel[channel * 2 + 1]]))
                })
            })
            .collect()
    }

    fn scene_harness(test: &str) -> Option<(Harness, [ProbeReader; 3])> {
        let mut readers = None;
        let mut harness = harness(test, SIZE, Format::Rgba8Unorm, |builder| {
            let (builder, split) = configure_split(base_builder(builder));
            readers = Some(split);
            builder
        })?;
        objects(&mut harness);
        lights(&mut harness);
        Some((harness, readers?))
    }

    #[test]
    fn the_test_scene_matches_its_golden_image_and_has_no_nan() {
        let Some((mut harness, [hdr, _, _])) = scene_harness("the_test_scene_matches_its_golden_image_and_has_no_nan") else {
            return;
        };
        assert_no_errors(&harness.render(golden_camera()));
        let device = harness.renderer.device().clone();
        let device: &dyn pathon_rhi::DeviceApi = &*device;

        let texels = halves(&hdr, device);
        assert!(
            texels.iter().all(|texel| texel.iter().all(|value| value.is_finite() && *value >= 0.0)),
            "hdr has NaN, infinite or negative values"
        );
        let lit = texels.iter().filter(|texel| texel[..3].iter().any(|value| *value > 0.02)).count();
        assert!(lit > texels.len() / 2, "the scene is mostly black: {lit} lit pixels");
        let texture = harness.renderer.target_texture(harness.target).expect("offscreen texture");
        let image = read_texture(device, texture, Access::SAMPLED).expect("readback");
        assert_golden("pbr_scene", &image, Tolerance::cross_backend());
    }

    #[test]
    fn diffuse_and_specular_add_up_to_hdr() {
        let Some((mut harness, [hdr, diffuse, specular])) = scene_harness("diffuse_and_specular_add_up_to_hdr") else {
            return;
        };
        assert_no_errors(&harness.render(golden_camera()));
        let device = harness.renderer.device().clone();
        let device: &dyn pathon_rhi::DeviceApi = &*device;
        let (hdr, diffuse, specular) = (halves(&hdr, device), halves(&diffuse, device), halves(&specular, device));

        let mut worst = 0.0_f32;
        for ((total, d), s) in hdr.iter().zip(&diffuse).zip(&specular) {
            if total[3] == 0.0 && d[3] == 0.0 {
                continue;
            }
            for channel in 0..3 {
                let error = (total[channel] - d[channel] - s[channel]).abs() / (total[channel].abs() + 0.01);
                worst = worst.max(error);
            }
        }
        assert!(worst < 5.0e-3, "diffuse + specular differ from hdr by {worst} (relative)");
        assert!(diffuse.iter().any(|texel| texel[0] > 0.1) && specular.iter().any(|texel| texel[0] > 0.1));
    }

    fn thousand_lights(harness: &mut Harness) {
        let mut random = Lcg(5);
        harness
            .scene
            .extract(|writer| {
                for _ in 0..1000 {
                    let position = Vec3::new(random.between(-6.0, 6.0), random.between(-0.8, 2.5), random.between(-5.0, 5.0));
                    let color = Vec3::new(random.between(0.2, 1.0), random.between(0.2, 1.0), random.between(0.2, 1.0));
                    writer.spawn_light(
                        &Light::point(Affine3A::from_translation(position), 60.0)
                            .with_color(color)
                            .with_range(random.between(0.6, 1.6)),
                    )?;
                }
                Ok::<_, pathon_scene::SceneError>(())
            })
            .expect("lights");
    }

    #[test]
    fn clustered_lights_equal_testing_every_light_with_a_thousand_of_them() {
        let Some((mut harness, [hdr, _, _])) = scene_harness("clustered_lights_equal_testing_every_light_with_a_thousand_of_them")
        else {
            return;
        };
        thousand_lights(&mut harness);
        let device = harness.renderer.device().clone();
        let device: &dyn pathon_rhi::DeviceApi = &*device;

        assert_no_errors(&harness.render(golden_camera()));
        let clustered = halves(&hdr, device);
        harness.renderer.shared_mut().insert(PbrSettings {
            light_assignment: LightAssignment::BruteForce,
            ..PbrSettings::default()
        });
        assert_no_errors(&harness.render(golden_camera()));
        let brute_force = halves(&hdr, device);

        let mut differing = 0;
        for (a, b) in clustered.iter().zip(&brute_force) {
            // Summation order differs (clusters list lights in order, so it does not here): any difference is a
            // light the clusters dropped or added.
            if (0..3).any(|channel| (a[channel] - b[channel]).abs() > 1.0e-3 * a[channel].abs().max(0.05)) {
                differing += 1;
            }
        }
        assert!(
            differing * 1000 <= clustered.len(),
            "{differing} of {} pixels differ between clustered and brute-force lights",
            clustered.len()
        );
        let glowing = clustered.iter().filter(|texel| texel[0] > 1.0).count();
        assert!(glowing > 0, "the thousand lights light something");
    }
}
