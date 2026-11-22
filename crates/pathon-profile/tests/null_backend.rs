// The profiler against a real renderer on the null backend, which rejects bad timestamp indices and resolves.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, Color, ColorAttachment, Command, ComputePassDesc, Features, Format, PhysicalSize, RenderPassDesc, RhiError,
    Transfer,
};
use pathon_api::{Camera, RecordCtx, RenderFeature};
use pathon_core::{FrameInput, RenderError, Renderer, RendererBuilder, ViewInput};
use pathon_profile::{CollectingSink, GpuPassTiming, Profiler, ProfilerBuilder, SpanEvent};
use pathon_rhi::{Device, NullDevice};

// Records one graph pass with a compute pass and two render passes inside.
struct BusyPass;

impl RenderFeature for BusyPass {
    fn name(&self) -> &'static str {
        "busy pass"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let texture = ctx
            .graph()
            .create_texture("scratch", GraphTextureDesc::output(Format::Rgba8Unorm));
        let mut pass = ctx.graph().add_pass(PassDesc::new("busy", Stage::Opaque).side_effect());
        let written = pass.write(texture, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(written)?;
            drop(pass.commands().compute_pass(ComputePassDesc {
                label: "cull",
                timestamps: None,
            }));
            for label in ["draw opaque", "draw decals"] {
                drop(pass.commands().render_pass(RenderPassDesc {
                    label,
                    color: vec![ColorAttachment::clear(view, Color::BLACK)],
                    depth: None,
                    timestamps: None,
                }));
            }
            Ok(())
        });
        Ok(())
    }
}

// Draws one pass; while `failing` is set the pass reports a lost device instead.
struct FlakyPass {
    failing: Arc<AtomicBool>,
}

impl RenderFeature for FlakyPass {
    fn name(&self) -> &'static str {
        "flaky pass"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let texture = ctx
            .graph()
            .create_texture("flaky", GraphTextureDesc::output(Format::Rgba8Unorm));
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("flaky", Stage::PostProcess).side_effect());
        let written = pass.write(texture, Access::COLOR_ATTACHMENT);
        let failing = Arc::clone(&self.failing);
        pass.record(move |pass| {
            if failing.load(Ordering::Relaxed) {
                return Err(RhiError::DeviceLost);
            }
            let view = pass.view(written)?;
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "flaky draw",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        Ok(())
    }
}

fn renderer_with(builder: ProfilerBuilder, features: Features) -> (Renderer, Arc<Profiler>, Arc<CollectingSink>) {
    let sink = Arc::new(CollectingSink::new());
    let profiler = builder.with_sink(Arc::<CollectingSink>::clone(&sink)).build();
    let renderer = RendererBuilder::new()
        .with_device(Arc::new(Device::from(NullDevice::with_features(features))))
        .with_feature(BusyPass)
        .with_frame_observer(Arc::<Profiler>::clone(&profiler))
        .build()
        .expect("renderer");
    (renderer, profiler, sink)
}

fn with_timestamps() -> Features {
    Features::TIMESTAMP_QUERY
}

fn one_view(renderer: &mut Renderer) -> FrameInput {
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(32, 32), Format::Rgba8Unorm)
        .expect("target");
    FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    }
}

fn names(timings: &[(u64, GpuPassTiming)]) -> Vec<(&'static str, &'static str)> {
    timings.iter().map(|(_, timing)| (timing.pass, timing.label)).collect()
}

#[test]
fn spans_and_gpu_timings_carry_the_names_of_the_graph_passes() {
    let (mut renderer, profiler, sink) = renderer_with(Profiler::builder(), with_timestamps());
    let input = one_view(&mut renderer);

    renderer.render_frame(&input).expect("frame");
    profiler.flush(renderer.device().as_ref());

    let collected = sink.take();
    assert_eq!(
        collected.spans,
        [
            SpanEvent::Begin("clear output"),
            SpanEvent::End("clear output"),
            SpanEvent::Begin("busy"),
            SpanEvent::End("busy"),
        ]
    );
    assert_eq!(
        names(&collected.gpu),
        [
            ("clear output", "clear output"),
            ("busy", "cull"),
            ("busy", "draw opaque"),
            ("busy", "draw decals"),
        ]
    );
    assert!(collected.gpu.iter().all(|(frame, _)| *frame == 0));
}

#[test]
fn timings_arrive_after_the_latency_and_flush_reads_the_rest() {
    let (mut renderer, profiler, sink) = renderer_with(Profiler::builder().with_gpu_latency(2), with_timestamps());
    let input = one_view(&mut renderer);

    renderer.render_frame(&input).expect("frame 0");
    renderer.render_frame(&input).expect("frame 1");
    assert!(sink.take().gpu.is_empty(), "frame 0 is not old enough yet");

    renderer.render_frame(&input).expect("frame 2");
    let frames: Vec<u64> = sink.take().gpu.iter().map(|(frame, _)| *frame).collect();
    assert!(frames.iter().all(|frame| *frame == 0) && !frames.is_empty());

    profiler.flush(renderer.device().as_ref());
    let rest: Vec<u64> = sink.take().gpu.iter().map(|(frame, _)| *frame).collect();
    assert!(rest.contains(&1) && rest.contains(&2));
    assert!(!rest.contains(&0), "a frame is reported once");
}

#[test]
fn passes_over_the_capacity_are_run_but_not_timed() {
    let (mut renderer, profiler, sink) =
        renderer_with(Profiler::builder().with_gpu_pass_capacity(2), with_timestamps());
    let input = one_view(&mut renderer);

    renderer.render_frame(&input).expect("frame");
    profiler.flush(renderer.device().as_ref());

    assert_eq!(
        names(&sink.take().gpu),
        [("clear output", "clear output"), ("busy", "cull")]
    );
}

#[test]
fn a_device_without_timestamps_still_gets_cpu_spans() {
    let (mut renderer, profiler, sink) = renderer_with(Profiler::builder(), Features::empty());
    let input = one_view(&mut renderer);

    renderer.render_frame(&input).expect("frame");
    profiler.flush(renderer.device().as_ref());

    let collected = sink.take();
    assert_eq!(collected.gpu, []);
    assert_eq!(collected.spans.len(), 4);
}

#[test]
fn a_cpu_only_profiler_adds_nothing_to_the_command_list() {
    let (mut renderer, _profiler, _sink) = renderer_with(Profiler::builder().without_gpu_timings(), with_timestamps());
    let input = one_view(&mut renderer);

    let commands = renderer.record_frame(&input).expect("recorded");

    let timed_passes = commands.commands().iter().any(|command| match command {
        Command::BeginRenderPass(desc) => desc.timestamps.is_some(),
        Command::BeginComputePass(desc) => desc.timestamps.is_some(),
        _ => false,
    });
    let resolves = commands
        .commands()
        .iter()
        .any(|command| matches!(command, Command::Transfer(transfer) if matches!(**transfer, Transfer::ResolveQueries { .. })));
    assert!(!timed_passes && !resolves);
}

#[test]
fn a_profiled_frame_resolves_exactly_the_queries_it_wrote() {
    let (mut renderer, _profiler, _sink) = renderer_with(Profiler::builder(), with_timestamps());
    let input = one_view(&mut renderer);

    let commands = renderer.record_frame(&input).expect("recorded");

    let resolved: Vec<_> = commands
        .commands()
        .iter()
        .filter_map(|command| match command {
            Command::Transfer(transfer) => match &**transfer {
                Transfer::ResolveQueries { queries, .. } => Some(queries.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(resolved, vec![0..8], "four GPU passes, two queries each");
}

#[test]
fn an_abandoned_frame_is_never_reported_and_the_next_one_is() {
    let failing = Arc::new(AtomicBool::new(true));
    let sink = Arc::new(CollectingSink::new());
    let profiler = Profiler::builder()
        .with_sink(Arc::<CollectingSink>::clone(&sink))
        .build();
    let mut renderer = RendererBuilder::new()
        .with_device(Arc::new(Device::from(NullDevice::new())))
        .with_feature(FlakyPass {
            failing: Arc::clone(&failing),
        })
        .with_frame_observer(Arc::<Profiler>::clone(&profiler))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer);

    assert!(matches!(renderer.render_frame(&input), Err(RenderError::DeviceLost)));
    profiler.flush(renderer.device().as_ref());
    assert!(
        sink.take().gpu.is_empty(),
        "nothing of the abandoned frame was submitted"
    );

    failing.store(false, Ordering::Relaxed);
    renderer.render_frame(&input).expect("the next frame");
    profiler.flush(renderer.device().as_ref());
    let timings = sink.take().gpu;
    assert_eq!(
        names(&timings),
        [("clear output", "clear output"), ("flaky", "flaky draw")]
    );
    assert!(
        timings.iter().all(|(frame, _)| *frame == 0),
        "an abandoned frame does not use up a number"
    );
}

#[test]
fn profiling_goes_on_after_the_device_was_replaced() {
    let (mut renderer, profiler, sink) = renderer_with(Profiler::builder(), with_timestamps());
    let input = one_view(&mut renderer);
    renderer.render_frame(&input).expect("frame before");
    sink.take();

    renderer
        .reset(Arc::new(Device::from(NullDevice::new())))
        .expect("reset onto a new device");
    let input = one_view(&mut renderer);
    renderer.render_frame(&input).expect("frame after");
    profiler.flush(renderer.device().as_ref());

    assert_eq!(names(&sink.take().gpu).len(), 4);
}

#[test]
fn released_objects_are_made_again_on_the_next_frame() {
    let (mut renderer, profiler, sink) = renderer_with(Profiler::builder(), with_timestamps());
    let input = one_view(&mut renderer);
    renderer.render_frame(&input).expect("frame");

    profiler.release(renderer.device().as_ref());
    renderer.render_frame(&input).expect("frame after release");
    profiler.flush(renderer.device().as_ref());

    assert_ne!(sink.take().gpu, []);
}
