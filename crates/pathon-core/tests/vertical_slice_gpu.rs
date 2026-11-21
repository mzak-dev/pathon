#![cfg(feature = "gpu-tests")]

use pathon_api::Camera;
use pathon_api::glam::Vec3;
use pathon_core::{FrameInput, Renderer, ViewInput};
use pathon_rhi::{
    BufferCopy, BufferDesc, BufferUsage, Color, CommandList, DeviceApi, Extent3d, Format, PhysicalSize, TexelLayout,
    TextureCopy,
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
