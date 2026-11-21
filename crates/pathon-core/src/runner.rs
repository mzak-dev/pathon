use std::collections::HashMap;

use pathon_diag::Diagnostic;
use pathon_graph::{
    FrameGraph, FrameSizes, GraphCache, GraphCapture, GraphStats, HistoryStore, PassDesc, ResourcePool, Stage,
    TextureHandle,
};
use pathon_resources::DeferredRelease;
use pathon_rhi::{
    Access, Color, ColorAttachment, CommandList, DeviceApi, PhysicalSize, RenderPassDesc, RhiError, TextureDesc,
    TextureId,
};

use crate::error::RenderError;
use crate::target::TargetId;

#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputTarget {
    pub(crate) texture: TextureId,
    pub(crate) desc: TextureDesc,
    pub(crate) initial: Access,
    pub(crate) final_access: Access,
    // Host textures keep their contents (mode A); targets pathon owns start cleared.
    pub(crate) keeps_contents: bool,
}

pub(crate) struct FrameEnv<'frame> {
    pub(crate) device: &'frame dyn DeviceApi,
    pub(crate) release: &'frame mut DeferredRelease,
    pub(crate) frame: u64,
}

pub(crate) struct ViewRun {
    pub(crate) commands: CommandList,
    pub(crate) stats: GraphStats,
    pub(crate) compiled: bool,
    pub(crate) capture: Option<GraphCapture>,
}

// Compiles and executes one view's frame graph; compiled graphs are cached per target, so view order does not matter.
pub(crate) struct GraphRunner {
    pool: ResourcePool,
    caches: HashMap<TargetId, GraphCache>,
    histories: HashMap<TargetId, HistoryStore>,
    // Targets whose history this frame wrote: their next frame may read it only once the frame is committed.
    written_this_frame: Vec<TargetId>,
    capture: bool,
    clear_color: Color,
}

impl GraphRunner {
    pub(crate) fn new(clear_color: Color) -> Self {
        Self {
            pool: ResourcePool::new(),
            caches: HashMap::new(),
            histories: HashMap::new(),
            written_this_frame: Vec::new(),
            capture: false,
            clear_color,
        }
    }

    pub(crate) fn set_capture(&mut self, enabled: bool) {
        self.capture = enabled;
    }

    pub(crate) fn forget_target(&mut self, target: TargetId, release: &mut DeferredRelease, frame: u64) {
        self.caches.remove(&target);
        self.written_this_frame.retain(|written| *written != target);
        if let Some(mut history) = self.histories.remove(&target) {
            history.release_all(&mut self.pool, release, frame);
        }
    }

    // The frame reached the GPU (or the host): what its views wrote to history is real.
    pub(crate) fn commit_frame(&mut self) {
        self.pool.commit_states();
        self.written_this_frame.clear();
    }

    // The frame was dropped after recording, so its history was never written and its states are wrong.
    pub(crate) fn abandon_frame(&mut self, release: &mut DeferredRelease, frame: u64) {
        self.pool.abandon_states();
        for target in std::mem::take(&mut self.written_this_frame) {
            if let Some(history) = self.histories.get_mut(&target) {
                history.release_all(&mut self.pool, release, frame);
            }
        }
    }

    pub(crate) fn forget_device(&mut self) {
        self.pool.forget_all();
        self.caches.clear();
        self.written_this_frame.clear();
        for history in self.histories.values_mut() {
            history.forget_all();
        }
    }

    // Diagnostics are collected into `diagnostics` whatever the result, for the renderer to report once.
    pub(crate) fn run_view(
        &mut self,
        target: TargetId,
        output: OutputTarget,
        sizes: FrameSizes,
        fill: impl FnOnce(&mut FrameGraph, TextureHandle) -> Vec<Diagnostic>,
        mut env: FrameEnv<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<ViewRun, RenderError> {
        let mut graph = FrameGraph::for_view(self.histories.entry(target).or_default(), sizes);
        let output_handle = self.import_output(&mut graph, output);
        diagnostics.extend(fill(&mut graph, output_handle));
        graph.set_capture(self.capture);

        let cache = self.caches.entry(target).or_default();
        let (compiled, fresh) = match cache.compile(&graph, sizes) {
            Ok(result) => result,
            Err(errors) => {
                diagnostics.extend(errors);
                return self.fallback_run(output, &mut env);
            }
        };
        let fresh_warnings = if fresh {
            compiled.warnings().to_vec()
        } else {
            Vec::new()
        };
        let history = self.histories.entry(target).or_default();
        let outcome = graph.execute(compiled, env.device, &mut self.pool, history, env.release, env.frame);
        diagnostics.extend(fresh_warnings);
        match outcome {
            Ok(execution) => {
                if history.has_textures() && !self.written_this_frame.contains(&target) {
                    self.written_this_frame.push(target);
                }
                Ok(ViewRun {
                    commands: execution.commands,
                    stats: execution.stats,
                    compiled: fresh,
                    capture: execution.capture,
                })
            }
            Err(RhiError::DeviceLost) => Err(RenderError::DeviceLost),
            Err(error) => {
                diagnostics.push(Diagnostic::error("F0105", format!("a pass failed to record: {error}")));
                self.fallback_run(output, &mut env)
            }
        }
    }

    // A host image (mode A) is kept as the host rendered it; every other target starts cleared.
    fn import_output(&self, graph: &mut FrameGraph, output: OutputTarget) -> TextureHandle {
        if output.keeps_contents {
            graph.import_texture(
                "output",
                output.texture,
                output.desc,
                output.initial,
                output.final_access,
            )
        } else {
            self.add_output_clear(graph, output)
        }
    }

    fn add_output_clear(&self, graph: &mut FrameGraph, output: OutputTarget) -> TextureHandle {
        let handle = graph.import_texture(
            "output",
            output.texture,
            output.desc,
            output.initial,
            output.final_access,
        );
        let clear_color = self.clear_color;
        let mut pass = graph.add_pass(PassDesc::new("clear output", Stage::Setup));
        let cleared = pass.write(handle, Access::COLOR_ATTACHMENT);
        pass.record(move |ctx| {
            let view = ctx.view(cleared)?;
            drop(ctx.commands().render_pass(RenderPassDesc {
                label: "clear output",
                color: vec![ColorAttachment::clear(view, clear_color)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        cleared
    }

    // A broken graph still hands the target back in its final state: cleared, or untouched for a host image.
    fn fallback_run(&mut self, output: OutputTarget, env: &mut FrameEnv<'_>) -> Result<ViewRun, RenderError> {
        let mut graph = FrameGraph::new();
        self.import_output(&mut graph, output);
        let compiled = graph
            .compile(FrameSizes::same(PhysicalSize::new(
                output.desc.size.width,
                output.desc.size.height,
            )))
            .map_err(|mut errors| RenderError::Diagnostic(errors.remove(0)))?;
        let execution = graph.execute(
            &compiled,
            env.device,
            &mut self.pool,
            &mut HistoryStore::new(),
            env.release,
            env.frame,
        )?;
        Ok(ViewRun {
            commands: execution.commands,
            stats: execution.stats,
            compiled: true,
            capture: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::graph::GraphTextureDesc;
    use pathon_rhi::{Command, Device, Format, Pool};

    use super::*;
    use crate::target::{self, OFFSCREEN_FINAL};

    #[test]
    fn a_graph_that_fails_to_compile_still_clears_the_target() {
        let device = Device::null();
        let size = PhysicalSize::new(32, 16);
        let desc = target::offscreen_desc(size, Format::Rgba8Unorm);
        let output = OutputTarget {
            texture: device.create_texture(&desc).expect("texture"),
            desc,
            initial: Access::empty(),
            final_access: OFFSCREEN_FINAL,
            keeps_contents: false,
        };
        let target = Pool::<TargetId, ()>::new().insert(());
        let mut release = DeferredRelease::new(2);
        let mut runner = GraphRunner::new(Color::BLACK);
        let mut diagnostics = Vec::new();

        let run = runner
            .run_view(
                target,
                output,
                FrameSizes::same(size),
                |graph, _output| {
                    let texture = graph.create_texture("never written", GraphTextureDesc::render(Format::Rgba8Unorm));
                    let mut pass = graph.add_pass(PassDesc::new("reads unwritten", Stage::PostProcess).side_effect());
                    pass.read(texture, Access::SAMPLED);
                    Vec::new()
                },
                FrameEnv {
                    device: &device,
                    release: &mut release,
                    frame: 0,
                },
                &mut diagnostics,
            )
            .expect("fallback run");

        assert_eq!(diagnostics[0].code, "G0003");
        assert!(
            run.commands
                .commands()
                .iter()
                .any(|command| matches!(command, Command::PushDebugGroup("clear output")))
        );
        device.submit(run.commands).expect("valid commands");
    }

    #[test]
    fn forgetting_a_target_releases_its_history_textures_and_views() {
        let device = Device::null();
        let size = PhysicalSize::new(32, 16);
        let desc = target::offscreen_desc(size, Format::Rgba8Unorm);
        let output = OutputTarget {
            texture: device.create_texture(&desc).expect("texture"),
            desc,
            initial: Access::empty(),
            final_access: OFFSCREEN_FINAL,
            keeps_contents: false,
        };
        let target = Pool::<TargetId, ()>::new().insert(());
        let mut release = DeferredRelease::new(2);
        let mut runner = GraphRunner::new(Color::BLACK);
        let mut diagnostics = Vec::new();
        runner
            .run_view(
                target,
                output,
                FrameSizes::same(size),
                |graph, _output| {
                    let history = graph.history_texture("history", GraphTextureDesc::render(Format::Rgba8Unorm));
                    let mut pass = graph.add_pass(PassDesc::new("writes history", Stage::PostProcess));
                    let current = pass.write(history.current, Access::COLOR_ATTACHMENT);
                    pass.record(move |ctx| {
                        ctx.view(current)?;
                        Ok(())
                    });
                    Vec::new()
                },
                FrameEnv {
                    device: &device,
                    release: &mut release,
                    frame: 0,
                },
                &mut diagnostics,
            )
            .expect("run");
        let before = release.len();

        runner.forget_target(target, &mut release, 1);

        assert!(runner.histories.is_empty());
        assert_eq!(release.len(), before + 3, "two textures and the view of `current`");
    }
}
