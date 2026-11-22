mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Gate, Stats, TestDevice, ThreadedPool, lock, p99, wait_until};
use pathon_api::{SerialTaskPool, TaskPool};
use pathon_rhi::{
    ColorTargetState, ComputePipelineDesc, Device, Format, PrimitiveState, RenderPipelineDesc, ShaderModuleId,
    ShaderStage, VertexAttribute, VertexBufferLayout, VertexFormat, VertexStepMode,
};
use pathon_rhi_types::{DeviceApi, Resource};
use pathon_shader::{
    BackgroundCompiler, BackgroundConfig, BuildError, Lookup, MissPolicy, ShaderError, ShaderFeatures, ShaderKey,
    ShaderLibrary,
};

const ROOT: &str = "
@if(wide) const FACTOR: u32 = 4u;
@if(!wide) const FACTOR: u32 = 1u;
override a: f32 = 0.0;
override b: f32 = 0.0;
@vertex fn vs(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position * f32(FACTOR), 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
";

// A pack holding the compiler must stay movable to the render thread (ADR-0004).
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<BackgroundCompiler>();
};

const COMPILE_DELAY: Duration = Duration::from_millis(40);

fn key(flags: &[&str]) -> ShaderKey {
    ShaderKey {
        root: "lit".to_owned(),
        features: flags.iter().fold(ShaderFeatures::default(), |features, flag| {
            features.with(flag).expect("flag")
        }),
    }
}

fn render_desc(label: &'static str, module: ShaderModuleId) -> RenderPipelineDesc {
    RenderPipelineDesc {
        label,
        bind_group_layouts: vec![],
        vertex: ShaderStage {
            module,
            entry_point: "vs",
            constants: vec![],
        },
        vertex_buffers: vec![VertexBufferLayout {
            stride: 8,
            step_mode: VertexStepMode::Vertex,
            attributes: vec![VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: 0,
                location: 0,
            }],
        }],
        fragment: Some(ShaderStage {
            module,
            entry_point: "fs",
            constants: vec![],
        }),
        color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
        primitive: PrimitiveState::default(),
        depth: None,
        sample_count: 1,
        immediate_size: 0,
    }
}

struct Rig {
    device: Arc<TestDevice>,
    compiler: BackgroundCompiler,
}

fn rig(device: TestDevice, pool: Arc<dyn TaskPool>, config: BackgroundConfig) -> Rig {
    let device = Arc::new(device);
    let handle: Arc<dyn DeviceApi> = Arc::<TestDevice>::clone(&device);
    let mut compiler = BackgroundCompiler::new(handle, pool, config);
    compiler.add_root("lit", ROOT);
    Rig { device, compiler }
}

fn threaded(threads: usize) -> Arc<dyn TaskPool> {
    Arc::new(ThreadedPool::new(threads))
}

fn serial() -> Arc<dyn TaskPool> {
    Arc::new(SerialTaskPool)
}

impl Rig {
    fn stats(&self) -> &Stats {
        &self.device.stats
    }

    fn module_when_ready(&mut self, flags: &[&str]) -> ShaderModuleId {
        let key = key(flags);
        let mut ready = None;
        wait_until("a module", || {
            self.compiler.pump();
            ready = self.compiler.shader_module(&key).ready();
            ready.is_some()
        });
        ready.expect("waited for it")
    }
}

mod lookups {
    use super::*;

    #[test]
    fn a_permutation_is_pending_until_a_pump_collects_the_finished_build() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        let key = key(&["wide"]);

        assert_eq!(rig.compiler.shader_module(&key), Lookup::Pending);
        let module = rig.module_when_ready(&["wide"]);

        assert_eq!(rig.compiler.shader_module(&key), Lookup::Ready(module));
        assert_eq!(rig.compiler.pending(), 0);
        let wgsl = rig.compiler.wgsl(&key).expect("the composed source is kept");
        assert!(wgsl.contains("const FACTOR: u32 = 4u"), "{wgsl}");
    }

    #[test]
    fn asking_again_while_a_build_runs_builds_once() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        let key = key(&["wide", "shadows"]);

        for _ in 0..5 {
            assert_eq!(rig.compiler.shader_module(&key), Lookup::Pending);
        }
        let first = rig.module_when_ready(&["wide", "shadows"]);
        let again = rig.module_when_ready(&["shadows", "wide"]);

        assert_eq!(first, again, "flag order is not part of the key");
        assert_eq!(Stats::count(&rig.stats().modules), 1);
    }

    #[test]
    fn a_pipeline_is_built_once_from_a_ready_module_whatever_the_constant_order() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        let module = rig.module_when_ready(&[]);
        let mut unsorted = render_desc("lit", module);
        unsorted.vertex.constants = vec![("b", 1.0.into()), ("a", 2.0.into())];
        let mut sorted = unsorted.clone();
        sorted.vertex.constants = vec![("a", 2.0.into()), ("b", 1.0.into())];

        assert_eq!(rig.compiler.render_pipeline(&unsorted), Lookup::Pending);
        assert_eq!(rig.compiler.render_pipeline(&sorted), Lookup::Pending);
        let mut pipeline = None;
        wait_until("a pipeline", || {
            rig.compiler.pump();
            pipeline = rig.compiler.render_pipeline(&unsorted).ready();
            pipeline.is_some()
        });

        assert_eq!(Stats::count(&rig.stats().pipelines), 1);
        assert_eq!(rig.compiler.render_pipeline(&sorted).ready(), pipeline);
    }

    #[test]
    fn a_compute_pipeline_is_built_the_same_way() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        let module = rig.module_when_ready(&[]);
        let desc = ComputePipelineDesc {
            label: "lit-compute",
            bind_group_layouts: vec![],
            shader: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            immediate_size: 0,
        };

        assert_eq!(rig.compiler.compute_pipeline(&desc), Lookup::Pending);
        wait_until("a compute pipeline", || {
            rig.compiler.pump();
            rig.compiler.compute_pipeline(&desc).ready().is_some()
        });
    }

    #[test]
    fn the_miss_policy_chooses_between_skipping_and_a_placeholder() {
        let device = Device::null();
        let placeholder = device
            .create_shader_module(&pathon_rhi::ShaderModuleDesc {
                label: "placeholder",
                wgsl: ROOT,
            })
            .expect("module");

        for miss in [Lookup::Pending, Lookup::Failed] {
            assert_eq!(miss.resolve(MissPolicy::Skip), None);
            assert_eq!(miss.resolve(MissPolicy::Placeholder(placeholder)), Some(placeholder));
        }
        let ready = Lookup::Ready(placeholder);
        assert_eq!(ready.resolve(MissPolicy::Skip), Some(placeholder));
        assert_eq!(ready.ready(), Some(placeholder));
    }
}

mod failures {
    use super::*;

    #[test]
    fn an_unknown_root_fails_at_once_with_one_s0001_and_is_not_retried_every_frame() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        let missing = ShaderKey {
            root: "nope".to_owned(),
            features: ShaderFeatures::default(),
        };

        assert_eq!(rig.compiler.shader_module(&missing), Lookup::Failed);
        assert_eq!(rig.compiler.shader_module(&missing), Lookup::Failed);

        let failures = rig.compiler.take_failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].code(), "S0001");
        assert!(matches!(
            failures[0].error,
            BuildError::Shader(ShaderError::UnknownRoot { .. })
        ));
        let text = failures[0].diagnostic().to_string();
        assert!(text.contains("shader `nope`") && text.contains("S0001"), "{text}");
        assert!(rig.compiler.take_failures().is_empty());
    }

    #[test]
    fn a_shader_that_does_not_compose_becomes_a_diagnostic_not_a_panic() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        rig.compiler
            .add_root("broken", "import package::missing::thing; fn f( {");
        let broken = ShaderKey {
            root: "broken".to_owned(),
            features: ShaderFeatures::default().with("wide").expect("flag"),
        };

        assert_eq!(rig.compiler.shader_module(&broken), Lookup::Pending);
        wait_until("the failure", || {
            rig.compiler.pump();
            rig.compiler.shader_module(&broken) == Lookup::Failed
        });

        let failures = rig.compiler.take_failures();
        assert_eq!(failures.len(), 1);
        assert!(matches!(
            failures[0].error,
            BuildError::Shader(ShaderError::Compose { .. })
        ));
        assert!(failures[0].subject.contains("[wide]"), "{}", failures[0].subject);
        assert_eq!(Stats::count(&rig.stats().modules), 0);
    }

    #[test]
    fn registering_changed_source_releases_a_failed_permutation() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        rig.compiler.add_root("fixable", "fn f( {");
        let fixable = ShaderKey {
            root: "fixable".to_owned(),
            features: ShaderFeatures::default(),
        };
        wait_until("the failure", || {
            rig.compiler.pump();
            rig.compiler.shader_module(&fixable) == Lookup::Failed
        });

        rig.compiler.add_root("fixable", ROOT);

        assert_eq!(rig.compiler.shader_module(&fixable), Lookup::Pending);
        wait_until("the fixed module", || {
            rig.compiler.pump();
            rig.compiler.shader_module(&fixable).ready().is_some()
        });
    }

    #[test]
    fn a_pipeline_failure_and_a_panicking_build_become_s0002_and_the_rest_keeps_building() {
        let mut device = TestDevice::new(Duration::ZERO);
        device.fail_label = Some("rejected");
        device.panic_label = Some("explodes");
        let mut rig = rig(device, threaded(3), BackgroundConfig::new());
        let module = rig.module_when_ready(&[]);
        let (rejected, explodes, fine) = (
            render_desc("rejected", module),
            render_desc("explodes", module),
            render_desc("fine", module),
        );

        for desc in [&rejected, &explodes, &fine] {
            assert_eq!(rig.compiler.render_pipeline(desc), Lookup::Pending);
        }
        wait_until("all three to settle", || {
            rig.compiler.pump();
            [&rejected, &explodes, &fine]
                .iter()
                .all(|desc| rig.compiler.render_pipeline(desc) != Lookup::Pending)
        });

        assert_eq!(rig.compiler.render_pipeline(&rejected), Lookup::Failed);
        assert_eq!(rig.compiler.render_pipeline(&explodes), Lookup::Failed);
        assert!(rig.compiler.render_pipeline(&fine).ready().is_some());
        let mut failures = rig.compiler.take_failures();
        failures.sort_by(|a, b| a.subject.cmp(&b.subject));
        assert_eq!(failures.len(), 2);
        assert!(failures.iter().all(|failure| failure.code() == "S0002"));
        assert!(
            matches!(&failures[0].error, BuildError::Panicked { message } if message.contains("explodes")),
            "{:?}",
            failures[0]
        );
        assert!(matches!(failures[1].error, BuildError::Rhi(_)), "{:?}", failures[1]);
        assert_eq!(
            rig.compiler.pending(),
            0,
            "a panic must not leave a build counted as running"
        );
    }

    #[test]
    fn retry_failed_asks_the_device_again() {
        let mut device = TestDevice::new(Duration::ZERO);
        device.fail_label = Some("rejected");
        let mut rig = rig(device, threaded(2), BackgroundConfig::new());
        let module = rig.module_when_ready(&[]);
        let desc = render_desc("rejected", module);
        wait_until("the failure", || {
            rig.compiler.pump();
            rig.compiler.render_pipeline(&desc) == Lookup::Failed
        });
        let entered = Stats::count(&rig.stats().entered);

        assert_eq!(rig.compiler.render_pipeline(&desc), Lookup::Failed);
        rig.compiler.pump();
        assert_eq!(Stats::count(&rig.stats().entered), entered, "no retry without asking");
        rig.compiler.retry_failed();
        assert_eq!(rig.compiler.render_pipeline(&desc), Lookup::Pending);
        wait_until("the second failure", || {
            rig.compiler.pump();
            rig.compiler.render_pipeline(&desc) == Lookup::Failed
        });
        assert_eq!(Stats::count(&rig.stats().entered), entered + 1);
    }
}

mod scheduling {
    use super::*;

    #[test]
    fn a_pool_without_threads_builds_at_most_the_inline_budget_per_pump() {
        let mut rig = rig(
            TestDevice::new(Duration::ZERO),
            serial(),
            BackgroundConfig::new().with_inline_jobs_per_pump(2),
        );
        let keys: Vec<ShaderKey> = (0..5).map(|index| key(&[&format!("f{index}")])).collect();
        for key in &keys {
            assert_eq!(rig.compiler.shader_module(key), Lookup::Pending);
        }

        let mut built_per_pump = Vec::new();
        let mut before = 0;
        while rig.compiler.pending() > 0 {
            let report = rig.compiler.pump();
            let now = Stats::count(&rig.stats().modules);
            built_per_pump.push(now - before);
            before = now;
            assert!(report.started <= 2, "{report:?}");
        }

        assert_eq!(built_per_pump, [2, 2, 1]);
        assert!(keys.iter().all(|key| rig.compiler.shader_module(key).ready().is_some()));
        let caller = std::thread::current().id();
        assert!(
            lock(&rig.stats().threads).iter().all(|thread| *thread == caller),
            "serial builds run on the calling thread only"
        );
    }

    #[test]
    fn running_builds_never_exceed_the_limit_and_do_overlap_below_it() {
        let gate = Gate::closed();
        let mut device = TestDevice::new(Duration::ZERO);
        device.gate = Some(Arc::clone(&gate));
        let mut rig = rig(device, threaded(8), BackgroundConfig::new().with_max_in_flight(2));
        let keys: Vec<ShaderKey> = (0..6).map(|index| key(&[&format!("f{index}")])).collect();
        for key in &keys {
            rig.compiler.shader_module(key);
        }
        rig.compiler.pump();
        wait_until("two builds to run side by side", || {
            Stats::count(&rig.stats().running) == 2
        });
        rig.compiler.pump();
        assert_eq!(
            Stats::count(&rig.stats().entered),
            2,
            "a third build waits for a free slot"
        );
        gate.open();

        wait_until("all builds", || {
            rig.compiler.pump();
            keys.iter().all(|key| rig.compiler.shader_module(key).ready().is_some())
        });

        assert_eq!(Stats::count(&rig.stats().max_running), 2);
        assert_eq!(lock(&rig.stats().threads).len(), 6, "each build ran on a pool thread");
    }

    #[test]
    fn finish_all_drains_the_queue_on_both_kinds_of_pool() {
        for pool in [serial(), threaded(3)] {
            let mut rig = rig(TestDevice::new(Duration::ZERO), pool, BackgroundConfig::new());
            let keys: Vec<ShaderKey> = (0..7).map(|index| key(&[&format!("f{index}")])).collect();
            for key in &keys {
                rig.compiler.shader_module(key);
            }

            assert!(rig.compiler.finish_all(Duration::from_secs(30)));

            assert!(keys.iter().all(|key| rig.compiler.shader_module(key).ready().is_some()));
        }
    }

    #[test]
    fn changed_source_discards_a_build_that_started_against_the_old_one() {
        let gate = Gate::closed();
        let mut device = TestDevice::new(Duration::ZERO);
        device.gate = Some(Arc::clone(&gate));
        let mut rig = rig(device, threaded(2), BackgroundConfig::new());
        let key = key(&["wide"]);
        rig.compiler.shader_module(&key);
        rig.compiler.pump();
        wait_until("the build to start", || Stats::count(&rig.stats().entered) == 1);

        rig.compiler
            .add_root("lit", "@compute @workgroup_size(1) fn other() {}");
        gate.open();

        wait_until("the stale result to be thrown away", || {
            rig.compiler.pump();
            rig.compiler.pending() == 0
        });
        let stale = lock(&rig.stats().created).clone();
        assert_eq!(stale.len(), 1);
        assert!(
            lock(&rig.stats().destroyed).contains(&stale[0]),
            "the outdated module is destroyed"
        );
        assert_eq!(
            rig.compiler.shader_module(&key),
            Lookup::Pending,
            "and the key is asked for again"
        );
        wait_until("the fresh module", || {
            rig.compiler.pump();
            rig.compiler.shader_module(&key).ready().is_some()
        });
        let wgsl = rig.compiler.wgsl(&key).expect("composed source");
        assert!(wgsl.contains("fn other"), "{wgsl}");
    }

    #[test]
    fn release_all_hands_back_every_module_and_pipeline() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        let module = rig.module_when_ready(&[]);
        let desc = render_desc("lit", module);
        rig.compiler.render_pipeline(&desc);
        wait_until("the pipeline", || {
            rig.compiler.pump();
            rig.compiler.render_pipeline(&desc).ready().is_some()
        });

        let released = rig.compiler.release_all();

        assert_eq!(released.len(), 2);
        assert!(released.contains(&Resource::ShaderModule(module)));
        assert!(
            released
                .iter()
                .any(|resource| matches!(resource, Resource::RenderPipeline(_)))
        );
        assert_eq!(
            rig.compiler.shader_module(&key(&[])),
            Lookup::Pending,
            "released keys build again"
        );
    }
}

mod dropping {
    use super::*;

    #[test]
    fn dropping_the_compiler_does_not_wait_and_leaks_nothing_the_running_builds_create() {
        let gate = Gate::closed();
        let mut device = TestDevice::new(Duration::ZERO);
        device.gate = Some(Arc::clone(&gate));
        let mut rig = rig(device, threaded(3), BackgroundConfig::new().with_max_in_flight(2));
        for index in 0..8 {
            rig.compiler.shader_module(&key(&[&format!("f{index}")]));
        }
        rig.compiler.pump();
        let stats = Arc::clone(&rig.device.stats);
        wait_until("two builds to be running", || Stats::count(&stats.running) == 2);

        drop(rig.compiler);
        // The builds are still blocked on the gate: reaching this line means the drop did not join them.
        assert_eq!(Stats::count(&stats.running), 2);
        gate.open();

        wait_until("the builds to finish", || Stats::count(&stats.running) == 0);
        wait_until("their results to be destroyed", || stats.leaked().is_empty());
        assert_eq!(Stats::count(&stats.entered), 2, "queued builds never start");
        assert_eq!(lock(&stats.created).len(), 2);
    }

    #[test]
    fn results_nobody_collected_are_destroyed_with_the_compiler() {
        let mut rig = rig(TestDevice::new(Duration::ZERO), threaded(2), BackgroundConfig::new());
        rig.compiler.shader_module(&key(&["wide"]));
        rig.compiler.pump();
        let stats = Arc::clone(&rig.device.stats);
        wait_until("the build", || Stats::count(&stats.modules) == 1);

        drop(rig.compiler);

        assert!(stats.leaked().is_empty(), "{:?}", stats.leaked());
    }

    #[test]
    fn dropping_with_nothing_requested_is_quiet() {
        drop(rig(TestDevice::new(Duration::ZERO), serial(), BackgroundConfig::new()));
    }
}

// The M3-04 acceptance test: N new permutations injected into running frames keep the frame cost of the compiler (the
// pump plus every lookup a pack makes) inside the budget.
mod frame_budget {
    use super::*;

    // A 60 Hz frame is 16.6 ms; the compiler may take 10 ms of it. Each build costs 40 ms (four times the budget), so a
    // compiler that waited for even one build would break the budget by a wide margin, while a correct one needs
    // microseconds and has a thousandfold margin against scheduler noise. More than 100 frames are measured, so the
    // p99 ignores the single worst frame.
    const FRAME_BUDGET: Duration = Duration::from_millis(10);
    const NEW_PERMUTATIONS: usize = 40;

    struct Pack {
        keys: Vec<ShaderKey>,
        pipelines: Vec<Lookup<pathon_rhi::RenderPipelineId>>,
    }

    impl Pack {
        fn new() -> Self {
            let keys: Vec<ShaderKey> = (0..NEW_PERMUTATIONS)
                .map(|index| key(&[&format!("f{index}")]))
                .collect();
            Self {
                pipelines: vec![Lookup::Pending; keys.len()],
                keys,
            }
        }

        // What a pack does in a frame: pump, then walk the permutations it wants and ask for the next stage.
        fn frame(&mut self, compiler: &mut BackgroundCompiler) {
            compiler.pump();
            for (index, key) in self.keys.iter().enumerate() {
                if self.pipelines[index].ready().is_some() {
                    continue;
                }
                if let Some(module) = compiler.shader_module(key).ready() {
                    self.pipelines[index] = compiler.render_pipeline(&render_desc("lit", module));
                }
            }
        }

        fn all_ready(&self) -> bool {
            self.pipelines.iter().all(|pipeline| pipeline.ready().is_some())
        }
    }

    #[test]
    fn injecting_new_permutations_keeps_the_p99_frame_cost_in_budget() {
        let mut rig = rig(TestDevice::new(COMPILE_DELAY), threaded(4), BackgroundConfig::new());
        let mut pack = Pack::new();
        let mut frames = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(60);

        while !pack.all_ready() || frames.len() < 100 {
            assert!(Instant::now() < deadline, "the permutations never finished");
            let started = Instant::now();
            pack.frame(&mut rig.compiler);
            frames.push(started.elapsed());
            std::thread::sleep(Duration::from_millis(1));
        }

        let p99 = p99(&mut frames);
        println!(
            "p99 frame cost {p99:?} over {} frames (budget {FRAME_BUDGET:?})",
            frames.len()
        );
        assert!(
            p99 < FRAME_BUDGET,
            "p99 frame cost {p99:?} over {} frames",
            frames.len()
        );
        assert_eq!(Stats::count(&rig.stats().modules), NEW_PERMUTATIONS);
        assert!(rig.compiler.take_failures().is_empty());
    }

    // Control: composing and creating the same permutations the blocking way (`ShaderLibrary::module`) would stall the
    // frame for at least one build, which is exactly what the budget above forbids.
    #[test]
    fn building_the_same_permutations_on_the_frame_thread_breaks_the_budget() {
        let device = TestDevice::new(COMPILE_DELAY);
        let mut library = ShaderLibrary::new();
        library.add_root("lit", ROOT);

        let started = Instant::now();
        library.module(&device, &key(&["f0"])).expect("module");

        assert!(started.elapsed() >= COMPILE_DELAY);
        assert!(COMPILE_DELAY > FRAME_BUDGET * 3);
    }

    // Without pool threads (wasm) a build runs inside a pump, so the guarantee is a count, not a time: one build per
    // frame at the default budget. The time is only reported, as the plan asks for wasm.
    #[test]
    fn without_threads_a_frame_runs_at_most_one_build_and_the_time_is_only_reported() {
        let mut rig = rig(
            TestDevice::new(Duration::from_millis(5)),
            serial(),
            BackgroundConfig::new(),
        );
        let mut pack = Pack::new();
        let mut frame_times = Vec::new();

        while !pack.all_ready() {
            let before = Stats::count(&rig.stats().entered);
            let started = Instant::now();
            pack.frame(&mut rig.compiler);
            frame_times.push(started.elapsed());
            let builds = Stats::count(&rig.stats().entered) - before;
            assert!(builds <= 1, "a frame builds at most the inline budget: {builds}");
        }

        let worst = frame_times.iter().max().copied().unwrap_or_default();
        println!(
            "serial mode: {} frames for {NEW_PERMUTATIONS} permutations, worst frame {worst:?}",
            frame_times.len()
        );
        assert!(
            frame_times.len() >= 2 * NEW_PERMUTATIONS,
            "a module and a pipeline per permutation, one build per frame"
        );
    }
}
