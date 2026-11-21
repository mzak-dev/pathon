#![cfg(feature = "gpu-tests")]

use std::sync::{Arc, Mutex};

use pathon_api::glam::Vec3;
use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::{Camera, RecordCtx, RenderFeature};
use pathon_core::{FrameInput, Renderer, ViewInput};
use pathon_rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, Color, ColorAttachment, CommandList, DepthAttachment, DeviceApi,
    Extent3d, Format, FormatKind, PhysicalSize, RenderPassDesc, RhiError, SubresourceRange, TexelLayout, TextureCopy,
};
use pathon_test::{GeometryPack, UnlitMeshPack, UpscalerPack, VendorDummy};

const SIZE: u32 = 64;
const CLEAR: Color = Color::rgba(0.0, 0.0, 1.0, 1.0);

fn read_pixels(renderer: &Renderer, target: pathon_core::TargetId) -> Vec<u8> {
    let device = renderer.device();
    let texture = renderer.target_texture(target).expect("offscreen texture");
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: u64::from(SIZE * SIZE * 4),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("buffer");
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(texture),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: SIZE * 4,
                rows_per_image: SIZE,
            },
        },
        Extent3d::new_2d(SIZE, SIZE),
    );
    device.submit(list).expect("copy");
    device.read_buffer(readback).expect("read")
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let start = ((y * SIZE + x) * 4) as usize;
    pixels[start..start + 4].try_into().expect("four bytes")
}

#[test]
fn unlit_cube_goes_through_the_whole_stack() {
    let mut renderer = Renderer::builder()
        .with_clear_color(CLEAR)
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("GPU renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba8Unorm)
        .expect("target");
    let camera = Camera::look_at(Vec3::new(0.0, 0.0, 2.5), Vec3::ZERO, std::f32::consts::FRAC_PI_3, 1.0);
    let input = FrameInput {
        views: vec![ViewInput { target, camera }],
        time_seconds: 0.4,
        ..FrameInput::default()
    };

    renderer.render_frame(&input).expect("frame");
    let pixels = read_pixels(&renderer, target);

    assert_eq!(pixel(&pixels, 0, 0), [0, 0, 255, 255], "corner shows the clear colour");
    assert_ne!(
        pixel(&pixels, SIZE / 2, SIZE / 2),
        [0, 0, 255, 255],
        "centre shows the cube"
    );
}

#[test]
fn vendor_external_command_inside_a_pass_submits_on_wgpu() {
    let vendor = VendorDummy::default();
    let called = std::sync::Arc::clone(&vendor.called);
    let mut renderer = Renderer::builder()
        .with_feature(GeometryPack::new("geometry", Color::BLACK, 0))
        .with_feature(vendor)
        .with_feature(UpscalerPack::new(1.0))
        .build()
        .expect("GPU renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba16Float)
        .expect("target");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };

    renderer
        .render_frame(&input)
        .expect("the frame with an external command submits");
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn an_external_callback_may_call_the_pathon_device() {
    let device = std::sync::Arc::new(pathon_rhi::Device::build(&pathon_rhi::DeviceRequest::default()).expect("GPU"));
    let inner = std::sync::Arc::clone(&device);
    let mut list = CommandList::new();
    list.external(pathon_rhi::ExternalCommand {
        label: "calls back into pathon",
        accesses: Vec::new(),
        callback: Box::new(move |_: &mut dyn std::any::Any| {
            let buffer = inner.create_buffer(&BufferDesc {
                label: "created by a vendor callback",
                size: 4,
                usage: BufferUsage::COPY_DST,
            });
            assert!(buffer.is_ok());
        }),
    });
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(device.submit(list));
    });
    let submitted = finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("submit returns instead of deadlocking");
    submitted.expect("submit");
}

struct HistoryOnGpu {
    format: Format,
    validity: Arc<Mutex<Vec<bool>>>,
}

impl RenderFeature for HistoryOnGpu {
    fn name(&self) -> &'static str {
        "history on gpu"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let is_color = self.format.kind() == FormatKind::Color;
        let history = ctx
            .graph()
            .history_texture("gpu history", GraphTextureDesc::render(self.format));
        self.validity.lock().expect("mutex").push(history.previous_valid);
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("history writer", Stage::PostProcess));
        pass.read(history.previous, Access::SAMPLED);
        let written = if is_color {
            Access::COLOR_ATTACHMENT
        } else {
            Access::DEPTH_WRITE
        };
        let current = pass.write(history.current, written);
        pass.record(move |pass| {
            let view = pass.view(current)?;
            let (color, depth) = if is_color {
                (vec![ColorAttachment::clear(view, Color::BLACK)], None)
            } else {
                (Vec::new(), Some(DepthAttachment::clear_reverse_z(view)))
            };
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "history writer",
                color,
                depth,
                timestamps: None,
            }));
            Ok(())
        });
        Ok(())
    }
}

#[test]
fn history_of_every_kind_of_format_runs_on_wgpu() {
    for format in [Format::Rgba8Unorm, Format::Depth32Float, Format::Depth24PlusStencil8] {
        let validity = Arc::new(Mutex::new(Vec::new()));
        let mut renderer = Renderer::builder()
            .with_feature(HistoryOnGpu {
                format,
                validity: Arc::clone(&validity),
            })
            .build()
            .expect("GPU renderer");
        let target = renderer
            .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba8Unorm)
            .expect("target");
        let input = FrameInput {
            views: vec![ViewInput {
                target,
                camera: Camera::default(),
            }],
            ..FrameInput::default()
        };

        for _ in 0..3 {
            renderer
                .render_frame(&input)
                .unwrap_or_else(|error| panic!("{format:?}: {error}"));
        }

        assert_eq!(*validity.lock().expect("mutex"), [false, true, true], "{format:?}");
    }
}
