// M2-12: the vertical slice rendered headless with `pathon-profile` watching every pass. GPU timings are printed once
// a second; with `--features tracy` the passes also stream to Tracy (zones, frame marks and a GPU context), with
// `--features puffin` to puffin. Usage: `profile [frames]` (default 600).

use std::sync::Arc;
use std::time::{Duration, Instant};

use pathon::rhi::{DeviceRequest, Features, Format, PhysicalSize};
use pathon::{FrameInput, Renderer, ViewInput};
use pathon_examples::{init_logging, orbit_camera};
use pathon_profile::{GpuPassTiming, ProfileSink, Profiler};
use pathon_test::UnlitMeshPack;

const SIZE: u32 = 512;
const DEFAULT_FRAMES: u64 = 600;
const PRINT_EVERY_FRAMES: u64 = 60;
const FRAME_BUDGET: Duration = Duration::from_millis(16);

struct PrintSink;

impl ProfileSink for PrintSink {
    #[expect(clippy::print_stderr, reason = "the example reports to the console")]
    fn gpu_timings(&self, frame: u64, period_ns: f32, passes: &[GpuPassTiming]) {
        if !frame.is_multiple_of(PRINT_EVERY_FRAMES) {
            return;
        }
        eprintln!("frame {frame}:");
        for timing in passes {
            eprintln!(
                "  {:<16} {:<16} {:>8.1} us",
                timing.pass,
                timing.label,
                timing.duration_ns(period_ns) / 1000.0
            );
        }
    }
}

fn profiler() -> Arc<Profiler> {
    let builder = Profiler::builder().with_sink(Arc::new(PrintSink));
    #[cfg(feature = "tracy")]
    let builder = builder.with_sink(Arc::new(pathon_profile::TracySink::new()));
    #[cfg(feature = "puffin")]
    let builder = builder.with_sink(Arc::new(pathon_profile::PuffinSink::new()));
    builder.build()
}

fn main() {
    init_logging();
    let frames = std::env::args()
        .nth(1)
        .and_then(|frames| frames.parse().ok())
        .unwrap_or(DEFAULT_FRAMES);
    let profiler = profiler();
    let mut renderer = Renderer::builder()
        .with_device_request(DeviceRequest {
            label: "profile example",
            optional_features: Features::TIMESTAMP_QUERY,
            ..DeviceRequest::default()
        })
        .with_feature(UnlitMeshPack::new())
        .with_frame_observer(Arc::<Profiler>::clone(&profiler))
        .build()
        .expect("renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba8Unorm)
        .expect("offscreen target");

    let start = Instant::now();
    for _ in 0..frames {
        let frame_start = Instant::now();
        let time_seconds = start.elapsed().as_secs_f64();
        let input = FrameInput {
            views: vec![ViewInput {
                target,
                camera: orbit_camera(time_seconds, 1.0),
            }],
            time_seconds,
            ..FrameInput::default()
        };
        renderer.render_frame(&input).expect("frame");
        std::thread::sleep(FRAME_BUDGET.saturating_sub(frame_start.elapsed()));
    }
    profiler.flush(renderer.device().as_ref());
    profiler.release(renderer.device().as_ref());
}
