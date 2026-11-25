use pathon_api::graph::{GraphBufferDesc, GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, BindingType,
    BufferUsage, Caps, ComputePassDesc, ComputePipelineDesc, ComputePipelineId, DeviceApi, Format, FormatCaps,
    RhiError, ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess, TextureSampleType, TextureViewDimension,
};
use pathon_api::{Negotiation, RecordCtx, RenderFeature, SetupCtx, Slot};

use crate::gpu::{COMMON_WGSL, PerRecordUniforms, SettingsGate, joined_wgsl, texture_entry, uniform_entry};
use crate::reference::{self, HISTOGRAM_BINS};
use crate::settings::{AutoExposure, ExposureMode, PostSettings};

const EXPOSURE_WGSL: &str = include_str!("exposure.wgsl");
const HISTOGRAM_WORKGROUP: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ExposureParams {
    min_log2: f32,
    log2_range: f32,
    low_fraction: f32,
    high_fraction: f32,
    key: f32,
    manual_multiplier: f32,
    brighter_coefficient: f32,
    darker_coefficient: f32,
    previous_valid: u32,
    padding: [u32; 3],
}

const _: () = assert!(size_of::<ExposureParams>() == 48);

#[derive(Clone, Copy, Debug)]
struct ExposureGpu {
    auto_layout: BindGroupLayoutId,
    manual_layout: BindGroupLayoutId,
    clear: ComputePipelineId,
    build: ComputePipelineId,
    reduce: ComputePipelineId,
    manual: ComputePipelineId,
}

/// Writes `Slot::EXPOSURE`: a manual multiplier, or one adapted over time to the luminance histogram of
/// `Slot::HDR`.
#[derive(Debug)]
pub struct ExposureFeature {
    gpu: Option<ExposureGpu>,
    uniforms: Option<PerRecordUniforms>,
    settings: SettingsGate,
}

impl Default for ExposureFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl ExposureFeature {
    pub fn new() -> Self {
        Self {
            gpu: None,
            uniforms: None,
            settings: SettingsGate::new(true),
        }
    }

    fn pipeline(
        device: &dyn DeviceApi,
        module: pathon_api::rhi::ShaderModuleId,
        layout: BindGroupLayoutId,
        label: &'static str,
        entry_point: &'static str,
    ) -> Result<ComputePipelineId, RhiError> {
        device.create_compute_pipeline(&ComputePipelineDesc {
            label,
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point,
                constants: vec![],
            },
            immediate_size: 0,
        })
    }

    fn record_auto(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        gpu: ExposureGpu,
        settings: &PostSettings,
        auto: &AutoExposure,
        hdr: pathon_api::graph::TextureHandle,
    ) -> Result<(), RhiError> {
        let view = ctx.view();
        let (size, delta_seconds, record_epoch) = (view.render_size, view.delta_seconds, view.record_epoch);
        let history = ctx
            .graph()
            .history_texture("post exposure", GraphTextureDesc::fixed(1, 1, Format::R32Float));
        let histogram = ctx.graph().create_buffer(
            "post exposure histogram",
            GraphBufferDesc::new(HISTOGRAM_BINS as u64 * 4, BufferUsage::STORAGE),
        );
        let params = ExposureParams {
            min_log2: auto.min_ev,
            log2_range: auto.max_ev - auto.min_ev,
            low_fraction: auto.low_percentile,
            high_fraction: auto.high_percentile,
            key: auto.target_luminance * settings.exposure.compensation_ev.exp2(),
            manual_multiplier: 1.0,
            brighter_coefficient: reference::adaptation_coefficient(delta_seconds, auto.to_brighter_seconds),
            darker_coefficient: reference::adaptation_coefficient(delta_seconds, auto.to_darker_seconds),
            previous_valid: u32::from(history.previous_valid),
            padding: [0; 3],
        };
        let uniform = self
            .uniforms
            .as_mut()
            .ok_or(RhiError::Unsupported("exposure used before setup"))?
            .write(ctx.device(), record_epoch, bytemuck::bytes_of(&params))?;

        let mut pass = ctx.graph().add_pass(PassDesc::new("post exposure", Stage::PostProcess));
        let hdr = pass.read(hdr, Access::SAMPLED);
        let previous = pass.read(history.previous, Access::SAMPLED);
        let histogram = pass.write_buffer(histogram, Access::STORAGE_WRITE);
        let current = pass.write(history.current, Access::STORAGE_WRITE);
        pass.record(move |pass| {
            let hdr_view = pass.view(hdr)?;
            let previous_view = pass.view(previous)?;
            let current_view = pass.view(current)?;
            let histogram_buffer = pass.buffer(histogram);
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "post exposure",
                layout: gpu.auto_layout,
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
                        resource: BindingResource::Buffer {
                            buffer: histogram_buffer,
                            offset: 0,
                            size: None,
                        },
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::TextureView(previous_view),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::TextureView(current_view),
                    },
                ],
            })?;
            let mut compute = pass.commands().compute_pass(ComputePassDesc {
                label: "post exposure",
                timestamps: None,
            });
            compute.set_pipeline(gpu.clear);
            compute.set_bind_group(0, group);
            compute.dispatch(1, 1, 1);
            compute.set_pipeline(gpu.build);
            compute.set_bind_group(0, group);
            compute.dispatch(
                size.width.div_ceil(HISTOGRAM_WORKGROUP),
                size.height.div_ceil(HISTOGRAM_WORKGROUP),
                1,
            );
            compute.set_pipeline(gpu.reduce);
            compute.set_bind_group(0, group);
            compute.dispatch(1, 1, 1);
            Ok(())
        });
        ctx.set_slot(Slot::EXPOSURE, current);
        Ok(())
    }

    fn record_manual(&mut self, ctx: &mut RecordCtx<'_>, gpu: ExposureGpu, multiplier: f32) -> Result<(), RhiError> {
        let record_epoch = ctx.view().record_epoch;
        let target = ctx
            .graph()
            .create_texture("post exposure", GraphTextureDesc::fixed(1, 1, Format::R32Float));
        let params = ExposureParams {
            min_log2: 0.0,
            log2_range: 1.0,
            low_fraction: 0.0,
            high_fraction: 1.0,
            key: 1.0,
            manual_multiplier: multiplier,
            brighter_coefficient: 1.0,
            darker_coefficient: 1.0,
            previous_valid: 0,
            padding: [0; 3],
        };
        let uniform = self
            .uniforms
            .as_mut()
            .ok_or(RhiError::Unsupported("exposure used before setup"))?
            .write(ctx.device(), record_epoch, bytemuck::bytes_of(&params))?;

        let mut pass = ctx.graph().add_pass(PassDesc::new("post exposure", Stage::PostProcess));
        let target = pass.write(target, Access::STORAGE_WRITE);
        pass.record(move |pass| {
            let target_view = pass.view(target)?;
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "post exposure manual",
                layout: gpu.manual_layout,
                entries: vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: uniform.binding(),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::TextureView(target_view),
                    },
                ],
            })?;
            let mut compute = pass.commands().compute_pass(ComputePassDesc {
                label: "post exposure manual",
                timestamps: None,
            });
            compute.set_pipeline(gpu.manual);
            compute.set_bind_group(0, group);
            compute.dispatch(1, 1, 1);
            Ok(())
        });
        ctx.set_slot(Slot::EXPOSURE, target);
        Ok(())
    }
}

impl RenderFeature for ExposureFeature {
    fn name(&self) -> &'static str {
        "post exposure"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        if caps.format(Format::R32Float).contains(FormatCaps::STORAGE)
            && caps.format(Format::Rgba16Float).contains(FormatCaps::SAMPLED)
        {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "exposure needs R32Float storage writes and sampled Rgba16Float".to_owned(),
            }
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let wgsl = joined_wgsl(&[COMMON_WGSL, EXPOSURE_WGSL]);
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "post exposure",
            wgsl: &wgsl,
        })?;
        let auto_layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "post exposure",
            entries: vec![
                uniform_entry(0, ShaderStages::COMPUTE),
                texture_entry(1, ShaderStages::COMPUTE, TextureSampleType::Float),
                pathon_api::rhi::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageBuffer {
                        access: StorageAccess::ReadWrite,
                    },
                    count: None,
                },
                texture_entry(3, ShaderStages::COMPUTE, TextureSampleType::UnfilterableFloat),
                exposure_output_entry(),
            ],
        })?;
        let manual_layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "post exposure manual",
            entries: vec![uniform_entry(0, ShaderStages::COMPUTE), exposure_output_entry()],
        })?;
        self.gpu = Some(ExposureGpu {
            auto_layout,
            manual_layout,
            clear: Self::pipeline(device, module, auto_layout, "post exposure clear", "clear")?,
            build: Self::pipeline(device, module, auto_layout, "post exposure histogram", "build")?,
            reduce: Self::pipeline(device, module, auto_layout, "post exposure reduce", "reduce")?,
            manual: Self::pipeline(device, module, manual_layout, "post exposure manual", "manual")?,
        });
        self.uniforms = Some(PerRecordUniforms::new(device, "post exposure uniforms")?);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(hdr) = ctx.slot(Slot::HDR) else {
            return Ok(());
        };
        let gpu = self.gpu.ok_or(RhiError::Unsupported("exposure used before setup"))?;
        let settings = self.settings.resolve(ctx);
        match &settings.exposure.mode {
            ExposureMode::Auto(auto) => self.record_auto(ctx, gpu, &settings, auto, hdr),
            ExposureMode::Manual { ev } => {
                let multiplier = reference::manual_multiplier(*ev, settings.exposure.compensation_ev);
                self.record_manual(ctx, gpu, multiplier)
            }
        }
    }
}

fn exposure_output_entry() -> pathon_api::rhi::BindGroupLayoutEntry {
    pathon_api::rhi::BindGroupLayoutEntry {
        binding: 4,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::StorageTexture {
            access: StorageAccess::WriteOnly,
            format: Format::R32Float,
            dimension: TextureViewDimension::D2,
        },
        count: None,
    }
}
