// `IblPack` in a real renderer on the null device, which checks every command list the builder records: barriers,
// usage flags, bind groups against their layouts.

use std::sync::Arc;

use pathon_api::Camera;
use pathon_api::rhi::{Backend, DeviceRequest, Format, PhysicalSize};
use pathon_core::{FrameInput, Renderer, ViewInput};
use pathon_ibl::{EquirectImage, IblEnvironment, IblPack, IblSettings, IblSource, SyntheticSky};

fn settings() -> IblSettings {
    IblSettings {
        cube_size: 32,
        irradiance_size: 8,
        prefilter_samples: 16,
        lut_size: 16,
        lut_samples: 16,
    }
}

fn picture() -> Arc<EquirectImage> {
    Arc::new(SyntheticSky::default().image(64, 32).expect("picture"))
}

fn renderer(source: Option<IblSource>) -> (Renderer, FrameInput) {
    let mut builder = Renderer::builder()
        .with_device_request(DeviceRequest {
            backend: Some(Backend::Null),
            ..DeviceRequest::default()
        })
        .with_pack(IblPack::new());
    if let Some(source) = source {
        builder = builder.with_shared(source);
    }
    let mut renderer = builder.build().expect("renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(16, 16), Format::Rgba8Unorm)
        .expect("target");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    (renderer, input)
}

#[test]
fn without_a_source_nothing_is_published() {
    let (mut renderer, input) = renderer(None);
    let report = renderer.render_frame(&input).expect("frame");
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(renderer.shared().get::<IblEnvironment>().is_none());
}

#[test]
fn the_environment_is_built_once_per_source_and_rebuilt_when_it_changes() {
    let image = picture();
    let (mut renderer, input) = renderer(Some(IblSource::new(Arc::clone(&image), settings())));

    let report = renderer.render_frame(&input).expect("frame");
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let first = *renderer
        .shared()
        .get::<IblEnvironment>()
        .expect("published after the first frame");
    assert_eq!(first.generation, 1);
    assert_eq!(first.prefiltered_levels, 3);

    renderer.render_frame(&input).expect("frame");
    assert_eq!(
        renderer.shared().get::<IblEnvironment>().copied(),
        Some(first),
        "the same source is not built again"
    );

    renderer.shared_mut().insert(IblSource::new(picture(), settings()));
    renderer.render_frame(&input).expect("frame");
    let second = *renderer.shared().get::<IblEnvironment>().expect("rebuilt");
    assert_eq!(second.generation, 2);
    assert_ne!(second.prefiltered.texture, first.prefiltered.texture);
    assert_eq!(
        second.brdf_lut, first.brdf_lut,
        "the table is cached across environments"
    );
}

#[test]
fn a_bad_source_is_reported_once_and_keeps_the_previous_environment() {
    let (mut renderer, input) = renderer(Some(IblSource::new(picture(), settings())));
    renderer.render_frame(&input).expect("frame");
    let good = *renderer.shared().get::<IblEnvironment>().expect("built");

    let broken = IblSettings {
        cube_size: 33,
        ..settings()
    };
    renderer.shared_mut().insert(IblSource::new(picture(), broken));
    let report = renderer.render_frame(&input).expect("frame");
    assert_eq!(report.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(), ["I0001"]);
    assert_eq!(renderer.shared().get::<IblEnvironment>().copied(), Some(good));

    let report = renderer.render_frame(&input).expect("frame");
    assert!(
        report.diagnostics.is_empty(),
        "not retried every frame: {:?}",
        report.diagnostics
    );
}
