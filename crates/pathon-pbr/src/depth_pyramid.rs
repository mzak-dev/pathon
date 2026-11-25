use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, BindingType,
    Caps, ComputePassDesc, ComputePipelineId, Format, FormatCaps, RhiError, SubresourceRange, TextureSampleType,
    TextureViewDesc,
};
use pathon_api::{Diagnostic, Negotiation, RecordCtx, RenderFeature, Slot};
use pathon_bindless::TextureBinder;

use crate::context::PbrContext;
use crate::prepass::with_context;

const GROUP_SIZE: u32 = 8;

/// `Params` of `depth_pyramid.wesl`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PyramidParams {
    source_size: [u32; 2],
    target_size: [u32; 2],
}

/// Number of mips of the pyramid of a `width` by `height` depth image: down to one texel.
pub fn pyramid_levels(width: u32, height: u32) -> u32 {
    width.max(height).max(1).ilog2() + 1
}

fn mip_size(size: u32, level: u32) -> u32 {
    (size >> level).max(1)
}

// R32Float is not filterable: reflection reports a `Float` sample type for `texture_2d<f32>`.
fn unfilterable_previous(desc: &mut BindGroupLayoutDesc) {
    for entry in &mut desc.entries {
        if let BindingType::SampledTexture { sample_type, .. } = &mut entry.ty
            && *sample_type == TextureSampleType::Float
        {
            *sample_type = TextureSampleType::UnfilterableFloat;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PyramidPipelines {
    copy: (ComputePipelineId, BindGroupLayoutId),
    downsample: (ComputePipelineId, BindGroupLayoutId),
}

/// Builds the `depth_pyramid` slot (Hi-Z: the minimum, so the farthest, reverse-Z depth of each block) in compute when
/// another pack requests it.
#[derive(Debug, Default)]
pub struct DepthPyramidFeature {
    pipelines: Option<PyramidPipelines>,
    reported_unsupported: bool,
}

impl DepthPyramidFeature {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RenderFeature for DepthPyramidFeature {
    fn name(&self) -> &'static str {
        "pbr depth pyramid"
    }

    fn stage(&self) -> Stage {
        Stage::Prepass
    }

    fn after(&self) -> &[&'static str] {
        &["pbr prepass"]
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        if caps.format(Format::R32Float).contains(FormatCaps::STORAGE | FormatCaps::SAMPLED) {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "L0006: R32Float cannot be a storage texture on this device".to_owned(),
            }
        }
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        if !ctx.view().is_requested(Slot::DEPTH_PYRAMID) {
            return Ok(());
        }
        let Some(depth) = ctx.slot(Slot::DEPTH) else {
            if !std::mem::replace(&mut self.reported_unsupported, true) {
                ctx.report(Diagnostic::warning("L0004", "depth_pyramid needs the depth slot of the PBR prepass"));
            }
            return Ok(());
        };
        let pipelines = match self.pipelines {
            Some(pipelines) => pipelines,
            None => {
                let built = with_context(ctx, |ctx, pbr, binder| build_pipelines(ctx, pbr, binder.as_deref()))?;
                self.pipelines = Some(built);
                built
            }
        };
        let view = ctx.view().clone();
        let levels = pyramid_levels(view.render_size.width, view.render_size.height);
        let mut pyramid = ctx.graph().create_texture(
            "depth pyramid",
            GraphTextureDesc::render(Format::R32Float).with_mips(levels),
        );
        let mut retired = Vec::new();
        for level in 0..levels {
            let params = PyramidParams {
                source_size: [
                    mip_size(view.render_size.width, level.saturating_sub(1)),
                    mip_size(view.render_size.height, level.saturating_sub(1)),
                ],
                target_size: [mip_size(view.render_size.width, level), mip_size(view.render_size.height, level)],
            };
            let slice = with_context(ctx, |ctx, pbr, _| {
                let (slice, old) = pbr.arena.write(ctx.device(), view.record_epoch, bytemuck::bytes_of(&params))?;
                retired.extend(old);
                Ok::<_, RhiError>(slice)
            })?;
            let mut pass = ctx.graph().add_pass(PassDesc::new("pbr depth pyramid", Stage::Prepass));
            let source = if level == 0 {
                pass.read(depth, Access::SAMPLED)
            } else {
                pass.read_range(pyramid, Access::SAMPLED, SubresourceRange::mip(level - 1))
            };
            pyramid = pass.write_range(pyramid, Access::STORAGE_WRITE, SubresourceRange::mip(level));
            let written = pyramid;
            let (pipeline, layout) = if level == 0 { pipelines.copy } else { pipelines.downsample };
            let groups = (params.target_size[0].div_ceil(GROUP_SIZE), params.target_size[1].div_ceil(GROUP_SIZE));
            pass.record(move |pass| {
                let source_view = if level == 0 {
                    pass.view(source)?
                } else {
                    pass.view_with(source, &TextureViewDesc::single_mip(level - 1))?
                };
                let target_view = pass.view_with(written, &TextureViewDesc::single_mip(level))?;
                let source_binding = if level == 0 { 1 } else { 2 };
                let group = pass.create_bind_group(&BindGroupDesc {
                    label: "pbr depth pyramid",
                    layout,
                    entries: vec![
                        BindGroupEntry {
                            binding: 0,
                            resource: slice.binding(),
                        },
                        BindGroupEntry {
                            binding: source_binding,
                            resource: BindingResource::TextureView(source_view),
                        },
                        BindGroupEntry {
                            binding: 3,
                            resource: BindingResource::TextureView(target_view),
                        },
                    ],
                })?;
                let mut compute = pass.commands().compute_pass(ComputePassDesc {
                    label: "pbr depth pyramid",
                    timestamps: None,
                });
                compute.set_pipeline(pipeline);
                compute.set_bind_group(0, group);
                compute.dispatch(groups.0, groups.1, 1);
                Ok(())
            });
        }
        for old in retired {
            ctx.release_later(old);
        }
        ctx.set_slot(Slot::DEPTH_PYRAMID, pyramid);
        Ok(())
    }
}

fn build_pipelines(
    ctx: &mut RecordCtx<'_>,
    pbr: &mut PbrContext,
    binder: Option<&TextureBinder>,
) -> Result<PyramidPipelines, RhiError> {
    let hooks = ctx.shared().get::<pathon_material::MaterialHooks>().cloned();
    let device = ctx.device();
    let shaders = pbr
        .shaders(hooks.as_ref(), binder)
        .map_err(|error| RhiError::validation("L0006", error.to_string()))?;
    Ok(PyramidPipelines {
        copy: shaders
            .compute_pipeline(device, "depth_pyramid", "copy_depth", "pbr depth copy", |_| {})?,
        downsample: shaders
            .compute_pipeline(device, "depth_pyramid", "downsample", "pbr depth downsample", unfilterable_previous)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_counts_end_at_one_texel() {
        assert_eq!(pyramid_levels(1, 1), 1);
        assert_eq!(pyramid_levels(256, 256), 9);
        assert_eq!(pyramid_levels(1920, 1080), 11);
        assert_eq!(pyramid_levels(5, 3), 3);
        assert_eq!(mip_size(5, 1), 2);
        assert_eq!(mip_size(1, 3), 1);
    }
}
