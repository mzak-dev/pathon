// The GPU half of the shadows of point and spot lights: the atlas texture that outlives a frame, the plan of the frame
// (`crate::local`), one render pass that redraws only the tiles whose content changed, and what the lighting reads.

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::resources::UniformSlice;
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindingResource, DepthAccess, DepthAttachment, DrawIndexedIndirectArgs,
    Features, Format, LoadOp, RenderPassDesc, RhiError, ScissorRect, StoreOp, TextureDesc, TextureId, TextureUsage,
    Viewport,
};
use pathon_api::{Diagnostic, RecordCtx};

use crate::atlas::Tile;
use crate::feature::{CascadePass, PassBatch, ShadowFeature, draw_batches, hardware_bias};
use crate::frame::SHADOW_MAP_ACCESS;
use crate::input::ShadowScene;
use crate::local::{Frustum, LocalPlan, LocalPlanner, ViewPoint};
use crate::local_frame::{
    LocalLightReport, LocalShadowFrame, LocalShadowReport, LocalShadowUniforms, SHADOW_LOCAL_MAP,
};
use crate::settings::ShadowSettings;

const ATLAS_FORMAT: Format = Format::Depth32Float;

#[derive(Debug)]
struct AtlasTexture {
    texture: TextureId,
    desc: TextureDesc,
    // Nothing was ever drawn into it: the first pass clears all of it.
    fresh: bool,
}

#[derive(Clone, Copy, Debug)]
struct Memo {
    epoch: u64,
    frame: Option<LocalShadowFrame>,
}

/// What the pack remembers about local lights between frames.
#[derive(Debug, Default)]
pub(crate) struct LocalState {
    planner: Option<LocalPlanner>,
    atlas: Option<AtlasTexture>,
    memo: Option<Memo>,
    reported_full: bool,
    reported_invalid: bool,
}

// A tile to redraw: where in the atlas, and what to draw with which matrix.
struct TileDraw {
    tile: Tile,
    pass: CascadePass,
}

impl ShadowFeature {
    // `after_cascades`: the pass of the cascades is in this graph, and the atlas pass comes after it.
    pub(crate) fn record_local(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        settings: &ShadowSettings,
        after_cascades: bool,
    ) -> Result<(), RhiError> {
        let epoch = ctx.view().record_epoch;
        let offered = ctx
            .shared()
            .get::<ShadowScene>()
            .is_some_and(|scene| !scene.local_lights().is_empty());
        if settings.max_local_lights == 0 || !offered {
            self.local.memo = None;
            self.local.reported_full = false;
            self.local.reported_invalid = false;
            ctx.shared_mut().remove::<LocalShadowFrame>();
            ctx.shared_mut().remove::<LocalShadowReport>();
            return Ok(());
        }
        if let Some(memo) = self.local.memo.filter(|memo| memo.epoch == epoch) {
            // A second view of the frame: the maps are drawn already, this view only reads them.
            return self.publish_again(ctx, memo.frame);
        }
        let frame = self.plan_and_draw_local(ctx, settings, after_cascades)?;
        self.local.memo = Some(Memo { epoch, frame });
        Ok(())
    }

    fn publish_again(&mut self, ctx: &mut RecordCtx<'_>, frame: Option<LocalShadowFrame>) -> Result<(), RhiError> {
        let Some(frame) = frame else {
            ctx.shared_mut().remove::<LocalShadowFrame>();
            return Ok(());
        };
        let atlas = self
            .local
            .atlas
            .as_ref()
            .ok_or(RhiError::Unsupported("local shadows used before they were planned"))?;
        let handle = ctx.graph().import_texture(
            "shadow local atlas",
            atlas.texture,
            atlas.desc,
            SHADOW_MAP_ACCESS,
            SHADOW_MAP_ACCESS,
        );
        ctx.set_slot(SHADOW_LOCAL_MAP, handle);
        ctx.shared_mut().insert(frame);
        Ok(())
    }

    // The atlas and the planner for these settings, created when there are none or the settings moved.
    fn ensure_atlas(&mut self, ctx: &mut RecordCtx<'_>, settings: &ShadowSettings) -> Result<(), RhiError> {
        let fits = self.local.atlas.is_some()
            && self
                .local
                .planner
                .as_ref()
                .is_some_and(|planner| planner.fits(settings));
        if fits {
            return Ok(());
        }
        if let Some(old) = self.local.atlas.take() {
            ctx.release_later(old.texture);
        }
        let size = settings.local_atlas_size;
        let desc = TextureDesc::new_2d(
            "shadow local atlas",
            size,
            size,
            ATLAS_FORMAT,
            TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED,
        );
        let texture = ctx.device().create_texture(&desc)?;
        self.local.atlas = Some(AtlasTexture {
            texture,
            desc,
            fresh: true,
        });
        self.local.planner = Some(LocalPlanner::new(size, settings.local_min_resolution));
        Ok(())
    }

    fn report_problems(&mut self, ctx: &mut RecordCtx<'_>, plan: &LocalPlan) {
        let stats = &plan.stats;
        if stats.dropped_for_space == 0 {
            self.local.reported_full = false;
        } else if !self.local.reported_full {
            self.local.reported_full = true;
            ctx.report(
                Diagnostic::warning(
                    "D0101",
                    format!(
                        "{} local light(s) found no room for a shadow map in the atlas",
                        stats.dropped_for_space
                    ),
                )
                .with_hint("raise local_atlas_size, lower the map sizes or max_local_lights"),
            );
        }
        if stats.invalid == 0 {
            self.local.reported_invalid = false;
        } else if !self.local.reported_invalid {
            self.local.reported_invalid = true;
            ctx.report(
                Diagnostic::warning(
                    "D0102",
                    format!(
                        "{} local light(s) are not usable for shadows and were skipped",
                        stats.invalid
                    ),
                )
                .with_hint("a position, range, intensity or spot cone is not finite or not positive"),
            );
        }
    }

    // First view of a frame: choose the lights, plan the tiles, draw the ones that changed, publish the result.
    #[expect(clippy::too_many_lines, reason = "one pass: plan, upload, draw, publish")]
    fn plan_and_draw_local(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        settings: &ShadowSettings,
        after_cascades: bool,
    ) -> Result<Option<LocalShadowFrame>, RhiError> {
        let gpu = self.gpu.ok_or(RhiError::Unsupported("shadows used before setup"))?;
        self.ensure_atlas(ctx, settings)?;
        let invalidated = ctx
            .shared_mut()
            .get_mut::<ShadowScene>()
            .is_some_and(ShadowScene::take_local_invalidation);
        let camera = ctx.view().camera;
        let view = ViewPoint {
            position: camera.position,
            frustum: Frustum::from_view_projection(camera.view_projection()),
        };
        let frame_number = ctx.view().frame;
        let plan = {
            let scene = ctx
                .shared()
                .get::<ShadowScene>()
                .ok_or(RhiError::Unsupported("the shadow scene is gone"))?;
            let planner = self
                .local
                .planner
                .as_mut()
                .ok_or(RhiError::Unsupported("local shadows used before setup"))?;
            if invalidated {
                planner.invalidate();
            }
            planner.plan(scene.local_lights(), scene.casters(), &view, settings, frame_number)
        };
        self.report_problems(ctx, &plan);
        let report = LocalShadowReport {
            lights: plan
                .lights
                .iter()
                .map(|planned| LocalLightReport {
                    key: planned.light.key,
                    light_index: planned.light.light_index,
                    tile_size: plan.tiles[planned.first_tile].tile.size,
                })
                .collect(),
            candidates: plan.stats.candidates,
            out_of_view: plan.stats.out_of_view,
            dropped_for_space: plan.stats.dropped_for_space,
            rendered_tiles: plan.stats.rendered_tiles,
            cached_tiles: plan.stats.cached_tiles,
            drawn_casters: plan
                .dirty_tiles()
                .flat_map(|tile| &tile.batches)
                .map(|batch| batch.draws.len())
                .sum(),
            atlas_size: settings.local_atlas_size,
        };
        ctx.shared_mut().insert(report);
        if plan.lights.is_empty() {
            ctx.shared_mut().remove::<LocalShadowFrame>();
            return Ok(None);
        }

        let uniforms = LocalShadowUniforms::new(&plan, settings.local_atlas_size, settings);
        let uniform_buffer = {
            let buffers = self
                .buffers
                .as_mut()
                .ok_or(RhiError::Unsupported("shadows used before setup"))?;
            buffers.write_uniform(ctx, bytemuck::bytes_of(&uniforms))?
        };
        let (draws, models) = self.prepare_tile_draws(ctx, settings, &plan)?;
        let (texture, desc, fresh) = {
            let atlas = self
                .local
                .atlas
                .as_mut()
                .ok_or(RhiError::Unsupported("local shadows used before setup"))?;
            let state = (atlas.texture, atlas.desc, atlas.fresh);
            if !draws.is_empty() || atlas.fresh {
                atlas.fresh = false;
            }
            state
        };
        // A texture nothing was drawn into is in no state yet; from then on it rests as a sampled depth map.
        let initial = if fresh { Access::empty() } else { SHADOW_MAP_ACCESS };
        let mut handle = ctx
            .graph()
            .import_texture("shadow local atlas", texture, desc, initial, SHADOW_MAP_ACCESS);
        if !draws.is_empty() || fresh {
            let desc = PassDesc::new("shadow local", Stage::Shadows).side_effect();
            let mut pass = ctx.graph().add_pass(if after_cascades {
                desc.after("shadow cascades")
            } else {
                desc
            });
            let atlas = if fresh {
                pass.write(handle, Access::DEPTH_WRITE)
            } else {
                pass.modify(handle, Access::DEPTH_WRITE)
            };
            handle = atlas;
            pass.record(move |pass| {
                let view = pass.view(atlas)?;
                let mut groups = Vec::with_capacity(draws.len());
                for draw in &draws {
                    let group = match (&models, draw.pass.batches.is_empty()) {
                        (Some(models), false) => Some(pass.create_bind_group(&BindGroupDesc {
                            label: "shadow local tile",
                            layout: gpu.layout,
                            entries: vec![
                                BindGroupEntry {
                                    binding: 0,
                                    resource: draw.pass.uniform.binding(),
                                },
                                BindGroupEntry {
                                    binding: 1,
                                    resource: models.clone(),
                                },
                            ],
                        })?),
                        _ => None,
                    };
                    groups.push(group);
                }
                let load = if fresh { LoadOp::Clear(0.0) } else { LoadOp::Load };
                let mut render = pass.commands().render_pass(RenderPassDesc {
                    label: "shadow local",
                    color: vec![],
                    depth: Some(DepthAttachment {
                        view,
                        load,
                        store: StoreOp::Store,
                        access: DepthAccess::ReadWrite,
                        clear_stencil: None,
                    }),
                    timestamps: None,
                });
                for (draw, group) in draws.iter().zip(groups) {
                    let tile = draw.tile;
                    #[expect(clippy::cast_precision_loss, reason = "tiles are at most 8192 texels")]
                    render.set_viewport(Viewport {
                        x: tile.x as f32,
                        y: tile.y as f32,
                        width: tile.size as f32,
                        height: tile.size as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    });
                    render.set_scissor(ScissorRect {
                        x: tile.x,
                        y: tile.y,
                        width: tile.size,
                        height: tile.size,
                    });
                    render.set_pipeline(gpu.clear_pipeline);
                    render.draw(0..3, 0..1);
                    if let Some(group) = group {
                        draw_batches(&mut render, group, &draw.pass.batches);
                    }
                }
                Ok(())
            });
        }
        ctx.set_slot(SHADOW_LOCAL_MAP, handle);
        let frame = LocalShadowFrame {
            uniform_buffer,
            sampler: gpu.sampler,
            light_count: u32::try_from(plan.lights.len()).unwrap_or(0),
            atlas_size: settings.local_atlas_size,
        };
        ctx.shared_mut().insert(frame);
        Ok(Some(frame))
    }

    // Uniforms, indirect arguments and the model matrices of the tiles that are redrawn.
    fn prepare_tile_draws(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        settings: &ShadowSettings,
        plan: &LocalPlan,
    ) -> Result<(Vec<TileDraw>, Option<BindingResource>), RhiError> {
        let dirty: Vec<_> = plan.dirty_tiles().collect();
        if dirty.is_empty() {
            return Ok((Vec::new(), None));
        }
        let indirect = settings.use_indirect_draws && ctx.device().caps().has(Features::INDIRECT_FIRST_INSTANCE);
        let bias = hardware_bias(settings);
        let mut draws = Vec::with_capacity(dirty.len());
        let mut arguments = Vec::new();
        let mut argument_batches = Vec::new();
        for tile in dirty {
            let uniform: UniformSlice = {
                let buffers = self
                    .buffers
                    .as_mut()
                    .ok_or(RhiError::Unsupported("shadows used before setup"))?;
                buffers.write_uniform(ctx, bytemuck::bytes_of(&tile.view_projection.to_cols_array_2d()))?
            };
            let mut batches = Vec::with_capacity(tile.batches.len());
            for batch in &tile.batches {
                let pipeline = self.pipeline(ctx.device(), batch.vertex_stride, bias)?;
                if indirect {
                    argument_batches.push((draws.len(), batches.len(), arguments.len() as u64));
                    for draw in &batch.draws {
                        arguments.extend_from_slice(
                            &DrawIndexedIndirectArgs {
                                index_count: draw.index_count,
                                instance_count: 1,
                                first_index: draw.first_index,
                                base_vertex: draw.base_vertex,
                                first_instance: draw.caster,
                            }
                            .to_bytes(),
                        );
                    }
                }
                batches.push(PassBatch {
                    pipeline,
                    vertex_buffer: batch.vertex_buffer,
                    index_buffer: batch.index_buffer,
                    draws: batch.draws.clone(),
                    arguments: None,
                });
            }
            draws.push(TileDraw {
                tile: tile.tile,
                pass: CascadePass { uniform, batches },
            });
        }
        let buffers = self
            .buffers
            .as_mut()
            .ok_or(RhiError::Unsupported("shadows used before setup"))?;
        let models = if draws.iter().any(|draw| !draw.pass.batches.is_empty()) {
            let bytes: Vec<f32> = plan
                .models
                .iter()
                .flat_map(pathon_api::glam::Mat4::to_cols_array)
                .collect();
            let (buffer, offset) = buffers.models.write(ctx, bytemuck::cast_slice(&bytes))?;
            Some(BindingResource::Buffer {
                buffer,
                offset,
                size: Some((bytes.len() * 4) as u64),
            })
        } else {
            None
        };
        if !arguments.is_empty() {
            let (buffer, base) = buffers.arguments.write(ctx, &arguments)?;
            for (draw, batch, offset) in argument_batches {
                draws[draw].pass.batches[batch].arguments = Some((buffer, base + offset));
            }
        }
        Ok((draws, models))
    }
}
