// The slots of the prepass against what the scene says they must be: depth (reverse-Z), oct-encoded world normals,
// motion in pixels (moving object, moving camera), material ids, roughness and albedo on demand and the Hi-Z pyramid
// against a CPU reference. The null half validates every barrier and binding; the GPU half reads the slots back.

mod support;

use pathon_api::Slot;
use pathon_api::glam::{Vec3, Vec4Swizzles};
use pathon_pbr::PbrPack;
use pathon_rhi::{Format, NullDevice};
use support::scene::{Harness, add_content, camera, instance, null_harness, scale_translate};

const SIZE: u32 = 128;

struct Spheres {
    floor_id: u32,
    ball_id: u32,
    center: Vec3,
    radius: f32,
}

fn populate(harness: &mut Harness, ball_center: Vec3) -> (Spheres, pathon_scene::InstanceId) {
    let content = add_content(&mut harness.renderer, false);
    instance(
        &mut harness.scene,
        content.ground,
        content.floor_material,
        scale_translate(4.0, Vec3::new(0.0, -1.0, 0.0)),
    );
    let radius = 0.9;
    let ball = instance(
        &mut harness.scene,
        content.ball,
        content.plastic_material,
        scale_translate(radius, ball_center),
    );
    (
        Spheres {
            floor_id: content.floor_material.index(),
            ball_id: content.plastic_material.index(),
            center: ball_center,
            radius,
        },
        ball,
    )
}

#[test]
fn the_prepass_validates_on_the_null_device() {
    let _ = NullDevice::new();
    let mut harness = null_harness(SIZE, Format::Rgba8Unorm, |builder| builder.with_pack(PbrPack::new()));
    populate(&mut harness, Vec3::new(-1.3, -0.1, 0.0));

    let diagnostics = harness.render(camera(SIZE));

    let errors: Vec<_> = diagnostics.iter().filter(|d| d.severity == pathon_api::Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_api::glam::{Mat4, Vec2};
    use pathon_api::rhi::PhysicalSize;
    use pathon_api::{Camera, Severity};
    use pathon_pbr::reference::{depth_pyramid, motion_of, oct_decode};
    use pathon_pbr::pyramid_levels;
    use support::probe::{Probe, ProbeReader};
    use support::scene::harness;

    use super::*;

    const CENTER: Vec3 = Vec3::new(-1.3, -0.1, 0.0);

    struct Readers {
        depth: ProbeReader,
        normals: ProbeReader,
        motion: ProbeReader,
        material: ProbeReader,
        roughness: ProbeReader,
        albedo: ProbeReader,
        pyramid: ProbeReader,
    }

    fn prepass_harness(test: &str) -> Option<(Harness, Readers)> {
        let size = (SIZE, SIZE);
        let mut readers = None;
        let harness = harness(test, SIZE, Format::Rgba8Unorm, |builder| {
            let (depth, depth_reader) = Probe::new(Slot::DEPTH, size, 1, 4);
            let (normals, normals_reader) = Probe::new(Slot::NORMALS, size, 1, 4);
            let (motion, motion_reader) = Probe::new(Slot::MOTION, size, 1, 4);
            let (material, material_reader) = Probe::new(Slot::MATERIAL_ID, size, 1, 4);
            let (roughness, roughness_reader) = Probe::new(Slot::ROUGHNESS, size, 1, 1);
            let (albedo, albedo_reader) = Probe::new(Slot::ALBEDO, size, 1, 4);
            let (pyramid, pyramid_reader) = Probe::new(Slot::DEPTH_PYRAMID, size, pyramid_levels(SIZE, SIZE), 4);
            readers = Some(Readers {
                depth: depth_reader,
                normals: normals_reader,
                motion: motion_reader,
                material: material_reader,
                roughness: roughness_reader,
                albedo: albedo_reader,
                pyramid: pyramid_reader,
            });
            builder
                .with_pack(PbrPack::new())
                .with_feature(depth)
                .with_feature(normals)
                .with_feature(motion)
                .with_feature(material)
                .with_feature(roughness)
                .with_feature(albedo)
                .with_feature(pyramid)
        })?;
        Some((harness, readers?))
    }

    fn ray(camera: &Camera, pixel: Vec2) -> (Vec3, Vec3) {
        let ndc = Vec2::new(
            (pixel.x + 0.5) / SIZE as f32 * 2.0 - 1.0,
            1.0 - (pixel.y + 0.5) / SIZE as f32 * 2.0,
        );
        let inverse = camera.view_projection().inverse();
        let near = inverse * ndc.extend(1.0).extend(1.0);
        let far = inverse * ndc.extend(0.5).extend(1.0);
        let (near, far) = (near.xyz() / near.w, far.xyz() / far.w);
        (near, (far - near).normalize())
    }

    fn hit_sphere(origin: Vec3, direction: Vec3, center: Vec3, radius: f32) -> Option<Vec3> {
        let offset = origin - center;
        let b = offset.dot(direction);
        let discriminant = b * b - (offset.dot(offset) - radius * radius);
        (discriminant > 0.0).then(|| origin + direction * (-b - discriminant.sqrt()))
    }

    fn hit_floor(origin: Vec3, direction: Vec3) -> Option<Vec3> {
        let t = (-1.0 - origin.y) / direction.y;
        (t > 0.0).then(|| origin + direction * t)
    }

    // The world point under a pixel and what it belongs to, from the scene's geometry. `None` where the polygonal sphere or
    // the edge of the floor could disagree with the analytic shape: grazing views of the ball, its silhouette, the
    // floor next to it and beyond the quad.
    fn expected_point(camera: &Camera, spheres: &Spheres, pixel: Vec2) -> Option<(Vec3, bool)> {
        let (origin, direction) = ray(camera, pixel);
        if let Some(point) = hit_sphere(origin, direction, spheres.center, spheres.radius) {
            let normal = (point - spheres.center).normalize();
            return (normal.dot(-direction) > 0.5).then_some((point, true));
        }
        if hit_sphere(origin, direction, spheres.center, spheres.radius + 0.15).is_some() {
            return None;
        }
        let point = hit_floor(origin, direction)?;
        (point.x.abs() < 3.9 && point.z.abs() < 3.9).then_some((point, false))
    }

    fn assert_no_errors(diagnostics: &[pathon_api::Diagnostic]) {
        let errors: Vec<_> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn depth_normals_and_material_ids_are_what_the_scene_says() {
        let Some((mut harness, readers)) = prepass_harness("depth_normals_and_material_ids_are_what_the_scene_says") else {
            return;
        };
        let (spheres, _) = populate(&mut harness, CENTER);
        let camera = camera(SIZE);
        assert_no_errors(&harness.render(camera));
        let device = harness.renderer.device().clone();
        let device: &dyn pathon_rhi::DeviceApi = &*device;
        let (depth, normals, materials) = (
            readers.depth.f32s(device, 0),
            readers.normals.half_pairs(device, 0),
            readers.material.u32s(device, 0),
        );
        let view_projection = camera.view_projection();

        let mut checked = [0, 0];
        for y in (6..SIZE - 6).step_by(5) {
            for x in (6..SIZE - 6).step_by(5) {
                let pixel = Vec2::new(x as f32, y as f32);
                let Some((point, on_ball)) = expected_point(&camera, &spheres, pixel) else {
                    continue;
                };
                let index = (y * SIZE + x) as usize;
                let clip = view_projection * point.extend(1.0);
                let expected_depth = clip.z / clip.w;
                assert!(
                    (depth[index] - expected_depth).abs() < 2.0e-4 + 1.0e-3 * expected_depth,
                    "depth at ({x}, {y}): {} vs {expected_depth}",
                    depth[index]
                );
                let expected_normal = if on_ball { (point - spheres.center).normalize() } else { Vec3::Y };
                let decoded = oct_decode(Vec2::from(normals[index]));
                assert!(decoded.dot(expected_normal) > 0.998, "normal at ({x}, {y}): {decoded} vs {expected_normal}");
                let expected_material = if on_ball { spheres.ball_id } else { spheres.floor_id };
                assert_eq!(materials[index], expected_material, "material id at ({x}, {y})");
                checked[usize::from(on_ball)] += 1;
            }
        }
        assert!(checked[0] > 100 && checked[1] > 30, "too few pixels checked: {checked:?}");
        let sky = materials.iter().position(|id| *id == u32::MAX).expect("some pixel is sky");
        assert_eq!(depth[sky], 0.0, "reverse-Z is cleared to 0 where nothing was drawn");
        assert!(depth.iter().all(|value| (0.0..=1.0).contains(value)), "depth stays inside 0..=1");
    }

    #[test]
    fn roughness_and_albedo_are_written_on_request() {
        let Some((mut harness, readers)) = prepass_harness("roughness_and_albedo_are_written_on_request") else {
            return;
        };
        let (spheres, _) = populate(&mut harness, CENTER);
        let camera = camera(SIZE);
        assert_no_errors(&harness.render(camera));
        let device = harness.renderer.device().clone();
        let device: &dyn pathon_rhi::DeviceApi = &*device;
        let (roughness, albedo) = (readers.roughness.texels(device, 0), readers.albedo.texels(device, 0));

        let mut found = [false, false];
        for y in (6..SIZE - 6).step_by(5) {
            for x in (6..SIZE - 6).step_by(5) {
                let Some((_, on_ball)) = expected_point(&camera, &spheres, Vec2::new(x as f32, y as f32)) else {
                    continue;
                };
                let index = (y * SIZE + x) as usize;
                // Floor: base colour (0.55, 0.55, 0.5), roughness 0.55; plastic: (0.8, 0.12, 0.1), roughness 0.35.
                let (expected_roughness, expected_color) = if on_ball {
                    (0.35, [0.8, 0.12, 0.1])
                } else {
                    (0.55, [0.55, 0.55, 0.5])
                };
                let got = f32::from(roughness[index]) / 255.0;
                assert!((got - expected_roughness).abs() < 0.01, "roughness at ({x}, {y}): {got}");
                for (channel, expected) in expected_color.into_iter().enumerate() {
                    let got = f32::from(albedo[index * 4 + channel]) / 255.0;
                    assert!((got - expected).abs() < 0.01, "albedo {channel} at ({x}, {y}): {got} vs {expected}");
                }
                assert!(albedo[index * 4 + 3] < 3, "dielectrics store metallic 0 in alpha");
                found[usize::from(on_ball)] = true;
            }
        }
        assert_eq!(found, [true, true]);
    }

    fn motion_between(
        first: (Vec3, Camera),
        second: (Vec3, Camera),
        test: &str,
    ) -> Option<(Vec<[f32; 2]>, Spheres, Camera, Vec3)> {
        let (mut harness, readers) = prepass_harness(test)?;
        let (spheres, ball) = populate(&mut harness, first.0);
        assert_no_errors(&harness.render(first.1));
        let moved = second.0 - first.0;
        harness
            .scene
            .extract(|writer| writer.set_transform(ball, scale_translate(spheres.radius, second.0)))
            .expect("move");
        assert_no_errors(&harness.render(second.1));
        let device = harness.renderer.device().clone();
        let motion = readers.motion.half_pairs(&*device, 0);
        Some((
            motion,
            Spheres {
                center: second.0,
                ..spheres
            },
            second.1,
            moved,
        ))
    }

    fn check_motion(motion: &[[f32; 2]], spheres: &Spheres, current: &Camera, previous: &Camera, moved: Vec3) {
        let size = PhysicalSize::new(SIZE, SIZE);
        let (view_projection, previous_view_projection): (Mat4, Mat4) = (current.view_projection(), previous.view_projection());
        let mut checked = 0;
        let mut moving_checked = 0;
        for y in (6..SIZE - 6).step_by(4) {
            for x in (6..SIZE - 6).step_by(4) {
                let pixel = Vec2::new(x as f32, y as f32);
                let Some((point, on_ball)) = expected_point(current, spheres, pixel) else {
                    continue;
                };
                let previous_point = if on_ball { point - moved } else { point };
                let expected = motion_of(view_projection, previous_view_projection, point, previous_point, size);
                // The pixel centre is where the rasteriser evaluates, so project from the centre of the texel.
                let got = Vec2::from(motion[(y * SIZE + x) as usize]);
                assert!(
                    (got - expected).abs().max_element() < 0.5,
                    "motion at ({x}, {y}) on the {}: {got} vs {expected}",
                    if on_ball { "ball" } else { "floor" }
                );
                checked += 1;
                moving_checked += i32::from(on_ball && expected.length() > 1.0);
            }
        }
        assert!(checked > 200, "{checked} pixels checked");
        assert!(moving_checked > 0 || moved == Vec3::ZERO, "the moving ball was sampled");
    }

    #[test]
    fn motion_follows_a_moving_object_in_pixels() {
        let still = camera(SIZE);
        let Some((motion, spheres, current, moved)) = motion_between(
            (CENTER, still),
            (CENTER + Vec3::new(0.35, 0.1, -0.2), still),
            "motion_follows_a_moving_object_in_pixels",
        ) else {
            return;
        };

        check_motion(&motion, &spheres, &current, &still, moved);
        let on_ball = motion[(SIZE / 2 * SIZE + SIZE / 4) as usize];
        assert!(on_ball[0].abs() + on_ball[1].abs() > 1.0, "the ball moved by more than a pixel: {on_ball:?}");
    }

    #[test]
    fn motion_follows_a_moving_camera_in_pixels() {
        let first = camera(SIZE);
        let second = Camera::look_at(Vec3::new(0.4, 2.0, 6.0), Vec3::new(0.1, -0.4, 0.0), 40.0_f32.to_radians(), 1.0);
        let Some((motion, spheres, current, moved)) =
            motion_between((CENTER, first), (CENTER, second), "motion_follows_a_moving_camera_in_pixels")
        else {
            return;
        };

        check_motion(&motion, &spheres, &current, &first, moved);
        let floor = motion[(SIZE * 5 / 6 * SIZE + SIZE / 2) as usize];
        assert!(floor[0].abs() + floor[1].abs() > 1.0, "the camera moved by more than a pixel: {floor:?}");
    }

    #[test]
    fn the_depth_pyramid_matches_the_cpu_reference() {
        let Some((mut harness, readers)) = prepass_harness("the_depth_pyramid_matches_the_cpu_reference") else {
            return;
        };
        populate(&mut harness, CENTER);
        assert_no_errors(&harness.render(camera(SIZE)));
        let device = harness.renderer.device().clone();
        let device: &dyn pathon_rhi::DeviceApi = &*device;

        let depth = readers.depth.f32s(device, 0);
        let expected = depth_pyramid(SIZE, SIZE, &depth);

        assert_eq!(readers.pyramid.mips().len(), expected.len());
        for (mip, level) in expected.iter().enumerate() {
            let got = readers.pyramid.f32s(device, mip);
            assert_eq!(got.len(), level.texels.len());
            let differing = got.iter().zip(&level.texels).filter(|(a, b)| a != b).count();
            assert_eq!(differing, 0, "mip {mip} ({}x{}) differs from the reference in {differing} texels", level.width, level.height);
        }
        assert!(expected[0].texels.iter().any(|value| *value > 0.0), "something is in front of the far plane");
        assert_eq!(expected.last().expect("levels").texels, [0.0], "the top mip keeps the farthest depth: the cleared sky");
    }
}
