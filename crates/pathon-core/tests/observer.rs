// A frame observer (a profiler, ADR-0026) sees every frame and pass without the core knowing what it is.

use std::sync::{Arc, Mutex};

use pathon_api::graph::{FrameObserver, GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, Backend, Color, ColorAttachment, Command, CommandList, DeviceApi, DeviceRequest, Format, PassTimestamps,
    PhysicalSize, QuerySetDesc, RenderPassDesc, RhiError,
};
use pathon_api::{Camera, RecordCtx, RenderFeature};
use pathon_core::{FrameInput, RenderError, Renderer, RendererBuilder, ViewInput};

#[derive(Debug, PartialEq, Eq)]
enum Event {
    BeginFrame(u64),
    BeginPass(&'static str),
    EndPass(&'static str),
    GpuPass(&'static str, &'static str),
    EndFrame,
    Committed,
    Abandoned,
    DeviceReset,
}

#[derive(Default)]
struct Journal(Mutex<Vec<Event>>);

impl Journal {
    fn push(&self, event: Event) {
        self.0.lock().expect("journal").push(event);
    }

    fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.0.lock().expect("journal"))
    }
}

impl FrameObserver for Journal {
    fn begin_frame(&self, _device: &dyn DeviceApi, frame: u64) {
        self.push(Event::BeginFrame(frame));
    }

    fn begin_pass(&self, pass: &'static str) {
        self.push(Event::BeginPass(pass));
    }

    fn end_pass(&self, pass: &'static str) {
        self.push(Event::EndPass(pass));
    }

    fn gpu_pass(&self, device: &dyn DeviceApi, pass: &'static str, label: &'static str) -> Option<PassTimestamps> {
        self.push(Event::GpuPass(pass, label));
        let query_set = device
            .create_query_set(&QuerySetDesc {
                label: "journal",
                count: 2,
            })
            .ok()?;
        Some(PassTimestamps {
            query_set,
            begin_index: 0,
            end_index: 1,
        })
    }

    fn end_frame(&self, _device: &dyn DeviceApi, commands: &mut CommandList) {
        self.push(Event::EndFrame);
        commands.push_debug_group("observer");
        commands.pop_debug_group();
    }

    fn frame_committed(&self, _device: &dyn DeviceApi) {
        self.push(Event::Committed);
    }

    fn frame_abandoned(&self) {
        self.push(Event::Abandoned);
    }

    fn device_reset(&self) {
        self.push(Event::DeviceReset);
    }
}

struct DrawsOnePass;

impl RenderFeature for DrawsOnePass {
    fn name(&self) -> &'static str {
        "draws one pass"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let texture = ctx
            .graph()
            .create_texture("scratch", GraphTextureDesc::output(Format::Rgba8Unorm));
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("scratch pass", Stage::Opaque).side_effect());
        let written = pass.write(texture, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(written)?;
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "scratch",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        Ok(())
    }
}

struct LosesDevice;

impl RenderFeature for LosesDevice {
    fn name(&self) -> &'static str {
        "loses device"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let pass = ctx
            .graph()
            .add_pass(PassDesc::new("loses device", Stage::PostProcess).side_effect());
        pass.record(|_| Err(RhiError::DeviceLost));
        Ok(())
    }
}

fn null_builder() -> RendererBuilder {
    Renderer::builder().with_device_request(DeviceRequest {
        backend: Some(Backend::Null),
        ..DeviceRequest::default()
    })
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

#[test]
fn a_frame_is_observed_from_its_start_to_its_commit() {
    let journal = Arc::new(Journal::default());
    let mut renderer = null_builder()
        .with_feature(DrawsOnePass)
        .with_frame_observer(Arc::<Journal>::clone(&journal))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer);

    renderer.render_frame(&input).expect("frame");

    assert_eq!(
        journal.take(),
        [
            Event::BeginFrame(0),
            Event::BeginPass("clear output"),
            Event::GpuPass("clear output", "clear output"),
            Event::EndPass("clear output"),
            Event::BeginPass("scratch pass"),
            Event::GpuPass("scratch pass", "scratch"),
            Event::EndPass("scratch pass"),
            Event::EndFrame,
            Event::Committed,
        ]
    );

    renderer.render_frame(&input).expect("second frame");
    assert_eq!(journal.take().first(), Some(&Event::BeginFrame(1)));
}

#[test]
fn a_frame_that_fails_to_record_is_abandoned_and_never_ended() {
    let journal = Arc::new(Journal::default());
    let mut renderer = null_builder()
        .with_feature(LosesDevice)
        .with_frame_observer(Arc::<Journal>::clone(&journal))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer);

    let outcome = renderer.render_frame(&input);

    assert!(matches!(outcome, Err(RenderError::DeviceLost)));
    let events = journal.take();
    assert_eq!(events.first(), Some(&Event::BeginFrame(0)));
    assert_eq!(events.last(), Some(&Event::Abandoned));
    assert!(!events.contains(&Event::EndFrame));
    assert!(!events.contains(&Event::Committed));
}

#[test]
fn the_list_returned_to_a_host_ends_with_what_the_observer_appended() {
    let journal = Arc::new(Journal::default());
    let mut renderer = null_builder()
        .with_feature(DrawsOnePass)
        .with_frame_observer(Arc::<Journal>::clone(&journal))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer);

    let commands = renderer.record_frame(&input).expect("recorded");

    let recorded = commands.commands();
    assert!(matches!(
        recorded[recorded.len() - 2..],
        [Command::PushDebugGroup("observer"), Command::PopDebugGroup]
    ));
    assert_eq!(
        journal.take().last(),
        Some(&Event::Committed),
        "mode A commits when the list is handed over"
    );
}

#[test]
fn the_observer_hears_about_a_new_device() {
    let journal = Arc::new(Journal::default());
    let mut renderer = null_builder()
        .with_frame_observer(Arc::<Journal>::clone(&journal))
        .build()
        .expect("renderer");

    renderer.recover().expect("recover");

    assert_eq!(journal.take(), [Event::DeviceReset]);
}

#[test]
fn an_observer_set_later_replaces_and_removes_the_first() {
    let first = Arc::new(Journal::default());
    let second = Arc::new(Journal::default());
    let mut renderer = null_builder()
        .with_feature(DrawsOnePass)
        .with_frame_observer(Arc::<Journal>::clone(&first))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer);

    renderer.set_frame_observer(Some(Arc::<Journal>::clone(&second)));
    renderer.render_frame(&input).expect("frame");
    renderer.set_frame_observer(None);
    renderer.render_frame(&input).expect("unobserved frame");

    assert_eq!(first.take(), []);
    assert_eq!(second.take().first(), Some(&Event::BeginFrame(0)));
}
