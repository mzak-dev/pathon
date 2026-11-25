// `--bench N`: the frame time of the viewer without a window. Phase one renders N frames with no observer and divides
// the wall time by N (the CPU records and the GPU runs behind it, so this is the throughput); phase two renders with
// `pathon-profile` watching and adds up the GPU time of every pass by name (timestamps at the pass boundaries, so the
// gaps between passes are not counted).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pathon::FrameInput;
use pathon::rhi::DeviceApi;
use pathon_pbr::{ClusterGrid, ClusterSettings};
use pathon_profile::{GpuPassTiming, ProfileSink, Profiler};

use super::screenshot::{SCREENSHOT_FORMAT, ScreenshotRequest};
use super::session::Session;
use super::stack::ViewerConfig;
use crate::gltf_loader::LoadedScene;

const WARM_UP_FRAMES: u32 = 8;
const PROFILED_FRAMES: u32 = 60;
const FRAME_DELTA_SECONDS: f32 = 1.0 / 60.0;

#[derive(Default)]
struct PassSums {
    nanoseconds: BTreeMap<(&'static str, &'static str), (f64, u32)>,
}

#[derive(Default)]
struct SumSink {
    sums: Mutex<PassSums>,
}

impl ProfileSink for SumSink {
    fn gpu_timings(&self, _frame: u64, period_ns: f32, passes: &[GpuPassTiming]) {
        let mut sums = self.sums.lock().unwrap_or_else(PoisonError::into_inner);
        for timing in passes {
            let entry = sums.nanoseconds.entry((timing.pass, timing.label)).or_default();
            entry.0 += timing.duration_ns(period_ns);
            entry.1 += 1;
        }
    }
}

#[derive(Clone, Debug)]
pub struct PassTime {
    pub pass: &'static str,
    pub label: &'static str,
    pub milliseconds: f64,
}

#[derive(Clone, Debug)]
pub struct BenchReport {
    pub frames: u32,
    pub size: (u32, u32),
    pub lights: usize,
    pub clusters: u32,
    pub instances: usize,
    /// Wall time per frame with the GPU kept busy: the throughput of the whole renderer.
    pub frame_milliseconds: f64,
    /// Time `render_frame` takes on the CPU (recording and submitting), averaged.
    pub cpu_milliseconds: f64,
    pub passes: Vec<PassTime>,
}

impl BenchReport {
    pub fn gpu_milliseconds(&self) -> f64 {
        self.passes.iter().map(|pass| pass.milliseconds).sum()
    }
}

fn frame_input(session: &Session, request: &ScreenshotRequest, target: pathon::TargetId) -> FrameInput {
    FrameInput {
        time_seconds: 0.0,
        delta_seconds: FRAME_DELTA_SECONDS,
        views: vec![pathon::ViewInput {
            target,
            camera: session.camera_for(request.aspect_ratio()),
        }],
    }
}

/// Times `frames` frames of the camera of `request`.
pub fn run_benchmark(
    scene: &LoadedScene,
    config: &ViewerConfig,
    request: &ScreenshotRequest,
    frames: u32,
) -> Result<BenchReport, String> {
    let config = ViewerConfig {
        gpu_timestamps: true,
        ..config.clone()
    };
    let renderer = config.builder().build().map_err(|error| error.to_string())?;
    log::info!("adapter: {}", renderer.device().caps().adapter.name);
    let mut session = Session::new(renderer, scene, &config).map_err(|error| error.to_string())?;
    request.place_camera(&mut session)?;
    session.settings = request.settings();
    let size = request.render_size();
    let target = session
        .renderer
        .add_offscreen_target(size, SCREENSHOT_FORMAT)
        .map_err(|error| error.to_string())?;
    let input = frame_input(&session, request, target);
    let device = Arc::clone(session.renderer.device());

    for _ in 0..WARM_UP_FRAMES {
        session.render_input(&input).map_err(|error| error.to_string())?;
    }
    device.wait_idle().map_err(|error| error.to_string())?;

    let mut cpu = Duration::ZERO;
    let started = Instant::now();
    for _ in 0..frames {
        let before = Instant::now();
        session.render_input(&input).map_err(|error| error.to_string())?;
        cpu += before.elapsed();
    }
    device.wait_idle().map_err(|error| error.to_string())?;
    let wall = started.elapsed();

    let sink = Arc::new(SumSink::default());
    let profiler = Profiler::builder().with_sink(Arc::<SumSink>::clone(&sink)).build();
    session.renderer.set_frame_observer(Some(Arc::<Profiler>::clone(&profiler)));
    for _ in 0..PROFILED_FRAMES {
        session.render_input(&input).map_err(|error| error.to_string())?;
    }
    profiler.flush(device.as_ref());
    session.renderer.set_frame_observer(None);
    profiler.release(device.as_ref());

    let sums = sink.sums.lock().unwrap_or_else(PoisonError::into_inner);
    let passes = sums
        .nanoseconds
        .iter()
        .map(|(&(pass, label), &(total, count))| PassTime {
            pass,
            label,
            milliseconds: total / f64::from(count.max(1)) / 1.0e6,
        })
        .collect();
    let frames_f = f64::from(frames.max(1));
    Ok(BenchReport {
        frames,
        size: (size.width, size.height),
        lights: session.assets.stats.lights,
        clusters: ClusterGrid::new(size, &ClusterSettings::default()).count(),
        instances: session.assets.stats.instances,
        frame_milliseconds: wall.as_secs_f64() * 1000.0 / frames_f,
        cpu_milliseconds: cpu.as_secs_f64() * 1000.0 / frames_f,
        passes,
    })
}

/// A table for the log and for docs/scenes.md.
pub fn format_report(report: &BenchReport) -> String {
    let mut text = format!(
        "{}x{}, {} instances, {} lights, {} clusters\n\
         wall {:.2} ms/frame ({:.0} fps), CPU record+submit {:.2} ms, GPU passes {:.2} ms\n",
        report.size.0,
        report.size.1,
        report.instances,
        report.lights,
        report.clusters,
        report.frame_milliseconds,
        1000.0 / report.frame_milliseconds.max(f64::EPSILON),
        report.cpu_milliseconds,
        report.gpu_milliseconds(),
    );
    for pass in &report.passes {
        text.push_str(&format!(
            "  {:<24} {:<24} {:>7.3} ms\n",
            pass.pass, pass.label, pass.milliseconds
        ));
    }
    text
}
