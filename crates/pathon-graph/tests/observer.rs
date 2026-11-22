use std::sync::{Arc, Mutex};

use pathon_graph::{
    FrameGraph, FrameObserver, FrameSizes, HistoryStore, PassContext, PassDesc, ResourcePool, Stage, TextureHandle,
};
use pathon_resources::DeferredRelease;
use pathon_rhi_null::NullDevice;
use pathon_rhi_types::{
    Access, Color, ColorAttachment, Command, ComputePassDesc, DeviceApi, Format, PassTimestamps, PhysicalSize,
    QuerySetDesc, QuerySetId, RenderPassDesc, RhiError, TextureDesc, TextureUsage,
};

const SIZE: PhysicalSize = PhysicalSize::new(8, 8);

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Begin(&'static str),
    End(&'static str),
    Gpu(&'static str, &'static str),
}

// Hands out consecutive query pairs, like a profiler would.
struct Recorder {
    events: Mutex<Vec<Event>>,
    query_set: QuerySetId,
}

impl Recorder {
    fn new(device: &NullDevice) -> Arc<Self> {
        let query_set = device
            .create_query_set(&QuerySetDesc {
                label: "test",
                count: 16,
            })
            .expect("query set");
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            query_set,
        })
    }

    fn events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().expect("events"))
    }
}

impl FrameObserver for Recorder {
    fn begin_pass(&self, pass: &'static str) {
        self.events.lock().expect("events").push(Event::Begin(pass));
    }

    fn end_pass(&self, pass: &'static str) {
        self.events.lock().expect("events").push(Event::End(pass));
    }

    fn gpu_pass(&self, _device: &dyn DeviceApi, pass: &'static str, label: &'static str) -> Option<PassTimestamps> {
        let mut events = self.events.lock().expect("events");
        let gpu_passes =
            u32::try_from(events.iter().filter(|event| matches!(event, Event::Gpu(..))).count()).expect("small");
        events.push(Event::Gpu(pass, label));
        Some(PassTimestamps {
            query_set: self.query_set,
            begin_index: gpu_passes * 2,
            end_index: gpu_passes * 2 + 1,
        })
    }
}

fn import_output(graph: &mut FrameGraph, device: &NullDevice) -> TextureHandle {
    let desc = TextureDesc::new_2d(
        "output",
        SIZE.width,
        SIZE.height,
        Format::Rgba8Unorm,
        TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED,
    );
    let texture = device.create_texture(&desc).expect("output");
    graph.import_texture("output", texture, desc, Access::empty(), Access::SAMPLED)
}

fn clear_with(
    target: TextureHandle,
    label: &'static str,
    timestamps: Option<PassTimestamps>,
) -> impl FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> {
    move |context| {
        let view = context.view(target)?;
        drop(context.commands().render_pass(RenderPassDesc {
            label,
            color: vec![ColorAttachment::clear(view, Color::BLACK)],
            depth: None,
            timestamps,
        }));
        Ok(())
    }
}

fn execute(graph: FrameGraph, device: &NullDevice) -> Result<pathon_graph::Execution, RhiError> {
    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("graph compiles");
    graph.execute(
        &compiled,
        device,
        &mut ResourcePool::new(),
        &mut HistoryStore::new(),
        &mut DeferredRelease::new(2),
        0,
    )
}

fn timestamps_of(commands: &pathon_rhi_types::CommandList) -> Vec<Option<PassTimestamps>> {
    commands
        .commands()
        .iter()
        .filter_map(|command| match command {
            Command::BeginRenderPass(desc) => Some(desc.timestamps),
            Command::BeginComputePass(desc) => Some(desc.timestamps),
            _ => None,
        })
        .collect()
}

#[test]
fn passes_are_reported_in_execution_order_with_the_names_of_the_graph() {
    let device = NullDevice::new();
    let recorder = Recorder::new(&device);
    let mut graph = FrameGraph::new();
    graph.set_observer(Some(Arc::<Recorder>::clone(&recorder)));
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("first", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear_with(written, "first target", None));
    let mut pass = graph.add_pass(PassDesc::new("second", Stage::PostProcess));
    let written = pass.modify(written, Access::COLOR_ATTACHMENT);
    pass.record(clear_with(written, "second target", None));

    let execution = execute(graph, &device).expect("executes");

    assert_eq!(
        recorder.events(),
        [
            Event::Begin("first"),
            Event::Gpu("first", "first target"),
            Event::End("first"),
            Event::Begin("second"),
            Event::Gpu("second", "second target"),
            Event::End("second"),
        ]
    );
    let timestamps = timestamps_of(&execution.commands);
    assert_eq!(timestamps.len(), 2);
    assert_eq!(timestamps[0].map(|stamps| stamps.begin_index), Some(0));
    assert_eq!(timestamps[1].map(|stamps| stamps.begin_index), Some(2));
    device
        .submit(execution.commands)
        .expect("timestamps pass null validation");
}

#[test]
fn a_pass_with_timestamps_of_its_own_is_left_alone() {
    let device = NullDevice::new();
    let recorder = Recorder::new(&device);
    let own = PassTimestamps {
        query_set: recorder.query_set,
        begin_index: 10,
        end_index: 11,
    };
    let mut graph = FrameGraph::new();
    graph.set_observer(Some(Arc::<Recorder>::clone(&recorder)));
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("timed by the pack", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear_with(written, "own", Some(own)));

    let execution = execute(graph, &device).expect("executes");

    assert_eq!(timestamps_of(&execution.commands), [Some(own)]);
    assert_eq!(
        recorder.events(),
        [Event::Begin("timed by the pack"), Event::End("timed by the pack")]
    );
}

#[test]
fn compute_passes_and_several_gpu_passes_in_one_graph_pass_are_all_named_after_it() {
    let device = NullDevice::new();
    let recorder = Recorder::new(&device);
    let mut graph = FrameGraph::new();
    graph.set_observer(Some(Arc::<Recorder>::clone(&recorder)));
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("busy", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(move |context| {
        let view = context.view(written)?;
        drop(context.commands().compute_pass(ComputePassDesc {
            label: "cull",
            timestamps: None,
        }));
        for label in ["draw a", "draw b"] {
            drop(context.commands().render_pass(RenderPassDesc {
                label,
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            }));
        }
        Ok(())
    });

    let execution = execute(graph, &device).expect("executes");

    assert_eq!(
        recorder.events(),
        [
            Event::Begin("busy"),
            Event::Gpu("busy", "cull"),
            Event::Gpu("busy", "draw a"),
            Event::Gpu("busy", "draw b"),
            Event::End("busy"),
        ]
    );
    device
        .submit(execution.commands)
        .expect("timestamps pass null validation");
}

#[test]
fn a_failing_pass_still_closes_its_span_and_gets_no_timestamps() {
    let device = NullDevice::new();
    let recorder = Recorder::new(&device);
    let mut graph = FrameGraph::new();
    graph.set_observer(Some(Arc::<Recorder>::clone(&recorder)));
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("broken", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(move |context| {
        let view = context.view(written)?;
        drop(context.commands().render_pass(RenderPassDesc {
            label: "half done",
            color: vec![ColorAttachment::clear(view, Color::BLACK)],
            depth: None,
            timestamps: None,
        }));
        Err(RhiError::InvalidHandle("on purpose"))
    });

    let outcome = execute(graph, &device);

    assert!(outcome.is_err());
    assert_eq!(recorder.events(), [Event::Begin("broken"), Event::End("broken")]);
}

#[test]
fn without_an_observer_no_pass_gets_timestamps() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("plain", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear_with(written, "plain", None));

    let execution = execute(graph, &device).expect("executes");

    assert_eq!(timestamps_of(&execution.commands), [None]);
}
