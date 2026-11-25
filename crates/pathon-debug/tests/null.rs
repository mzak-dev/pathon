// The whole pack on `rhi-null`: its command validation checks barriers, bind groups and buffer ranges without a GPU.

use std::sync::Arc;

use pathon_api::glam::{Mat4, Vec3};
use pathon_api::rhi::{Features, Format, PhysicalSize, Profile};
use pathon_api::{Camera, Severity, Slot};
use pathon_core::{FeatureStatus, FrameInput, FrameReport, Renderer, RendererBuilder, TargetId, ViewInput};
use pathon_debug::{
    Corner, DebugColor, DebugDraw, DebugPack, DebugSettings, LineStyle, PreviewSlot, SlotPreview, WireframeMode,
};
use pathon_rhi::{Device, NullDevice};
use pathon_test::SyntheticSlots;

fn builder(features: Features) -> RendererBuilder {
    Renderer::builder().with_device(Arc::new(Device::from(NullDevice::with_features(features))))
}

fn desktop() -> Features {
    Profile::Desktop.features()
}

fn frame(renderer: &mut Renderer, target: TargetId) -> FrameReport {
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    renderer.render_frame(&input).expect("frame")
}

fn target(renderer: &mut Renderer) -> TargetId {
    renderer
        .add_offscreen_target(PhysicalSize::new(64, 64), Format::Rgba8Unorm)
        .expect("target")
}

fn codes(report: &FrameReport) -> Vec<&str> {
    report.diagnostics.iter().map(|diagnostic| diagnostic.code).collect()
}

fn draw(renderer: &mut Renderer) -> &mut DebugDraw {
    renderer.shared_mut().get_mut::<DebugDraw>().expect("debug pack")
}

fn settings(renderer: &mut Renderer) -> &mut DebugSettings {
    renderer.shared_mut().get_mut::<DebugSettings>().expect("debug pack")
}

#[test]
fn lines_wireframes_and_a_grid_of_every_slot_record_without_a_diagnostic() {
    let mut renderer = builder(desktop())
        .with_pack(SyntheticSlots::new())
        .with_pack(DebugPack::new())
        .build()
        .expect("renderer");
    let target = target(&mut renderer);
    settings(&mut renderer).preview = Some(SlotPreview::grid(PreviewSlot::ALL.to_vec()));
    let positions = [Vec3::ZERO, Vec3::X, Vec3::Y];
    let debug = draw(&mut renderer);
    debug.aabb(
        Vec3::splat(-0.5),
        Vec3::splat(0.5),
        LineStyle::new(DebugColor::YELLOW).depth_tested(),
    );
    debug.sphere(Vec3::ZERO, 0.4, DebugColor::CYAN);
    debug.axes(Mat4::IDENTITY, 1.0, 2.0);
    debug.wireframe(&positions, &[0, 1, 2], Mat4::IDENTITY, DebugColor::WHITE);

    for _ in 0..3 {
        let report = frame(&mut renderer, target);
        assert_eq!(codes(&report), Vec::<&str>::new(), "{:?}", report.diagnostics);
    }
}

#[test]
fn two_views_of_one_frame_get_their_own_line_regions() {
    let mut renderer = builder(desktop())
        .with_pack(DebugPack::new())
        .build()
        .expect("renderer");
    let (first, second) = (target(&mut renderer), target(&mut renderer));
    draw(&mut renderer).sphere(Vec3::ZERO, 1.0, DebugColor::GREEN);
    let input = FrameInput {
        views: [first, second]
            .map(|target| ViewInput {
                target,
                camera: Camera::default(),
            })
            .to_vec(),
        ..FrameInput::default()
    };

    let report = renderer.render_frame(&input).expect("frame");

    assert!(
        report
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Error)
    );
    assert_eq!(report.views.len(), 2);
}

#[test]
fn the_wireframe_pack_falls_back_without_line_polygon_mode_and_runs_fully_with_it() {
    let status_of = |features: Features| {
        let renderer = builder(features).with_pack(DebugPack::new()).build().expect("renderer");
        renderer
            .features()
            .into_iter()
            .find(|(name, _)| *name == "debug wireframe")
            .expect("registered")
            .1
    };

    assert!(matches!(status_of(desktop()), FeatureStatus::Fallback { .. }));
    assert_eq!(
        status_of(desktop() | Features::POLYGON_MODE_LINE),
        FeatureStatus::Active
    );
}

#[test]
fn both_wireframe_paths_record_cleanly() {
    for (features, mode) in [
        (desktop() | Features::POLYGON_MODE_LINE, WireframeMode::Auto),
        (desktop() | Features::POLYGON_MODE_LINE, WireframeMode::Barycentric),
        (desktop(), WireframeMode::Auto),
    ] {
        let mut renderer = builder(features)
            .with_pack(SyntheticSlots::new())
            .with_pack(DebugPack::new())
            .build()
            .expect("renderer");
        let target = target(&mut renderer);
        settings(&mut renderer).wireframe = mode;
        draw(&mut renderer).wireframe(
            &[Vec3::ZERO, Vec3::X, Vec3::Y],
            &[0, 1, 2],
            Mat4::IDENTITY,
            LineStyle::new(DebugColor::WHITE).thickness(2.0),
        );

        let report = frame(&mut renderer, target);

        assert_eq!(codes(&report), Vec::<&str>::new(), "{features:?} {mode:?}");
    }
}

#[test]
fn a_slot_nobody_writes_is_a_checkered_tile_and_one_u0001() {
    let mut renderer = builder(desktop())
        .with_pack(SyntheticSlots::new().without(Slot::NORMALS))
        .with_pack(DebugPack::new())
        .build()
        .expect("renderer");
    let target = target(&mut renderer);
    settings(&mut renderer).preview = Some(SlotPreview::grid(vec![PreviewSlot::Normals, PreviewSlot::Depth]));

    let reported: Vec<_> = (0..3).map(|_| codes(&frame(&mut renderer, target)).len()).collect();

    assert_eq!(reported, [1, 0, 0], "once, until the slot comes back");
}

#[test]
fn the_preview_asks_for_on_demand_slots_and_never_makes_them() {
    let preview = |packs: bool| {
        let mut builder = builder(desktop()).with_pack(DebugPack::new());
        if packs {
            builder = builder.with_pack(SyntheticSlots::new());
        }
        let mut renderer = builder.build().expect("renderer");
        let target = target(&mut renderer);
        settings(&mut renderer).preview = Some(SlotPreview::fullscreen(PreviewSlot::Roughness));
        codes(&frame(&mut renderer, target))
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };

    assert_eq!(preview(false), ["U0001"], "no producer, no slot");
    assert!(
        preview(true).is_empty(),
        "the producer wrote it because the preview asked"
    );
}

#[test]
fn a_full_draw_list_drops_and_reports_u0002_once() {
    let mut renderer = builder(desktop())
        .with_pack(DebugPack::new().with_draw(DebugDraw::with_capacity(2, 2)))
        .build()
        .expect("renderer");
    let target = target(&mut renderer);
    for _ in 0..5 {
        draw(&mut renderer).line(Vec3::ZERO, Vec3::X, DebugColor::WHITE);
    }

    let first = codes(&frame(&mut renderer, target))
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let second = codes(&frame(&mut renderer, target)).len();

    assert_eq!(first, ["U0002"]);
    assert_eq!(second, 0);
}

#[test]
fn broken_preview_settings_are_repaired_and_reported_as_u0003() {
    let mut renderer = builder(desktop())
        .with_pack(SyntheticSlots::new())
        .with_pack(DebugPack::new())
        .build()
        .expect("renderer");
    let target = target(&mut renderer);
    let mut preview = SlotPreview::corner(PreviewSlot::Hdr, Corner::BottomLeft);
    preview.exposure = -3.0;
    settings(&mut renderer).preview = Some(preview);

    let report = frame(&mut renderer, target);

    assert_eq!(codes(&report), ["U0003"]);
    assert_eq!(report.diagnostics[0].severity, Severity::Warning);
}

#[test]
fn lines_with_a_lifetime_outlive_frames_and_the_rest_leave_after_one() {
    let mut renderer = builder(desktop())
        .with_pack(DebugPack::new())
        .build()
        .expect("renderer");
    let target = target(&mut renderer);
    let debug = draw(&mut renderer);
    debug.line(Vec3::ZERO, Vec3::X, DebugColor::WHITE);
    debug.line(Vec3::ZERO, Vec3::Y, LineStyle::new(DebugColor::RED).persistent(3));

    let counts: Vec<_> = (0..4)
        .map(|_| {
            frame(&mut renderer, target);
            draw(&mut renderer).line_count()
        })
        .collect();

    assert_eq!(
        counts,
        [2, 1, 1, 0],
        "counted after each frame: the second line shows for three frames"
    );
}
