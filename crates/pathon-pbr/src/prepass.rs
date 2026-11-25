use std::sync::Arc;

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindingResource, Caps, Color, ColorAttachment, DepthAttachment, Format,
    FormatCaps, IndexFormat, RenderPassDesc, RhiError,
};
use pathon_api::{Diagnostic, Negotiation, RecordCtx, RenderFeature, SetupCtx, Slot};
use pathon_bindless::TextureBinder;
use pathon_material::{AlphaKind, MaterialHooks, MaterialTable};
use pathon_mesh::GeometryPool;
use pathon_scene::Snapshot;

use crate::cluster::ClusterGrid;
use crate::context::{PbrContext, ViewFrame, previous_view_projection, report_once};
use crate::draw::{DrawItem, SceneDraws, SceneSources, gather};
use crate::frame::{FrameInputs, FrameUniforms, ViewMatrices};
use crate::light::PackedLights;
use crate::settings::PbrSettings;
use crate::shaders::{PrepassKey, RenderPipeline};

/// Takes the shared state of the pack and the texture binder out of `Shared` for the duration of one record, so the
/// rest of `Shared` can be read while they are mutated, and puts them back whatever happens.
pub(crate) fn with_context<R>(
    ctx: &mut RecordCtx<'_>,
    body: impl FnOnce(&mut RecordCtx<'_>, &mut PbrContext, Option<&mut TextureBinder>) -> R,
) -> R {
    let mut pbr = ctx.shared_mut().remove::<PbrContext>().unwrap_or_default();
    let mut binder = ctx.shared_mut().remove::<TextureBinder>();
    let result = body(ctx, &mut pbr, binder.as_mut());
    if let Some(binder) = binder {
        ctx.shared_mut().insert(binder);
    }
    ctx.shared_mut().insert(pbr);
    result
}

/// Depth, normals, motion vectors and material ids of everything opaque and masked, plus roughness and albedo when
/// another pack asks for them. Also prepares what the colour pass needs: instances, lights and frame constants.
#[derive(Debug, Default)]
pub struct PrepassFeature;

impl PrepassFeature {
    pub fn new() -> Self {
        Self
    }
}

fn can_render(caps: &Caps, format: Format) -> bool {
    caps.format(format).contains(FormatCaps::RENDER_TARGET)
}

impl RenderFeature for PrepassFeature {
    fn name(&self) -> &'static str {
        "pbr prepass"
    }

    fn stage(&self) -> Stage {
        Stage::Prepass
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let formats = [
            Format::Depth32Float,
            Format::Rg16Float,
            Format::R32Uint,
            Format::R8Unorm,
            Format::Rgba8Unorm,
            Format::Rgba16Float,
        ];
        match formats.into_iter().find(|format| !can_render(caps, *format)) {
            None => Negotiation::Full,
            Some(format) => Negotiation::Disabled {
                reason: format!("L0006: {format:?} cannot be a render target on this device"),
            },
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        // The old pipelines and buffers died with the device: start the context again.
        if let Some(pbr) = ctx.shared().get_mut::<PbrContext>() {
            pbr.forget_gpu();
        } else {
            ctx.shared().insert(PbrContext::default());
        }
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        with_context(ctx, record_prepass)
    }
}

struct Inputs {
    world: Snapshot,
    settings: PbrSettings,
}

fn read_inputs(ctx: &mut RecordCtx<'_>, pbr: &mut PbrContext) -> Option<Inputs> {
    let shared = ctx.shared();
    let world = shared.get::<Snapshot>().cloned();
    let present = world.is_some() && shared.contains::<GeometryPool>() && shared.contains::<MaterialTable>();
    let requested = shared.get::<PbrSettings>().copied().unwrap_or_default();
    let (settings, problem) = requested.sanitized();
    if let Some(problem) = problem {
        report_once(
            ctx,
            &mut pbr.reported.settings,
            Diagnostic::warning("L0001", format!("PBR settings replaced by the defaults: {problem}"))
                .with_hint("keep PbrSettings inside the documented ranges"),
        );
    }
    if !present {
        report_once(
            ctx,
            &mut pbr.reported.missing_inputs,
            Diagnostic::warning(
                "L0004",
                "PBR needs a scene Snapshot, a GeometryPool and a MaterialTable in Shared",
            )
            .with_hint("insert them with Renderer::shared_mut(); the snapshot every frame"),
        );
        return None;
    }
    pbr.reported.missing_inputs = false;
    Some(Inputs {
        world: world?,
        settings,
    })
}

fn record_prepass(
    ctx: &mut RecordCtx<'_>,
    pbr: &mut PbrContext,
    mut binder: Option<&mut TextureBinder>,
) -> Result<(), RhiError> {
    let Some(Inputs { world, settings }) = read_inputs(ctx, pbr) else {
        return Ok(());
    };
    let view = ctx.view().clone();
    let device = ctx.device();

    // Materials: upload what changed; the old buffer of a grown array waits for the frames in flight.
    pbr.release().collect(view.frame, device);
    let material_buffer = {
        let release = pbr
            .material_release
            .get_or_insert_with(|| pathon_api::resources::DeferredRelease::new(4));
        let Some(mut table) = ctx.shared_mut().remove::<MaterialTable>() else {
            return Ok(());
        };
        let synced = table.sync(ctx.device(), release, view.frame);
        ctx.shared_mut().insert(table);
        synced?
    };

    let mut diagnostics = Vec::new();
    let draws = {
        let shared = ctx.shared();
        let (Some(pool), Some(materials)) = (shared.get::<GeometryPool>(), shared.get::<MaterialTable>()) else {
            return Ok(());
        };
        let sources = SceneSources {
            world: &world,
            pool,
            materials,
            camera_position: view.camera.position,
        };
        if let Some(binder) = binder.as_deref_mut() {
            binder
                .prepare(ctx.device())
                .map_err(|error| RhiError::validation("L0006", error.to_string()))?;
        }
        gather(&sources, ctx.device(), binder.as_deref_mut(), &mut diagnostics)
    };
    for diagnostic in diagnostics {
        if diagnostic.code != "L0003" || !std::mem::replace(&mut pbr.reported.skipped, true) {
            ctx.report(diagnostic);
        }
    }
    if draws.skipped == 0 {
        pbr.reported.skipped = false;
    }

    let hooks = ctx.shared().get::<MaterialHooks>().cloned();
    let texture_layout = binder.as_deref().map(TextureBinder::layout);
    let shaders = pbr
        .shaders(hooks.as_ref(), binder.as_deref())
        .map_err(|error| RhiError::validation("L0006", error.to_string()))?;

    let full = view.is_requested(Slot::ROUGHNESS) || view.is_requested(Slot::ALBEDO);
    let mut pipelines = [None, None];
    for (index, masked) in [false, true].into_iter().enumerate() {
        if draws
            .opaque
            .iter()
            .any(|item| (item.variant.alpha == AlphaKind::Masked) == masked)
        {
            let built = shaders.prepass_pipeline(
                ctx.device(),
                PrepassKey { masked, full },
                &draws.vertex_buffers,
                texture_layout,
            )?;
            pipelines[index] = Some(built);
        }
    }

    // Frame data shared with the colour pass.
    let lights = PackedLights::from_lights(world.lights().iter().map(|(_, light)| light));
    let matrices = ViewMatrices::from_view(&view);
    let previous = previous_view_projection(
        &mut pbr.histories,
        view.view,
        view.frame,
        matrices.unjittered_view_projection(),
    );
    let grid = ClusterGrid::new(view.render_size, &settings.clusters);
    let uniforms = FrameUniforms::new(&FrameInputs {
        matrices,
        previous_view_projection: previous,
        render_size: view.render_size,
        grid: &grid,
        directional_lights: lights.directional,
        total_lights: u32::try_from(lights.lights.len()).expect("fewer than 2^32 lights"),
        assignment: settings.light_assignment,
        environment_intensity: settings.environment_intensity,
    });
    let mut retired = Vec::new();
    let mut write = |ctx: &RecordCtx<'_>, arena: &mut crate::arena::BufferArena, bytes: &[u8]| {
        let (slice, old) = arena.write(ctx.device(), view.record_epoch, bytes)?;
        retired.extend(old);
        Ok::<_, RhiError>(slice)
    };
    let frame_uniforms = write(ctx, &mut pbr.arena, bytemuck::bytes_of(&uniforms))?;
    let instances = write(ctx, &mut pbr.arena, bytemuck::cast_slice(&draws.instances))?;
    let light_array = write(ctx, &mut pbr.arena, bytemuck::cast_slice(&lights.lights))?;
    for old in retired {
        ctx.release_later(old);
    }
    let draws = Arc::new(draws);
    pbr.views.insert(
        view.view,
        ViewFrame {
            record_epoch: view.record_epoch,
            draws: Arc::clone(&draws),
            lights,
            grid,
            settings,
            matrices,
            frame_uniforms,
            instances,
            light_array,
            materials: material_buffer,
        },
    );

    add_prepass(
        ctx,
        pipelines,
        draws,
        full,
        frame_uniforms.binding(),
        instances.binding(),
        material_buffer,
    );
    Ok(())
}

fn add_prepass(
    ctx: &mut RecordCtx<'_>,
    pipelines: [Option<RenderPipeline>; 2],
    draws: Arc<SceneDraws>,
    full: bool,
    frame_binding: BindingResource,
    instances_binding: BindingResource,
    materials: pathon_api::rhi::BufferId,
) {
    let depth = ctx
        .create_slot_texture(Slot::DEPTH, "depth")
        .expect("depth has a format");
    let normals = ctx
        .create_slot_texture(Slot::NORMALS, "normals")
        .expect("normals has a format");
    let motion = ctx
        .create_slot_texture(Slot::MOTION, "motion")
        .expect("motion has a format");
    let material_id = ctx
        .create_slot_texture(Slot::MATERIAL_ID, "material id")
        .expect("material id has a format");
    let extras = full.then(|| {
        (
            ctx.create_slot_texture(Slot::ROUGHNESS, "roughness")
                .expect("roughness has a format"),
            ctx.create_slot_texture(Slot::ALBEDO, "albedo")
                .expect("albedo has a format"),
        )
    });

    let mut pass = ctx.graph().add_pass(PassDesc::new("pbr prepass", Stage::Prepass));
    let depth = pass.write(depth, Access::DEPTH_WRITE);
    let normals = pass.write(normals, Access::COLOR_ATTACHMENT);
    let motion = pass.write(motion, Access::COLOR_ATTACHMENT);
    let material_id = pass.write(material_id, Access::COLOR_ATTACHMENT);
    let extras = extras.map(|(roughness, albedo)| {
        (
            pass.write(roughness, Access::COLOR_ATTACHMENT),
            pass.write(albedo, Access::COLOR_ATTACHMENT),
        )
    });
    pass.record(move |pass| {
        let mut color = vec![
            ColorAttachment::clear(pass.view(normals)?, Color::BLACK),
            ColorAttachment::clear(pass.view(motion)?, Color::BLACK),
            // No surface: an id no material has.
            ColorAttachment::clear(
                pass.view(material_id)?,
                Color {
                    r: f64::from(u32::MAX),
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
            ),
        ];
        if let Some((roughness, albedo)) = extras {
            color.push(ColorAttachment::clear(pass.view(roughness)?, Color::BLACK));
            color.push(ColorAttachment::clear(pass.view(albedo)?, Color::BLACK));
        }
        let depth_view = pass.view(depth)?;
        let frame_group = |pass: &mut pathon_api::graph::PassContext<'_>, pipeline: &RenderPipeline| {
            pass.create_bind_group(&BindGroupDesc {
                label: "pbr prepass frame",
                layout: pipeline.frame_layout,
                entries: vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: frame_binding.clone(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::whole_buffer(materials),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: instances_binding.clone(),
                    },
                ],
            })
        };
        let groups = [
            pipelines[0]
                .as_ref()
                .map(|pipeline| frame_group(pass, pipeline))
                .transpose()?,
            pipelines[1]
                .as_ref()
                .map(|pipeline| frame_group(pass, pipeline))
                .transpose()?,
        ];
        let mut render = pass.commands().render_pass(RenderPassDesc {
            label: "pbr prepass",
            color,
            depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
            timestamps: None,
        });
        draw_list(&mut render, &draws, &draws.opaque, |item| {
            let index = usize::from(item.variant.alpha == AlphaKind::Masked);
            pipelines[index]
                .zip(groups[index])
                .map(|(pipeline, group)| (pipeline.pipeline, group))
        });
        Ok(())
    });
    ctx.set_slot(Slot::DEPTH, depth);
    ctx.set_slot(Slot::NORMALS, normals);
    ctx.set_slot(Slot::MOTION, motion);
    ctx.set_slot(Slot::MATERIAL_ID, material_id);
    if let Some((roughness, albedo)) = extras {
        ctx.set_slot(Slot::ROUGHNESS, roughness);
        ctx.set_slot(Slot::ALBEDO, albedo);
    }
}

/// Records `items` (grouped by what `select` returns, then by chunk) into an open render pass: pipeline and frame
/// group when they change, material textures when the group changes, vertex and index buffers when the chunk changes.
pub(crate) fn draw_list(
    render: &mut pathon_api::rhi::RenderPassEncoder<'_>,
    draws: &SceneDraws,
    items: &[DrawItem],
    select: impl Fn(&DrawItem) -> Option<(pathon_api::rhi::RenderPipelineId, pathon_api::rhi::BindGroupId)>,
) {
    let mut current_pipeline = None;
    let mut current_chunk = None;
    let mut current_textures = None;
    for item in items {
        let Some((pipeline, group)) = select(item) else {
            continue;
        };
        if current_pipeline != Some(pipeline) {
            render.set_pipeline(pipeline);
            render.set_bind_group(0, group);
            current_pipeline = Some(pipeline);
        }
        if current_chunk != Some(item.chunk) {
            let chunk = &draws.chunks[item.chunk];
            for (slot, buffer) in (0..).zip(&chunk.vertex) {
                render.set_vertex_buffer(slot, *buffer, 0);
            }
            render.set_index_buffer(chunk.index, IndexFormat::Uint32, 0);
            current_chunk = Some(item.chunk);
        }
        if item.texture_group.is_some() && current_textures != item.texture_group {
            if let Some(textures) = item.texture_group {
                render.set_bind_group(crate::shaders::TEXTURE_GROUP, textures);
            }
            current_textures = item.texture_group;
        }
        render.draw_indexed(
            item.first_index..item.first_index + item.index_count,
            item.base_vertex,
            item.instance..item.instance + 1,
        );
    }
}
