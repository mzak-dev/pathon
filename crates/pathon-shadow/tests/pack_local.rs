// The shadows of point and spot lights through a real renderer on the null device, which checks every command list the
// pack records: barriers, usage flags, bind groups against their layouts, indirect arguments, the persistent atlas.

mod support;

use pathon_api::glam::Vec3;
use pathon_api::rhi::{Backend, DeviceRequest, Format, PhysicalSize};
use pathon_api::{Camera, Severity};
use pathon_core::{FrameInput, FrameReport, Renderer, ViewInput};
use pathon_shadow::{
    LocalShadowFrame, LocalShadowLight, LocalShadowReport, LocalShadowShape, ShadowFrame, ShadowPack, ShadowSettings,
};
use support::local::{LocalShading, Mode, ShadingMode};
use support::{Host, Kind, LocalLights, Object};

fn objects() -> Vec<Object> {
    vec![
        Object::new(Kind::Plane, Vec3::ZERO, Vec3::new(100.0, 1.0, 100.0)),
        Object::new(Kind::Cube, Vec3::new(0.0, 1.0, -5.0), Vec3::splat(2.0)),
        Object::new(Kind::Sphere, Vec3::new(3.0, 1.0, -9.0), Vec3::splat(2.0)),
    ]
}

fn point(index: u32, at: Vec3) -> LocalShadowLight {
    LocalShadowLight {
        key: u64::from(index) + 10,
        light_index: index,
        shape: LocalShadowShape::Point,
        position: at,
        direction: Vec3::NEG_Y,
        range: 20.0,
        intensity: 1000.0,
    }
}

fn spot(index: u32, at: Vec3) -> LocalShadowLight {
    LocalShadowLight {
        shape: LocalShadowShape::Spot { outer_angle: 0.6 },
        direction: Vec3::new(0.0, -1.0, -0.2).normalize(),
        ..point(index, at)
    }
}

fn small(use_indirect_draws: bool) -> ShadowSettings {
    ShadowSettings {
        resolution: 64,
        use_indirect_draws,
        local_atlas_size: 1024,
        local_min_resolution: 128,
        local_spot_resolution: 256,
        local_point_resolution: 128,
        ..ShadowSettings::DESKTOP
    }
}

fn renderer(settings: ShadowSettings, lights: Vec<LocalShadowLight>) -> (Renderer, FrameInput) {
    let mut renderer = Renderer::builder()
        .with_device_request(DeviceRequest {
            backend: Some(Backend::Null),
            ..DeviceRequest::default()
        })
        .with_feature(Host::new(objects()))
        .with_feature(LocalShading::new())
        .with_pack(ShadowPack::with_settings(settings))
        .build()
        .expect("renderer");
    renderer.shared_mut().insert(LocalLights {
        lights,
        with_sun: false,
    });
    renderer.shared_mut().insert(ShadingMode(Mode::Lit));
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

fn errors(report: &FrameReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(ToString::to_string)
        .collect()
}

fn codes(report: &FrameReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|diagnostic| diagnostic.code).collect()
}

fn local_report(renderer: &Renderer) -> LocalShadowReport {
    renderer
        .shared()
        .get::<LocalShadowReport>()
        .expect("the pack reports")
        .clone()
}

fn three_lights() -> Vec<LocalShadowLight> {
    vec![
        point(0, Vec3::new(1.0, 3.0, -3.0)),
        spot(1, Vec3::new(-2.0, 4.0, -4.0)),
        point(2, Vec3::new(2.0, 2.5, -7.0)),
    ]
}

#[test]
fn a_frame_with_local_lights_validates_with_one_by_one_and_indirect_draws() {
    for indirect in [false, true] {
        let (mut renderer, input) = renderer(small(indirect), three_lights());

        for frame in 0..3 {
            let report = renderer.render_frame(&input).expect("frame");
            assert!(
                errors(&report).is_empty(),
                "indirect {indirect} frame {frame}: {:?}",
                report.diagnostics
            );
            assert!(
                codes(&report).is_empty(),
                "indirect {indirect} frame {frame}: {:?}",
                report.diagnostics
            );
        }

        let frame = renderer.shared().get::<LocalShadowFrame>().expect("published");
        assert_eq!((frame.light_count, frame.atlas_size), (3, 1024));
        assert!(renderer.shared().get::<ShadowFrame>().is_none(), "there is no sun");
    }
}

#[test]
fn only_what_changed_is_drawn_again() {
    let (mut renderer, input) = renderer(small(true), three_lights());

    renderer.render_frame(&input).expect("first frame");
    let first = local_report(&renderer);
    renderer.render_frame(&input).expect("second frame");
    let second = local_report(&renderer);

    assert_eq!(first.rendered_tiles, 6 + 1 + 6);
    assert_eq!((first.cached_tiles, first.lights.len()), (0, 3));
    assert!(first.drawn_casters > 0);
    assert_eq!(
        (second.rendered_tiles, second.cached_tiles, second.drawn_casters),
        (0, 13, 0),
        "nothing moved, nothing is drawn"
    );
}

#[test]
fn a_light_that_moves_redraws_its_own_tiles_only() {
    let (mut renderer, input) = renderer(small(true), three_lights());
    renderer.render_frame(&input).expect("first frame");

    let mut moved = three_lights();
    moved[1].position += Vec3::new(0.1, 0.0, 0.0);
    renderer.shared_mut().insert(LocalLights {
        lights: moved,
        with_sun: false,
    });
    renderer.render_frame(&input).expect("second frame");

    let report = local_report(&renderer);
    assert_eq!((report.rendered_tiles, report.cached_tiles), (1, 12));
}

#[test]
fn invalidating_the_maps_redraws_everything() {
    let (mut renderer, input) = renderer(small(true), three_lights());
    renderer.render_frame(&input).expect("first frame");

    renderer
        .shared_mut()
        .get_mut::<pathon_shadow::ShadowScene>()
        .expect("scene")
        .invalidate_local_maps();
    renderer.render_frame(&input).expect("second frame");

    assert_eq!(local_report(&renderer).rendered_tiles, 13);
}

#[test]
fn a_light_outside_the_view_renders_no_map() {
    let behind = point(5, Vec3::new(4.0, 3.0, 60.0));
    let (mut renderer, input) = renderer(small(true), vec![behind]);

    let report = renderer.render_frame(&input).expect("frame");

    assert!(errors(&report).is_empty(), "{:?}", report.diagnostics);
    let local = local_report(&renderer);
    assert_eq!((local.candidates, local.out_of_view, local.rendered_tiles), (1, 1, 0));
    assert_eq!(local.lights, []);
    assert!(
        renderer.shared().get::<LocalShadowFrame>().is_none(),
        "no map, so the lighting shader keeps the permutation without local shadows"
    );
}

#[test]
fn at_most_max_local_lights_get_maps_and_the_strongest_are_chosen() {
    let mut lights = three_lights();
    lights[0].intensity = 100.0;
    let (mut renderer, input) = renderer(
        ShadowSettings {
            max_local_lights: 2,
            ..small(true)
        },
        lights,
    );

    renderer.render_frame(&input).expect("frame");

    let report = local_report(&renderer);
    let chosen: Vec<u32> = report.lights.iter().map(|light| light.light_index).collect();
    assert_eq!(chosen.len(), 2);
    assert!(!chosen.contains(&0), "the dim light has no map: {chosen:?}");
}

#[test]
fn turning_local_shadows_off_publishes_nothing() {
    let (mut renderer, input) = renderer(
        ShadowSettings {
            max_local_lights: 0,
            ..small(true)
        },
        three_lights(),
    );

    let report = renderer.render_frame(&input).expect("frame");

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(renderer.shared().get::<LocalShadowFrame>().is_none());
    assert!(renderer.shared().get::<LocalShadowReport>().is_none());
}

#[test]
fn a_full_atlas_is_reported_once_and_the_light_keeps_working_without_a_map() {
    // 256 x 256 holds four tiles of the smallest size, a point light needs six.
    let settings = ShadowSettings {
        local_atlas_size: 256,
        ..small(true)
    };
    let (mut renderer, input) = renderer(settings, vec![point(0, Vec3::new(1.0, 3.0, -3.0))]);

    let first = renderer.render_frame(&input).expect("frame");
    let second = renderer.render_frame(&input).expect("frame");

    assert_eq!(codes(&first), ["D0101"]);
    assert!(codes(&second).is_empty(), "reported once: {:?}", second.diagnostics);
    assert_eq!(local_report(&renderer).dropped_for_space, 1);
}

#[test]
fn an_unusable_light_is_reported_and_skipped() {
    let mut broken = point(0, Vec3::new(1.0, 3.0, -3.0));
    broken.range = f32::NAN;
    let (mut renderer, input) = renderer(small(true), vec![broken, spot(1, Vec3::new(-2.0, 4.0, -4.0))]);

    let report = renderer.render_frame(&input).expect("frame");

    assert_eq!(codes(&report), ["D0102"]);
    assert_eq!(local_report(&renderer).lights.len(), 1);
}

#[test]
fn two_views_of_one_frame_share_the_maps() {
    let (mut renderer, mut input) = renderer(small(true), three_lights());
    let second = renderer
        .add_offscreen_target(PhysicalSize::new(32, 32), Format::Rgba8Unorm)
        .expect("second target");
    input.views.push(ViewInput {
        target: second,
        camera: Camera::look_at(Vec3::new(-4.0, 3.0, 4.0), Vec3::new(0.0, 0.0, -5.0), 1.0, 1.0),
    });

    for frame in 0..2 {
        let report = renderer.render_frame(&input).expect("frame");
        assert!(errors(&report).is_empty(), "frame {frame}: {:?}", report.diagnostics);
    }

    let report = local_report(&renderer);
    assert_eq!(report.lights.len(), 3);
    assert_eq!(report.rendered_tiles, 0, "the second frame reuses everything");
}

#[test]
fn the_sun_and_the_local_lights_work_in_one_frame() {
    let mut renderer_and_input = renderer(small(true), three_lights());
    renderer_and_input.0.shared_mut().insert(LocalLights {
        lights: three_lights(),
        with_sun: true,
    });
    let (mut renderer, input) = renderer_and_input;

    let report = renderer.render_frame(&input).expect("frame");

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(renderer.shared().get::<ShadowFrame>().is_some());
    assert!(renderer.shared().get::<LocalShadowFrame>().is_some());
}
