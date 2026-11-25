// What `PbrPack` leaves out (it draws no sky): the environment of `IblPack` as the backdrop of the windows. A stand-in
// for a sky pack, written as a host feature because the viewer needs it today; it runs after the pbr passes, writes
// only the pixels no geometry touched (reverse-Z depth 0) and so also replaces whatever blended glass drew over them.

use pathon::api::glam::Mat4;
use pathon::api::graph::{PassDesc, Stage};
use pathon::api::resources::UniformRing;
use pathon::api::{RecordCtx, RenderFeature, Slot};
use pathon::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, ColorAttachment, ColorTargetState, CompareFunction, DepthAccess, DepthAttachment,
    DepthBias, DepthState, DeviceApi, Format, LoadOp, PrimitiveState, RenderPassDesc, RenderPipelineDesc,
    RenderPipelineId, RhiError, SamplerBindingType, ShaderModuleDesc, ShaderStage, ShaderStages, StoreOp,
    TextureSampleType, TextureViewDimension,
};
use pathon_ibl::IblEnvironment;

use super::settings::ViewerSettings;

const SOURCE: &str = include_str!("sky.wgsl");
const HDR_FORMAT: Format = Format::Rgba16Float;
const DEPTH_FORMAT: Format = Format::Depth32Float;
const UNIFORM_RING_BYTES: u64 = 4 << 10;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyUniform {
    inverse_view_projection: [[f32; 4]; 4],
    camera: [f32; 4],
    params: [f32; 4],
}

#[derive(Debug)]
struct Gpu {
    layout: BindGroupLayoutId,
    pipeline: RenderPipelineId,
    ring: UniformRing,
    ring_epoch: Option<u64>,
}

impl Gpu {
    fn new(device: &dyn DeviceApi) -> Result<Self, RhiError> {
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "viewer sky",
            wgsl: SOURCE,
        })?;
        let entry = |binding, visibility, ty| BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "viewer sky",
            entries: vec![
                entry(0, ShaderStages::FRAGMENT, BindingType::UniformBuffer),
                entry(
                    1,
                    ShaderStages::FRAGMENT,
                    BindingType::SampledTexture {
                        sample_type: TextureSampleType::Float,
                        dimension: TextureViewDimension::Cube,
                        multisampled: false,
                    },
                ),
                entry(
                    2,
                    ShaderStages::FRAGMENT,
                    BindingType::Sampler(SamplerBindingType::Filtering),
                ),
            ],
        })?;
        let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
            label: "viewer sky",
            bind_group_layouts: vec![layout],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: vec![],
            fragment: Some(ShaderStage {
                module,
                entry_point: "fs",
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(HDR_FORMAT)],
            primitive: PrimitiveState::default(),
            depth: Some(DepthState {
                format: DEPTH_FORMAT,
                write_enabled: false,
                compare: CompareFunction::Equal,
                bias: DepthBias::NONE,
            }),
            sample_count: 1,
            immediate_size: 0,
        })?;
        Ok(Self {
            layout,
            pipeline,
            ring: UniformRing::new(device, "viewer sky uniforms", UNIFORM_RING_BYTES)?,
            ring_epoch: None,
        })
    }
}

/// Draws the environment behind the scene; needs the `IblEnvironment` and the `hdr` and `depth` slots of the pbr pack.
#[derive(Debug, Default)]
pub struct SkyFeature {
    gpu: Option<Gpu>,
}

impl SkyFeature {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RenderFeature for SkyFeature {
    fn name(&self) -> &'static str {
        "viewer sky"
    }

    fn stage(&self) -> Stage {
        Stage::Transparent
    }

    fn after(&self) -> &[&'static str] {
        &["pbr lighting"]
    }

    fn on_device_reset(&mut self, _ctx: &mut pathon::api::SetupCtx<'_>) -> Result<(), RhiError> {
        self.gpu = None;
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let (Some(hdr), Some(depth)) = (ctx.slot(Slot::HDR), ctx.slot(Slot::DEPTH)) else {
            return Ok(());
        };
        let Some(environment) = ctx.shared().get::<IblEnvironment>().copied() else {
            return Ok(());
        };
        let intensity = ctx
            .shared()
            .get::<ViewerSettings>()
            .map_or(ViewerSettings::default().sky_intensity, |settings| settings.sky_intensity);
        if self.gpu.is_none() {
            self.gpu = Some(Gpu::new(ctx.device())?);
        }
        let gpu = self.gpu.as_mut().ok_or(RhiError::InvalidHandle("sky gpu state"))?;

        let view = ctx.view();
        let camera = view.camera;
        let record_epoch = view.record_epoch;
        let uniform = SkyUniform {
            inverse_view_projection: Mat4::inverse(&camera.view_projection()).to_cols_array_2d(),
            camera: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            params: [intensity, 0.0, 0.0, 0.0],
        };
        if gpu.ring_epoch != Some(record_epoch) {
            gpu.ring.begin_frame();
            gpu.ring_epoch = Some(record_epoch);
        }
        let slice = gpu.ring.write(ctx.device(), bytemuck::bytes_of(&uniform))?;
        let (layout, pipeline) = (gpu.layout, gpu.pipeline);

        let mut pass = ctx.graph().add_pass(PassDesc::new("viewer sky", Stage::Transparent));
        let depth = pass.read(depth, Access::DEPTH_READ);
        let hdr = pass.modify(hdr, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let (hdr_view, depth_view) = (pass.view(hdr)?, pass.view(depth)?);
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "viewer sky",
                layout,
                entries: vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: slice.binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::TextureView(environment.prefiltered.view),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::Sampler(environment.sampler),
                    },
                ],
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "viewer sky",
                color: vec![ColorAttachment::load(hdr_view)],
                depth: Some(DepthAttachment {
                    view: depth_view,
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                    access: DepthAccess::ReadOnly,
                    clear_stencil: None,
                }),
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.set_bind_group(0, group);
            render.draw(0..3, 0..1);
            Ok(())
        });
        ctx.set_slot(Slot::HDR, hdr);
        Ok(())
    }
}
