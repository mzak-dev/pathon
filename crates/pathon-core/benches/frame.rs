// CPU cost of one frame inside the core: prepare_view, graph building by packs, cached compilation, memory
// allocation from the pool and the barrier plan. `record_frame` stops before submission, so no backend work is
// measured. Results: docs/bench/README.md.

use divan::Bencher;
use pathon_api::Camera;
use pathon_core::{FrameInput, Renderer, ViewInput};
use pathon_rhi::{Backend, Color, DeviceRequest, Format, PhysicalSize};
use pathon_test::{GeometryPack, OnDemandConsumer, UnlitMeshPack, UpscalerPack};

fn main() {
    divan::main();
}

#[divan::bench(args = [1, 4])]
fn record_frame(bencher: Bencher<'_, '_>, views: usize) {
    let mut renderer = Renderer::builder()
        .with_device_request(DeviceRequest {
            backend: Some(Backend::Null),
            ..DeviceRequest::default()
        })
        .with_feature(GeometryPack::new("geometry", Color::BLACK, 0))
        .with_feature(OnDemandConsumer)
        .with_feature(UpscalerPack::new(0.67))
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let views = (0..views)
        .map(|_| {
            let target = renderer
                .add_offscreen_target(PhysicalSize::new(1920, 1080), Format::Rgba16Float)
                .expect("offscreen target");
            ViewInput {
                target,
                camera: Camera::default(),
            }
        })
        .collect();
    let mut input = FrameInput {
        views,
        ..FrameInput::default()
    };
    bencher.bench_local(|| {
        input.frame_index += 1;
        renderer.record_frame(&input).expect("frame").commands().len()
    });
}
