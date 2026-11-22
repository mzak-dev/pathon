// Probe for `cargo xtask bench` (M4-17, ADR-0035): CPU cost of one frame on the null backend, plus the counts that do
// not depend on the machine (commands recorded, allocations made). Run by hand: cargo bench -p pathon-core --bench
// probe

use std::time::{Duration, Instant};

use pathon_api::Camera;
use pathon_bench::{CountingAllocator, Metric, count_allocations, emit};
use pathon_core::{FrameInput, Renderer, ViewInput};
use pathon_rhi::{Backend, Color, DeviceRequest, Format, PhysicalSize};
use pathon_test::{GeometryPack, OnDemandConsumer, UnlitMeshPack, UpscalerPack};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SAMPLES: usize = 31;
const FRAMES_PER_SAMPLE: u32 = 200;
const WARMUP_FRAMES: usize = 30;
const WARMUP_BATCHES: usize = 10;

fn renderer_with_views(views: usize) -> (Renderer, FrameInput) {
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
    let input = FrameInput {
        views,
        ..FrameInput::default()
    };
    (renderer, input)
}

fn record(renderer: &mut Renderer, input: &FrameInput) -> usize {
    renderer.record_frame(input).expect("frame").commands().len()
}

fn measure(views: usize) {
    let (mut renderer, input) = renderer_with_views(views);
    for _ in 0..WARMUP_FRAMES {
        record(&mut renderer, &input);
    }

    let (commands, used) = count_allocations(|| record(&mut renderer, &input));
    let mut batch = || {
        let start = Instant::now();
        for _ in 0..FRAMES_PER_SAMPLE {
            record(&mut renderer, &input);
        }
        start.elapsed() / FRAMES_PER_SAMPLE
    };
    // The CPU needs a moment to reach its steady clock: the first batches would only add noise.
    for _ in 0..WARMUP_BATCHES {
        batch();
    }
    let samples: Vec<Duration> = (0..SAMPLES).map(|_| batch()).collect();

    emit(&Metric::timing(&format!("core.frame.record_{views}view"), &samples));
    emit(&Metric::count(&format!("core.frame.commands_{views}view"), commands));
    emit(&Metric::count(
        &format!("core.frame.allocations_{views}view"),
        used.allocations,
    ));
    emit(&Metric::bytes(
        &format!("core.frame.allocated_bytes_{views}view"),
        used.bytes,
    ));
}

fn main() {
    measure(1);
    measure(4);
}
