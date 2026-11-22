#![cfg(any(feature = "tracing", feature = "puffin", feature = "tracy"))]

// The sinks behind features, driven by a real renderer on the null backend.

use std::sync::Arc;

use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{Access, Color, ColorAttachment, Format, PhysicalSize, RenderPassDesc, RhiError};
use pathon_api::{Camera, RecordCtx, RenderFeature};
use pathon_core::{FrameInput, Renderer, RendererBuilder, ViewInput};
use pathon_profile::{ProfileSink, Profiler};
use pathon_rhi::{Device, NullDevice};

struct NamedPass;

impl RenderFeature for NamedPass {
    fn name(&self) -> &'static str {
        "named pass"
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
            .add_pass(PassDesc::new("shadow cascade", Stage::Opaque).side_effect());
        let written = pass.write(texture, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(written)?;
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "cascade 0",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        Ok(())
    }
}

// Two frames through a profiler with `sink`, all GPU results read back before returning.
fn profile_two_frames(sink: Arc<dyn ProfileSink>) {
    let profiler = Profiler::builder().with_sink(sink).build();
    let mut renderer: Renderer = RendererBuilder::new()
        .with_device(Arc::new(Device::from(NullDevice::new())))
        .with_feature(NamedPass)
        .with_frame_observer(Arc::<Profiler>::clone(&profiler))
        .build()
        .expect("renderer");
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
    renderer.render_frame(&input).expect("frame 0");
    renderer.render_frame(&input).expect("frame 1");
    profiler.flush(renderer.device().as_ref());
}

#[cfg(feature = "tracing")]
mod tracing_sink {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use pathon_profile::{GPU_TARGET, PASS_TARGET, TracingSink};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::*;

    #[derive(Default)]
    struct Fields {
        name: Option<String>,
        label: Option<String>,
        duration_ns: Option<f64>,
    }

    impl Visit for Fields {
        fn record_str(&mut self, field: &Field, value: &str) {
            match field.name() {
                "name" => self.name = Some(value.to_owned()),
                "label" => self.label = Some(value.to_owned()),
                _ => {}
            }
        }

        fn record_f64(&mut self, field: &Field, value: f64) {
            if field.name() == "duration_ns" {
                self.duration_ns = Some(value);
            }
        }

        fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
    }

    #[derive(Debug, PartialEq)]
    enum Seen {
        SpanOpened {
            target: String,
            name: String,
        },
        Entered(u64),
        Exited(u64),
        Gpu {
            name: String,
            label: String,
            has_duration: bool,
        },
    }

    #[derive(Default)]
    struct Recorder {
        next_id: AtomicU64,
        seen: Mutex<Vec<Seen>>,
        names: Mutex<Vec<String>>,
    }

    // `Subscriber` is not implemented for `Arc<T>`, so the test keeps its own handle to read back what was seen.
    struct Handle(Arc<Recorder>);

    impl Subscriber for Handle {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, attributes: &Attributes<'_>) -> Id {
            let mut fields = Fields::default();
            attributes.record(&mut fields);
            let id = self.0.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            let name = fields.name.unwrap_or_default();
            self.0.names.lock().expect("names").push(name.clone());
            self.0.seen.lock().expect("seen").push(Seen::SpanOpened {
                target: attributes.metadata().target().to_owned(),
                name,
            });
            Id::from_u64(id)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields::default();
            event.record(&mut fields);
            assert_eq!(event.metadata().target(), GPU_TARGET);
            self.0.seen.lock().expect("seen").push(Seen::Gpu {
                name: fields.name.unwrap_or_default(),
                label: fields.label.unwrap_or_default(),
                has_duration: fields.duration_ns.is_some(),
            });
        }

        fn enter(&self, span: &Id) {
            self.0.seen.lock().expect("seen").push(Seen::Entered(span.into_u64()));
        }

        fn exit(&self, span: &Id) {
            self.0.seen.lock().expect("seen").push(Seen::Exited(span.into_u64()));
        }
    }

    #[test]
    fn passes_become_spans_named_after_the_graph_and_gpu_passes_become_events() {
        let recorder = Arc::new(Recorder::default());

        tracing::subscriber::with_default(Handle(Arc::<Recorder>::clone(&recorder)), || {
            profile_two_frames(Arc::new(TracingSink));
        });

        let seen = recorder.seen.lock().expect("seen");
        let opened: Vec<_> = seen
            .iter()
            .filter_map(|seen| match seen {
                Seen::SpanOpened { target, name } => Some((target.as_str(), name.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            opened,
            [
                (PASS_TARGET, "clear output"),
                (PASS_TARGET, "shadow cascade"),
                (PASS_TARGET, "clear output"),
                (PASS_TARGET, "shadow cascade"),
            ]
        );
        let entered = seen.iter().filter(|seen| matches!(seen, Seen::Entered(_))).count();
        let exited = seen.iter().filter(|seen| matches!(seen, Seen::Exited(_))).count();
        assert_eq!((entered, exited), (4, 4), "every span is closed");
        let gpu: Vec<_> = seen
            .iter()
            .filter_map(|seen| match seen {
                Seen::Gpu {
                    name,
                    label,
                    has_duration,
                } => Some((name.as_str(), label.as_str(), *has_duration)),
                _ => None,
            })
            .collect();
        assert_eq!(
            gpu,
            [
                ("clear output", "clear output", true),
                ("shadow cascade", "cascade 0", true),
                ("clear output", "clear output", true),
                ("shadow cascade", "cascade 0", true),
            ]
        );
    }
}

#[cfg(feature = "puffin")]
mod puffin_sink {
    use std::sync::Mutex;

    use pathon_profile::PuffinSink;

    use super::*;

    #[test]
    fn puffin_learns_the_names_of_cpu_and_gpu_scopes() {
        let scope_names = Arc::new(Mutex::new(Vec::<String>::new()));
        let collected = Arc::clone(&scope_names);
        puffin::GlobalProfiler::lock().add_sink(Box::new(move |frame| {
            let mut names = collected.lock().expect("names");
            names.extend(frame.scope_delta.iter().map(|scope| scope.name().to_string()));
        }));

        profile_two_frames(Arc::new(PuffinSink::new()));
        // GPU scopes arrive after the last committed frame; the next frame boundary publishes them.
        puffin::GlobalProfiler::lock().new_frame();

        let names = scope_names.lock().expect("names");
        for expected in ["clear output", "shadow cascade", "shadow cascade (GPU: cascade 0)"] {
            assert!(
                names.iter().any(|name| name == expected),
                "{expected} missing from {names:?}"
            );
        }
    }
}

#[cfg(feature = "tracy")]
mod tracy_sink {
    use pathon_profile::TracySink;

    use super::*;

    // Without a Tracy server the data goes nowhere; this checks the whole path runs (zones, frame marks, GPU context).
    #[test]
    fn the_tracy_sink_runs_a_profiled_frame() {
        profile_two_frames(Arc::new(TracySink::new()));

        assert!(tracy_client::Client::running().is_some());
    }
}
