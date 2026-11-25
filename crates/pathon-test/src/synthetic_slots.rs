use std::collections::HashMap;

use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, Color, ColorAttachment, ColorTargetState, CompareFunction, DepthAttachment, DepthState, Format,
    OverrideValue, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError, ShaderModuleDesc,
    ShaderModuleId, ShaderStage, SubresourceRange, TextureViewDesc,
};
use pathon_api::{RecordCtx, RenderFeature, SetupCtx, Slot};

const WGSL: &str = include_str!("synthetic_slots.wgsl");

/// Mip levels of the synthetic `depth_pyramid`.
pub const SYNTHETIC_PYRAMID_LEVELS: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pattern {
    Float(u32),
    Depth,
    MaterialId,
}

fn pattern_of(slot: Slot) -> Option<Pattern> {
    let float_kinds = [
        (Slot::DEPTH_PYRAMID, 0),
        (Slot::NORMALS, 1),
        (Slot::MOTION, 2),
        (Slot::ROUGHNESS, 3),
        (Slot::ALBEDO, 4),
        (Slot::REACTIVE, 5),
        (Slot::DIFFUSE, 6),
        (Slot::SPECULAR, 7),
        (Slot::HDR, 8),
        (Slot::AO, 9),
        (Slot::EXPOSURE, 10),
    ];
    if slot == Slot::DEPTH {
        return Some(Pattern::Depth);
    }
    if slot == Slot::MATERIAL_ID {
        return Some(Pattern::MaterialId);
    }
    float_kinds
        .into_iter()
        .find(|(candidate, _)| *candidate == slot)
        .map(|(_, kind)| Pattern::Float(kind))
}

// Stands in for a PBR producer: writes every base slot with a known pattern (`synthetic_slots.wgsl`), so that debug
// views, goldens and examples have inputs before the real producers exist. On-demand slots appear only when some
// pack asks for them, like a real producer would do.
#[derive(Debug, Default)]
pub struct SyntheticSlots {
    skipped: Vec<Slot>,
    module: Option<ShaderModuleId>,
    pipelines: HashMap<(Pattern, Format), RenderPipelineId>,
}

impl SyntheticSlots {
    pub fn new() -> Self {
        Self::default()
    }

    /// Leaves `slot` unwritten, to test what a consumer does about a missing one.
    #[must_use]
    pub fn without(mut self, slot: Slot) -> Self {
        self.skipped.push(slot);
        self
    }

    fn pipeline(
        &mut self,
        ctx: &RecordCtx<'_>,
        pattern: Pattern,
        format: Format,
    ) -> Result<RenderPipelineId, RhiError> {
        if let Some(pipeline) = self.pipelines.get(&(pattern, format)) {
            return Ok(*pipeline);
        }
        let module = self.module.ok_or(RhiError::Unsupported("synthetic slots used before setup"))?;
        let (entry_point, constants, color_targets, depth) = match pattern {
            Pattern::Float(kind) => (
                "fs_float",
                vec![("KIND", OverrideValue::from(kind))],
                vec![ColorTargetState::opaque(format)],
                None,
            ),
            Pattern::MaterialId => ("fs_material_id", vec![], vec![ColorTargetState::opaque(format)], None),
            Pattern::Depth => (
                "fs_depth",
                vec![],
                vec![],
                Some(DepthState {
                    compare: CompareFunction::Always,
                    ..DepthState::reverse_z(format)
                }),
            ),
        };
        let pipeline = ctx.device().create_render_pipeline(&RenderPipelineDesc {
            label: "synthetic slots",
            bind_group_layouts: vec![],
            vertex: ShaderStage {
                module,
                entry_point: "vs_fullscreen",
                constants: vec![],
            },
            vertex_buffers: vec![],
            fragment: Some(ShaderStage {
                module,
                entry_point,
                constants,
            }),
            color_targets,
            primitive: PrimitiveState::default(),
            depth,
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.pipelines.insert((pattern, format), pipeline);
        Ok(pipeline)
    }
}

impl RenderFeature for SyntheticSlots {
    fn name(&self) -> &'static str {
        "synthetic slots"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.module = Some(ctx.device().create_shader_module(&ShaderModuleDesc {
            label: "synthetic slots",
            wgsl: WGSL,
        })?);
        self.pipelines.clear();
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        // The passes are independent; a priority each keeps their order from depending on registration (G0006).
        let mut order = 0;
        for slot in Slot::BUILT_IN {
            let wanted = !self.skipped.contains(&slot) && (!slot.is_on_demand() || ctx.view().is_requested(slot));
            let (Some(pattern), Some(format), true) = (pattern_of(slot), slot.format(), wanted) else {
                continue;
            };
            let pipeline = self.pipeline(ctx, pattern, format)?;
            let levels = if slot == Slot::DEPTH_PYRAMID { SYNTHETIC_PYRAMID_LEVELS } else { 1 };
            let desc = if slot == Slot::EXPOSURE {
                GraphTextureDesc::fixed(1, 1, format)
            } else {
                GraphTextureDesc::render(format).with_mips(levels)
            };
            let mut texture = ctx.graph().create_texture(slot.name(), desc);
            for level in 0..levels {
                let mut pass = ctx.graph().add_pass(PassDesc::new("synthetic slots", Stage::Opaque).priority(order));
                order += 1;
                let written = if pattern == Pattern::Depth {
                    pass.write(texture, Access::DEPTH_WRITE)
                } else {
                    pass.write_range(texture, Access::COLOR_ATTACHMENT, SubresourceRange::mip(level))
                };
                texture = written;
                pass.record(move |pass| {
                    let view = pass.view_with(written, &TextureViewDesc::single_mip(level))?;
                    let (color, depth) = if pattern == Pattern::Depth {
                        (vec![], Some(DepthAttachment::clear_reverse_z(view)))
                    } else {
                        (vec![ColorAttachment::clear(view, Color::BLACK)], None)
                    };
                    let mut render = pass.commands().render_pass(RenderPassDesc {
                        label: "synthetic slots",
                        color,
                        depth,
                        timestamps: None,
                    });
                    render.set_pipeline(pipeline);
                    render.draw(0..3, 0..1);
                    Ok(())
                });
            }
            ctx.set_slot(slot, texture);
        }
        Ok(())
    }
}
