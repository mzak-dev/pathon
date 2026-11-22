use std::collections::HashMap;

use pathon_api::glam::{Mat4, Vec3};
use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::resources::UniformRing;
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId, BindingType,
    BufferDesc, BufferId, BufferUsage, ColorAttachment, ColorTargetState, CullMode, DepthAttachment, DepthState,
    DeviceApi, Format, IndexFormat, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError,
    ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexStepMode,
};
use pathon_api::{RecordCtx, RenderFeature, SetupCtx, Slot};

const SHADER: &str = r"
struct Uniforms {
    view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip = uniforms.view_projection * uniforms.model * vec4<f32>(input.position, 1.0);
    out.color = input.color;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
";

const CUBE_INDICES: [u16; 36] = [
    0, 1, 2, 2, 3, 0, // -z
    5, 4, 7, 7, 6, 5, // +z
    4, 0, 3, 3, 7, 4, // -x
    1, 5, 6, 6, 2, 1, // +x
    3, 2, 6, 6, 7, 3, // +y
    4, 5, 1, 1, 0, 4, // -y
];

fn cube_vertices() -> Vec<f32> {
    let corners = [
        [-0.5, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    let mut vertices = Vec::with_capacity(corners.len() * 6);
    for [x, y, z] in corners {
        vertices.extend([x, y, z, x + 0.5, y + 0.5, z + 0.5]);
    }
    vertices
}

#[derive(Debug, Clone, Copy)]
struct GpuObjects {
    vertices: BufferId,
    indices: BufferId,
    shader: ShaderModuleId,
    layout: BindGroupLayoutId,
}

// The M2-18 vertical slice: one coloured cube with a hard-coded WGSL shader, drawn straight into `Output`.
#[derive(Debug, Default)]
pub struct UnlitMeshPack {
    objects: Option<GpuObjects>,
    pipelines: HashMap<Format, RenderPipelineId>,
    uniforms: Option<UniformRing>,
    ring_epoch: Option<u64>,
}

impl UnlitMeshPack {
    pub fn new() -> Self {
        Self::default()
    }

    fn pipeline(&mut self, device: &dyn DeviceApi, format: Format) -> Result<RenderPipelineId, RhiError> {
        if let Some(pipeline) = self.pipelines.get(&format) {
            return Ok(*pipeline);
        }
        let objects = self
            .objects
            .ok_or(RhiError::Unsupported("unlit mesh used before setup"))?;
        let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
            label: "unlit mesh",
            bind_group_layouts: vec![objects.layout],
            vertex: ShaderStage {
                module: objects.shader,
                entry_point: "vs_main",
                constants: vec![],
            },
            vertex_buffers: vec![VertexBufferLayout {
                stride: 24,
                step_mode: VertexStepMode::Vertex,
                attributes: vec![
                    VertexAttribute {
                        format: VertexFormat::Float32x3,
                        offset: 0,
                        location: 0,
                    },
                    VertexAttribute {
                        format: VertexFormat::Float32x3,
                        offset: 12,
                        location: 1,
                    },
                ],
            }],
            fragment: Some(ShaderStage {
                module: objects.shader,
                entry_point: "fs_main",
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(format)],
            primitive: PrimitiveState {
                cull_mode: CullMode::Back,
                ..PrimitiveState::default()
            },
            depth: Some(DepthState::reverse_z(Format::Depth32Float)),
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.pipelines.insert(format, pipeline);
        Ok(pipeline)
    }
}

impl RenderFeature for UnlitMeshPack {
    fn name(&self) -> &'static str {
        "unlit mesh"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let vertex_data = cube_vertices();
        let vertices = device.create_buffer(&BufferDesc {
            label: "cube vertices",
            size: (vertex_data.len() * 4) as u64,
            usage: BufferUsage::VERTEX | BufferUsage::COPY_DST,
        })?;
        device.write_buffer(vertices, 0, bytemuck::cast_slice(&vertex_data))?;
        let indices = device.create_buffer(&BufferDesc {
            label: "cube indices",
            size: (CUBE_INDICES.len() * 2) as u64,
            usage: BufferUsage::INDEX | BufferUsage::COPY_DST,
        })?;
        device.write_buffer(indices, 0, bytemuck::cast_slice(&CUBE_INDICES))?;
        let shader = device.create_shader_module(&ShaderModuleDesc {
            label: "unlit mesh",
            wgsl: SHADER,
        })?;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "unlit mesh",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::UniformBuffer,
                count: None,
            }],
        })?;
        self.objects = Some(GpuObjects {
            vertices,
            indices,
            shader,
            layout,
        });
        self.pipelines.clear();
        self.uniforms = Some(UniformRing::new(device, "unlit mesh uniforms", 64 << 10)?);
        self.ring_epoch = None;
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(output) = ctx.slot(Slot::OUTPUT) else {
            return Ok(());
        };
        let format = ctx
            .graph()
            .texture_format(output)
            .ok_or(RhiError::InvalidHandle("output"))?;
        let pipeline = self.pipeline(ctx.device(), format)?;
        let objects = self
            .objects
            .ok_or(RhiError::Unsupported("unlit mesh used before setup"))?;

        let view = ctx.view();
        #[expect(clippy::cast_possible_truncation, reason = "animation time only needs f32 precision")]
        let time = view.time_seconds as f32;
        let model = Mat4::from_rotation_y(time) * Mat4::from_rotation_x(time * 0.5);
        let aspect = view.output_size.width as f32 / view.output_size.height.max(1) as f32;
        let mut camera = view.camera;
        camera.projection =
            pathon_api::Camera::look_at(camera.position, Vec3::ZERO, std::f32::consts::FRAC_PI_3, aspect).projection;
        let mut uniform_data = [0.0_f32; 32];
        uniform_data[..16].copy_from_slice(&camera.view_projection().to_cols_array());
        uniform_data[16..].copy_from_slice(&model.to_cols_array());
        let record_epoch = view.record_epoch;
        let ring = self
            .uniforms
            .as_mut()
            .ok_or(RhiError::Unsupported("unlit mesh used before setup"))?;
        if self.ring_epoch != Some(record_epoch) {
            ring.begin_frame();
            self.ring_epoch = Some(record_epoch);
        }
        let uniform = ring.write(ctx.device(), bytemuck::cast_slice(&uniform_data))?;

        let depth = ctx
            .graph()
            .create_texture("unlit depth", GraphTextureDesc::output(Format::Depth32Float));
        let mut pass = ctx.graph().add_pass(PassDesc::new("unlit mesh", Stage::Opaque));
        let color = pass.modify(output, Access::COLOR_ATTACHMENT);
        let depth = pass.write(depth, Access::DEPTH_WRITE);
        pass.record(move |pass| {
            let color_view = pass.view(color)?;
            let depth_view = pass.view(depth)?;
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "unlit mesh",
                layout: objects.layout,
                entries: vec![BindGroupEntry {
                    binding: 0,
                    resource: uniform.binding(),
                }],
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "unlit mesh",
                color: vec![ColorAttachment::load(color_view)],
                depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.set_bind_group(0, group);
            render.set_vertex_buffer(0, objects.vertices, 0);
            render.set_index_buffer(objects.indices, IndexFormat::Uint16, 0);
            render.draw_indexed(0..36, 0, 0..1);
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, color);
        Ok(())
    }
}
