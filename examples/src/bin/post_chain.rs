// M4-09: the post-processing chain on a synthetic HDR frame: a ramp over 16 stops, colour bars and two bright spots.
// `PostPack` measures the frame (auto-exposure adapts over 90 frames of 1/60 s), blooms the highlights and tonemaps
// into an 8-bit target. The picture goes to a PNG file: `post_chain [aces|neutral] [file.png]`.

use std::fs::File;
use std::io::BufWriter;

use pathon::api::graph::{PassDesc, Stage};
use pathon::api::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList, DeviceApi,
    Extent3d, Format, PhysicalSize, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError,
    ShaderModuleDesc, ShaderStage, SubresourceRange, TexelLayout, TextureCopy,
};
use pathon::api::{Camera, RecordCtx, RenderFeature, SetupCtx, Slot};
use pathon::{FrameInput, Renderer, ViewInput};
use pathon_post::{PostPack, PostSettings, Tonemapper};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 320;
const FRAMES: u32 = 90;

// Scene luminance is 16 times brighter than middle grey would need: auto-exposure has to pull it down.
const SCENE_SHADER: &str = "
override SCENE_SCALE: f32 = 16.0;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

fn bar(index: u32) -> vec3<f32> {
    switch index {
        case 0u: { return vec3<f32>(1.0, 1.0, 1.0); }
        case 1u: { return vec3<f32>(1.0, 1.0, 0.0); }
        case 2u: { return vec3<f32>(0.0, 1.0, 1.0); }
        case 3u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 4u: { return vec3<f32>(1.0, 0.0, 1.0); }
        case 5u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 6u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.05); }
    }
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / vec2<f32>(512.0, 320.0);
    var color = vec3<f32>(0.02);
    if (uv.y < 0.33) {
        color = vec3<f32>(0.18 * exp2(uv.x * 16.0 - 8.0));
    } else if (uv.y < 0.66) {
        color = bar(u32(uv.x * 8.0)) * select(1.0, 8.0, uv.y > 0.5);
    } else {
        let sun = distance(position.xy, vec2<f32>(160.0, 260.0));
        let lamp = distance(position.xy, vec2<f32>(400.0, 260.0));
        if (sun < 8.0) {
            color = vec3<f32>(300.0, 280.0, 240.0);
        } else if (lamp < 14.0) {
            color = vec3<f32>(6.0, 3.0, 1.0);
        }
    }
    return vec4<f32>(color * SCENE_SCALE, 1.0);
}
";

#[derive(Debug, Default)]
struct SyntheticScene {
    pipeline: Option<RenderPipelineId>,
}

impl RenderFeature for SyntheticScene {
    fn name(&self) -> &'static str {
        "synthetic scene"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "synthetic scene",
            wgsl: SCENE_SHADER,
        })?;
        self.pipeline = Some(device.create_render_pipeline(&RenderPipelineDesc {
            label: "synthetic scene",
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
            color_targets: vec![ColorTargetState::opaque(Format::Rgba16Float)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })?);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let pipeline = self.pipeline.ok_or(RhiError::Unsupported("scene used before setup"))?;
        let hdr = ctx
            .create_slot_texture(Slot::HDR, "synthetic hdr")
            .ok_or(RhiError::Unsupported("hdr slot has no format"))?;
        let mut pass = ctx.graph().add_pass(PassDesc::new("synthetic scene", Stage::Opaque));
        let hdr = pass.write(hdr, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(hdr)?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "synthetic scene",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.draw(0..3, 0..1);
            Ok(())
        });
        ctx.set_slot(Slot::HDR, hdr);
        Ok(())
    }
}

fn write_png(path: &str, rgba: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("create the png"));
    let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn main() {
    pathon_examples::init_logging();
    let mut arguments = std::env::args().skip(1);
    let operator = match arguments.next().as_deref() {
        Some("neutral") => Tonemapper::PbrNeutral,
        _ => Tonemapper::AcesFitted,
    };
    let output_path = arguments.next().unwrap_or_else(|| "post_chain.png".to_owned());

    let mut settings = PostSettings::default();
    settings.tonemap.operator = operator;
    let mut renderer = Renderer::builder()
        .with_feature(SyntheticScene::default())
        .with_pack(PostPack::with_settings(settings))
        .build()
        .expect("a GPU renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(WIDTH, HEIGHT), Format::Rgba8Unorm)
        .expect("offscreen target");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        delta_seconds: 1.0 / 60.0,
        ..FrameInput::default()
    };
    for _ in 0..FRAMES {
        let report = renderer.render_frame(&input).expect("frame");
        for diagnostic in &report.diagnostics {
            log::warn!("{diagnostic}");
        }
    }

    let device = renderer.device();
    let texture = renderer.target_texture(target).expect("target texture");
    let row_bytes = WIDTH * 4;
    let readback = device
        .create_buffer(&BufferDesc {
            label: "post chain readback",
            size: u64::from(row_bytes * HEIGHT),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback buffer");
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(texture),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: row_bytes,
                rows_per_image: HEIGHT,
            },
        },
        Extent3d::new_2d(WIDTH, HEIGHT),
    );
    device.submit(list).expect("submit");
    write_png(&output_path, &device.read_buffer(readback).expect("read"));
    log::info!("wrote {output_path} ({operator:?}, {FRAMES} frames of auto-exposure)");
}
