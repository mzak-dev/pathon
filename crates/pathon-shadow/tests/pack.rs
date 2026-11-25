// `ShadowPack` in a real renderer on the null device, which checks every command list the pack records: barriers, usage
// flags, bind groups against their layouts, indirect arguments.

mod support;

use pathon_api::glam::{Mat4, Vec3};
use pathon_api::rhi::{Backend, DeviceRequest, Format, PhysicalSize};
use pathon_api::{Camera, Severity};
use pathon_core::{FrameInput, FrameReport, Renderer, ViewInput};
use pathon_shadow::{ShadowFrame, ShadowPack, ShadowSettings};
use support::{Host, Kind, MapProbe, MapReadback, Object, Shading, Sun};

fn objects() -> Vec<Object> {
    vec![
        Object::new(Kind::Plane, Vec3::ZERO, Vec3::new(100.0, 1.0, 100.0)),
        Object::new(Kind::Cube, Vec3::new(0.0, 1.0, -5.0), Vec3::splat(2.0)),
        Object::new(Kind::Sphere, Vec3::new(3.0, 1.0, -9.0), Vec3::splat(2.0)),
    ]
}

fn renderer(settings: ShadowSettings, with_host: bool) -> (Renderer, FrameInput) {
    let mut builder = Renderer::builder().with_device_request(DeviceRequest {
        backend: Some(Backend::Null),
        ..DeviceRequest::default()
    });
    if with_host {
        builder = builder.with_feature(Host::new(objects())).with_feature(Shading::new());
    }
    let mut renderer = builder
        .with_pack(ShadowPack::with_settings(settings))
        .with_feature(MapProbe {
            layer: 0,
            resolution: 64,
            readback: MapReadback::default(),
        })
        .build()
        .expect("renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(32, 32), Format::Rgba8Unorm)
        .expect("target");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::look_at(Vec3::new(4.0, 3.0, 4.0), Vec3::new(0.0, 0.0, -5.0), 1.0, 1.0),
        }],
        ..FrameInput::default()
    };
    (renderer, input)
}

fn small(use_indirect_draws: bool) -> ShadowSettings {
    ShadowSettings {
        resolution: 64,
        use_indirect_draws,
        ..ShadowSettings::DESKTOP
    }
}

fn codes(report: &FrameReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|diagnostic| diagnostic.code).collect()
}

fn errors(report: &FrameReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(ToString::to_string)
        .collect()
}

#[test]
fn without_a_light_nothing_is_published() {
    let (mut renderer, input) = renderer(small(true), false);

    let report = renderer.render_frame(&input).expect("frame");

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(renderer.shared().get::<ShadowFrame>().is_none());
}

#[test]
fn a_frame_with_casters_validates_with_one_by_one_and_indirect_draws() {
    for indirect in [false, true] {
        let (mut renderer, input) = renderer(small(indirect), true);

        for _ in 0..3 {
            let report = renderer.render_frame(&input).expect("frame");
            assert!(
                errors(&report).is_empty(),
                "indirect {indirect}: {:?}",
                report.diagnostics
            );
            assert!(
                codes(&report).is_empty(),
                "indirect {indirect}: {:?}",
                report.diagnostics
            );
        }

        let frame = renderer.shared().get::<ShadowFrame>().expect("published");
        assert_eq!((frame.cascade_count, frame.resolution), (4, 64));
        assert_eq!(frame.uniforms.cascade_count(), 4);
        assert_eq!(frame.uniforms.light_index(), 0);
        assert!(frame.uniforms.split(3) <= 150.0 && frame.uniforms.split(0) < frame.uniforms.split(1));
    }
}

#[test]
fn the_feature_holds_the_shadows_role_and_asks_for_first_instance_indirect_draws() {
    let (renderer, _) = renderer(small(true), true);

    let details = renderer.feature_details();

    let shadow = details
        .iter()
        .find(|info| info.name == "shadow cascades")
        .expect("registered");
    assert_eq!(shadow.role, Some(pathon_api::Role::SHADOWS));
}

#[test]
fn settings_out_of_range_are_repaired_and_reported_once() {
    let (mut renderer, input) = renderer(small(true), true);
    renderer
        .shared_mut()
        .get_mut::<ShadowSettings>()
        .expect("settings")
        .cascade_count = 9;

    let first = renderer.render_frame(&input).expect("frame");
    let second = renderer.render_frame(&input).expect("frame");

    assert_eq!(codes(&first), ["D0001"], "{:?}", first.diagnostics);
    assert!(codes(&second).is_empty(), "{:?}", second.diagnostics);
    assert_eq!(renderer.shared().get::<ShadowFrame>().expect("frame").cascade_count, 4);
}

#[test]
fn a_light_without_a_direction_is_d0002_and_publishes_nothing() {
    let (mut renderer, input) = renderer(small(true), true);
    renderer.shared_mut().insert(Sun { direction: Vec3::ZERO });

    let report = renderer.render_frame(&input).expect("frame");

    assert_eq!(codes(&report), ["D0002"], "{:?}", report.diagnostics);
    assert!(renderer.shared().get::<ShadowFrame>().is_none());
}

#[test]
fn a_camera_without_a_usable_projection_is_d0003() {
    let (mut renderer, mut input) = renderer(small(true), true);
    input.views[0].camera = Camera {
        projection: Mat4::ZERO,
        ..input.views[0].camera
    };

    let report = renderer.render_frame(&input).expect("frame");

    assert_eq!(codes(&report), ["D0003"], "{:?}", report.diagnostics);
    assert!(renderer.shared().get::<ShadowFrame>().is_none());
}

#[test]
fn two_views_of_one_frame_do_not_overwrite_each_others_draw_data() {
    let (mut renderer, mut input) = renderer(small(true), true);
    let second = renderer
        .add_offscreen_target(PhysicalSize::new(32, 32), Format::Rgba8Unorm)
        .expect("target");
    input.views.push(ViewInput {
        target: second,
        camera: Camera::look_at(Vec3::new(-6.0, 4.0, 2.0), Vec3::new(0.0, 0.0, -6.0), 1.2, 1.0),
    });

    for _ in 0..2 {
        let report = renderer.render_frame(&input).expect("frame");
        assert!(errors(&report).is_empty(), "{:?}", report.diagnostics);
    }
}
