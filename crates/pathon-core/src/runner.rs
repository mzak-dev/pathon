use std::collections::HashMap;

use pathon_diag::Diagnostic;
use pathon_graph::{
    FrameGraph, FrameSizes, GraphCache, GraphCapture, GraphStats, PassDesc, Stage, TextureHandle, TexturePool,
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
    pool: TexturePool,
    caches: HashMap<TargetId, GraphCache>,
    capture: bool,
    clear_color: Color,
}

impl GraphRunner {
    pub(crate) fn new(clear_color: Color) -> Self {
        Self {
            pool: TexturePool::new(),
            caches: HashMap::new(),
            capture: false,
            clear_color,
        }
    }

    pub(crate) fn set_capture(&mut self, enabled: bool) {
        self.capture = enabled;
    }

    pub(crate) fn forget_target(&mut self, target: TargetId) {
        self.caches.remove(&target);
    }

    pub(crate) fn forget_device(&mut self) {
        self.pool.forget_all();
        self.caches.clear();
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
        let mut graph = FrameGraph::new();
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
        let outcome = graph.execute(compiled, env.device, &mut self.pool, env.release, env.frame);
        diagnostics.extend(fresh_warnings);
        match outcome {
            Ok(execution) => Ok(ViewRun {
                commands: execution.commands,
                stats: execution.stats,
                compiled: fresh,
                capture: execution.capture,
            }),
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
        let execution = graph.execute(&compiled, env.device, &mut self.pool, env.release, env.frame)?;
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
                    pass.read(texture, Access::SHADER_READ);
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
}
