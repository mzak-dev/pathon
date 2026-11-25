use std::collections::HashMap;

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, BlendState,
    ColorAttachment, ColorTargetState, Format, OverrideValue, PrimitiveState, RenderPassDesc, RenderPipelineDesc,
    RenderPipelineId, RhiError, ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages, TextureSampleType,
};
use pathon_api::{Diagnostic, RecordCtx, RenderFeature, SetupCtx, Slot};

use crate::draw::DebugDraw;
use crate::gpu::{
    FRAME_WGSL, FrameUniform, PerRecordUniforms, ReportOnce, VertexArena, joined_wgsl, linearizes_for, settings,
    texture_entry, uniform_entry,
};
use crate::vertices::{LINE_FLAG_DEPTH_TEST, LineInstance};

const LINES_WGSL: &str = include_str!("lines.wgsl");
const DEPTH_ON_WGSL: &str = include_str!("depth_on.wgsl");
const DEPTH_OFF_WGSL: &str = include_str!("depth_off.wgsl");
const VERTICES_PER_LINE: u32 = 6;

#[derive(Clone, Copy, Debug)]
struct Variant {
    module: ShaderModuleId,
    layout: BindGroupLayoutId,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct PipelineKey {
    target: Format,
    has_depth: bool,
}

/// Draws the lines of `DebugDraw` over `Slot::OUTPUT`: screen-space quads, so the width is in pixels and the edges are
/// antialiased. Lines marked `depth_tested` are hidden behind `Slot::DEPTH`.
#[derive(Debug)]
pub struct LinesFeature {
    // Index 0: no depth texture bound, index 1: with it.
    variants: Option<[Variant; 2]>,
    pipelines: HashMap<PipelineKey, RenderPipelineId>,
    uniforms: Option<PerRecordUniforms>,
    arena: VertexArena,
    scratch: Vec<LineInstance>,
    reports: ReportOnce,
}

impl Default for LinesFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl LinesFeature {
    pub fn new() -> Self {
        Self {
            variants: None,
            pipelines: HashMap::new(),
            uniforms: None,
            arena: VertexArena::new("debug lines"),
            scratch: Vec::new(),
            reports: ReportOnce::default(),
        }
    }

    fn pipeline(
        &mut self,
        ctx: &RecordCtx<'_>,
        variant: Variant,
        key: PipelineKey,
    ) -> Result<RenderPipelineId, RhiError> {
        if let Some(pipeline) = self.pipelines.get(&key) {
            return Ok(*pipeline);
        }
        let pipeline = ctx.device().create_render_pipeline(&RenderPipelineDesc {
            label: "debug lines",
            bind_group_layouts: vec![variant.layout],
            vertex: ShaderStage {
                module: variant.module,
                entry_point: "vs_line",
                constants: vec![],
            },
            vertex_buffers: vec![LineInstance::layout()],
            fragment: Some(ShaderStage {
                module: variant.module,
                entry_point: "fs_line",
                constants: vec![("LINEARIZE", OverrideValue::from(linearizes_for(key.target)))],
            }),
            color_targets: vec![ColorTargetState {
                blend: Some(BlendState::PREMULTIPLIED_ALPHA),
                ..ColorTargetState::opaque(key.target)
            }],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.pipelines.insert(key, pipeline);
        Ok(pipeline)
    }
}

impl RenderFeature for LinesFeature {
    fn name(&self) -> &'static str {
        "debug lines"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn priority(&self) -> i32 {
        20
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let visibility = ShaderStages::VERTEX | ShaderStages::FRAGMENT;
        let make = |label: &'static str, depth: bool| -> Result<Variant, RhiError> {
            let wgsl = joined_wgsl(&[
                FRAME_WGSL,
                if depth { DEPTH_ON_WGSL } else { DEPTH_OFF_WGSL },
                LINES_WGSL,
            ]);
            let module = device.create_shader_module(&ShaderModuleDesc { label, wgsl: &wgsl })?;
            let mut entries = vec![uniform_entry(0, visibility)];
            if depth {
                entries.push(texture_entry(1, ShaderStages::FRAGMENT, TextureSampleType::Depth));
            }
            let layout = device.create_bind_group_layout(&BindGroupLayoutDesc { label, entries })?;
            Ok(Variant { module, layout })
        };
        self.variants = Some([make("debug lines", false)?, make("debug lines depth", true)?]);
        self.pipelines.clear();
        self.uniforms = Some(PerRecordUniforms::new(device, "debug lines uniforms")?);
        self.arena.forget();
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let settings = settings(ctx);
        let frame = ctx.view().frame;
        self.scratch.clear();
        let dropped = match ctx.shared_mut().get_mut::<DebugDraw>() {
            Some(draw) => {
                draw.collect_lines(frame, &mut self.scratch);
                draw.take_dropped_lines()
            }
            None => 0,
        };
        self.reports.when("lines dropped", dropped > 0, ctx, || {
            Diagnostic::warning("U0002", format!("{dropped} debug lines dropped: DebugDraw is full"))
                .with_hint("raise the limit with DebugDraw::with_capacity or draw fewer lines")
        });
        let Some(output) = ctx.slot(Slot::OUTPUT) else {
            return Ok(());
        };
        if !settings.lines || self.scratch.is_empty() {
            return Ok(());
        }

        let wants_depth = self.scratch.iter().any(|line| line.flags & LINE_FLAG_DEPTH_TEST != 0);
        let depth = if wants_depth { ctx.slot(Slot::DEPTH) } else { None };
        let variants = self
            .variants
            .ok_or(RhiError::Unsupported("debug lines used before setup"))?;
        let variant = variants[usize::from(depth.is_some())];
        let target = ctx
            .graph()
            .texture_format(output)
            .ok_or(RhiError::InvalidHandle("output"))?;
        let pipeline = self.pipeline(
            ctx,
            variant,
            PipelineKey {
                target,
                has_depth: depth.is_some(),
            },
        )?;

        let view = ctx.view();
        let frame_uniform = FrameUniform::new(
            view.camera.view_projection(),
            view.output_size,
            view.render_size,
            settings.depth_bias(),
        );
        let record_epoch = view.record_epoch;
        let uniform = self
            .uniforms
            .as_mut()
            .ok_or(RhiError::Unsupported("debug lines used before setup"))?
            .write(ctx.device(), record_epoch, bytemuck::bytes_of(&frame_uniform))?;
        let (buffer, offset) = self.arena.write(ctx, bytemuck::cast_slice(&self.scratch))?;
        let instance_count = u32::try_from(self.scratch.len()).unwrap_or(u32::MAX);

        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("debug lines", Stage::Overlay).priority(20));
        let depth = depth.map(|depth| pass.read(depth, Access::SAMPLED | Access::DEPTH_READ));
        let output = pass.modify(output, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let mut entries = vec![BindGroupEntry {
                binding: 0,
                resource: uniform.binding(),
            }];
            if let Some(depth) = depth {
                entries.push(BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(pass.view(depth)?),
                });
            }
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "debug lines",
                layout: variant.layout,
                entries,
            })?;
            let target = pass.view(output)?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "debug lines",
                color: vec![ColorAttachment::load(target)],
                depth: None,
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.set_bind_group(0, group);
            render.set_vertex_buffer(0, buffer, offset);
            render.draw(0..VERTICES_PER_LINE, 0..instance_count);
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, output);
        Ok(())
    }
}
