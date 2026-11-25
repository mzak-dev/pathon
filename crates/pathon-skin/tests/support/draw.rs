use pathon_api::rhi::{
    Access, Color, ColorAttachment, ColorTargetState, CommandList, DepthAttachment, DepthState, Format, IndexFormat,
    PrimitiveState, RenderPassDesc, RenderPipelineDesc, ShaderModuleDesc, ShaderStage, SubresourceRange, TextureDesc,
    TextureUsage, TextureViewDesc, Viewport,
};
use pathon_golden::GoldenImage;
use pathon_skin::DeformedSurface;

use super::Rig;

const DRAW_WGSL: &str = "
struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
}
@vertex fn vs(@location(0) position: vec4<f32>, @location(1) normal: vec3<f32>) -> Varying {
    // Orthographic, looking down -Z: x in -0.3..2.3, y in -1.3..1.3, reverse-Z depth.
    return Varying(vec4<f32>((position.x - 1.0) / 1.3, position.y / 1.3, 0.5 + position.z, 1.0), normal);
}
@fragment fn fs(in: Varying) -> @location(0) vec4<f32> {
    return vec4<f32>(normalize(in.normal) * 0.5 + 0.5, 1.0);
}
";

pub const TILE: u32 = 96;

impl Rig {
    /// Draws every instance of the last frame into its own `TILE` x `TILE` tile of one image, from the deformed
    /// buffers as vertex buffers: the path a prepass or a forward pass takes. Normals are the colour.
    pub fn draw_instances(&self) -> GoldenImage {
        let device = self.device();
        let frame = self.output().frame;
        let width = TILE * u32::try_from(self.ranges.len()).expect("few tiles");
        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "skin test draw",
                wgsl: DRAW_WGSL,
            })
            .expect("module");
        let pipeline = device
            .create_render_pipeline(&RenderPipelineDesc {
                label: "skin test draw",
                bind_group_layouts: vec![],
                vertex: ShaderStage {
                    module,
                    entry_point: "vs",
                    constants: vec![],
                },
                vertex_buffers: pathon_skin::deformed_vertex_buffers(),
                fragment: Some(ShaderStage {
                    module,
                    entry_point: "fs",
                    constants: vec![],
                }),
                color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
                primitive: PrimitiveState::default(),
                depth: Some(DepthState::reverse_z(Format::Depth32Float)),
                sample_count: 1,
                immediate_size: 0,
            })
            .expect("pipeline");
        let color = device
            .create_texture(&TextureDesc::new_2d(
                "skin test color",
                width,
                TILE,
                Format::Rgba8Unorm,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            ))
            .expect("color");
        let depth = device
            .create_texture(&TextureDesc::new_2d(
                "skin test depth",
                width,
                TILE,
                Format::Depth32Float,
                TextureUsage::RENDER_TARGET,
            ))
            .expect("depth");
        let color_view = device
            .create_texture_view(color, &TextureViewDesc::default())
            .expect("view");
        let depth_view = device
            .create_texture_view(depth, &TextureViewDesc::default())
            .expect("view");
        let info = self.pool.mesh(self.mesh).expect("arm").location;
        let index_buffer = self.pool.index_buffer(info.index_chunk).expect("indices");

        let mut list = CommandList::new();
        list.buffer_access(self.positions.buffer(), Access::VERTEX);
        list.buffer_access(self.surface.buffer(), Access::VERTEX);
        list.texture_access(color, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
        list.texture_access(depth, SubresourceRange::ALL, Access::empty(), Access::DEPTH_WRITE);
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "skin test draw",
                color: vec![ColorAttachment::clear(color_view, Color::BLACK)],
                depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_index_buffer(index_buffer, IndexFormat::Uint32, 0);
            for (tile, range) in self.ranges.iter().enumerate() {
                let tile = u32::try_from(tile).expect("few tiles");
                pass.set_viewport(Viewport {
                    x: (tile * TILE) as f32,
                    y: 0.0,
                    width: TILE as f32,
                    height: TILE as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                });
                pass.set_vertex_buffer(0, self.positions.buffer(), self.positions.current_offset(*range, frame));
                pass.set_vertex_buffer(1, self.surface.buffer(), DeformedSurface::offset(range.first_vertex));
                pass.draw_indexed(info.indices(), 0, 0..1);
            }
        }
        device.submit(list).expect("submit");
        pathon_golden::read_texture(device, color, Access::COLOR_ATTACHMENT).expect("pixels")
    }
}
