// The sky pack in a real renderer on the null device, which checks every command list the passes record: barriers,
// usage flags, bind groups against their layouts. The pixels are the GPU tests' business (tests/gpu.rs).

mod support;

use pathon_api::Camera;
use pathon_api::glam::Vec3;
use pathon_ibl::{IblEnvironment, IblSource};
use pathon_sky::{SkyIblSettings, SkySettings, SunLight};
use support::{Harness, Options};

fn harness(settings: SkySettings, with_wall: bool) -> Harness {
    Harness::new(
        "null_device",
        Options {
            settings,
            with_ibl: true,
            with_wall,
            null_device: true,
            ..Options::default()
        },
    )
    .expect("the null device is always there")
}

fn codes(report: &pathon_core::FrameReport) -> Vec<&str> {
    report.diagnostics.iter().map(|diagnostic| diagnostic.code).collect()
}

#[test]
fn frames_validate_with_and_without_a_depth_slot() {
    for with_wall in [false, true] {
        let mut harness = harness(SkySettings::default(), with_wall);
        for _ in 0..3 {
            let camera = harness.camera();
            let report = harness.render(camera);
            assert!(
                report.diagnostics.is_empty(),
                "wall {with_wall}: {:?}",
                report.diagnostics
            );
        }
    }
}

#[test]
fn the_sun_light_follows_the_settings_every_frame() {
    let mut harness = harness(SkySettings::default().with_sun(60.0, 30.0), false);
    let camera = harness.camera();
    harness.render(camera);
    let noon = *harness.renderer.shared().get::<SunLight>().expect("published");
    assert!((noon.direction_to_sun - SkySettings::default().with_sun(60.0, 30.0).sun_direction).length() < 1e-6);
    assert!(noon.luminous_illuminance() > 80_000.0);

    harness.set_settings(SkySettings::default().with_sun(2.0, 30.0));
    harness.render(camera);
    let low = *harness.renderer.shared().get::<SunLight>().expect("published");
    assert!(low.illuminance_lux[2] < 0.5 * low.illuminance_lux[0], "{low:?}");

    harness.set_settings(SkySettings::default().with_sun(-10.0, 30.0));
    harness.render(camera);
    let night = *harness.renderer.shared().get::<SunLight>().expect("published");
    assert_eq!(night.illuminance_lux, [0.0; 3]);
}

#[test]
fn the_environment_is_rebuilt_after_a_big_change_and_not_after_a_small_one() {
    let base = SkySettings {
        ibl: SkyIblSettings {
            image_width: 32,
            min_interval_seconds: 1.0,
            ..SkyIblSettings::default()
        },
        ..SkySettings::default().with_sun(50.0, 0.0)
    };
    let mut harness = harness(base.clone(), false);
    let camera = harness.camera();
    let generation = |harness: &Harness| {
        harness
            .renderer
            .shared()
            .get::<IblEnvironment>()
            .map(|environment| environment.generation)
    };

    harness.time_seconds = 5.0;
    harness.render(camera);
    assert_eq!(generation(&harness), Some(1), "built on the first frame");

    harness.set_settings(SkySettings {
        sun_direction: SkySettings::default().with_sun(50.4, 0.0).sun_direction,
        ..base.clone()
    });
    harness.render(camera);
    assert_eq!(generation(&harness), Some(1), "0.4 degrees is below the threshold");

    harness.time_seconds = 5.2;
    harness.set_settings(SkySettings {
        sun_direction: SkySettings::default().with_sun(35.0, 0.0).sun_direction,
        ..base.clone()
    });
    harness.render(camera);
    assert_eq!(generation(&harness), Some(1), "a big change waits for the interval");

    harness.time_seconds = 6.5;
    harness.render(camera);
    assert_eq!(
        generation(&harness),
        Some(2),
        "and is built when the interval has passed"
    );

    harness.render(camera);
    assert_eq!(generation(&harness), Some(2), "the same sky is not built twice");
}

#[test]
fn a_disabled_environment_leaves_the_ibl_source_alone() {
    let mut settings = SkySettings::default();
    settings.ibl.enabled = false;
    let mut harness = harness(settings, false);
    let camera = harness.camera();
    harness.render(camera);
    assert!(harness.renderer.shared().get::<IblSource>().is_none());
    assert!(harness.renderer.shared().get::<SunLight>().is_some());
}

#[test]
fn broken_settings_are_repaired_and_reported_once() {
    let broken = SkySettings {
        sun_direction: Vec3::ZERO,
        cloud_coverage: 7.0,
        ..SkySettings::default()
    };
    let mut harness = harness(broken.clone(), false);
    let camera = harness.camera();
    assert_eq!(codes(&harness.render(camera)), ["A0001"]);
    assert!(codes(&harness.render(camera)).is_empty(), "reported once");
    harness.set_settings(SkySettings::default());
    harness.render(camera);
    harness.set_settings(broken);
    assert_eq!(
        codes(&harness.render(camera)),
        ["A0001"],
        "and again after it was fixed in between"
    );
}

#[test]
fn a_camera_outside_the_atmosphere_is_clamped_and_reported_once() {
    let mut harness = harness(SkySettings::default(), false);
    let camera = Camera::look_at(Vec3::new(0.0, 500_000.0, 0.0), Vec3::ZERO, 1.0, 1.3);
    assert_eq!(codes(&harness.render(camera)), ["A0003"]);
    assert_eq!(codes(&harness.render(camera)), Vec::<&str>::new());
}
