#![cfg(feature = "gpu-tests")]

// The vertical slice on a real device: timestamps written by the GPU come back under the names of the graph passes.

use std::sync::Arc;

use pathon_api::Camera;
use pathon_api::glam::Vec3;
use pathon_api::rhi::{DeviceApi, DeviceRequest, Features, Format, PhysicalSize};
use pathon_core::{FrameInput, Renderer, ViewInput};
use pathon_profile::{CollectingSink, Profiler, SpanEvent};
use pathon_test::UnlitMeshPack;

#[test]
fn gpu_timings_of_the_vertical_slice_are_named_after_its_graph_passes() {
    let sink = Arc::new(CollectingSink::new());
    let profiler = Profiler::builder()
        .with_sink(Arc::<CollectingSink>::clone(&sink))
        .build();
    let mut renderer = Renderer::builder()
        .with_device_request(DeviceRequest {
            label: "profile",
            optional_features: Features::TIMESTAMP_QUERY,
            ..DeviceRequest::default()
        })
        .with_feature(UnlitMeshPack::new())
        .with_frame_observer(Arc::<Profiler>::clone(&profiler))
        .build()
        .expect("a GPU adapter is required for gpu-tests");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(64, 64), Format::Rgba8Unorm)
        .expect("target");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::look_at(Vec3::new(0.0, 0.0, 2.5), Vec3::ZERO, std::f32::consts::FRAC_PI_3, 1.0),
        }],
        ..FrameInput::default()
    };

    for _ in 0..4 {
        renderer.render_frame(&input).expect("frame");
    }
    profiler.flush(renderer.device().as_ref());

    let collected = sink.take();
    assert!(collected.spans.contains(&SpanEvent::Begin("unlit mesh")));
    assert!(collected.spans.contains(&SpanEvent::End("unlit mesh")));
    if !renderer.device().caps().has(Features::TIMESTAMP_QUERY) {
        assert!(collected.gpu.is_empty(), "no timestamps on this adapter");
        return;
    }
    let mesh: Vec<_> = collected
        .gpu
        .iter()
        .filter(|(_, timing)| timing.pass == "unlit mesh")
        .collect();
    assert_eq!(mesh.len(), 4, "one timed GPU pass per frame");
    assert!(mesh.iter().all(|(_, timing)| timing.end_ticks >= timing.begin_ticks));
    assert!(
        mesh.iter().any(|(_, timing)| timing.begin_ticks > 0),
        "the GPU wrote real timestamps"
    );
    assert!(collected.gpu.iter().any(|(_, timing)| timing.pass == "clear output"));
}
