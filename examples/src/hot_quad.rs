// The host side of the `hot_reload` example and its test: a full-screen triangle whose shader lives in files that are
// reloaded while it draws (M3-06, docs/crates/pathon-hot.md). A pack would do the same lookups every frame.

use std::path::Path;
use std::sync::Arc;

use pathon::api::SerialTaskPool;
use pathon::rhi::{
    Access, BufferCopy, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList,
    Device, DeviceApi, Extent3d, Format, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId,
    ShaderStage, SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureId, TextureUsage, TextureViewDesc,
    TextureViewId,
};
use pathon_hot::{HotConfig, HotError, HotReloader, HotReport};
use pathon_shader::{BackgroundCompiler, BackgroundConfig, Lookup, ShaderFeatures, ShaderKey};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 8;
const ROW_BYTES: u32 = WIDTH * 4;
pub const SHADER_NAME: &str = "quad";

#[derive(Debug)]
pub struct HotFrame {
    /// The first pixel of the picture, RGBA, once something could be drawn.
    pub pixel: Option<[u8; 4]>,
    pub report: HotReport,
}

#[derive(Debug)]
pub struct HotQuad {
    device: Arc<Device>,
    compiler: BackgroundCompiler,
    reloader: HotReloader,
    key: ShaderKey,
    // The pipeline drawn with: replaced only when a new one is ready, so a shader that does not compile never
    // blanks the picture.
    pipeline: Option<RenderPipelineId>,
    color: TextureId,
    view: TextureViewId,
    readback: BufferId,
    // Where the last frame left the colour target, which the next frame starts from.
    color_access: Access,
}

impl HotQuad {
    pub fn new(device: Arc<Device>, shader_dir: &Path, module_dir: &Path) -> Result<Self, HotError> {
        let config = HotConfig::new().with_shader_dir(shader_dir).with_module_dir(module_dir);
        Self::with_config(device, &config)
    }

    pub fn with_config(device: Arc<Device>, config: &HotConfig) -> Result<Self, HotError> {
        let handle: Arc<dyn DeviceApi> = Arc::<Device>::clone(&device);
        let compiler = BackgroundCompiler::new(
            handle,
            Arc::new(SerialTaskPool),
            BackgroundConfig::new().with_inline_jobs_per_pump(4),
        );
        let reloader = HotReloader::new(config)?;
        let color = device
            .create_texture(&TextureDesc::new_2d(
                "hot color",
                WIDTH,
                HEIGHT,
                Format::Rgba8Unorm,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            ))
            .expect("color target");
        let view = device
            .create_texture_view(color, &TextureViewDesc::default())
            .expect("color view");
        let readback = device
            .create_buffer(&BufferDesc {
                label: "hot readback",
                size: u64::from(ROW_BYTES * HEIGHT),
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback buffer");
        Ok(Self {
            device,
            compiler,
            reloader,
            key: ShaderKey {
                root: SHADER_NAME.to_owned(),
                features: ShaderFeatures::default(),
            },
            pipeline: None,
            color,
            view,
            readback,
            color_access: Access::empty(),
        })
    }

    /// One frame: pump the compiler and the reloader, look the shader up, draw with the newest pipeline there is.
    pub fn frame(&mut self) -> HotFrame {
        self.compiler.pump();
        let report = self.reloader.pump(&mut self.compiler);
        self.destroy_retired(report.retired.clone());
        if let Lookup::Ready(module) = self.compiler.shader_module(&self.key)
            && let Lookup::Ready(pipeline) = self.compiler.render_pipeline(&Self::pipeline_desc(module))
        {
            self.pipeline = Some(pipeline);
        }
        let pixel = self.pipeline.map(|pipeline| self.draw(pipeline));
        HotFrame { pixel, report }
    }

    pub fn has_pipeline(&self) -> bool {
        self.pipeline.is_some()
    }

    // The reloader hands modules and pipelines back once their replacements are in use; a host with frames in flight
    // would give them to `release_later` instead.
    fn destroy_retired(&self, retired: Vec<pathon::rhi::Resource>) {
        if retired.is_empty() {
            return;
        }
        self.device.wait_idle().expect("the device idles");
        for resource in retired {
            self.device.destroy(resource);
        }
    }

    fn pipeline_desc(module: pathon::rhi::ShaderModuleId) -> RenderPipelineDesc {
        RenderPipelineDesc {
            label: "hot quad",
            bind_group_layouts: vec![],
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
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        }
    }

    fn draw(&mut self, pipeline: RenderPipelineId) -> [u8; 4] {
        let mut list = CommandList::new();
        list.texture_access(
            self.color,
            SubresourceRange::ALL,
            self.color_access,
            Access::COLOR_ATTACHMENT,
        );
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "hot quad",
                color: vec![ColorAttachment::clear(self.view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.draw(0..3, 0..1);
        }
        list.texture_access(
            self.color,
            SubresourceRange::ALL,
            Access::COLOR_ATTACHMENT,
            Access::COPY_SRC,
        );
        list.copy_texture_to_buffer(
            TextureCopy::whole(self.color),
            BufferCopy {
                buffer: self.readback,
                layout: TexelLayout {
                    offset: 0,
                    bytes_per_row: ROW_BYTES,
                    rows_per_image: HEIGHT,
                },
            },
            Extent3d::new_2d(WIDTH, HEIGHT),
        );
        self.device.submit(list).expect("submit");
        self.color_access = Access::COPY_SRC;
        let bytes = self.device.read_buffer(self.readback).expect("read the picture back");
        [bytes[0], bytes[1], bytes[2], bytes[3]]
    }
}
