use std::collections::HashMap;

use pathon_api::glam::Mat4;
use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::resources::{UniformRing, UniformSlice};
use pathon_api::rhi::{
    Access, AddressMode, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, BufferDesc, BufferId, BufferUsage, Caps, CompareFunction, DepthAttachment, DepthBias,
    DepthState, DeviceApi, DrawIndexedIndirectArgs, Features, FilterMode, Format, FormatCaps, PrimitiveState,
    RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError, SamplerDesc, SamplerId, ShaderModuleDesc,
    ShaderStage, ShaderStages, StorageAccess, TextureViewDesc, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexStepMode,
};
use pathon_api::{Diagnostic, Negotiation, RecordCtx, RenderFeature, Requirements, Role, SetupCtx};

use crate::cascade::{CameraFrustum, LightBasis};
use crate::frame::{SHADOW_MAP, ShadowFrame, ShadowUniforms};
use crate::input::{DirectionalShadowLight, ShadowScene};
use crate::plan::{FramePlan, plan_frame};
use crate::settings::ShadowSettings;

const DEPTH_WGSL: &str = include_str!("../wgsl/depth.wgsl");
const DEPTH_FORMAT: Format = Format::Depth32Float;
const MIN_FRAME_BUFFER_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug)]
struct ShadowGpu {
    module: pathon_api::rhi::ShaderModuleId,
    layout: BindGroupLayoutId,
    sampler: SamplerId,
}

// A buffer the pack fills while it records: every record attempt starts at the front, every write goes behind the
// previous one (a second view of the same frame must not overwrite the first), and a buffer that is too small is
// replaced by a larger one, the old one living on for the writes already made to it.
#[derive(Debug)]
struct FrameBuffer {
    label: &'static str,
    usage: BufferUsage,
    alignment: u64,
    buffer: Option<BufferId>,
    capacity: u64,
    cursor: u64,
    epoch: Option<u64>,
}

impl FrameBuffer {
    fn new(label: &'static str, usage: BufferUsage, alignment: u64) -> Self {
        Self {
            label,
            usage: usage | BufferUsage::COPY_DST,
            alignment: alignment.max(4),
            buffer: None,
            capacity: 0,
            cursor: 0,
            epoch: None,
        }
    }

    fn write(&mut self, ctx: &mut RecordCtx<'_>, bytes: &[u8]) -> Result<(BufferId, u64), RhiError> {
        let epoch = ctx.view().record_epoch;
        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.cursor = 0;
        }
        let length = bytes.len() as u64;
        let mut offset = self.cursor.next_multiple_of(self.alignment);
        let buffer = match self.buffer {
            Some(buffer) if offset + length <= self.capacity => buffer,
            _ => {
                let capacity = length.next_power_of_two().max(MIN_FRAME_BUFFER_BYTES);
                let buffer = ctx.device().create_buffer(&BufferDesc {
                    label: self.label,
                    size: capacity,
                    usage: self.usage,
                })?;
                if let Some(old) = self.buffer.replace(buffer) {
                    ctx.release_later(old);
                }
                self.capacity = capacity;
                offset = 0;
                buffer
            }
        };
        ctx.device().write_buffer(buffer, offset, bytes)?;
        self.cursor = offset + length;
        Ok((buffer, offset))
    }
}

#[derive(Debug)]
struct ShadowBuffers {
    uniforms: UniformRing,
    uniform_epoch: Option<u64>,
    models: FrameBuffer,
    arguments: FrameBuffer,
}

impl ShadowBuffers {
    fn write_uniform(&mut self, ctx: &mut RecordCtx<'_>, bytes: &[u8]) -> Result<UniformSlice, RhiError> {
        let epoch = ctx.view().record_epoch;
        if self.uniform_epoch != Some(epoch) {
            self.uniforms.begin_frame();
            self.uniform_epoch = Some(epoch);
        }
        self.uniforms.write(ctx.device(), bytes)
    }
}

// What the closure that records the depth pass owns: everything it draws, resolved while the graph was built.
struct CascadePass {
    uniform: UniformSlice,
    batches: Vec<PassBatch>,
}

struct PassBatch {
    pipeline: RenderPipelineId,
    vertex_buffer: BufferId,
    index_buffer: BufferId,
    draws: Vec<crate::plan::Draw>,
    // Where the batch's arguments start in the shared arguments buffer; `None` draws one by one.
    arguments: Option<(BufferId, u64)>,
}

#[derive(Debug, Default)]
struct Reported {
    settings: bool,
    direction: bool,
    camera: bool,
}

/// Renders the cascaded depth maps of a directional light into [`SHADOW_MAP`] and publishes a [`ShadowFrame`] for the
/// lighting shader. Holds `Role::SHADOWS`.
#[derive(Debug, Default)]
pub struct ShadowFeature {
    gpu: Option<ShadowGpu>,
    buffers: Option<ShadowBuffers>,
    pipelines: HashMap<(u32, DepthBias), RenderPipelineId>,
    reported: Reported,
}

impl ShadowFeature {
    pub fn new() -> Self {
        Self::default()
    }

    fn pipeline(&mut self, device: &dyn DeviceApi, stride: u32, bias: DepthBias) -> Result<RenderPipelineId, RhiError> {
        if let Some(pipeline) = self.pipelines.get(&(stride, bias)) {
            return Ok(*pipeline);
        }
        let gpu = self.gpu.ok_or(RhiError::Unsupported("shadows used before setup"))?;
        let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
            label: "shadow cascade",
            bind_group_layouts: vec![gpu.layout],
            vertex: ShaderStage {
                module: gpu.module,
                entry_point: "vs_depth",
                constants: vec![],
            },
            vertex_buffers: vec![VertexBufferLayout {
                stride: u64::from(stride),
                step_mode: VertexStepMode::Vertex,
                attributes: vec![VertexAttribute {
                    format: VertexFormat::Float32x3,
                    offset: 0,
                    location: 0,
                }],
            }],
            fragment: None,
            color_targets: vec![],
            primitive: PrimitiveState::default(),
            depth: Some(DepthState::reverse_z(DEPTH_FORMAT).with_bias(bias)),
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.pipelines.insert((stride, bias), pipeline);
        Ok(pipeline)
    }

    fn settings(&mut self, ctx: &mut RecordCtx<'_>) -> ShadowSettings {
        let limits = &ctx.device().caps().limits;
        let (max_size, max_layers) = (limits.max_texture_dimension_2d, limits.max_texture_array_layers);
        let requested = ctx.shared().get::<ShadowSettings>().cloned().unwrap_or_default();
        let (settings, problems) = requested.sanitized(max_size, max_layers);
        if problems.is_empty() {
            self.reported.settings = false;
        } else if !self.reported.settings {
            self.reported.settings = true;
            ctx.report(
                Diagnostic::warning("D0001", format!("shadow settings repaired: {}", problems.join("; ")))
                    .with_hint("keep ShadowSettings inside the documented ranges"),
            );
        }
        settings
    }

    // The plan of this frame, or `None` (reported once) when there is nothing to render.
    fn plan(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        settings: &ShadowSettings,
    ) -> Option<(FramePlan, DirectionalShadowLight)> {
        let scene = ctx.shared().get::<ShadowScene>()?;
        let light = scene.light?;
        let Some(basis) = LightBasis::new(light.direction) else {
            if !self.reported.direction {
                self.reported.direction = true;
                ctx.report(
                    Diagnostic::warning(
                        "D0002",
                        format!("shadow light direction {} is not usable", light.direction),
                    )
                    .with_hint("use a finite, non-zero direction"),
                );
            }
            return None;
        };
        self.reported.direction = false;
        let camera = ctx.view().camera;
        let Some(frustum) = CameraFrustum::from_camera(&camera) else {
            if !self.reported.camera {
                self.reported.camera = true;
                ctx.report(
                    Diagnostic::warning(
                        "D0003",
                        "the camera's projection is not a reverse-Z perspective or orthographic one",
                    )
                    .with_hint("build the camera with Camera::look_at or Projection::matrix"),
                );
            }
            return None;
        };
        self.reported.camera = false;
        let scene = ctx.shared().get::<ShadowScene>()?;
        Some((plan_frame(scene.casters(), &frustum, basis, settings), light))
    }
}

impl RenderFeature for ShadowFeature {
    fn name(&self) -> &'static str {
        "shadow cascades"
    }

    fn stage(&self) -> Stage {
        Stage::Shadows
    }

    fn role(&self) -> Option<Role> {
        Some(Role::SHADOWS)
    }

    fn requirements(&self) -> Requirements {
        Requirements {
            optional_features: Features::INDIRECT_FIRST_INSTANCE,
            ..Requirements::default()
        }
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let usable = FormatCaps::SAMPLED | FormatCaps::FILTERABLE | FormatCaps::RENDER_TARGET;
        if caps.format(DEPTH_FORMAT).contains(usable) {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "shadow maps need a Depth32Float texture that can be rendered to and sampled with filtering"
                    .to_owned(),
            }
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let limits = &device.caps().limits;
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "shadow cascade",
            wgsl: DEPTH_WGSL,
        })?;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "shadow cascade",
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::UniformBuffer,
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::StorageBuffer {
                        access: StorageAccess::ReadOnly,
                    },
                    count: None,
                },
            ],
        })?;
        let sampler = device.create_sampler(&SamplerDesc {
            label: "shadow compare",
            address_mode: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            compare: Some(CompareFunction::GreaterEqual),
            anisotropy_clamp: 1,
        })?;
        self.gpu = Some(ShadowGpu {
            module,
            layout,
            sampler,
        });
        self.pipelines.clear();
        self.buffers = Some(ShadowBuffers {
            uniforms: UniformRing::new(device, "shadow uniforms", 16 << 10)?,
            uniform_epoch: None,
            models: FrameBuffer::new(
                "shadow models",
                BufferUsage::STORAGE,
                u64::from(limits.min_storage_buffer_offset_alignment),
            ),
            arguments: FrameBuffer::new("shadow draw arguments", BufferUsage::INDIRECT, 4),
        });
        ctx.shared().remove::<ShadowFrame>();
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let gpu = self.gpu.ok_or(RhiError::Unsupported("shadows used before setup"))?;
        let settings = self.settings(ctx);
        let Some((plan, light)) = self.plan(ctx, &settings) else {
            ctx.shared_mut().remove::<ShadowFrame>();
            return Ok(());
        };
        if plan.cascades.is_empty() {
            ctx.shared_mut().remove::<ShadowFrame>();
            return Ok(());
        }
        let indirect = settings.use_indirect_draws && ctx.device().caps().has(Features::INDIRECT_FIRST_INSTANCE);
        // Hardware bias in the sign of a reverse-Z buffer: away from the light is smaller.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the constant is clamped to 1e6 by the settings"
        )]
        let bias = DepthBias::new(
            -(settings.polygon_offset_constant.round() as i32),
            -settings.polygon_offset_slope,
            0.0,
        );

        let mut passes = Vec::with_capacity(plan.cascades.len());
        let mut arguments = Vec::new();
        let mut argument_batches = Vec::new();
        for (cascade, batches) in plan.cascades.iter().zip(&plan.batches) {
            let uniform = {
                let buffers = self
                    .buffers
                    .as_mut()
                    .ok_or(RhiError::Unsupported("shadows used before setup"))?;
                buffers.write_uniform(
                    ctx,
                    bytemuck::bytes_of(&cascade.caster_view_projection.to_cols_array_2d()),
                )?
            };
            let mut pass_batches = Vec::with_capacity(batches.len());
            for batch in batches {
                let pipeline = self.pipeline(ctx.device(), batch.vertex_stride, bias)?;
                if indirect {
                    argument_batches.push((passes.len(), pass_batches.len(), arguments.len() as u64));
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
                pass_batches.push(PassBatch {
                    pipeline,
                    vertex_buffer: batch.vertex_buffer,
                    index_buffer: batch.index_buffer,
                    draws: batch.draws.clone(),
                    arguments: None,
                });
            }
            passes.push(CascadePass {
                uniform,
                batches: pass_batches,
            });
        }

        let models = if plan.draw_count() > 0 {
            let bytes: Vec<f32> = plan.models.iter().flat_map(Mat4::to_cols_array).collect();
            let buffers = self
                .buffers
                .as_mut()
                .ok_or(RhiError::Unsupported("shadows used before setup"))?;
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
            let buffers = self
                .buffers
                .as_mut()
                .ok_or(RhiError::Unsupported("shadows used before setup"))?;
            let (buffer, base) = buffers.arguments.write(ctx, &arguments)?;
            for (pass, batch, offset) in argument_batches {
                passes[pass].batches[batch].arguments = Some((buffer, base + offset));
            }
        }

        let direction = plan.basis.forward;
        let uniforms = ShadowUniforms::new(&plan.cascades, direction, light, &settings);
        let uniform_buffer = {
            let buffers = self
                .buffers
                .as_mut()
                .ok_or(RhiError::Unsupported("shadows used before setup"))?;
            buffers.write_uniform(ctx, bytemuck::bytes_of(&uniforms))?
        };

        let cascade_count = plan.cascades.len();
        let layers = u32::try_from(cascade_count).unwrap_or(1);
        let map = ctx.graph().create_texture(
            "shadow cascades",
            GraphTextureDesc {
                layers,
                ..GraphTextureDesc::fixed(settings.resolution, settings.resolution, DEPTH_FORMAT)
            },
        );
        let mut pass = ctx.graph().add_pass(PassDesc::new("shadow cascades", Stage::Shadows));
        let map = pass.write(map, Access::DEPTH_WRITE);
        pass.record(move |pass| {
            let mut targets = Vec::with_capacity(passes.len());
            for (layer, cascade) in (0..).zip(&passes) {
                let view = pass.view_with(map, &TextureViewDesc::layer(layer))?;
                let group = match (&models, cascade.batches.is_empty()) {
                    (Some(models), false) => Some(pass.create_bind_group(&BindGroupDesc {
                        label: "shadow cascade",
                        layout: gpu.layout,
                        entries: vec![
                            BindGroupEntry {
                                binding: 0,
                                resource: cascade.uniform.binding(),
                            },
                            BindGroupEntry {
                                binding: 1,
                                resource: models.clone(),
                            },
                        ],
                    })?),
                    _ => None,
                };
                targets.push((view, group));
            }
            for ((view, group), cascade) in targets.into_iter().zip(&passes) {
                let mut render = pass.commands().render_pass(RenderPassDesc {
                    label: "shadow cascade",
                    color: vec![],
                    depth: Some(DepthAttachment::clear_reverse_z(view)),
                    timestamps: None,
                });
                let Some(group) = group else {
                    continue;
                };
                for batch in &cascade.batches {
                    render.set_pipeline(batch.pipeline);
                    render.set_bind_group(0, group);
                    render.set_vertex_buffer(0, batch.vertex_buffer, 0);
                    render.set_index_buffer(batch.index_buffer, pathon_api::rhi::IndexFormat::Uint32, 0);
                    match batch.arguments {
                        Some((buffer, offset)) => {
                            let count = u32::try_from(batch.draws.len()).unwrap_or(u32::MAX);
                            render.multi_draw_indexed_indirect(buffer, offset, count);
                        }
                        None => {
                            for draw in &batch.draws {
                                render.draw_indexed(
                                    draw.first_index..draw.first_index + draw.index_count,
                                    draw.base_vertex,
                                    draw.caster..draw.caster + 1,
                                );
                            }
                        }
                    }
                }
            }
            Ok(())
        });
        ctx.set_slot(SHADOW_MAP, map);
        ctx.shared_mut().insert(ShadowFrame {
            uniforms,
            uniform_buffer,
            sampler: gpu.sampler,
            cascade_count: layers,
            resolution: settings.resolution,
        });
        Ok(())
    }

    fn on_device_reset(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.reported = Reported::default();
        self.setup(ctx)
    }
}
