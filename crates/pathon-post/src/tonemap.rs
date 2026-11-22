use std::collections::HashMap;

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, Caps, Color,
    ColorAttachment, Format, FormatCaps, OverrideValue, RenderPassDesc, RenderPipelineId, RhiError, SamplerDesc,
    SamplerId, ShaderModuleDesc, ShaderModuleId, ShaderStages, TextureSampleType,
};
use pathon_api::{Diagnostic, Negotiation, RecordCtx, RenderFeature, Role, SetupCtx, Slot};

use crate::bloom::BLOOM;
use crate::gpu::{
    COMMON_WGSL, FULLSCREEN_WGSL, PerRecordUniforms, SettingsGate, fullscreen_pipeline, joined_wgsl, needs_manual_oetf,
    sampler_entry, texture_entry, uniform_entry,
};
use crate::settings::Tonemapper;

const TONEMAP_WGSL: &str = include_str!("tonemap.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct TonemapParams {
    output_size: [f32; 2],
    bloom_intensity: f32,
    padding: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct PipelineKey {
    target: Format,
    operator: Tonemapper,
    has_bloom: bool,
}

#[derive(Clone, Copy, Debug)]
struct TonemapGpu {
    module: ShaderModuleId,
    layout: BindGroupLayoutId,
    sampler: SamplerId,
}

/// Last pass of the chain: exposure, optional bloom, tone curve and the output transfer, written to
/// `Slot::OUTPUT`. Holds `Role::TONEMAP`.
#[derive(Debug)]
pub struct TonemapFeature {
    gpu: Option<TonemapGpu>,
    pipelines: HashMap<PipelineKey, RenderPipelineId>,
    uniforms: Option<PerRecordUniforms>,
    settings: SettingsGate,
    reported_missing_exposure: bool,
    reported_unimplemented_transfer: bool,
}

impl Default for TonemapFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl TonemapFeature {
    pub fn new() -> Self {
        Self {
            gpu: None,
            pipelines: HashMap::new(),
            uniforms: None,
            settings: SettingsGate::new(false),
            reported_missing_exposure: false,
            reported_unimplemented_transfer: false,
        }
    }

    fn pipeline(
        &mut self,
        ctx: &RecordCtx<'_>,
        gpu: TonemapGpu,
        key: PipelineKey,
    ) -> Result<RenderPipelineId, RhiError> {
        if let Some(pipeline) = self.pipelines.get(&key) {
            return Ok(*pipeline);
        }
        let constants = vec![
            ("OPERATOR", OverrideValue::from(key.operator.shader_constant())),
            ("ENCODE_SRGB", OverrideValue::from(needs_manual_oetf(key.target))),
            ("HAS_BLOOM", OverrideValue::from(key.has_bloom)),
        ];
        let pipeline = fullscreen_pipeline(
            ctx.device(),
            "post tonemap",
            gpu.layout,
            gpu.module,
            "fs_tonemap",
            constants,
            key.target,
        )?;
        self.pipelines.insert(key, pipeline);
        Ok(pipeline)
    }
}

impl RenderFeature for TonemapFeature {
    fn name(&self) -> &'static str {
        "post tonemap"
    }

    fn stage(&self) -> Stage {
        Stage::Tonemap
    }

    fn role(&self) -> Option<Role> {
        Some(Role::TONEMAP)
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let usable = FormatCaps::SAMPLED | FormatCaps::FILTERABLE;
        if caps.format(Format::Rgba16Float).contains(usable) {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "tonemapping needs a filterable Rgba16Float hdr texture".to_owned(),
            }
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let wgsl = joined_wgsl(&[COMMON_WGSL, FULLSCREEN_WGSL, TONEMAP_WGSL]);
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "post tonemap",
            wgsl: &wgsl,
        })?;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "post tonemap",
            entries: vec![
                uniform_entry(0, ShaderStages::FRAGMENT),
                texture_entry(1, ShaderStages::FRAGMENT, TextureSampleType::Float),
                texture_entry(2, ShaderStages::FRAGMENT, TextureSampleType::Float),
                texture_entry(3, ShaderStages::FRAGMENT, TextureSampleType::UnfilterableFloat),
                sampler_entry(4, ShaderStages::FRAGMENT),
            ],
        })?;
        self.gpu = Some(TonemapGpu {
            module,
            layout,
            sampler: device.create_sampler(&SamplerDesc::LINEAR_CLAMP)?,
        });
        self.pipelines.clear();
        self.uniforms = Some(PerRecordUniforms::new(device, "post tonemap uniforms")?);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let (Some(hdr), Some(output)) = (ctx.slot(Slot::HDR), ctx.slot(Slot::OUTPUT)) else {
            return Ok(());
        };
        let Some(exposure) = ctx.slot(Slot::EXPOSURE) else {
            if !self.reported_missing_exposure {
                self.reported_missing_exposure = true;
                ctx.report(
                    Diagnostic::warning("Q0003", "tonemapping skipped: no 'exposure' slot this frame").with_hint(
                        "register ExposureFeature (PostPack does) or write Slot::EXPOSURE from your own pack",
                    ),
                );
            }
            return Ok(());
        };
        let gpu = self.gpu.ok_or(RhiError::Unsupported("tonemap used before setup"))?;
        let settings = self.settings.resolve(ctx);
        if settings.tonemap.output.is_implemented() {
            self.reported_unimplemented_transfer = false;
        } else if !self.reported_unimplemented_transfer {
            self.reported_unimplemented_transfer = true;
            ctx.report(
                Diagnostic::warning(
                    "Q0002",
                    format!(
                        "output transfer {} is not implemented, using sRGB",
                        settings.tonemap.output.name()
                    ),
                )
                .with_hint("HDR output arrives with M4-16; use OutputTransfer::Srgb"),
            );
        }
        let target = ctx
            .graph()
            .texture_format(output)
            .ok_or(RhiError::InvalidHandle("output"))?;
        let bloom = ctx.slot(BLOOM);
        let key = PipelineKey {
            target,
            operator: settings.tonemap.operator,
            has_bloom: bloom.is_some(),
        };
        let pipeline = self.pipeline(ctx, gpu, key)?;
        let view = ctx.view();
        let (output_size, record_epoch) = (view.output_size, view.record_epoch);
        let params = TonemapParams {
            output_size: [output_size.width as f32, output_size.height as f32],
            bloom_intensity: settings.bloom.intensity,
            padding: 0.0,
        };
        let uniform = self
            .uniforms
            .as_mut()
            .ok_or(RhiError::Unsupported("tonemap used before setup"))?
            .write(ctx.device(), record_epoch, bytemuck::bytes_of(&params))?;

        let mut pass = ctx.graph().add_pass(PassDesc::new("post tonemap", Stage::Tonemap));
        let hdr = pass.read(hdr, Access::SAMPLED);
        let exposure = pass.read(exposure, Access::SAMPLED);
        let bloom = bloom.map(|bloom| pass.read(bloom, Access::SAMPLED));
        let output = pass.write(output, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let hdr_view = pass.view(hdr)?;
            // Without bloom the binding still needs a texture; the shader never reads it.
            let bloom_view = match bloom {
                Some(bloom) => pass.view(bloom)?,
                None => hdr_view,
            };
            let exposure_view = pass.view(exposure)?;
            let output_view = pass.view(output)?;
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "post tonemap",
                layout: gpu.layout,
                entries: vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: uniform.binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::TextureView(hdr_view),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::TextureView(bloom_view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::TextureView(exposure_view),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::Sampler(gpu.sampler),
                    },
                ],
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "post tonemap",
                color: vec![ColorAttachment::clear(output_view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.set_bind_group(0, group);
            render.draw(0..3, 0..1);
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, output);
        Ok(())
    }
}
