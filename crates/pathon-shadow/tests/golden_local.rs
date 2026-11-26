#![cfg(feature = "gpu-tests")]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "test images are small and their coordinates fit every integer type"
)]
// M4-07 extension: the shadows of point and spot lights through a real renderer. A feature written in the test draws the
// world and writes either the visibility `local_shadow_factor` returns for a light (so that the assertions measure the
// shadow and nothing else) or N.L times it. The numbers are the point; the golden image `shadows_local` is what a human
// looks at. `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

mod support;

use pathon_api::glam::{Quat, Vec3, Vec4};
use pathon_api::rhi::{Access, Backend, DeviceApi, DeviceRequest, Features, Format, PhysicalSize};
use pathon_api::{Camera, Severity};
use pathon_core::{FrameInput, RenderError, Renderer, TargetId, ViewInput};
use pathon_golden::{GoldenImage, Tolerance, announce_adapter, assert_golden, gpu_or_skip, read_texture};
use pathon_shadow::{LocalShadowLight, LocalShadowReport, LocalShadowShape, ShadowFilter, ShadowPack, ShadowSettings};
use support::local::{Box3, LocalShading, Mode, ShadingMode};
use support::{Host, Kind, LocalLights, Object};

const SIDE: u32 = 512;

fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_TEST_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        other => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
    }
}

fn settings() -> ShadowSettings {
    ShadowSettings {
        local_atlas_size: 4096,
        ..ShadowSettings::DESKTOP
    }
}

struct Scene {
    renderer: Renderer,
    target: TargetId,
}

fn build(test: &str, settings: &ShadowSettings, objects: Vec<Object>, lights: Vec<LocalShadowLight>) -> Option<Scene> {
    let built = Renderer::builder()
        .with_device_request(DeviceRequest {
            label: "local shadow test",
            backend: backend_from_environment(),
            ..DeviceRequest::default()
        })
        .with_feature(Host::new(objects))
        .with_pack(ShadowPack::with_settings(settings.clone()))
        .with_feature(LocalShading::new())
        .build();
    let mut renderer = gpu_or_skip(test, built, |error| {
        matches!(error, RenderError::Rhi(pathon_api::rhi::RhiError::NoAdapter(_)))
    })?;
    announce_adapter(test, renderer.device().caps());
    renderer.shared_mut().insert(LocalLights {
        lights,
        with_sun: false,
    });
    renderer.shared_mut().insert(ShadingMode(Mode::Visibility(0)));
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(SIDE, SIDE), Format::Rgba8Unorm)
        .expect("target");
    Some(Scene { renderer, target })
}

impl Scene {
    fn render(&mut self, camera: Camera) -> GoldenImage {
        let input = FrameInput {
            views: vec![ViewInput {
                target: self.target,
                camera,
            }],
            ..FrameInput::default()
        };
        let report = self.renderer.render_frame(&input).expect("frame");
        let errors: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        let texture = self.renderer.target_texture(self.target).expect("offscreen texture");
        read_texture(&**self.renderer.device(), texture, Access::SAMPLED).expect("readback")
    }

    fn set_lights(&mut self, lights: Vec<LocalShadowLight>) {
        self.renderer.shared_mut().insert(LocalLights {
            lights,
            with_sun: false,
        });
    }

    fn set_mode(&mut self, mode: Mode) {
        self.renderer.shared_mut().insert(ShadingMode(mode));
    }

    fn report(&self) -> LocalShadowReport {
        self.renderer
            .shared()
            .get::<LocalShadowReport>()
            .expect("the pack reported")
            .clone()
    }
}

fn spot(key: u64, at: Vec3, towards: Vec3, outer: f32) -> LocalShadowLight {
    LocalShadowLight {
        key,
        light_index: u32::try_from(key).expect("small key"),
        shape: LocalShadowShape::Spot { outer_angle: outer },
        position: at,
        direction: (towards - at).normalize(),
        range: 40.0,
        intensity: 1000.0,
    }
}

fn point(key: u64, at: Vec3) -> LocalShadowLight {
    LocalShadowLight {
        shape: LocalShadowShape::Point,
        ..spot(key, at, at + Vec3::NEG_Y, 0.5)
    }
}

// ---- reading the image back in world terms ----

fn project(camera: &Camera, point: Vec3) -> Option<(u32, u32)> {
    let clip = camera.view_projection() * Vec4::new(point.x, point.y, point.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let (x, y) = (
        (clip.x / clip.w * 0.5 + 0.5) * SIDE as f32,
        (0.5 - clip.y / clip.w * 0.5) * SIDE as f32,
    );
    (x >= 0.0 && y >= 0.0 && x < SIDE as f32 && y < SIDE as f32).then_some((x as u32, y as u32))
}

/// The visibility the image holds at a world point (the visibility mode writes it as the grey level).
fn visibility(image: &GoldenImage, camera: &Camera, point: Vec3) -> Option<f32> {
    let (x, y) = project(camera, point)?;
    Some(f32::from(image.pixel(x, y)[1]) / 255.0)
}

fn camera_over(eye: Vec3, target: Vec3) -> Camera {
    Camera::look_at(eye, target, 60.0_f32.to_radians(), 1.0)
}

// ---- spot light ----

const FLOOR: Vec3 = Vec3::new(400.0, 1.0, 400.0);

fn floor() -> Object {
    Object::new(Kind::Plane, Vec3::ZERO, FLOOR)
}

fn spot_scene() -> (Vec<Object>, Vec<LocalShadowLight>) {
    (
        vec![floor(), Object::new(Kind::Cube, Vec3::new(0.0, 0.5, 0.0), Vec3::ONE)],
        vec![spot(0, Vec3::new(1.5, 4.0, 0.0), Vec3::ZERO, 0.7)],
    )
}

#[test]
fn a_spot_light_shadows_the_floor_behind_a_cube_and_nothing_else() {
    let test = "a_spot_light_shadows_the_floor_behind_a_cube_and_nothing_else";
    let (objects, lights) = spot_scene();
    let Some(mut scene) = build(test, &settings(), objects, lights) else {
        return;
    };
    let camera = camera_over(Vec3::new(0.0, 6.0, 5.0), Vec3::new(0.0, 0.0, 0.0));
    let image = scene.render(camera);
    let at = |x: f32, z: f32| visibility(&image, &camera, Vec3::new(x, 0.0, z)).expect("on screen");

    // The shadow of the cube falls to -x, from the wall at x = -0.5 to about -1.17 for the top edge.
    let shadowed = [at(-0.9, 0.0), at(-0.7, 0.2), at(-0.8, -0.2)];
    let lit = [at(2.0, 0.5), at(1.0, -1.5), at(0.0, 1.8), at(-0.3, -1.4)];
    eprintln!("in shadow: {shadowed:?}, lit: {lit:?}");
    assert!(shadowed.iter().all(|value| *value < 0.05), "{shadowed:?}");
    assert!(lit.iter().all(|value| *value > 0.97), "{lit:?}");
    // The cube's own faces: the one towards the light is lit, the one away is not.
    let side = |x: f32| visibility(&image, &camera, Vec3::new(x, 0.5, 0.0)).expect("on screen");
    assert!(side(0.5) > 0.9, "the lit face of the cube");
}

#[test]
fn there_is_no_acne_on_the_floor_at_a_grazing_angle() {
    let test = "there_is_no_acne_on_the_floor_at_a_grazing_angle";
    // The light skims the floor at 14 degrees: the angle at which depth bias is hardest to get right.
    let lights = vec![spot(0, Vec3::new(-6.0, 1.5, 0.0), Vec3::new(2.0, 0.0, 0.0), 0.5)];
    let Some(mut scene) = build(test, &settings(), vec![floor()], lights) else {
        return;
    };
    let camera = camera_over(Vec3::new(2.0, 8.0, 3.0), Vec3::new(2.0, 0.0, 0.0));
    let image = scene.render(camera);

    let mut worst = (1.0_f32, Vec3::ZERO);
    let mut count = 0;
    for x in 0..40 {
        for z in -8..=8 {
            let point = Vec3::new(-1.0 + x as f32 * 0.2, 0.0, z as f32 * 0.2);
            let Some(value) = visibility(&image, &camera, point) else {
                continue;
            };
            count += 1;
            if value < worst.0 {
                worst = (value, point);
            }
        }
    }
    eprintln!(
        "{count} floor points under a 14 degree light, worst visibility {} at {}",
        worst.0, worst.1
    );
    assert!(count > 400);
    assert!(worst.0 >= 0.97, "acne at {}: visibility {}", worst.1, worst.0);
}

#[test]
fn the_shadow_reaches_the_foot_of_the_cube() {
    let test = "the_shadow_reaches_the_foot_of_the_cube";
    let (objects, lights) = spot_scene();
    let Some(mut scene) = build(test, &settings(), objects, lights) else {
        return;
    };
    let camera = camera_over(Vec3::new(-2.5, 3.0, 3.0), Vec3::new(-0.7, 0.0, 0.0));
    let image = scene.render(camera);

    // Distances from the wall of the cube at x = -0.5, along the floor, on the side the shadow falls to.
    let gaps: Vec<(f32, f32)> = [0.01_f32, 0.03, 0.05, 0.10, 0.2]
        .iter()
        .map(|gap| {
            (
                *gap,
                visibility(&image, &camera, Vec3::new(-0.5 - gap, 0.0, 0.0)).expect("on screen"),
            )
        })
        .collect();
    eprintln!("visibility at 1, 3, 5, 10, 20 cm from the wall: {gaps:?}");
    assert!(
        gaps[1].1 < 0.5,
        "the shadow starts further than 3 cm from the wall: {gaps:?}"
    );
    assert!(
        gaps[2].1 < 0.2,
        "the shadow starts further than 5 cm from the wall: {gaps:?}"
    );
    assert!(gaps[3].1 < 0.05, "no peter-panning at 10 cm: {gaps:?}");
}

// ---- point light: six faces, no seam ----

struct Room {
    objects: Vec<Object>,
    boxes: Vec<Box3>,
}

// An 8 x 6 x 8 m room (planes facing inwards) with three cubes; the first stands on the diagonal between two cube
// faces of a light at the middle of the room, the others elsewhere.
fn room() -> Room {
    let wall = |centre: Vec3, normal: Vec3| {
        Object::new(Kind::Plane, centre, Vec3::new(8.0, 1.0, 8.0)).rotated(Quat::from_rotation_arc(Vec3::Y, normal))
    };
    let cubes = [
        (Vec3::new(2.0, 3.0, 2.0), 1.0),
        (Vec3::new(-1.5, 4.5, 0.5), 0.8),
        (Vec3::new(0.5, 1.2, -2.2), 0.8),
    ];
    let mut objects = vec![
        wall(Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
        wall(Vec3::new(0.0, 6.0, 0.0), Vec3::NEG_Y),
        wall(Vec3::new(4.0, 3.0, 0.0), Vec3::NEG_X),
        wall(Vec3::new(-4.0, 3.0, 0.0), Vec3::X),
        wall(Vec3::new(0.0, 3.0, 4.0), Vec3::NEG_Z),
        wall(Vec3::new(0.0, 3.0, -4.0), Vec3::Z),
    ];
    let mut boxes = Vec::new();
    for (centre, size) in cubes {
        objects.push(Object::new(Kind::Cube, centre, Vec3::splat(size)));
        boxes.push(Box3::around(centre, Vec3::splat(size * 0.5)));
    }
    Room { objects, boxes }
}

const LIGHT: Vec3 = Vec3::new(0.0, 3.0, 0.0);
const EYE: Vec3 = Vec3::new(-0.8, 2.2, 0.9);

// The camera at `EYE` looking along an axis with a 90 degree field of view: six of them see the whole room.
fn axis_camera(forward: Vec3) -> Camera {
    let up = if forward.y.abs() > 0.5 { Vec3::Z } else { Vec3::Y };
    Camera {
        world_to_view: pathon_api::glam::camera::rh::view::look_to_mat4(EYE, forward, up),
        projection: pathon_api::glam::camera::rh::proj::directx::perspective_infinite_reverse(
            std::f32::consts::FRAC_PI_2,
            1.0,
            0.05,
        ),
        position: EYE,
    }
}

fn axis_of(offset: Vec3) -> Vec3 {
    let abs = offset.abs();
    if abs.x >= abs.y && abs.x >= abs.z {
        Vec3::X * offset.x.signum()
    } else if abs.y >= abs.z {
        Vec3::Y * offset.y.signum()
    } else {
        Vec3::Z * offset.z.signum()
    }
}

const AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

/// The points of the room's six surfaces the camera sees, on a grid, each with the visibility measured in the image of
/// its axis camera.
struct Measured {
    point: Vec3,
    expected_lit: bool,
    visibility: f32,
}

fn surface_points(grid: u32) -> Vec<Vec3> {
    let mut points = Vec::new();
    let steps = |index: u32| (index as f32 + 0.5) / grid as f32;
    for a in 0..grid {
        for b in 0..grid {
            let (u, v) = (steps(a) * 7.9 - 3.95, steps(b) * 7.9 - 3.95);
            let height = steps(b) * 5.6 + 0.2;
            points.push(Vec3::new(u, 0.0, v));
            points.push(Vec3::new(u, 6.0, v));
            points.push(Vec3::new(3.999, height, u));
            points.push(Vec3::new(-3.999, height, u));
            points.push(Vec3::new(u, height, 3.999));
            points.push(Vec3::new(u, height, -3.999));
        }
    }
    points
}

// What the geometry says about a receiver: lit, in shadow, or too close to a shadow edge to tell (the cubes grown and
// shrunk by 6 cm disagree).
fn expected(boxes: &[Box3], point: Vec3) -> Option<bool> {
    let blocked = |grow: f32| boxes.iter().any(|b| b.grown(grow).blocks(LIGHT, point));
    let (outer, inner) = (blocked(0.06), blocked(-0.06));
    (outer == inner).then_some(!outer)
}

fn measure_room(scene: &mut Scene, room: &Room, grid: u32) -> Vec<Measured> {
    let images: Vec<(Camera, GoldenImage)> = AXES
        .iter()
        .map(|axis| {
            let camera = axis_camera(*axis);
            let image = scene.render(camera);
            (camera, image)
        })
        .collect();
    let mut measured = Vec::new();
    for point in surface_points(grid) {
        let Some(expected_lit) = expected(&room.boxes, point) else {
            continue;
        };
        // The receiver must be seen from the eye: a cube in the way shows the cube.
        if room.boxes.iter().any(|b| b.grown(0.05).blocks(EYE, point)) {
            continue;
        }
        let axis = axis_of(point - EYE);
        let index = AXES.iter().position(|candidate| *candidate == axis).expect("an axis");
        let (camera, image) = &images[index];
        if let Some(value) = visibility(image, camera, point) {
            measured.push(Measured {
                point,
                expected_lit,
                visibility: value,
            });
        }
    }
    measured
}

fn room_scene(test: &str, settings: &ShadowSettings, light: LocalShadowLight) -> Option<(Scene, Room)> {
    let room = room();
    let scene = build(test, settings, room.objects.clone(), vec![light])?;
    Some((scene, room))
}

#[test]
fn a_point_light_shadows_every_wall_of_a_room_without_a_seam_or_acne() {
    let test = "a_point_light_shadows_every_wall_of_a_room_without_a_seam_or_acne";
    let Some((mut scene, room)) = room_scene(test, &settings(), point(0, LIGHT)) else {
        return;
    };

    let measured = measure_room(&mut scene, &room, 40);

    let (lit, dark): (Vec<_>, Vec<_>) = measured.iter().partition(|sample| sample.expected_lit);
    let leaks: Vec<_> = dark.iter().filter(|sample| sample.visibility > 0.15).collect();
    let acne: Vec<_> = lit.iter().filter(|sample| sample.visibility < 0.9).collect();
    eprintln!(
        "{} receivers: {} lit, {} in shadow; {} leaks, {} acne; report {:?}",
        measured.len(),
        lit.len(),
        dark.len(),
        leaks.len(),
        acne.len(),
        scene.report()
    );
    for sample in leaks.iter().chain(&acne).take(12) {
        eprintln!(
            "  at {} expected lit {} measured {}",
            sample.point, sample.expected_lit, sample.visibility
        );
    }
    assert!(
        dark.len() > 150 && lit.len() > 6000,
        "the room has both kinds of receiver"
    );
    assert!(leaks.is_empty(), "light leaks into shadow at {} points", leaks.len());
    assert!(
        acne.is_empty(),
        "shadow on lit surfaces (acne) at {} points",
        acne.len()
    );
}

#[test]
fn the_shadow_of_a_cube_on_the_seam_between_two_faces_is_whole() {
    let test = "the_shadow_of_a_cube_on_the_seam_between_two_faces_is_whole";
    let Some((mut scene, room)) = room_scene(test, &settings(), point(0, LIGHT)) else {
        return;
    };
    let measured = measure_room(&mut scene, &room, 56);

    // The first cube stands at (2, 3, 2), on the plane x = z; its shadow on the walls x = 4 and z = 4 straddles the corner
    // where those faces of the light's map meet. Receivers within 0.8 m of the diagonal plane that the geometry puts in
    // shadow must all be dark.
    let near_seam: Vec<_> = measured
        .iter()
        .filter(|sample| (sample.point.x - sample.point.z).abs() < 0.8 && sample.point.x > 0.0 && !sample.expected_lit)
        .collect();
    let broken: Vec<_> = near_seam.iter().filter(|sample| sample.visibility > 0.15).collect();
    eprintln!(
        "{} shadowed receivers near the seam, {} lit",
        near_seam.len(),
        broken.len()
    );
    assert!(
        near_seam.len() > 40,
        "the test has receivers on the seam: {}",
        near_seam.len()
    );
    assert!(
        broken.is_empty(),
        "the shadow has a gap at the seam: {:?}",
        broken.iter().map(|s| s.point).collect::<Vec<_>>()
    );
}

// ---- cache ----

#[test]
fn an_unchanged_frame_is_the_same_image_and_draws_nothing_and_a_moved_light_matches_a_fresh_render() {
    let test = "an_unchanged_frame_is_the_same_image_and_draws_nothing_and_a_moved_light_matches_a_fresh_render";
    let room = room();
    let near = point(0, LIGHT);
    let moved = point(0, LIGHT + Vec3::new(0.7, -0.4, 0.3));
    let Some(mut cached) = build(test, &settings(), room.objects.clone(), vec![near]) else {
        return;
    };
    let camera = axis_camera(Vec3::NEG_Z);
    cached.set_mode(Mode::Lit);
    let first = cached.render(camera);
    assert_eq!(cached.report().rendered_tiles, 6);
    let second = cached.render(camera);
    assert_eq!(cached.report().rendered_tiles, 0, "nothing changed");
    assert_eq!(
        first.as_rgba8(),
        second.as_rgba8(),
        "the cached atlas gives the same image"
    );

    cached.set_lights(vec![moved]);
    let after_move = cached.render(camera);
    assert_eq!(
        cached.report().rendered_tiles,
        6,
        "the light moved, so its maps are drawn again"
    );

    let mut fresh = build(test, &settings(), room.objects, vec![moved]).expect("same device");
    fresh.set_mode(Mode::Lit);
    let reference = fresh.render(camera);
    assert_eq!(
        after_move.as_rgba8(),
        reference.as_rgba8(),
        "a redrawn tile is as clean as a new one (the old content is cleared)"
    );
}

#[test]
fn indirect_and_one_by_one_draws_make_the_same_image() {
    let test = "local_indirect_and_one_by_one_draws_make_the_same_image";
    let room = room();
    let Some(mut scene) = build(test, &settings(), room.objects, vec![point(0, LIGHT)]) else {
        return;
    };
    if !scene.renderer.device().caps().has(Features::INDIRECT_FIRST_INSTANCE) {
        eprintln!("{test}: the device has no first-instance indirect draws, only one path exists");
        return;
    }
    let camera = axis_camera(Vec3::X);
    scene
        .renderer
        .shared_mut()
        .get_mut::<ShadowSettings>()
        .expect("settings")
        .use_indirect_draws = true;
    let indirect = scene.render(camera);
    scene
        .renderer
        .shared_mut()
        .get_mut::<pathon_shadow::ShadowScene>()
        .expect("scene")
        .invalidate_local_maps();
    scene
        .renderer
        .shared_mut()
        .get_mut::<ShadowSettings>()
        .expect("settings")
        .use_indirect_draws = false;
    let one_by_one = scene.render(camera);

    assert_eq!(scene.report().rendered_tiles, 6, "drawn again with the other path");
    assert_eq!(indirect.as_rgba8(), one_by_one.as_rgba8());
}

#[test]
fn a_light_outside_the_view_renders_no_map_on_the_gpu_either() {
    let test = "a_light_outside_the_view_renders_no_map_on_the_gpu_either";
    let Some(mut scene) = build(
        test,
        &settings(),
        vec![floor()],
        vec![point(0, Vec3::new(0.0, 3.0, 60.0))],
    ) else {
        return;
    };

    scene.render(camera_over(Vec3::new(0.0, 4.0, 5.0), Vec3::ZERO));

    let report = scene.report();
    assert_eq!(
        (report.out_of_view, report.rendered_tiles, report.lights.len()),
        (1, 0, 0)
    );
}

// ---- softness ----

#[test]
fn softer_filters_widen_the_edge_of_a_local_shadow() {
    let test = "softer_filters_widen_the_edge_of_a_local_shadow";
    let (objects, lights) = spot_scene();
    let Some(mut scene) = build(test, &settings(), objects, lights) else {
        return;
    };
    let camera = camera_over(Vec3::new(-0.3, 5.0, 0.2), Vec3::new(-0.3, 0.0, 0.0));
    let mut widths = Vec::new();
    for filter in [ShadowFilter::Hard, ShadowFilter::Pcf3, ShadowFilter::Pcf5] {
        scene
            .renderer
            .shared_mut()
            .get_mut::<ShadowSettings>()
            .expect("settings")
            .filter = filter;
        let image = scene.render(camera);
        let row = |z: f32| -> Vec<f32> {
            (0..SIDE)
                .map(|x| {
                    f32::from(image.pixel(x, project(&camera, Vec3::new(-1.0, 0.0, z)).expect("on screen").1)[1])
                        / 255.0
                })
                .collect()
        };
        let values = row(0.0);
        widths.push(values.iter().filter(|value| **value > 0.1 && **value < 0.9).count());
    }
    eprintln!("edge width in pixels (hard, pcf3, pcf5): {widths:?}");
    assert!(widths[0] < widths[1], "pcf3 is softer than hard: {widths:?}");
    assert!(
        widths[1] <= widths[2] + 1,
        "pcf5 is at least as soft as pcf3: {widths:?}"
    );
}

// ---- a picture ----

fn three_lights() -> (Vec<Object>, Vec<LocalShadowLight>) {
    let objects = vec![
        floor(),
        Object::new(Kind::Cube, Vec3::new(-2.0, 0.75, 0.0), Vec3::new(1.5, 1.5, 1.5)),
        Object::new(Kind::Sphere, Vec3::new(1.0, 0.8, -0.5), Vec3::splat(1.6)),
        Object::new(Kind::Cube, Vec3::new(3.0, 0.5, 1.0), Vec3::new(1.0, 1.0, 2.0)),
        Object::new(Kind::Cube, Vec3::new(0.0, 2.2, 1.5), Vec3::new(2.5, 0.2, 0.2)),
    ];
    let lights = vec![
        spot(0, Vec3::new(-4.0, 4.5, 3.0), Vec3::new(-1.0, 0.0, 0.0), 0.55),
        point(1, Vec3::new(2.5, 3.0, -2.5)),
        point(2, Vec3::new(0.5, 3.8, 3.5)),
    ];
    (objects, lights)
}

#[test]
fn three_lights_with_shadows_make_the_golden_image() {
    let test = "three_lights_with_shadows_make_the_golden_image";
    let (objects, lights) = three_lights();
    let Some(mut scene) = build(test, &settings(), objects, lights) else {
        return;
    };
    let camera = Camera::look_at(
        Vec3::new(0.5, 6.0, 8.5),
        Vec3::new(0.0, 0.3, 0.0),
        55.0_f32.to_radians(),
        1.0,
    );
    // The spot light's own shadow of the cube lands on the floor at about (-1.6, 0, -0.6).
    scene.set_mode(Mode::Visibility(0));
    let visibility_image = scene.render(camera);
    let spot_at = |x: f32, z: f32| visibility(&visibility_image, &camera, Vec3::new(x, 0.0, z)).expect("on screen");
    let (behind_cube, beside_cube) = (spot_at(-1.5, -0.6), spot_at(-0.2, 1.5));
    eprintln!("the spot light's visibility behind the cube {behind_cube}, beside it {beside_cube}");
    assert!(behind_cube < 0.1 && beside_cube > 0.9, "{behind_cube} vs {beside_cube}");

    scene.set_mode(Mode::Lit);
    let image = scene.render(camera);
    let report = scene.report();
    assert_eq!(report.lights.len(), 3, "{report:?}");

    assert_golden("shadows_local", &image, Tolerance::cross_backend());
}
