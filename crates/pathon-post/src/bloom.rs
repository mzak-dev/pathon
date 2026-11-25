use pathon_api::graph::{GraphBuilder, GraphTextureDesc, PassDesc, Stage, TextureHandle};
use pathon_api::resources::UniformSlice;
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, Caps, Color,
    ColorAttachment, Format, FormatCaps, OverrideValue, RenderPassDesc, RenderPipelineId, RhiError, SamplerDesc,
    SamplerId, ShaderModuleDesc, ShaderStages, TextureSampleType,
};
use pathon_api::{Negotiation, RecordCtx, RenderFeature, SetupCtx, Slot};

use crate::gpu::{
    COMMON_WGSL, FULLSCREEN_WGSL, PerRecordUniforms, SettingsGate, fullscreen_pipeline, joined_wgsl, sampler_entry,
    texture_entry, uniform_entry,
};
use crate::settings::BloomSettings;

/// The blurred bright part of the frame in exposed units, at half the render size. Written by the bloom feature,
/// added to the image by the tonemap pass.
pub const BLOOM: Slot = Slot::new("post.bloom", Format::Rgba16Float);

const BLOOM_WGSL: &str = include_str!("bloom.wgsl");
const BLOOM_FORMAT: Format = Format::Rgba16Float;
const SAMPLER_BINDING: u32 = 5;

const PREFILTER_PASS: &str = "post bloom prefilter";
const DOWN_PASSES: [&str; 8] = [
    "post bloom down 0",
    "post bloom down 1",
    "post bloom down 2",
    "post bloom down 3",
    "post bloom down 4",
    "post bloom down 5",
    "post bloom down 6",
    "post bloom down 7",
];
const UP_PASSES: [&str; 8] = [
    "post bloom up 0",
    "post bloom up 1",
    "post bloom up 2",
    "post bloom up 3",
    "post bloom up 4",
    "post bloom up 5",
    "post bloom up 6",
    "post bloom up 7",
];
const DOWN_TEXTURES: [&str; 8] = [
    "bloom down 0",
    "bloom down 1",
    "bloom down 2",
    "bloom down 3",
    "bloom down 4",
    "bloom down 5",
    "bloom down 6",
    "bloom down 7",
];
const UP_TEXTURES: [&str; 8] = [
    "bloom up 0",
    "bloom up 1",
    "bloom up 2",
    "bloom up 3",
    "bloom up 4",
    "bloom up 5",
    "bloom up 6",
    "bloom up 7",
];

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct BloomParams {
    threshold: f32,
    knee: f32,
    scatter: f32,
    padding: f32,
}

#[derive(Clone, Copy, Debug)]
struct BloomGpu {
    prefilter_layout: BindGroupLayoutId,
    down_layout: BindGroupLayoutId,
    up_layout: BindGroupLayoutId,
    prefilter_karis: RenderPipelineId,
    prefilter_plain: RenderPipelineId,
    down: RenderPipelineId,
    up: RenderPipelineId,
    sampler: SamplerId,
}

struct FilterPass {
    name: &'static str,
    pipeline: RenderPipelineId,
    layout: BindGroupLayoutId,
    uniform: UniformSlice,
    inputs: Vec<(u32, TextureHandle)>,
    sampler: Option<SamplerId>,
    target: TextureHandle,
}

fn add_filter_pass(graph: &mut GraphBuilder<'_>, spec: &FilterPass) -> TextureHandle {
    let mut pass = graph.add_pass(PassDesc::new(spec.name, Stage::PostProcess));
    let inputs: Vec<(u32, TextureHandle)> = spec
        .inputs
        .iter()
        .map(|(binding, handle)| (*binding, pass.read(*handle, Access::SAMPLED)))
        .collect();
    let target = pass.write(spec.target, Access::COLOR_ATTACHMENT);
    let FilterPass {
        name,
        pipeline,
        layout,
        uniform,
        sampler,
        ..
    } = *spec;
    pass.record(move |pass| {
        let mut entries = vec![BindGroupEntry {
            binding: 0,
            resource: uniform.binding(),
        }];
        for (binding, handle) in &inputs {
            entries.push(BindGroupEntry {
                binding: *binding,
                resource: BindingResource::TextureView(pass.view(*handle)?),
            });
        }
        if let Some(sampler) = sampler {
            entries.push(BindGroupEntry {
                binding: SAMPLER_BINDING,
                resource: BindingResource::Sampler(sampler),
            });
        }
        let target_view = pass.view(target)?;
        let group = pass.create_bind_group(&BindGroupDesc {
            label: name,
            layout,
            entries,
        })?;
        let mut render = pass.commands().render_pass(RenderPassDesc {
            label: name,
            color: vec![ColorAttachment::clear(target_view, Color::BLACK)],
            depth: None,
            timestamps: None,
        });
        render.set_pipeline(pipeline);
        render.set_bind_group(0, group);
        render.draw(0..3, 0..1);
        Ok(())
    });
    target
}

fn level_size(base: (u32, u32), level: usize) -> (u32, u32) {
    let shift = u32::try_from(level).unwrap_or(u32::MAX).min(31);
    (base.0.div_ceil(1 << shift).max(1), base.1.div_ceil(1 << shift).max(1))
}

/// Levels that still have at least one texel, up to what the settings ask for.
fn level_count(base: (u32, u32), requested: u32) -> usize {
    let fitting = base.0.min(base.1).max(1).ilog2() + 1;
    requested.min(fitting).max(1) as usize
}

/// Downsample/upsample pyramid of the exposed bright parts of `Slot::HDR`, published as [`BLOOM`].
#[derive(Debug)]
pub struct BloomFeature {
    gpu: Option<BloomGpu>,
    uniforms: Option<PerRecordUniforms>,
    settings: SettingsGate,
}

impl Default for BloomFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl BloomFeature {
    pub fn new() -> Self {
        Self {
            gpu: None,
            uniforms: None,
            settings: SettingsGate::new(false),
        }
    }
}

impl RenderFeature for BloomFeature {
    fn name(&self) -> &'static str {
        "post bloom"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn after(&self) -> &[&'static str] {
        &["post exposure"]
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let usable = FormatCaps::SAMPLED | FormatCaps::FILTERABLE | FormatCaps::RENDER_TARGET;
        if caps.format(BLOOM_FORMAT).contains(usable) {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "bloom needs a filterable Rgba16Float render target".to_owned(),
            }
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let wgsl = joined_wgsl(&[COMMON_WGSL, FULLSCREEN_WGSL, BLOOM_WGSL]);
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "post bloom",
            wgsl: &wgsl,
        })?;
        let layout =
            |label: &'static str, entries| device.create_bind_group_layout(&BindGroupLayoutDesc { label, entries });
        let prefilter_layout = layout(
            "post bloom prefilter",
            vec![
                uniform_entry(0, ShaderStages::FRAGMENT),
                texture_entry(1, ShaderStages::FRAGMENT, TextureSampleType::Float),
                texture_entry(2, ShaderStages::FRAGMENT, TextureSampleType::UnfilterableFloat),
            ],
        )?;
        let down_layout = layout(
            "post bloom down",
            vec![
                uniform_entry(0, ShaderStages::FRAGMENT),
                texture_entry(3, ShaderStages::FRAGMENT, TextureSampleType::Float),
                sampler_entry(SAMPLER_BINDING, ShaderStages::FRAGMENT),
            ],
        )?;
        let up_layout = layout(
            "post bloom up",
            vec![
                uniform_entry(0, ShaderStages::FRAGMENT),
                texture_entry(3, ShaderStages::FRAGMENT, TextureSampleType::Float),
                texture_entry(4, ShaderStages::FRAGMENT, TextureSampleType::Float),
                sampler_entry(SAMPLER_BINDING, ShaderStages::FRAGMENT),
            ],
        )?;
        let karis = |enabled: bool| vec![("KARIS", OverrideValue::from(enabled))];
        self.gpu = Some(BloomGpu {
            prefilter_layout,
            down_layout,
            up_layout,
            prefilter_karis: fullscreen_pipeline(
                device,
                "post bloom prefilter",
                prefilter_layout,
                module,
                "fs_prefilter",
                karis(true),
                BLOOM_FORMAT,
            )?,
            prefilter_plain: fullscreen_pipeline(
                device,
                "post bloom prefilter plain",
                prefilter_layout,
                module,
                "fs_prefilter",
                karis(false),
                BLOOM_FORMAT,
            )?,
            down: fullscreen_pipeline(
                device,
                "post bloom down",
                down_layout,
                module,
                "fs_down",
                vec![],
                BLOOM_FORMAT,
            )?,
            up: fullscreen_pipeline(
                device,
                "post bloom up",
                up_layout,
                module,
                "fs_up",
                vec![],
                BLOOM_FORMAT,
            )?,
            sampler: device.create_sampler(&SamplerDesc::LINEAR_CLAMP)?,
        });
        self.uniforms = Some(PerRecordUniforms::new(device, "post bloom uniforms")?);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let (Some(hdr), Some(exposure)) = (ctx.slot(Slot::HDR), ctx.slot(Slot::EXPOSURE)) else {
            return Ok(());
        };
        let gpu = self.gpu.ok_or(RhiError::Unsupported("bloom used before setup"))?;
        let BloomSettings {
            enabled,
            threshold,
            knee,
            scatter,
            levels,
            karis_average,
            ..
        } = self.settings.resolve(ctx).bloom;
        if !enabled {
            return Ok(());
        }
        let view = ctx.view();
        let record_epoch = view.record_epoch;
        let base = (
            view.render_size.width.div_ceil(2).max(1),
            view.render_size.height.div_ceil(2).max(1),
        );
        let levels = level_count(base, levels);
        let params = BloomParams {
            threshold,
            knee,
            scatter,
            padding: 0.0,
        };
        let uniform = self
            .uniforms
            .as_mut()
            .ok_or(RhiError::Unsupported("bloom used before setup"))?
            .write(ctx.device(), record_epoch, bytemuck::bytes_of(&params))?;

        let graph = ctx.graph();
        let texture = |graph: &mut GraphBuilder<'_>, names: [&'static str; 8], level: usize| {
            let (width, height) = level_size(base, level);
            graph.create_texture(names[level], GraphTextureDesc::fixed(width, height, BLOOM_FORMAT))
        };
        let first = texture(graph, DOWN_TEXTURES, 0);
        let mut down = vec![add_filter_pass(
            graph,
            &FilterPass {
                name: PREFILTER_PASS,
                pipeline: if karis_average {
                    gpu.prefilter_karis
                } else {
                    gpu.prefilter_plain
                },
                layout: gpu.prefilter_layout,
                uniform,
                inputs: vec![(1, hdr), (2, exposure)],
                sampler: None,
                target: first,
            },
        )];
        for level in 1..levels {
            let target = texture(graph, DOWN_TEXTURES, level);
            let previous = down[level - 1];
            down.push(add_filter_pass(
                graph,
                &FilterPass {
                    name: DOWN_PASSES[level],
                    pipeline: gpu.down,
                    layout: gpu.down_layout,
                    uniform,
                    inputs: vec![(3, previous)],
                    sampler: Some(gpu.sampler),
                    target,
                },
            ));
        }
        let mut result = down[levels - 1];
        for level in (0..levels - 1).rev() {
            let target = texture(graph, UP_TEXTURES, level);
            result = add_filter_pass(
                graph,
                &FilterPass {
                    name: UP_PASSES[level],
                    pipeline: gpu.up,
                    layout: gpu.up_layout,
                    uniform,
                    inputs: vec![(3, result), (4, down[level])],
                    sampler: Some(gpu.sampler),
                    target,
                },
            );
        }
        ctx.set_slot(BLOOM, result);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_halve_until_one_texel() {
        assert_eq!(level_size((32, 20), 0), (32, 20));
        assert_eq!(level_size((32, 20), 2), (8, 5));
        assert_eq!(level_size((32, 20), 5), (1, 1));
        assert_eq!(level_size((33, 1), 1), (17, 1));
    }

    #[test]
    fn level_count_never_asks_for_a_level_without_texels() {
        assert_eq!(level_count((32, 32), 6), 6);
        assert_eq!(level_count((32, 32), 8), 6);
        assert_eq!(level_count((1, 1), 6), 1);
        assert_eq!(level_count((64, 2), 6), 2);
        assert_eq!(level_count((64, 64), 3), 3);
    }
}
