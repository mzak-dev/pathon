use pathon_api::graph::{GraphBufferDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindingResource, BufferUsage, Caps, Color, ColorAttachment, DepthAccess,
    DepthAttachment, Format, FormatCaps, LoadOp, RenderPassDesc, RhiError, StoreOp,
};
use pathon_api::{Negotiation, RecordCtx, RenderFeature, Slot};
use pathon_bindless::TextureBinder;
use pathon_ibl::{IblBindings, IblEnvironment};
use pathon_material::{AlphaKind, MaterialHooks, MaterialVariant};

use crate::cluster::{COUNTERS_SIZE, ClusterBuffers, ClusterParams, ClusterPipeline, RANGE_SIZE};
use crate::context::PbrContext;
use crate::prepass::{draw_list, with_context};
use crate::shaders::{ForwardKey, IBL_FIRST_BINDING, RenderPipeline};

/// Assigns the lights to clusters in compute, then draws the opaque and masked meshes (`Stage::Opaque`) and the blended
/// ones, farthest first (`Stage::Transparent`), into `hdr`; `diffuse` and `specular` are written too when another pack
/// asked for them.
#[derive(Debug, Default)]
pub struct LightingFeature;

impl LightingFeature {
    pub fn new() -> Self {
        Self
    }
}

impl RenderFeature for LightingFeature {
    fn name(&self) -> &'static str {
        "pbr lighting"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn after(&self) -> &[&'static str] {
        &["pbr prepass"]
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let hdr = caps.format(Format::Rgba16Float);
        if !hdr.contains(FormatCaps::RENDER_TARGET | FormatCaps::BLENDABLE | FormatCaps::SAMPLED | FormatCaps::FILTERABLE)
        {
            return Negotiation::Disabled {
                reason: "L0006: Rgba16Float must be a blendable, filterable render target".to_owned(),
            };
        }
        Negotiation::Full
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        with_context(ctx, |ctx, pbr, binder| record_lighting(ctx, pbr, binder.as_deref()))
    }
}

fn record_lighting(
    ctx: &mut RecordCtx<'_>,
    pbr: &mut PbrContext,
    binder: Option<&TextureBinder>,
) -> Result<(), RhiError> {
    let view = ctx.view().clone();
    let Some(frame) = pbr.views.get(&view.view).filter(|frame| frame.record_epoch == view.record_epoch).cloned() else {
        return Ok(());
    };
    let Some(depth) = ctx.slot(Slot::DEPTH) else {
        return Ok(());
    };
    let device = ctx.device();
    let split = view.is_requested(Slot::DIFFUSE) || view.is_requested(Slot::SPECULAR);
    let texture_layout = binder.map(TextureBinder::layout);
    let hooks = ctx.shared().get::<MaterialHooks>().cloned();
    let published = ctx.shared().get::<IblEnvironment>().copied();
    let environment = pbr.environment(device, published)?;
    pbr.shaders(hooks.as_ref(), binder)
        .map_err(|error| RhiError::validation("L0006", error.to_string()))?;
    let cluster = match pbr.cluster {
        Some(cluster) => cluster,
        None => {
            let shaders = pbr.shaders.as_mut().expect("built above");
            let built = ClusterPipeline::new(device, shaders)
                .map_err(|error| RhiError::validation("L0006", error.to_string()))?;
            pbr.cluster = Some(built);
            built
        }
    };
    let shaders = pbr.shaders.as_mut().expect("built above");

    // One pipeline per permutation the scene uses.
    let mut variants: Vec<(MaterialVariant, RenderPipeline)> = Vec::new();
    for (items, split) in [(&frame.draws.opaque, split), (&frame.draws.transparent, false)] {
        for item in items {
            if !variants.iter().any(|(variant, _)| *variant == item.variant) {
                let pipeline = shaders
                    .forward_pipeline(
                        device,
                        ForwardKey {
                            variant: item.variant,
                            split: split && item.variant.alpha != AlphaKind::Transparent,
                        },
                        &frame.draws.vertex_buffers,
                        texture_layout,
                    )?;
                variants.push((item.variant, pipeline));
            }
        }
    }

    // Cluster buffers and the compute pass that fills them.
    let grid = frame.grid;
    let max_indices = u64::from(frame.settings.clusters.max_light_indices);
    let ranges = ctx.graph().create_buffer(
        "pbr cluster ranges",
        GraphBufferDesc::new(u64::from(grid.count()) * RANGE_SIZE, BufferUsage::STORAGE),
    );
    let indices = ctx.graph().create_buffer(
        "pbr cluster indices",
        GraphBufferDesc::new(max_indices * 4, BufferUsage::STORAGE),
    );
    let counters = ctx
        .graph()
        .create_buffer("pbr cluster counters", GraphBufferDesc::new(COUNTERS_SIZE, BufferUsage::STORAGE));
    let params = ClusterParams::new(
        &grid,
        frame.matrices.projection,
        frame.matrices.world_to_view,
        view.render_size,
        frame.lights.directional,
        u32::try_from(frame.lights.clustered().len()).expect("fewer than 2^32 lights"),
        frame.settings.clusters.max_light_indices,
    );
    let (params_slice, retired) = pbr.arena.write(ctx.device(), view.record_epoch, bytemuck::bytes_of(&params))?;
    if let Some(old) = retired {
        ctx.release_later(old);
    }
    let light_array = frame.light_array;
    let mut pass = ctx.graph().add_pass(PassDesc::new("pbr cluster lights", Stage::Opaque));
    let ranges = pass.write_buffer(ranges, Access::STORAGE_WRITE);
    let indices = pass.write_buffer(indices, Access::STORAGE_WRITE);
    let counters = pass.write_buffer(counters, Access::STORAGE_WRITE);
    pass.record(move |pass| {
        let buffers = ClusterBuffers {
            params: params_slice.binding(),
            lights: light_array.binding(),
            ranges: BindingResource::whole_buffer(pass.buffer(ranges)),
            indices: BindingResource::whole_buffer(pass.buffer(indices)),
            counters: BindingResource::whole_buffer(pass.buffer(counters)),
        };
        let group = pass.create_bind_group(&cluster.bind_group_desc(&buffers))?;
        cluster.record(pass.commands(), group, &grid);
        Ok(())
    });

    // The colour passes.
    let hdr = ctx.create_slot_texture(Slot::HDR, "hdr").expect("hdr has a format");
    let split_slots = split.then(|| {
        (
            ctx.create_slot_texture(Slot::DIFFUSE, "diffuse").expect("diffuse has a format"),
            ctx.create_slot_texture(Slot::SPECULAR, "specular").expect("specular has a format"),
        )
    });
    let pipeline_of = move |variants: &[(MaterialVariant, RenderPipeline)], variant: MaterialVariant| {
        variants.iter().find(|(known, _)| *known == variant).map(|(_, pipeline)| *pipeline)
    };

    let mut pass = ctx.graph().add_pass(PassDesc::new("pbr forward", Stage::Opaque));
    let depth_read = pass.read(depth, Access::DEPTH_READ);
    let ranges_read = pass.read_buffer(ranges, Access::STORAGE_READ);
    let indices_read = pass.read_buffer(indices, Access::STORAGE_READ);
    let hdr = pass.write(hdr, Access::COLOR_ATTACHMENT);
    let split_slots = split_slots.map(|(diffuse, specular)| {
        (
            pass.write(diffuse, Access::COLOR_ATTACHMENT),
            pass.write(specular, Access::COLOR_ATTACHMENT),
        )
    });
    let draws = frame.draws.clone();
    let (frame_uniforms, instances, light_slice, materials) =
        (frame.frame_uniforms, frame.instances, frame.light_array, frame.materials);
    let opaque_pipelines = variants.clone();
    pass.record(move |pass| {
        let mut color = vec![ColorAttachment::clear(pass.view(hdr)?, Color::BLACK)];
        if let Some((diffuse, specular)) = split_slots {
            color.push(ColorAttachment::clear(pass.view(diffuse)?, Color::BLACK));
            color.push(ColorAttachment::clear(pass.view(specular)?, Color::BLACK));
        }
        let depth_view = pass.view(depth_read)?;
        let ranges = pass.buffer(ranges_read);
        let indices = pass.buffer(indices_read);
        let groups = frame_groups(
            pass,
            &opaque_pipelines,
            &FrameBindings {
                frame_uniforms: frame_uniforms.binding(),
                materials,
                instances: instances.binding(),
                lights: light_slice.binding(),
                ranges,
                indices,
                environment,
            },
        )?;
        let mut render = pass.commands().render_pass(RenderPassDesc {
            label: "pbr forward",
            color,
            depth: Some(read_only_depth(depth_view)),
            timestamps: None,
        });
        draw_list(&mut render, &draws, &draws.opaque, |item| {
            let pipeline = pipeline_of(&opaque_pipelines, item.variant)?;
            let (_, group) = groups.iter().find(|(variant, _)| *variant == item.variant)?;
            Some((pipeline.pipeline, *group))
        });
        Ok(())
    });
    ctx.set_slot(Slot::HDR, hdr);
    if let Some((diffuse, specular)) = split_slots {
        ctx.set_slot(Slot::DIFFUSE, diffuse);
        ctx.set_slot(Slot::SPECULAR, specular);
    }

    if frame.draws.transparent.is_empty() {
        return Ok(());
    }
    let mut pass = ctx.graph().add_pass(PassDesc::new("pbr transparent", Stage::Transparent));
    let depth_read = pass.read(depth, Access::DEPTH_READ);
    let ranges_read = pass.read_buffer(ranges, Access::STORAGE_READ);
    let indices_read = pass.read_buffer(indices, Access::STORAGE_READ);
    let hdr = pass.modify(hdr, Access::COLOR_ATTACHMENT);
    let draws = frame.draws.clone();
    pass.record(move |pass| {
        let depth_view = pass.view(depth_read)?;
        let ranges = pass.buffer(ranges_read);
        let indices = pass.buffer(indices_read);
        let groups = frame_groups(
            pass,
            &variants,
            &FrameBindings {
                frame_uniforms: frame_uniforms.binding(),
                materials,
                instances: instances.binding(),
                lights: light_slice.binding(),
                ranges,
                indices,
                environment,
            },
        )?;
        let hdr_view = pass.view(hdr)?;
        let mut render = pass.commands().render_pass(RenderPassDesc {
            label: "pbr transparent",
            color: vec![ColorAttachment::load(hdr_view)],
            depth: Some(read_only_depth(depth_view)),
            timestamps: None,
        });
        draw_list(&mut render, &draws, &draws.transparent, |item| {
            let pipeline = pipeline_of(&variants, item.variant)?;
            let (_, group) = groups.iter().find(|(variant, _)| *variant == item.variant)?;
            Some((pipeline.pipeline, *group))
        });
        Ok(())
    });
    ctx.set_slot(Slot::HDR, hdr);
    Ok(())
}

fn read_only_depth(view: pathon_api::rhi::TextureViewId) -> DepthAttachment {
    DepthAttachment {
        view,
        load: LoadOp::Load,
        store: StoreOp::Store,
        access: DepthAccess::ReadOnly,
        clear_stencil: None,
    }
}

struct FrameBindings {
    frame_uniforms: BindingResource,
    materials: pathon_api::rhi::BufferId,
    instances: BindingResource,
    lights: BindingResource,
    ranges: pathon_api::rhi::BufferId,
    indices: pathon_api::rhi::BufferId,
    environment: IblEnvironment,
}

// One group 0 per pipeline layout in use; variants share a layout, so the groups are equal but each pipeline's own
// layout object is what the bind group is created against.
fn frame_groups(
    pass: &mut pathon_api::graph::PassContext<'_>,
    pipelines: &[(MaterialVariant, RenderPipeline)],
    bindings: &FrameBindings,
) -> Result<Vec<(MaterialVariant, pathon_api::rhi::BindGroupId)>, RhiError> {
    let mut groups = Vec::new();
    let mut by_layout: Vec<(pathon_api::rhi::BindGroupLayoutId, pathon_api::rhi::BindGroupId)> = Vec::new();
    for (variant, pipeline) in pipelines {
        let existing = by_layout.iter().find(|(layout, _)| *layout == pipeline.frame_layout).map(|(_, group)| *group);
        let group = match existing {
            Some(group) => group,
            None => {
                let mut entries = vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: bindings.frame_uniforms.clone(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::whole_buffer(bindings.materials),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: bindings.instances.clone(),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: bindings.lights.clone(),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::whole_buffer(bindings.ranges),
                    },
                    BindGroupEntry {
                        binding: 5,
                        resource: BindingResource::whole_buffer(bindings.indices),
                    },
                ];
                entries.extend(IblBindings::starting_at(IBL_FIRST_BINDING).bind_group_entries(&bindings.environment));
                let group = pass.create_bind_group(&BindGroupDesc {
                    label: "pbr forward frame",
                    layout: pipeline.frame_layout,
                    entries,
                })?;
                by_layout.push((pipeline.frame_layout, group));
                group
            }
        };
        groups.push((*variant, group));
    }
    Ok(groups)
}
