#![cfg(feature = "gpu-tests")]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test images are small and their coordinates fit every integer type"
)]

// Lines, wireframes and slot previews rendered on a real device and checked three ways: against the maths of the
// shaders (pixel positions, widths, colours), against each other (the two wireframe paths) and against golden images.

use pathon_api::glam::{Mat4, Vec2, Vec3};
use pathon_api::rhi::{Access, Color, Format, PhysicalSize, RhiError};
use pathon_api::{Camera, Severity, Slot};
use pathon_core::{FrameInput, RenderError, Renderer, TargetId, ViewInput};
use pathon_debug::reference::{barycentric_coverage, heat, material_color, motion_color, segment_coverage, srgb_oetf};
use pathon_debug::{
    Corner, DebugColor, DebugDraw, DebugPack, DebugSettings, LineStyle, PreviewSlot, SlotPreview, WireframeMode,
};
use pathon_golden::{GoldenImage, Tolerance, announce_adapter, assert_golden, gpu_or_skip, read_texture};
use pathon_rhi::{Backend, DeviceApi, DeviceRequest, Features};
use pathon_test::SyntheticSlots;

const SIZE: u32 = 128;

fn backend_from_env() -> Option<Backend> {
    match std::env::var("PATHON_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        _ => None,
    }
}

struct Scene {
    renderer: Renderer,
    target: TargetId,
}

fn scene(test: &str, size: u32, format: Format, synthetic: Option<SyntheticSlots>) -> Option<Scene> {
    let mut builder = Renderer::builder()
        .with_device_request(DeviceRequest {
            backend: backend_from_env(),
            ..DeviceRequest::default()
        })
        .with_clear_color(Color::BLACK);
    if let Some(synthetic) = synthetic {
        builder = builder.with_pack(synthetic);
    }
    let built = builder.with_pack(DebugPack::new()).build();
    let mut renderer = gpu_or_skip(test, built, |error| {
        matches!(error, RenderError::Rhi(RhiError::NoAdapter(_)))
    })?;
    announce_adapter(test, renderer.device().caps());
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(size, size), format)
        .expect("target");
    Some(Scene { renderer, target })
}

impl Scene {
    fn draw(&mut self) -> &mut DebugDraw {
        self.renderer.shared_mut().get_mut::<DebugDraw>().expect("debug pack")
    }

    fn settings(&mut self) -> &mut DebugSettings {
        self.renderer
            .shared_mut()
            .get_mut::<DebugSettings>()
            .expect("debug pack")
    }

    fn has_line_polygon_mode(&self) -> bool {
        self.renderer.device().caps().has(Features::POLYGON_MODE_LINE)
    }

    fn render(&mut self) -> Vec<String> {
        let input = FrameInput {
            views: vec![ViewInput {
                target: self.target,
                camera: Camera::default(),
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
        report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.to_owned())
            .collect()
    }

    fn image(&self) -> GoldenImage {
        let texture = self.renderer.target_texture(self.target).expect("offscreen texture");
        read_texture(&**self.renderer.device(), texture, Access::SAMPLED).expect("readback")
    }
}

fn pixel_rgb(image: &GoldenImage, x: u32, y: u32) -> [u8; 3] {
    let [red, green, blue, _] = image.pixel(x, y);
    [red, green, blue]
}

fn expect_rgb(image: &GoldenImage, x: u32, y: u32, expected: Vec3, tolerance: u8, what: &str) {
    let actual = pixel_rgb(image, x, y);
    let wanted = expected
        .to_array()
        .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
    let close = actual.iter().zip(wanted).all(|(a, w)| a.abs_diff(w) <= tolerance);
    assert!(close, "{what} at ({x}, {y}): got {actual:?}, wanted {wanted:?}");
}

fn project(camera: &Camera, point: Vec3, size: u32) -> Vec2 {
    let clip = camera.view_projection() * point.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    Vec2::new((ndc.x * 0.5 + 0.5) * size as f32, (0.5 - ndc.y * 0.5) * size as f32)
}

fn luminance(image: &GoldenImage, x: u32, y: u32) -> u32 {
    pixel_rgb(image, x, y)
        .iter()
        .map(|channel| u32::from(*channel))
        .max()
        .unwrap_or(0)
}

// ---- lines ----

struct ExpectedLine {
    start: Vec3,
    end: Vec3,
    thickness: f32,
    color: DebugColor,
}

fn expected_pixels(lines: &[ExpectedLine], size: u32) -> Vec<[f32; 3]> {
    let camera = Camera::default();
    let projected: Vec<_> = lines
        .iter()
        .map(|line| (project(&camera, line.start, size), project(&camera, line.end, size)))
        .collect();
    (0..size * size)
        .map(|index| {
            let center = Vec2::new((index % size) as f32 + 0.5, (index / size) as f32 + 0.5);
            let mut rgb = [0.0_f32; 3];
            for (line, (start, end)) in lines.iter().zip(&projected) {
                let alpha = line.color.a * segment_coverage(*start, *end, line.thickness, center);
                let source = [line.color.r, line.color.g, line.color.b];
                for (channel, value) in rgb.iter_mut().zip(source) {
                    *channel = value * alpha + *channel * (1.0 - alpha);
                }
            }
            rgb
        })
        .collect()
}

fn assert_matches_expected(image: &GoldenImage, expected: &[[f32; 3]], tolerance: u8) {
    let mut worst = (0_u8, 0_u32);
    for (index, rgb) in expected.iter().enumerate() {
        let (x, y) = (index as u32 % image.width(), index as u32 / image.width());
        let actual = pixel_rgb(image, x, y);
        for (a, e) in actual.iter().zip(rgb) {
            let difference = a.abs_diff((e * 255.0).round() as u8);
            if difference > worst.0 {
                worst = (difference, index as u32);
            }
        }
    }
    assert!(
        worst.0 <= tolerance,
        "pixel ({}, {}) is {} levels off: got {:?}, wanted {:?}",
        worst.1 % image.width(),
        worst.1 / image.width(),
        worst.0,
        pixel_rgb(image, worst.1 % image.width(), worst.1 / image.width()),
        expected[worst.1 as usize]
    );
}

#[test]
fn lines_land_on_the_pixels_their_projection_and_thickness_predict() {
    let Some(mut scene) = scene("lines_land_on_the_pixels", SIZE, Format::Rgba8Unorm, None) else {
        return;
    };
    let lines = [
        ExpectedLine {
            start: Vec3::new(-1.2, 0.8, 0.0),
            end: Vec3::new(1.2, 0.8, 0.0),
            thickness: 1.0,
            color: DebugColor::WHITE,
        },
        ExpectedLine {
            start: Vec3::new(-1.2, 0.4, 0.0),
            end: Vec3::new(1.2, 0.05, 0.0),
            thickness: 3.0,
            color: DebugColor::YELLOW,
        },
        ExpectedLine {
            start: Vec3::new(-1.0, -0.4, 0.0),
            end: Vec3::new(0.2, -1.2, 0.0),
            thickness: 6.0,
            color: DebugColor::RED,
        },
        ExpectedLine {
            start: Vec3::new(0.9, -0.2, 0.0),
            end: Vec3::new(0.9, -1.0, 0.0),
            thickness: 2.0,
            color: DebugColor::new(0.2, 0.6, 1.0, 0.5),
        },
    ];
    for line in &lines {
        scene.draw().line(
            line.start,
            line.end,
            LineStyle::new(line.color).thickness(line.thickness),
        );
    }

    assert_eq!(scene.render(), [] as [String; 0]);
    let image = scene.image();

    assert_matches_expected(&image, &expected_pixels(&lines, SIZE), 3);
    assert_golden("debug_lines", &image, Tolerance::cross_backend());
}

#[test]
fn thickness_is_in_pixels_whatever_the_distance() {
    let Some(mut scene) = scene("thickness_is_in_pixels", SIZE, Format::Rgba8Unorm, None) else {
        return;
    };
    scene.draw().line(
        Vec3::new(-1.0, 1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        LineStyle::new(DebugColor::WHITE).thickness(4.0),
    );
    scene.draw().line(
        Vec3::new(-8.0, -10.0, -30.0),
        Vec3::new(8.0, -10.0, -30.0),
        LineStyle::new(DebugColor::WHITE).thickness(4.0),
    );

    scene.render();
    let image = scene.image();

    let energy =
        |x: u32, rows: std::ops::Range<u32>| -> f32 { rows.map(|y| f32::from(image.pixel(x, y)[0]) / 255.0).sum() };
    let (near, far) = (energy(64, 0..64), energy(64, 64..128));
    assert!((near - 4.0).abs() < 0.1, "near line is {near} px wide");
    assert!((far - 4.0).abs() < 0.1, "far line is {far} px wide");
}

#[test]
fn a_box_is_drawn_along_its_twelve_projected_edges() {
    let Some(mut scene) = scene("a_box_is_drawn", SIZE, Format::Rgba8Unorm, None) else {
        return;
    };
    let (min, max) = (Vec3::splat(-0.5), Vec3::splat(0.5));
    scene
        .draw()
        .aabb(min, max, LineStyle::new(DebugColor::YELLOW).thickness(2.0));
    scene.render();
    let image = scene.image();
    let camera = Camera::default();

    let corner = |bits: usize| {
        Vec3::new(
            if bits & 1 == 0 { min.x } else { max.x },
            if bits & 2 == 0 { min.y } else { max.y },
            if bits & 4 == 0 { min.z } else { max.z },
        )
    };
    let mut checked = 0;
    for bits in 0..8_usize {
        for bit in [1_usize, 2, 4] {
            if bits & bit != 0 {
                continue;
            }
            for step in 0..=8 {
                let point = corner(bits).lerp(corner(bits | bit), step as f32 / 8.0);
                let pixel = project(&camera, point, SIZE);
                let (x, y) = (pixel.x.floor() as u32, pixel.y.floor() as u32);
                assert!(luminance(&image, x, y) >= 190, "edge {bits}-{} at {pixel}", bits | bit);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 12 * 9);
    assert_eq!(
        luminance(&image, SIZE / 2, SIZE / 2),
        0,
        "the middle of the box is empty"
    );
    assert_golden("debug_aabb", &image, Tolerance::cross_backend());
}

#[test]
fn depth_tested_lines_hide_behind_the_scene_and_the_others_do_not() {
    let Some(mut scene) = scene(
        "depth_tested_lines",
        SIZE,
        Format::Rgba8Unorm,
        Some(SyntheticSlots::new()),
    ) else {
        return;
    };
    // The synthetic wall is 2 m from the camera at the left edge and 12 m at the right one; the lines are 3 m away.
    let (left, right) = (Vec3::new(-1.5, 0.0, 0.0), Vec3::new(1.5, 0.0, 0.0));
    let at = |y: f32| [left + Vec3::Y * y, right + Vec3::Y * y];
    let [tested_start, tested_end] = at(0.5);
    let [plain_start, plain_end] = at(-0.5);
    scene.draw().line(
        tested_start,
        tested_end,
        LineStyle::new(DebugColor::WHITE).thickness(3.0).depth_tested(),
    );
    scene
        .draw()
        .line(plain_start, plain_end, LineStyle::new(DebugColor::WHITE).thickness(3.0));
    scene.render();
    let image = scene.image();
    let camera = Camera::default();
    let row = |y: f32| project(&camera, Vec3::new(0.0, y, 0.0), SIZE).y.floor() as u32;

    assert_eq!(luminance(&image, 10, row(0.5)), 0, "nearer wall hides the tested line");
    assert!(luminance(&image, 20, row(0.5)) > 200, "past the wall edge it shows");
    assert!(luminance(&image, 10, row(-0.5)) > 200, "an untested line ignores depth");
}

// ---- wireframe ----

fn grid_mesh(cells: u32, half_extent: f32) -> (Vec<Vec3>, Vec<u32>) {
    let side = cells + 1;
    let positions = (0..side * side)
        .map(|index| {
            let (column, row) = ((index % side) as f32, (index / side) as f32);
            let at = |value: f32| (value / cells as f32 * 2.0 - 1.0) * half_extent;
            Vec3::new(at(column), at(row), 0.0)
        })
        .collect();
    let mut indices = Vec::new();
    for row in 0..cells {
        for column in 0..cells {
            let first = row * side + column;
            indices.extend([
                first,
                first + 1,
                first + side,
                first + 1,
                first + side + 1,
                first + side,
            ]);
        }
    }
    (positions, indices)
}

fn wireframe_image(test: &str, mode: WireframeMode) -> Option<(GoldenImage, bool)> {
    let mut scene = scene(test, SIZE, Format::Rgba8Unorm, None)?;
    let (positions, indices) = grid_mesh(5, 1.2);
    // Rotated a little so that no edge but the ones of the grid is axis-aligned and lands on a pixel boundary.
    scene.draw().wireframe(
        &positions,
        &indices,
        Mat4::from_rotation_z(0.11),
        LineStyle::new(DebugColor::WHITE),
    );
    scene.settings().wireframe = mode;
    scene.render();
    Some((scene.image(), scene.has_line_polygon_mode()))
}

fn lit_pixels(image: &GoldenImage) -> Vec<(u32, u32)> {
    (0..image.height())
        .flat_map(|y| (0..image.width()).map(move |x| (x, y)))
        .filter(|(x, y)| luminance(image, *x, *y) >= 100)
        .collect()
}

// The fallback rasterizes only the inside of a triangle, so along the border of a mesh a line pixel can sit up to two
// pixels from the nearest fallback pixel.
fn near_a_lit_pixel(image: &GoldenImage, x: u32, y: u32) -> bool {
    const REACH: u32 = 2;
    (y.saturating_sub(REACH)..=(y + REACH).min(image.height() - 1)).any(|row| {
        (x.saturating_sub(REACH)..=(x + REACH).min(image.width() - 1))
            .any(|column| luminance(image, column, row) >= 100)
    })
}

#[test]
fn the_barycentric_wireframe_draws_the_grid_and_matches_its_golden() {
    let Some((image, _)) = wireframe_image("barycentric_wireframe", WireframeMode::Barycentric) else {
        return;
    };

    let lit = lit_pixels(&image);
    assert!(
        lit.len() > 600,
        "a six by six grid has long edges: {} lit pixels",
        lit.len()
    );
    assert_eq!(luminance(&image, 2, 2), 0, "outside the mesh");
    // fwidth is coarse on some backends, so the antialiased fringe may differ a little.
    assert_golden("debug_wireframe_barycentric", &image, Tolerance::new(40, 0.05));
}

#[test]
fn polygon_mode_line_and_the_barycentric_fallback_draw_the_same_lines() {
    let Some((barycentric, _)) = wireframe_image("wireframe_fallback", WireframeMode::Barycentric) else {
        return;
    };
    let Some((hardware, has_line_mode)) = wireframe_image("wireframe_polygon_line", WireframeMode::Auto) else {
        return;
    };
    if !has_line_mode {
        eprintln!("SKIPPED hardware half: this device has no POLYGON_MODE_LINE, Auto used the fallback");
        return;
    }

    // Hardware line rasterization differs between backends by a pixel here and there; a shifted pixel counts twice.
    assert_golden("debug_wireframe_lines", &hardware, Tolerance::new(100, 0.10));
    let (hardware_lit, fallback_lit) = (lit_pixels(&hardware), lit_pixels(&barycentric));
    for (x, y) in &hardware_lit {
        assert!(
            near_a_lit_pixel(&barycentric, *x, *y),
            "line pixel ({x}, {y}) has no fallback pixel next to it"
        );
    }
    for (x, y) in &fallback_lit {
        assert!(
            near_a_lit_pixel(&hardware, *x, *y),
            "fallback pixel ({x}, {y}) has no line pixel next to it"
        );
    }
    let ratio = fallback_lit.len() as f32 / hardware_lit.len() as f32;
    assert!(
        (0.7..=1.6).contains(&ratio),
        "{} line pixels, {} fallback pixels",
        hardware_lit.len(),
        fallback_lit.len()
    );
}

#[test]
fn barycentric_thickness_widens_the_edge_by_the_reference_coverage() {
    let Some(mut scene) = scene("barycentric_thickness", SIZE, Format::Rgba8Unorm, None) else {
        return;
    };
    // One big triangle; its left edge is vertical at x = -1 (pixel 36.9), so the distance to it is known per column.
    let positions = [
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::new(-1.0, 1.0, 0.0),
    ];
    scene.draw().wireframe(
        &positions,
        &[0, 1, 2],
        Mat4::IDENTITY,
        LineStyle::new(DebugColor::WHITE).thickness(4.0),
    );
    scene.settings().wireframe = WireframeMode::Barycentric;
    scene.render();
    let image = scene.image();
    let edge_x = project(&Camera::default(), Vec3::new(-1.0, 0.0, 0.0), SIZE).x;

    for column in (edge_x.ceil() as u32)..(edge_x.ceil() as u32 + 5) {
        let distance = column as f32 + 0.5 - edge_x;
        let expected = barycentric_coverage(4.0, distance);
        let actual = f32::from(image.pixel(column, 64)[0]) / 255.0;
        assert!(
            (actual - expected).abs() < 0.05,
            "column {column}: {actual} vs {expected}"
        );
    }
}

// ---- slot previews ----

fn preview_scene(test: &str, size: u32, format: Format, preview: SlotPreview) -> Option<Scene> {
    let mut scene = scene(test, size, format, Some(SyntheticSlots::new()))?;
    scene.settings().preview = Some(preview);
    Some(scene)
}

fn uv_of(x: u32, y: u32, size: u32) -> Vec2 {
    Vec2::new((x as f32 + 0.5) / size as f32, (y as f32 + 0.5) / size as f32)
}

fn encoded(linear: Vec3) -> Vec3 {
    Vec3::new(
        srgb_oetf(linear.x.clamp(0.0, 1.0)),
        srgb_oetf(linear.y.clamp(0.0, 1.0)),
        srgb_oetf(linear.z.clamp(0.0, 1.0)),
    )
}

fn golden_name(slot: PreviewSlot) -> &'static str {
    match slot {
        PreviewSlot::Depth => "debug_slot_depth",
        PreviewSlot::DepthPyramid => "debug_slot_depth_pyramid",
        PreviewSlot::Normals => "debug_slot_normals",
        PreviewSlot::Motion => "debug_slot_motion",
        PreviewSlot::MaterialId => "debug_slot_material_id",
        PreviewSlot::Roughness => "debug_slot_roughness",
        PreviewSlot::Albedo => "debug_slot_albedo",
        PreviewSlot::Reactive => "debug_slot_reactive",
        PreviewSlot::Diffuse => "debug_slot_diffuse",
        PreviewSlot::Specular => "debug_slot_specular",
        PreviewSlot::Hdr => "debug_slot_hdr",
        PreviewSlot::Ao => "debug_slot_ao",
        PreviewSlot::Exposure => "debug_slot_exposure",
    }
}

const PREVIEW_SIZE: u32 = 64;

#[test]
fn every_base_slot_has_a_preview_that_matches_its_golden_image() {
    for slot in PreviewSlot::ALL {
        let mut preview = SlotPreview::fullscreen(slot);
        if slot == PreviewSlot::DepthPyramid {
            preview.pyramid_level = 1;
        }
        let Some(mut scene) = preview_scene(golden_name(slot), PREVIEW_SIZE, Format::Rgba8Unorm, preview) else {
            return;
        };

        assert!(scene.render().is_empty(), "{slot:?}");

        assert_golden(golden_name(slot), &scene.image(), Tolerance::cross_backend());
    }
}

fn fullscreen_image(slot: PreviewSlot) -> Option<GoldenImage> {
    let mut scene = preview_scene(
        "fullscreen_image",
        PREVIEW_SIZE,
        Format::Rgba8Unorm,
        SlotPreview::fullscreen(slot),
    )?;
    scene.render();
    Some(scene.image())
}

#[test]
fn depth_and_the_pyramid_show_the_view_distance_on_the_heat_ramp() {
    let Some(image) = fullscreen_image(PreviewSlot::Depth) else {
        return;
    };
    for (x, y) in [(2, 5), (10, 20), (32, 32), (50, 10), (62, 40)] {
        let distance = 2.0 + 10.0 * uv_of(x, y, PREVIEW_SIZE).x;
        let expected = heat(1.0 - (distance - 0.1) / (20.0 - 0.1));
        expect_rgb(&image, x, y, expected, 3, "depth");
    }
    expect_rgb(&image, 30, 60, Vec3::ZERO, 0, "the empty strip is black");
}

#[test]
fn normals_are_decoded_to_rgb() {
    let Some(image) = fullscreen_image(PreviewSlot::Normals) else {
        return;
    };
    for (x, y) in [(32, 32), (45, 28), (20, 50), (2, 2), (60, 60)] {
        let uv = uv_of(x, y, PREVIEW_SIZE);
        let p = Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0) * 1.25;
        let normal = if p.length_squared() < 1.0 {
            p.extend((1.0 - p.length_squared()).sqrt())
        } else {
            Vec3::Z
        };
        expect_rgb(&image, x, y, normal * 0.5 + Vec3::splat(0.5), 4, "normal");
    }
}

#[test]
fn motion_shows_direction_as_hue_and_length_as_brightness() {
    let Some(image) = fullscreen_image(PreviewSlot::Motion) else {
        return;
    };
    for (x, y) in [(60, 32), (3, 32), (32, 3), (32, 60), (40, 32), (50, 50)] {
        let uv = uv_of(x, y, PREVIEW_SIZE);
        let motion = Vec2::new((uv.x - 0.5) * 32.0, (uv.y - 0.5) * 32.0);
        expect_rgb(&image, x, y, motion_color(motion, 8.0), 5, "motion");
    }
}

#[test]
fn material_ids_get_their_hashed_colors() {
    let Some(image) = fullscreen_image(PreviewSlot::MaterialId) else {
        return;
    };
    for id in 0..16_u32 {
        let (column, row) = (id % 4, id / 4);
        expect_rgb(&image, column * 16 + 8, row * 16 + 8, material_color(id), 2, "material");
    }
}

#[test]
fn scalar_slots_are_gray_and_albedo_is_encoded() {
    for (slot, x, y, expected) in [
        (PreviewSlot::Roughness, 16, 40, uv_of(16, 40, PREVIEW_SIZE).x),
        (PreviewSlot::Roughness, 48, 10, uv_of(48, 10, PREVIEW_SIZE).x),
        (PreviewSlot::Reactive, 4, 4, 0.0),
        (PreviewSlot::Reactive, 12, 4, 1.0),
        (
            PreviewSlot::Ao,
            32,
            32,
            1.0 - 1.6 * (uv_of(32, 32, PREVIEW_SIZE) - Vec2::splat(0.5)).length(),
        ),
        (PreviewSlot::Ao, 2, 2, 0.0),
    ] {
        let Some(image) = fullscreen_image(slot) else {
            return;
        };
        expect_rgb(&image, x, y, Vec3::splat(expected), 3, &format!("{slot:?}"));
    }
    let Some(albedo) = fullscreen_image(PreviewSlot::Albedo) else {
        return;
    };
    expect_rgb(&albedo, 45, 10, encoded(Vec3::new(0.8, 0.0, 0.0)), 3, "red bar");
    let gray = uv_of(32, 50, PREVIEW_SIZE).x;
    expect_rgb(&albedo, 32, 50, encoded(Vec3::splat(gray)), 3, "gray ramp");
}

#[test]
fn hdr_is_exposed_clamped_and_encoded_and_infinity_is_magenta() {
    let Some(image) = fullscreen_image(PreviewSlot::Hdr) else {
        return;
    };
    for (x, y) in [(32, 32), (10, 20), (50, 50), (60, 5)] {
        let uv = uv_of(x, y, PREVIEW_SIZE);
        let base = 0.18 * (uv.x * 8.0 - 4.0).exp2();
        let linear = Vec3::new(1.0, 1.0 - 0.5 * uv.y, 1.0 - uv.y) * base;
        expect_rgb(&image, x, y, encoded(linear), 4, "hdr");
    }
    expect_rgb(&image, 1, 1, Vec3::new(1.0, 0.0, 1.0), 0, "infinity");

    let Some(mut scene) = preview_scene("hdr_exposure", PREVIEW_SIZE, Format::Rgba8Unorm, {
        let mut preview = SlotPreview::fullscreen(PreviewSlot::Hdr);
        preview.exposure = 0.25;
        preview
    }) else {
        return;
    };
    scene.render();
    let darker = scene.image();
    assert!(luminance(&darker, 50, 30) < luminance(&image, 50, 30));
}

#[test]
fn the_exposure_slot_can_drive_the_hdr_preview() {
    let manual = {
        let mut preview = SlotPreview::fullscreen(PreviewSlot::Hdr);
        preview.exposure = 2.0;
        preview
    };
    let from_slot = {
        let mut preview = SlotPreview::fullscreen(PreviewSlot::Hdr);
        preview.exposure_from_slot = true;
        preview
    };
    let (Some(mut a), Some(mut b)) = (
        preview_scene("exposure_manual", PREVIEW_SIZE, Format::Rgba8Unorm, manual),
        preview_scene("exposure_slot", PREVIEW_SIZE, Format::Rgba8Unorm, from_slot),
    ) else {
        return;
    };
    a.render();
    b.render();

    // The synthetic exposure slot holds 2.0.
    assert_eq!(a.image(), b.image());
}

#[test]
fn previews_look_the_same_on_srgb_and_plain_targets() {
    let preview = || {
        let mut preview = SlotPreview::grid(vec![
            PreviewSlot::Depth,
            PreviewSlot::Albedo,
            PreviewSlot::Hdr,
            PreviewSlot::Normals,
        ]);
        preview.exposure = 1.0;
        preview
    };
    let (Some(mut plain), Some(mut srgb)) = (
        preview_scene("srgb_plain", 64, Format::Rgba8Unorm, preview()),
        preview_scene("srgb_target", 64, Format::Rgba8UnormSrgb, preview()),
    ) else {
        return;
    };
    plain.render();
    srgb.render();
    let (plain, srgb) = (plain.image(), srgb.image());

    let mut worst = 0;
    for y in 0..64 {
        for x in 0..64 {
            for (a, b) in pixel_rgb(&plain, x, y).iter().zip(pixel_rgb(&srgb, x, y)) {
                worst = worst.max(a.abs_diff(b));
            }
        }
    }
    assert!(worst <= 3, "an sRGB target differs by {worst} levels");
}

#[test]
fn the_grid_shows_every_slot_on_a_dark_background() {
    let Some(mut scene) = preview_scene(
        "grid_of_all_slots",
        256,
        Format::Rgba8Unorm,
        SlotPreview::grid(PreviewSlot::ALL.to_vec()),
    ) else {
        return;
    };

    assert_eq!(scene.render(), [] as [String; 0]);
    let image = scene.image();

    // Thirteen slots in a 4 x 4 grid of 64 px cells: the last three cells are background.
    expect_rgb(&image, 0, 0, Vec3::splat(0.04), 1, "background");
    expect_rgb(&image, 96, 224, Vec3::splat(0.04), 1, "empty cell");
    assert!(luminance(&image, 32, 32) > 0, "the depth cell");
    assert_golden("debug_slots_grid", &image, Tolerance::cross_backend());
}

#[test]
fn a_corner_preview_sits_on_top_of_the_lines() {
    let Some(mut scene) = preview_scene(
        "corner_preview",
        SIZE,
        Format::Rgba8Unorm,
        SlotPreview::corner(PreviewSlot::Normals, Corner::TopRight),
    ) else {
        return;
    };
    scene
        .draw()
        .sphere(Vec3::ZERO, 1.0, LineStyle::new(DebugColor::GREEN).thickness(2.0));
    scene.draw().axes(Mat4::IDENTITY, 1.2, 3.0);

    scene.render();
    let image = scene.image();

    assert_eq!(luminance(&image, 2, 2), 0, "the other corners stay black");
    assert!(luminance(&image, 100, 25) > 0, "the preview rectangle at the top right");
    assert_golden("debug_slot_corner", &image, Tolerance::cross_backend());
}

#[test]
fn a_missing_slot_is_a_checkered_tile_and_a_u0001() {
    let Some(mut scene) = scene(
        "missing_slot",
        PREVIEW_SIZE,
        Format::Rgba8Unorm,
        Some(SyntheticSlots::new().without(Slot::NORMALS)),
    ) else {
        return;
    };
    scene.settings().preview = Some(SlotPreview::fullscreen(PreviewSlot::Normals));

    let reported = scene.render();
    let image = scene.image();

    assert_eq!(reported, ["U0001"]);
    expect_rgb(&image, 2, 2, Vec3::new(0.9, 0.0, 0.9), 1, "magenta cell");
    expect_rgb(&image, 10, 2, Vec3::splat(0.12), 1, "dark cell");
}
