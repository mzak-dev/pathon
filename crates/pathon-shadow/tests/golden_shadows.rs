#![cfg(feature = "gpu-tests")]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test images are small and their coordinates fit every integer type"
)]
// M4-07: cascaded shadow maps through a real renderer. One directional light, a floor, boxes, a sphere and a plane that
// faces the light, shaded with N.L times `shadow_factor_directional` by a feature written in the test (`pathon-pbr`
// does not exist yet). The numeric assertions are the point; the golden image `shadows_csm` is what a human looks at.
// `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

mod support;

use pathon_api::glam::{Quat, Vec3, Vec4};
use pathon_api::rhi::{Access, Backend, DeviceApi, DeviceRequest, Format, PhysicalSize};
use pathon_api::{Camera, Severity};
use pathon_core::{FrameInput, RenderError, Renderer, TargetId, ViewInput};
use pathon_golden::{GoldenImage, Tolerance, announce_adapter, assert_golden, gpu_or_skip, read_texture};
use pathon_shadow::{CascadeFit, ShadowFilter, ShadowFrame, ShadowPack, ShadowSettings};
use support::{ALBEDO, AMBIENT, Host, Kind, MapProbe, MapReadback, Object, Shading, read_map};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 384;
const RESOLUTION: u32 = 1024;
const SIDE: i32 = 1024;
const SUN: Vec3 = Vec3::new(-0.5, -0.45, -0.6);
const FACING_PLANE_CENTRE: Vec3 = Vec3::new(9.0, 3.0, -9.0);
const EYE: Vec3 = Vec3::new(-2.0, 6.0, 6.0);

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
        resolution: RESOLUTION,
        cascade_count: 4,
        max_distance: 80.0,
        ..ShadowSettings::DESKTOP
    }
}

// The plane that faces the sun: rotated so that its +Y normal points at the light.
fn facing_plane() -> Object {
    let rotation = Quat::from_rotation_arc(Vec3::Y, -SUN.normalize());
    Object::new(Kind::Plane, FACING_PLANE_CENTRE, Vec3::new(3.0, 1.0, 3.0)).rotated(rotation)
}

fn scene(floor_casts: bool) -> Vec<Object> {
    let floor = Object::new(Kind::Plane, Vec3::ZERO, Vec3::new(400.0, 1.0, 400.0));
    vec![
        if floor_casts { floor } else { floor.not_casting() },
        Object::new(Kind::Cube, Vec3::new(0.0, 1.0, -6.0), Vec3::splat(2.0)),
        Object::new(Kind::Cube, Vec3::new(0.0, 2.5, -6.0), Vec3::splat(1.0)),
        Object::new(Kind::Sphere, Vec3::new(-5.0, 1.0, -8.0), Vec3::splat(2.0)),
        facing_plane(),
        Object::new(Kind::Cube, Vec3::new(3.0, 2.0, -38.0), Vec3::splat(4.0)),
    ]
}

fn camera() -> Camera {
    Camera::look_at(
        EYE,
        Vec3::new(0.0, 0.0, -20.0),
        60.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
    )
}

// Whether a cube or the sphere of the scene stands between the camera and a point.
fn occluded(point: Vec3) -> bool {
    scene(true)
        .iter()
        .filter(|object| object.kind != Kind::Plane)
        .any(|object| {
            let half = 0.5;
            let (centre, scale) = (
                Vec3::from(object.model.translation),
                Vec3::from(object.model.matrix3.x_axis).length(),
            );
            let (low, high) = (centre - Vec3::splat(half * scale), centre + Vec3::splat(half * scale));
            let direction = point - EYE;
            if object.kind == Kind::Sphere {
                let along = ((centre - EYE).dot(direction) / direction.length_squared()).clamp(0.0, 0.999);
                return (EYE + direction * along).distance(centre) < half * scale;
            }
            let (mut near, mut far) = (0.0_f32, 0.999_f32);
            for axis in 0..3 {
                let (origin, step) = (EYE[axis], direction[axis]);
                if step.abs() < 1.0e-9 {
                    if origin < low[axis] || origin > high[axis] {
                        return false;
                    }
                } else {
                    let (a, b) = ((low[axis] - origin) / step, (high[axis] - origin) / step);
                    near = near.max(a.min(b));
                    far = far.min(a.max(b));
                }
            }
            near <= far
        })
}

struct Scene {
    renderer: Renderer,
    target: TargetId,
    map: MapReadback,
}

fn build(test: &str, settings: &ShadowSettings, objects: Vec<Object>) -> Option<Scene> {
    let map = MapReadback::default();
    let built = Renderer::builder()
        .with_device_request(DeviceRequest {
            label: "shadow test",
            backend: backend_from_environment(),
            ..DeviceRequest::default()
        })
        .with_feature(Host::new(objects))
        .with_pack(ShadowPack::with_settings(settings.clone()))
        .with_feature(Shading::new())
        .with_feature(MapProbe {
            layer: 0,
            resolution: settings.resolution,
            readback: map.clone(),
        })
        .build();
    let mut renderer = gpu_or_skip(test, built, |error| {
        matches!(error, RenderError::Rhi(pathon_api::rhi::RhiError::NoAdapter(_)))
    })?;
    announce_adapter(test, renderer.device().caps());
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(WIDTH, HEIGHT), Format::Rgba8Unorm)
        .expect("target");
    Some(Scene { renderer, target, map })
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

    fn frame(&self) -> ShadowFrame {
        *self.renderer.shared().get::<ShadowFrame>().expect("shadow frame")
    }

    fn settings(&mut self) -> &mut ShadowSettings {
        self.renderer
            .shared_mut()
            .get_mut::<ShadowSettings>()
            .expect("settings")
    }

    fn shadow_map(&self) -> Vec<f32> {
        read_map(&**self.renderer.device(), &self.map)
    }
}

// ---- reading the image back in world terms ----

fn linear(byte: u8) -> f32 {
    (f32::from(byte) / 255.0).powf(2.2)
}

/// Linear brightness of the pixel a world point lands on; `None` when it is off screen.
fn brightness(image: &GoldenImage, camera: &Camera, point: Vec3) -> Option<f32> {
    let clip = camera.view_projection() * Vec4::new(point.x, point.y, point.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let (x, y) = (
        (clip.x / clip.w * 0.5 + 0.5) * WIDTH as f32,
        (0.5 - clip.y / clip.w * 0.5) * HEIGHT as f32,
    );
    (x >= 0.0 && y >= 0.0 && x < WIDTH as f32 && y < HEIGHT as f32).then(|| linear(image.pixel(x as u32, y as u32)[1]))
}

fn lit_value(normal_dot_light: f32) -> f32 {
    ALBEDO * (AMBIENT + (1.0 - AMBIENT) * normal_dot_light)
}

fn shadowed_value() -> f32 {
    lit_value(0.0)
}

fn sun_direction() -> Vec3 {
    SUN.normalize()
}

/// Where the shadow of a point lands on the floor.
fn floor_shadow_of(point: Vec3) -> Vec3 {
    point + sun_direction() * (point.y / -sun_direction().y)
}

fn lit_floor() -> f32 {
    lit_value(-sun_direction().y)
}

fn shadow_fraction(value: f32) -> f32 {
    ((lit_floor() - value) / (lit_floor() - shadowed_value())).clamp(0.0, 1.0)
}

#[test]
fn shadows_fall_where_they_should_without_acne_or_light_leaks() {
    let test = "shadows_fall_where_they_should_without_acne_or_light_leaks";
    let Some(mut scene) = build(test, &settings(), scene(true)) else {
        return;
    };
    let camera = camera();
    let image = scene.render(camera);
    let frame = scene.frame();
    assert_eq!(frame.cascade_count, 4);
    eprintln!(
        "splits {:?}, texel {:?} cm",
        (0..4).map(|cascade| frame.uniforms.split(cascade)).collect::<Vec<_>>(),
        (0..4)
            .map(|cascade| frame.uniforms.texel_world_size(cascade) * 100.0)
            .collect::<Vec<_>>()
    );

    // 1. A point in the shadow of each caster is dark, the floor beside it is lit, and the two are far apart.
    let lit = lit_floor();
    let dark = shadowed_value();
    let shadows = [
        ("stack", floor_shadow_of(Vec3::new(0.0, 3.0, -6.0))),
        ("box top", floor_shadow_of(Vec3::new(-0.8, 2.0, -5.5))),
        (
            "sphere",
            floor_shadow_of(Vec3::new(-5.0, 1.0, -8.0)) + Vec3::new(-0.64, 0.0, -0.77),
        ),
        ("facing plane", floor_shadow_of(FACING_PLANE_CENTRE)),
        ("far box", floor_shadow_of(Vec3::new(3.0, 4.0, -38.0))),
    ];
    for (name, point) in shadows {
        assert!(
            !occluded(point),
            "{name}: the scene hides the shadow at {point} from the camera"
        );
        let value = brightness(&image, &camera, point).unwrap_or_else(|| panic!("{name}: off screen at {point}"));
        eprintln!("{name}: shadow at {point} reads {value:.3} (lit {lit:.3}, dark {dark:.3})");
        assert!(value < dark + 0.15 * (lit - dark), "{name}: {value} is not in shadow");
        assert!(
            value * 3.0 < lit,
            "{name}: shadow {value} is not much darker than light {lit}"
        );
    }
    let beside = brightness(&image, &camera, Vec3::new(13.0, 0.0, -20.0)).expect("on screen");
    assert!(
        (beside - lit).abs() < 0.03 * lit,
        "floor beside the shadows: {beside} instead of {lit}"
    );

    // 2. The box lights itself: the top of the stack is not in its own shadow (no acne on a surface at an angle), and
    //    neither is the plane that faces the light head on.
    let top = brightness(&image, &camera, Vec3::new(0.0, 3.0, -6.0)).expect("on screen");
    let top_expected = lit_value(-sun_direction().y);
    assert!(
        (top - top_expected).abs() < 0.04 * top_expected,
        "stack top: {top} instead of {top_expected}"
    );
    let facing_expected = lit_value(1.0);
    let plane = facing_plane().model;
    let mut facing = Vec::new();
    for i in 0..9 {
        for j in 0..9 {
            let local = Vec3::new(-0.4 + 0.1 * i as f32, 0.0, -0.4 + 0.1 * j as f32);
            if let Some(value) = brightness(&image, &camera, plane.transform_point3(local)) {
                facing.push(value);
            }
        }
    }
    assert!(
        facing.len() > 40,
        "only {} samples of the facing plane are on screen",
        facing.len()
    );
    let darkest = facing.iter().copied().fold(f32::MAX, f32::min);
    eprintln!(
        "facing plane: {} samples, darkest {darkest:.3}, expected {facing_expected:.3}",
        facing.len()
    );
    assert!(
        darkest > 0.97 * facing_expected,
        "acne on the plane that faces the light: {darkest} < {facing_expected}"
    );

    // 3. A lit floor at a grazing angle is flat all the way from the camera to the far cascade: no acne, no seams.
    let mut worst = f32::MAX;
    let mut count = 0;
    for z in (-40..=-8).step_by(2) {
        for x in [11.0_f32, 12.0, 13.0, 14.0] {
            if let Some(value) = brightness(&image, &camera, Vec3::new(x, 0.0, z as f32)) {
                worst = worst.min(value);
                count += 1;
            }
        }
    }
    eprintln!("lit floor: {count} samples, darkest {worst:.3} of {lit:.3}");
    assert!(count > 30, "{count} samples");
    assert!(worst > 0.97 * lit, "dark speckles on the lit floor: {worst} of {lit}");

    // 4. No peter-panning: the shadow reaches the foot of the box. The box's -x face is at x = -1, and the sun shines
    //    towards -x, so the floor just beside that face is in shadow right up to the wall.
    let mut gaps = Vec::new();
    for centimetres in [3.0_f32, 6.0, 10.0, 20.0] {
        let point = Vec3::new(-1.0 - centimetres / 100.0, 0.0, -6.0);
        gaps.push((
            centimetres,
            brightness(&image, &camera, point).map_or(1.0, shadow_fraction),
        ));
    }
    eprintln!("shadow fraction at the foot of the box: {gaps:?}");
    assert!(gaps[2].1 > 0.9, "light leaks 10 cm from the box: {gaps:?}");
    assert!(gaps[3].1 > 0.97, "light leaks 20 cm from the box: {gaps:?}");

    assert_golden("shadows_csm", &image, Tolerance::cross_backend());
}

#[test]
fn indirect_and_one_by_one_draws_make_the_same_image() {
    let test = "indirect_and_one_by_one_draws_make_the_same_image";
    let Some(mut scene) = build(test, &settings(), scene(true)) else {
        return;
    };
    if !scene
        .renderer
        .device()
        .caps()
        .has(pathon_api::rhi::Features::INDIRECT_FIRST_INSTANCE)
    {
        eprintln!("{test}: the device has no first-instance indirect draws, only one path exists");
        return;
    }
    scene.settings().use_indirect_draws = true;
    let indirect = scene.render(camera());
    scene.settings().use_indirect_draws = false;
    let one_by_one = scene.render(camera());

    assert_eq!(indirect.as_rgba8(), one_by_one.as_rgba8());
}

// ---- filters ----

// The pixels of a row of the floor where the shadow edge of the stack lies, left to right.
fn edge_row(image: &GoldenImage, camera: &Camera) -> Vec<f32> {
    let start = camera.view_projection() * Vec4::new(-3.0, 0.0, -10.0, 1.0);
    let y = ((0.5 - start.y / start.w * 0.5) * HEIGHT as f32) as u32;
    (0..WIDTH).map(|x| linear(image.pixel(x, y)[1])).collect()
}

fn transition_pixels(row: &[f32]) -> usize {
    let (lit, dark) = (lit_floor(), shadowed_value());
    let margin = 0.1 * (lit - dark);
    row.iter()
        .filter(|value| **value > dark + margin && **value < lit - margin)
        .count()
}

#[test]
fn softer_filters_widen_the_shadow_edge() {
    let test = "softer_filters_widen_the_shadow_edge";
    let Some(mut scene) = build(test, &settings(), scene(true)) else {
        return;
    };
    let camera = camera();
    let mut widths = Vec::new();
    for filter in [ShadowFilter::Hard, ShadowFilter::Pcf3, ShadowFilter::Pcf5] {
        scene.settings().filter = filter;
        let image = scene.render(camera);
        widths.push(transition_pixels(&edge_row(&image, &camera)));
    }
    eprintln!("edge width in pixels (hard, pcf3, pcf5): {widths:?}");
    assert!(widths[0] < widths[1], "pcf3 is softer than hard: {widths:?}");
    assert!(
        widths[1] <= widths[2] + 1,
        "pcf5 is at least as soft as pcf3: {widths:?}"
    );
}

// ---- stability ----

// Moving the camera must not make a shadow swim: for a static scene the depth map of the first cascade only shifts by
// whole texels, so it holds the same picture of the world.
fn covered(depths: &[f32], x: i32, y: i32) -> Option<bool> {
    let side = SIDE;
    ((0..side).contains(&x) && (0..side).contains(&y)).then(|| depths[(y * side + x) as usize] > 0.0)
}

fn map_mismatches(a: &[f32], b: &[f32], shift: (i32, i32)) -> (usize, usize) {
    let (mut compared, mut different) = (0, 0);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (Some(first), Some(second)) = (covered(a, x, y), covered(b, x + shift.0, y + shift.1)) else {
                continue;
            };
            compared += 1;
            different += usize::from(first != second);
        }
    }
    (compared, different)
}

fn texel_of(frame: &ShadowFrame, point: Vec3) -> (f32, f32) {
    let matrix = pathon_api::glam::Mat4::from_cols_array_2d(&frame.uniforms.view_projection(0));
    let clip = matrix * point.extend(1.0);
    (
        (clip.x * 0.5 + 0.5) * RESOLUTION as f32,
        (0.5 - clip.y * 0.5) * RESOLUTION as f32,
    )
}

fn moved_camera(offset: Vec3, yaw: f32) -> Camera {
    let eye = EYE + offset;
    let aim = eye + Quat::from_rotation_y(yaw) * (Vec3::new(0.0, 0.0, -20.0) - EYE);
    Camera::look_at(eye, aim, 60.0_f32.to_radians(), WIDTH as f32 / HEIGHT as f32)
}

#[test]
fn the_depth_map_only_moves_in_whole_texels_when_the_camera_moves() {
    let test = "the_depth_map_only_moves_in_whole_texels_when_the_camera_moves";
    let mut mismatches = Vec::new();
    for fit in [CascadeFit::Stable, CascadeFit::Focused] {
        // The floor does not cast here: with it the map would be covered everywhere and show nothing.
        let Some(mut scene) = build(test, &ShadowSettings { fit, ..settings() }, scene(false)) else {
            return;
        };
        scene.render(moved_camera(Vec3::ZERO, 0.0));
        let reference = (scene.frame(), scene.shadow_map());
        let probe = Vec3::new(0.0, 1.0, -6.0);
        let mut worst_fraction = 0.0_f32;
        for (offset, yaw) in [
            (Vec3::new(0.013, 0.0, 0.0), 0.0),
            (Vec3::new(0.0, 0.0, -0.171), 0.0),
            (Vec3::new(1.37, 0.4, -0.9), 0.0),
            (Vec3::new(0.0, 0.0, 0.0), 0.021),
            (Vec3::new(-0.6, 0.0, 0.8), -0.05),
        ] {
            scene.render(moved_camera(offset, yaw));
            let (frame, map) = (scene.frame(), scene.shadow_map());
            let (before, after) = (texel_of(&reference.0, probe), texel_of(&frame, probe));
            let shift = (after.0 - before.0, after.1 - before.1);
            let whole = ((shift.0 - shift.0.round()).abs(), (shift.1 - shift.1.round()).abs());
            let (compared, different) =
                map_mismatches(&reference.1, &map, (shift.0.round() as i32, shift.1.round() as i32));
            let fraction = different as f32 / compared.max(1) as f32;
            eprintln!(
                "{fit:?} offset {offset} yaw {yaw}: shift {shift:?} ({whole:?} off a whole texel), {different} of {compared} texels differ"
            );
            if fit == CascadeFit::Stable {
                assert!(
                    whole.0 < 0.02 && whole.1 < 0.02,
                    "the map moved by a fraction of a texel: {shift:?}"
                );
            }
            worst_fraction = worst_fraction.max(fraction);
        }
        mismatches.push(worst_fraction);
    }
    assert!(
        mismatches[0] < 0.0005,
        "a stable map is the same picture shifted by whole texels: {mismatches:?}"
    );
    assert!(
        mismatches[1] > 5.0 * mismatches[0].max(0.0001),
        "the focused map differs more, which shows the test can see a swimming shadow: {mismatches:?}"
    );
}
