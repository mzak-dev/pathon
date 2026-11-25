use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage, TextureHandle};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, Caps, Color,
    ColorAttachment, ColorTargetState, ComputePassDesc, ComputePipelineDesc, ComputePipelineId, DeviceApi, Format,
    FormatCaps, OverrideValue, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError,
    SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages, TextureSampleType,
};
use pathon_api::{Negotiation, RecordCtx, RenderFeature, SetupCtx, Slot};

use crate::SkyQuality;
use crate::frame::{SkyFrame, resolve_settings};
use crate::gpu::{
    LUT_FORMAT, PerRecordUniforms, lut_output_entry, multiscatter_wgsl, sampler_entry, sky_view_wgsl, sky_wgsl,
    texture_entry, transmittance_wgsl, uniform_entry,
};
use crate::uniforms::SkyUniform;

/// Transmittance of the atmosphere to space, `(zenith cosine, radius)` parametrised; written every frame.
pub const TRANSMITTANCE_LUT: Slot = Slot::new("sky.transmittance", LUT_FORMAT);
/// Multiple-scattering luminance per unit sun illuminance.
pub const MULTISCATTER_LUT: Slot = Slot::new("sky.multiscatter", LUT_FORMAT);
/// Sky luminance per unit sun illuminance in every direction of the hemisphere sphere, seen from the camera.
pub const SKY_VIEW_LUT: Slot = Slot::new("sky.sky_view", LUT_FORMAT);

const WORKGROUP: u32 = 8;

#[derive(Clone, Copy, Debug)]
struct ComputeStage {
    layout: BindGroupLayoutId,
    pipeline: ComputePipelineId,
}

#[derive(Clone, Copy, Debug)]
struct RenderStage {
    layout: BindGroupLayoutId,
    pipeline: RenderPipelineId,
}

#[derive(Clone, Copy, Debug)]
struct SkyGpu {
    transmittance: ComputeStage,
    multiscatter: ComputeStage,
    sky_view: ComputeStage,
    with_depth: RenderStage,
    without_depth: RenderStage,
    sampler: SamplerId,
}

fn compute_stage(
    device: &dyn DeviceApi,
    label: &'static str,
    wgsl: &str,
    entries: Vec<pathon_api::rhi::BindGroupLayoutEntry>,
    steps: u32,
) -> Result<ComputeStage, RhiError> {
    let module = device.create_shader_module(&ShaderModuleDesc { label, wgsl })?;
    let layout = device.create_bind_group_layout(&BindGroupLayoutDesc { label, entries })?;
    let pipeline = device.create_compute_pipeline(&ComputePipelineDesc {
        label,
        bind_group_layouts: vec![layout],
        shader: ShaderStage {
            module,
            entry_point: "main",
            constants: vec![("STEPS", OverrideValue::from(steps))],
        },
        immediate_size: 0,
    })?;
    Ok(ComputeStage { layout, pipeline })
}

fn render_stage(device: &dyn DeviceApi, with_depth: bool) -> Result<RenderStage, RhiError> {
    let label = if with_depth {
        "sky render"
    } else {
        "sky render (no depth)"
    };
    let wgsl = sky_wgsl(with_depth);
    let module: ShaderModuleId = device.create_shader_module(&ShaderModuleDesc { label, wgsl: &wgsl })?;
    let visibility = ShaderStages::FRAGMENT;
    let mut entries = vec![
        uniform_entry(visibility),
        texture_entry(1, visibility, TextureSampleType::Float),
        sampler_entry(visibility),
        texture_entry(3, visibility, TextureSampleType::Float),
    ];
    if with_depth {
        entries.push(texture_entry(4, visibility, TextureSampleType::Depth));
    }
    let layout = device.create_bind_group_layout(&BindGroupLayoutDesc { label, entries })?;
    let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
        label,
        bind_group_layouts: vec![layout],
        vertex: ShaderStage {
            module,
            entry_point: "vs_fullscreen",
            constants: vec![],
        },
        vertex_buffers: vec![],
        fragment: Some(ShaderStage {
            module,
            entry_point: "fs_sky",
            constants: vec![],
        }),
        color_targets: vec![ColorTargetState::opaque(Format::Rgba16Float)],
        primitive: PrimitiveState::default(),
        depth: None,
        sample_count: 1,
        immediate_size: 0,
    })?;
    Ok(RenderStage { layout, pipeline })
}

fn entry(binding: u32, resource: BindingResource) -> BindGroupEntry {
    BindGroupEntry { binding, resource }
}

/// Draws the atmosphere into `Slot::HDR` where the depth slot is empty (at the far plane): after the opaque geometry,
/// before transparents. The tables are recomputed every frame (ADR-0045).
#[derive(Debug)]
pub struct SkyFeature {
    quality: SkyQuality,
    gpu: Option<SkyGpu>,
    uniforms: Option<PerRecordUniforms>,
}

impl SkyFeature {
    pub fn new(quality: SkyQuality) -> Self {
        Self {
            quality,
            gpu: None,
            uniforms: None,
        }
    }

    fn dispatch_size(width: u32, height: u32) -> (u32, u32) {
        (width.div_ceil(WORKGROUP), height.div_ceil(WORKGROUP))
    }
}

impl RenderFeature for SkyFeature {
    fn name(&self) -> &'static str {
        "sky"
    }

    fn stage(&self) -> Stage {
        Stage::Sky
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let needed = FormatCaps::SAMPLED | FormatCaps::FILTERABLE | FormatCaps::STORAGE;
        if !caps.format(LUT_FORMAT).contains(needed) {
            return Negotiation::Disabled {
                reason: "the sky tables are written by compute into a filterable Rgba16Float".to_owned(),
            };
        }
        if caps.limits.max_compute_invocations_per_workgroup < WORKGROUP * WORKGROUP {
            return Negotiation::Disabled {
                reason: "the sky tables need compute workgroups of 64 invocations".to_owned(),
            };
        }
        Negotiation::Full
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let compute = ShaderStages::COMPUTE;
        let quality = self.quality;
        let lookups = || {
            vec![
                uniform_entry(compute),
                texture_entry(1, compute, TextureSampleType::Float),
                sampler_entry(compute),
            ]
        };
        let transmittance = compute_stage(
            device,
            "sky transmittance",
            &transmittance_wgsl(),
            vec![uniform_entry(compute), lut_output_entry()],
            quality.transmittance_steps,
        )?;
        let mut entries = lookups();
        entries.push(lut_output_entry());
        let multiscatter = compute_stage(
            device,
            "sky multiscatter",
            &multiscatter_wgsl(),
            entries,
            quality.multiscatter_steps,
        )?;
        let mut entries = lookups();
        entries.extend([texture_entry(3, compute, TextureSampleType::Float), lut_output_entry()]);
        let sky_view = compute_stage(device, "sky view", &sky_view_wgsl(), entries, quality.sky_view_steps)?;
        self.gpu = Some(SkyGpu {
            transmittance,
            multiscatter,
            sky_view,
            with_depth: render_stage(device, true)?,
            without_depth: render_stage(device, false)?,
            sampler: device.create_sampler(&SamplerDesc::LINEAR_CLAMP)?,
        });
        self.uniforms = Some(PerRecordUniforms::new(device, "sky uniforms")?);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let gpu = self.gpu.ok_or(RhiError::Unsupported("sky used before setup"))?;
        let (settings, _) = resolve_settings(ctx.shared());
        let view = ctx.view();
        let frame = SkyFrame::new(&settings, &view.camera);
        let render_size = (view.render_size.width, view.render_size.height);
        let record_epoch = view.record_epoch;
        let inverse_view_projection = view.camera.view_projection().inverse();
        let uniform = SkyUniform::new(&frame, &self.quality, inverse_view_projection, render_size);
        let uniform = self
            .uniforms
            .as_mut()
            .ok_or(RhiError::Unsupported("sky used before setup"))?
            .write(ctx.device(), record_epoch, bytemuck::bytes_of(&uniform))?;

        let quality = self.quality;
        let table = |ctx: &mut RecordCtx<'_>, name, (width, height)| {
            ctx.graph()
                .create_texture(name, GraphTextureDesc::fixed(width, height, LUT_FORMAT))
        };
        let transmittance = table(ctx, "sky transmittance", quality.transmittance_size);
        let multiscatter = table(
            ctx,
            "sky multiscatter",
            (quality.multiscatter_size, quality.multiscatter_size),
        );
        let sky_view = table(ctx, "sky view", quality.sky_view_size);

        let transmittance = {
            let mut pass = ctx.graph().add_pass(PassDesc::new("sky transmittance", Stage::Sky));
            let target = pass.write(transmittance, Access::STORAGE_WRITE);
            let (groups_x, groups_y) = Self::dispatch_size(quality.transmittance_size.0, quality.transmittance_size.1);
            pass.record(move |pass| {
                let target_view = pass.view(target)?;
                let group = pass.create_bind_group(&BindGroupDesc {
                    label: "sky transmittance",
                    layout: gpu.transmittance.layout,
                    entries: vec![
                        entry(0, uniform.binding()),
                        entry(4, BindingResource::TextureView(target_view)),
                    ],
                })?;
                let mut compute = pass.commands().compute_pass(ComputePassDesc {
                    label: "sky transmittance",
                    timestamps: None,
                });
                compute.set_pipeline(gpu.transmittance.pipeline);
                compute.set_bind_group(0, group);
                compute.dispatch(groups_x, groups_y, 1);
                Ok(())
            });
            target
        };
        let multiscatter = {
            let mut pass = ctx.graph().add_pass(PassDesc::new("sky multiscatter", Stage::Sky));
            let source = pass.read(transmittance, Access::SAMPLED);
            let target = pass.write(multiscatter, Access::STORAGE_WRITE);
            let groups = quality.multiscatter_size.div_ceil(WORKGROUP);
            pass.record(move |pass| {
                let (source_view, target_view) = (pass.view(source)?, pass.view(target)?);
                let group = pass.create_bind_group(&BindGroupDesc {
                    label: "sky multiscatter",
                    layout: gpu.multiscatter.layout,
                    entries: vec![
                        entry(0, uniform.binding()),
                        entry(1, BindingResource::TextureView(source_view)),
                        entry(2, BindingResource::Sampler(gpu.sampler)),
                        entry(4, BindingResource::TextureView(target_view)),
                    ],
                })?;
                let mut compute = pass.commands().compute_pass(ComputePassDesc {
                    label: "sky multiscatter",
                    timestamps: None,
                });
                compute.set_pipeline(gpu.multiscatter.pipeline);
                compute.set_bind_group(0, group);
                compute.dispatch(groups, groups, 1);
                Ok(())
            });
            target
        };
        let sky_view = {
            let mut pass = ctx.graph().add_pass(PassDesc::new("sky view", Stage::Sky));
            let transmittance_in = pass.read(transmittance, Access::SAMPLED);
            let multiscatter_in = pass.read(multiscatter, Access::SAMPLED);
            let target = pass.write(sky_view, Access::STORAGE_WRITE);
            let (groups_x, groups_y) = Self::dispatch_size(quality.sky_view_size.0, quality.sky_view_size.1);
            pass.record(move |pass| {
                let transmittance_view = pass.view(transmittance_in)?;
                let multiscatter_view = pass.view(multiscatter_in)?;
                let target_view = pass.view(target)?;
                let group = pass.create_bind_group(&BindGroupDesc {
                    label: "sky view",
                    layout: gpu.sky_view.layout,
                    entries: vec![
                        entry(0, uniform.binding()),
                        entry(1, BindingResource::TextureView(transmittance_view)),
                        entry(2, BindingResource::Sampler(gpu.sampler)),
                        entry(3, BindingResource::TextureView(multiscatter_view)),
                        entry(4, BindingResource::TextureView(target_view)),
                    ],
                })?;
                let mut compute = pass.commands().compute_pass(ComputePassDesc {
                    label: "sky view",
                    timestamps: None,
                });
                compute.set_pipeline(gpu.sky_view.pipeline);
                compute.set_bind_group(0, group);
                compute.dispatch(groups_x, groups_y, 1);
                Ok(())
            });
            target
        };

        let hdr = draw(ctx, gpu, uniform, transmittance, sky_view);
        ctx.set_slot(TRANSMITTANCE_LUT, transmittance);
        ctx.set_slot(MULTISCATTER_LUT, multiscatter);
        ctx.set_slot(SKY_VIEW_LUT, sky_view);
        ctx.set_slot(Slot::HDR, hdr);
        Ok(())
    }
}

// Fills `hdr` where nothing was drawn: loads the colour of the opaque pass, or starts one when there is none.
fn draw(
    ctx: &mut RecordCtx<'_>,
    gpu: SkyGpu,
    uniform: pathon_api::resources::UniformSlice,
    transmittance: TextureHandle,
    sky_view: TextureHandle,
) -> TextureHandle {
    let existing = ctx.slot(Slot::HDR);
    let hdr = existing.unwrap_or_else(|| {
        ctx.graph()
            .create_texture("sky hdr", GraphTextureDesc::render(Format::Rgba16Float))
    });
    let depth = ctx.slot(Slot::DEPTH);
    let stage = if depth.is_some() {
        gpu.with_depth
    } else {
        gpu.without_depth
    };

    let mut pass = ctx.graph().add_pass(PassDesc::new("sky render", Stage::Sky));
    let transmittance = pass.read(transmittance, Access::SAMPLED);
    let sky_view = pass.read(sky_view, Access::SAMPLED);
    let depth = depth.map(|depth| pass.read(depth, Access::SAMPLED | Access::DEPTH_READ));
    let loads = existing.is_some();
    let target = if loads {
        pass.modify(hdr, Access::COLOR_ATTACHMENT)
    } else {
        pass.write(hdr, Access::COLOR_ATTACHMENT)
    };
    pass.record(move |pass| {
        let transmittance_view = pass.view(transmittance)?;
        let sky_view_view = pass.view(sky_view)?;
        let target_view = pass.view(target)?;
        let mut entries = vec![
            entry(0, uniform.binding()),
            entry(1, BindingResource::TextureView(transmittance_view)),
            entry(2, BindingResource::Sampler(gpu.sampler)),
            entry(3, BindingResource::TextureView(sky_view_view)),
        ];
        if let Some(depth) = depth {
            entries.push(entry(4, BindingResource::TextureView(pass.view(depth)?)));
        }
        let group = pass.create_bind_group(&BindGroupDesc {
            label: "sky render",
            layout: stage.layout,
            entries,
        })?;
        let color = if loads {
            ColorAttachment::load(target_view)
        } else {
            ColorAttachment::clear(target_view, Color::BLACK)
        };
        let mut render = pass.commands().render_pass(RenderPassDesc {
            label: "sky render",
            color: vec![color],
            depth: None,
            timestamps: None,
        });
        render.set_pipeline(stage.pipeline);
        render.set_bind_group(0, group);
        render.draw(0..3, 0..1);
        Ok(())
    });
    target
}
