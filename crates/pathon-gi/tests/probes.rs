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

    #[test]
    fn every_texel_of_both_atlases_is_finite() {
        let Some(mut rig) = rig("finite", settings()) else {
            return;
        };
        furnace_room(&mut rig, 0.9, 2.0);
        rig.add_light(&pathon_scene::Light::point(
            pathon_api::glam::Affine3A::from_translation(Vec3::new(0.0, 2.0, 0.0)),
            4000.0,
        ));

        rig.frames(40);

        let atlases = rig.atlases();
        for texel in atlases.irradiance.iter().chain(&atlases.visibility) {
            assert!(texel.iter().all(|value| value.is_finite()), "{texel:?}");
        }
        assert!(
            atlases.irradiance.iter().any(|texel| texel[0] > 0.0),
            "the atlas is not empty"
        );
    }

    #[test]
    fn a_dark_room_behind_a_wall_does_not_borrow_the_light_of_the_bright_one() {
        let Some(mut rig) = rig("leak", settings()) else {
            return;
        };
        // Room A glows, room B is a dark room 0.4 m behind it.
        rig.add_room(
            Vec3::new(-4.0, 0.0, -2.0),
            Vec3::new(-0.2, 3.0, 2.0),
            [Wall::glowing(0.5, 1.0); 6],
        );
        rig.add_room(
            Vec3::new(0.2, 0.0, -2.0),
            Vec3::new(4.0, 3.0, 2.0),
            [Wall::grey(0.2); 6],
        );
        rig.frames(90);

        let normal_into_b = Vec3::X;
        let normal_into_a = Vec3::NEG_X;
        let beside_the_wall_in_b: Vec<(Vec3, Vec3)> = (0..6)
            .map(|step| (Vec3::new(0.45 + 0.1 * step as f32, 1.5, 0.0), normal_into_b))
            .collect();
        let beside_the_wall_in_a: Vec<(Vec3, Vec3)> = (0..6)
            .map(|step| (Vec3::new(-0.45 - 0.1 * step as f32, 1.5, 0.0), normal_into_a))
            .collect();
        let dark = rig.sample(&beside_the_wall_in_b);
        let bright = rig.sample(&beside_the_wall_in_a);

        for (index, sampled) in dark.iter().enumerate() {
            eprintln!("dark room, {index}: {sampled:?}");
            assert!(
                sampled.irradiance.x < 0.06,
                "light leaked into the dark room: {sampled:?}"
            );
        }
        for (index, sampled) in bright.iter().enumerate() {
            eprintln!("bright room, {index}: {sampled:?}");
            assert!(
                (sampled.irradiance.x / 2.0 - 1.0).abs() < 0.12,
                "the lit room is not lit: {sampled:?}"
            );
        }
    }

    fn cornell_walls() -> [Wall; 6] {
        let grey = Wall::grey(0.7);
        let (red, green) = (
            Wall {
                albedo: [0.7, 0.1, 0.1],
                emission: [0.0; 3],
            },
            Wall {
                albedo: [0.1, 0.6, 0.1],
                emission: [0.0; 3],
            },
        );
        // +x green, -x red, then ceiling, floor, back and front.
        [green, red, grey, grey, grey, grey]
    }

    fn cornell(rig: &mut Rig) -> pathon_gi::reference::Room {
        let (min, max) = (Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 4.0, 2.0));
        let walls = cornell_walls();
        rig.add_room(min, max, walls);
        let lumens = 40_000.0;
        let position = Vec3::new(0.0, 3.5, 0.0);
        rig.add_light(
            &pathon_scene::Light::point(pathon_api::glam::Affine3A::from_translation(position), lumens)
                .with_range(100.0),
        );
        pathon_gi::reference::Room {
            min,
            max,
            albedo: walls.map(|wall| Vec3::from(wall.albedo)),
            emission: [Vec3::ZERO; 6],
            lights: vec![pathon_gi::reference::PointLight {
                position,
                color_intensity: Vec3::splat(lumens / (4.0 * std::f32::consts::PI)),
            }],
        }
    }

    #[test]
    fn a_cornell_box_agrees_with_a_monte_carlo_estimate() {
        let Some(mut rig) = rig("cornell", settings()) else {
            return;
        };
        let room = cornell(&mut rig);
        rig.frames(160);

        let receivers = [
            (Vec3::new(0.0, 0.3, 0.0), Vec3::Y),
            (Vec3::new(-1.4, 0.3, 0.5), Vec3::Y),
            (Vec3::new(1.4, 0.3, -0.5), Vec3::Y),
            (Vec3::new(0.0, 2.0, -1.65), Vec3::Z),
            (Vec3::new(-1.65, 2.0, 0.0), Vec3::X),
            (Vec3::new(1.65, 2.0, 0.0), Vec3::NEG_X),
            (Vec3::new(0.0, 3.6, 0.5), Vec3::NEG_Y),
        ];
        let sampled = rig.sample(&receivers);
        let (mut bias_sum, mut worst) = (0.0_f32, 0.0_f32);
        for (index, ((position, normal), got)) in receivers.iter().zip(&sampled).enumerate() {
            let want = room.mean_cosine_radiance(*position, *normal, 200_000, 8, 7 + index as u64);
            let relative = (got.irradiance - want) / want.max(Vec3::splat(1.0e-3));
            let error = relative.abs().max_element();
            eprintln!(
                "cornell receiver {index}: GI {:?} reference {:?} relative error {:?}",
                got.irradiance, want, relative
            );
            bias_sum += (got.irradiance.length() / want.length()) - 1.0;
            worst = worst.max(error);
        }
        let bias = bias_sum / receivers.len() as f32;
        eprintln!("cornell: mean relative bias {bias:+.3}, worst channel error {worst:.3}");
        // The conservative voxel walls stand up to a cell (10 cm) inside the room: the surfaces are nearer the light, so
        // the field is a few percent brighter than the ideal room.
        assert!(bias > 0.0 && bias < 0.10, "bias {bias}");
        assert!(worst < 0.18, "worst channel error {worst}");
    }

    #[test]
    fn a_static_scene_does_not_flicker() {
        let Some(mut rig) = rig("steady", settings()) else {
            return;
        };
        cornell(&mut rig);
        rig.frames(120);
        let before = rig.atlases();
        rig.frames(30);
        let after = rig.atlases();

        let mut sum = 0.0_f32;
        let mut count = 0;
        let mut worst = 0.0_f32;
        for index in 0..before.probe_count() {
            if before.is_blank(index) {
                continue;
            }
            let (a, b) = (before.mean_irradiance(index), after.mean_irradiance(index));
            let change = (a - b).length() / a.length().max(1.0e-3);
            sum += change;
            count += 1;
            worst = worst.max(change);
        }
        let mean = sum / count as f32;
        eprintln!("steady: mean change over 30 frames {mean:.4}, worst probe {worst:.4}, {count} probes");
        assert!(mean < 0.01, "mean change {mean}");
        assert!(worst < 0.08, "worst change {worst}");
    }
}
