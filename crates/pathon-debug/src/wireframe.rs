use std::collections::HashMap;

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, BlendState, Caps,
    ColorAttachment, ColorTargetState, Features, Format, OverrideValue, PolygonMode, PrimitiveState, RenderPassDesc,
    RenderPipelineDesc, RenderPipelineId, RhiError, ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages,
    TextureSampleType,
};
use pathon_api::{Diagnostic, Negotiation, RecordCtx, RenderFeature, Requirements, SetupCtx, Slot};

use crate::draw::DebugDraw;
use crate::gpu::{
    FRAME_WGSL, FrameUniform, PerRecordUniforms, ReportOnce, VertexArena, joined_wgsl, linearizes_for, settings,
    texture_entry, uniform_entry,
};
use crate::settings::WireframeMode;
use crate::vertices::WireVertex;

const WIREFRAME_WGSL: &str = include_str!("wireframe.wgsl");
const DEPTH_ON_WGSL: &str = include_str!("depth_on.wgsl");
const DEPTH_OFF_WGSL: &str = include_str!("depth_off.wgsl");

#[derive(Clone, Copy, Debug)]
struct Variant {
    module: ShaderModuleId,
    layout: BindGroupLayoutId,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct PipelineKey {
    target: Format,
    has_depth: bool,
    hardware: bool,
}

/// Draws the meshes of `DebugDraw::wireframe` over `Slot::OUTPUT`.
///
/// With `Features::POLYGON_MODE_LINE` the pipeline rasterizes the triangle edges itself (1 px lines, ADR-0046). Without
/// it, or with `WireframeMode::Barycentric`, a fragment shader finds the edges from barycentric coordinates and honours
/// the thickness; negotiation reports the fallback as F0103.
#[derive(Debug)]
pub struct WireframeFeature {
    hardware_available: bool,
    variants: Option<[Variant; 2]>,
    pipelines: HashMap<PipelineKey, RenderPipelineId>,
    uniforms: Option<PerRecordUniforms>,
    arena: VertexArena,
    scratch: Vec<WireVertex>,
    reports: ReportOnce,
}

impl Default for WireframeFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl WireframeFeature {
    pub fn new() -> Self {
        Self {
            hardware_available: false,
            variants: None,
            pipelines: HashMap::new(),
            uniforms: None,
            arena: VertexArena::new("debug wireframe"),
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
            label: if key.hardware {
                "debug wireframe lines"
            } else {
                "debug wireframe barycentric"
            },
            bind_group_layouts: vec![variant.layout],
            vertex: ShaderStage {
                module: variant.module,
                entry_point: "vs_wire",
                constants: vec![],
            },
            vertex_buffers: vec![WireVertex::layout()],
            fragment: Some(ShaderStage {
                module: variant.module,
                entry_point: if key.hardware { "fs_hardware" } else { "fs_barycentric" },
                constants: vec![("LINEARIZE", OverrideValue::from(linearizes_for(key.target)))],
            }),
            color_targets: vec![ColorTargetState {
                blend: Some(BlendState::PREMULTIPLIED_ALPHA),
                ..ColorTargetState::opaque(key.target)
            }],
            primitive: PrimitiveState {
                polygon_mode: if key.hardware {
                    PolygonMode::Line
                } else {
                    PolygonMode::Fill
                },
                ..PrimitiveState::default()
            },
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.pipelines.insert(key, pipeline);
        Ok(pipeline)
    }
}

impl RenderFeature for WireframeFeature {
    fn name(&self) -> &'static str {
        "debug wireframe"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn priority(&self) -> i32 {
        10
    }

    fn requirements(&self) -> Requirements {
        Requirements {
            optional_features: Features::POLYGON_MODE_LINE,
            ..Requirements::default()
        }
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        self.hardware_available = caps.has(Features::POLYGON_MODE_LINE);
        if self.hardware_available {
            Negotiation::Full
        } else {
            Negotiation::Fallback {
                reason: "no line polygon mode on this device, wireframes use barycentric coordinates".to_owned(),
            }
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let visibility = ShaderStages::VERTEX | ShaderStages::FRAGMENT;
        let make = |label: &'static str, depth: bool| -> Result<Variant, RhiError> {
            let wgsl = joined_wgsl(&[
                FRAME_WGSL,
                if depth { DEPTH_ON_WGSL } else { DEPTH_OFF_WGSL },
                WIREFRAME_WGSL,
            ]);
            let module = device.create_shader_module(&ShaderModuleDesc { label, wgsl: &wgsl })?;
            let mut entries = vec![uniform_entry(0, visibility)];
            if depth {
                entries.push(texture_entry(1, ShaderStages::FRAGMENT, TextureSampleType::Depth));
            }
            let layout = device.create_bind_group_layout(&BindGroupLayoutDesc { label, entries })?;
            Ok(Variant { module, layout })
        };
        self.variants = Some([make("debug wireframe", false)?, make("debug wireframe depth", true)?]);
        self.pipelines.clear();
        self.uniforms = Some(PerRecordUniforms::new(device, "debug wireframe uniforms")?);
        self.arena.forget();
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let settings = settings(ctx);
        let frame = ctx.view().frame;
        self.scratch.clear();
        let dropped = match ctx.shared_mut().get_mut::<DebugDraw>() {
            Some(draw) => {
                draw.collect_wire_vertices(frame, &mut self.scratch);
                draw.take_dropped_wireframes()
            }
            None => 0,
        };
        self.reports.when("wireframes dropped", dropped > 0, ctx, || {
            Diagnostic::warning(
                "U0002",
                format!("{dropped} debug wireframes dropped: DebugDraw is full"),
            )
            .with_hint("raise the limit with DebugDraw::with_capacity or draw fewer triangles")
        });
        let Some(output) = ctx.slot(Slot::OUTPUT) else {
            return Ok(());
        };
        if self.scratch.is_empty() {
            return Ok(());
        }

        let depth = if settings.wireframe_depth_test {
            ctx.slot(Slot::DEPTH)
        } else {
            None
        };
        let variants = self
            .variants
            .ok_or(RhiError::Unsupported("debug wireframe used before setup"))?;
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
                hardware: self.hardware_available && settings.wireframe == WireframeMode::Auto,
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
            .ok_or(RhiError::Unsupported("debug wireframe used before setup"))?
            .write(ctx.device(), record_epoch, bytemuck::bytes_of(&frame_uniform))?;
        let (buffer, offset) = self.arena.write(ctx, bytemuck::cast_slice(&self.scratch))?;
        let vertex_count = u32::try_from(self.scratch.len()).unwrap_or(u32::MAX);

        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("debug wireframe", Stage::Overlay).priority(10));
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
                label: "debug wireframe",
                layout: variant.layout,
                entries,
            })?;
            let target = pass.view(output)?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "debug wireframe",
                color: vec![ColorAttachment::load(target)],
                depth: None,
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.set_bind_group(0, group);
            render.set_vertex_buffer(0, buffer, offset);
            render.draw(0..vertex_count, 0..1);
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, output);
        Ok(())
    }
}
