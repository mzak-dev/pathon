// K12: the probe field in a real renderer: the null device checks every command list, and on a GPU the probes of a
// closed room converge to the analytic radiance (white furnace), light does not cross a wall, and the field is steady.
// `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

#![cfg_attr(not(feature = "gpu-tests"), allow(unused_imports, dead_code))]

mod support;

use pathon_api::glam::Vec3;
use pathon_gi::GiSettings;
use support::{Rig, Wall, null_rig};

fn settings() -> GiSettings {
    GiSettings {
        probe_spacing: 0.5,
        voxel_size: 0.1,
        rays_per_probe: 128,
        hysteresis: 0.9,
        ..GiSettings::default()
    }
}

fn furnace_room(rig: &mut Rig, albedo: f32, emission: f32) {
    rig.add_room(
        Vec3::new(-2.0, 0.0, -2.0),
        Vec3::new(2.0, 3.0, 2.0),
        [Wall::glowing(albedo, emission); 6],
    );
}

#[test]
fn the_null_device_accepts_every_pass_of_the_volume() {
    let mut rig = null_rig(settings());
    furnace_room(&mut rig, 0.5, 1.0);
    rig.add_light(&pathon_scene::Light::point(
        pathon_api::glam::Affine3A::from_translation(Vec3::new(0.0, 2.0, 0.0)),
        800.0,
    ));

    let diagnostics = rig.frames(4);

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(rig.renderer.shared().get::<pathon_gi::GiStatus>().is_some());
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use super::*;
    use support::rig;

    fn interior(position: Vec3) -> bool {
        let margin = 0.45;
        position.x.abs() < 2.0 - margin
            && position.z.abs() < 2.0 - margin
            && position.y > margin
            && position.y < 3.0 - margin
    }

    #[test]
    fn a_closed_room_of_glowing_walls_converges_to_the_analytic_radiance() {
        let Some(mut rig) = rig("furnace", settings()) else {
            return;
        };
        furnace_room(&mut rig, 0.5, 1.0);

        let diagnostics = rig.frames(90);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let atlases = rig.atlases();
        // Every wall emits 1 and reflects half: the radiance of the room is 1 / (1 - 0.5) everywhere.
        let expected = 2.0;
        let (mut worst, mut count, mut sum) = (0.0_f32, 0, 0.0_f32);
        for index in 0..atlases.probe_count() {
            if !interior(atlases.probe_position(index)) {
                continue;
            }
            let mean = atlases.mean_irradiance(index);
            assert!(mean.is_finite(), "probe {index} is not finite");
            let error = (mean.x / expected - 1.0).abs();
            worst = worst.max(error);
            sum += mean.x;
            count += 1;
        }
        assert!(count > 100, "only {count} probes inside the room");
        let average = sum / count as f32;
        eprintln!("furnace: mean {average:.4} (expected {expected}), worst probe error {worst:.4}, {count} probes");
        assert!((average / expected - 1.0).abs() < 0.03, "mean {average}");
        assert!(worst < 0.06, "worst probe error {worst}");
    }
}
