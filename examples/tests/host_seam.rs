#![cfg(feature = "gpu-tests")]

// M2-19: the generic-core seam. A raw wgpu host owns the device; pathon adopts it and renders into the host's texture
// (mode B, must equal mode C pixel for pixel) or records into the host's encoder on top of the host's image (mode A).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pathon::api::Camera;
use pathon::api::glam::Vec3;
use pathon::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, Color, CommandList, Device, DeviceApi, Extent3d, Format, PhysicalSize,
    TexelLayout, TextureCopy,
};
use pathon::{FrameInput, Renderer, TargetId, ViewInput};
use pathon_examples::block_on_ready;
use pathon_rhi_wgpu::interop;
use pathon_test::UnlitMeshPack;

const SIZE: u32 = 64;
const SCENE_BACKGROUND: [u8; 4] = [0, 0, 255, 255];
const HOST_IMAGE: [u8; 4] = [255, 0, 0, 255];

fn frame(target: TargetId) -> FrameInput {
    let camera = Camera::look_at(Vec3::new(0.0, 0.0, 2.5), Vec3::ZERO, std::f32::consts::FRAC_PI_3, 1.0);
    FrameInput {
        views: vec![ViewInput { target, camera }],
        time_seconds: 0.4,
        ..FrameInput::default()
    }
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let start = ((y * SIZE + x) * 4) as usize;
    pixels[start..start + 4].try_into().expect("four bytes")
}

struct Host {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    color: wgpu::Texture,
}

impl Host {
    fn new() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on_ready(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let (device, queue) =
            block_on_ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("host color"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            instance,
            adapter,
            device,
            queue,
            color,
        }
    }

    fn clear(&self, rgba: [u8; 4]) -> wgpu::CommandEncoder {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let view = self.color.create_view(&wgpu::TextureViewDescriptor::default());
        let channel = |value: u8| f64::from(value) / 255.0;
        let color = wgpu::Color {
            r: channel(rgba[0]),
            g: channel(rgba[1]),
            b: channel(rgba[2]),
            a: channel(rgba[3]),
        };
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("host image"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        }));
        encoder
    }

    fn read(&self) -> Vec<u8> {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("host readback"),
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: Some(SIZE),
                },
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.expect("map"));
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("poll");
        buffer.slice(..).get_mapped_range().expect("mapped").to_vec()
    }

    fn adopt_into_renderer(&self) -> (Renderer, TargetId) {
        let adopted = interop::adopt(
            self.instance.clone(),
            self.adapter.clone(),
            self.device.clone(),
            self.queue.clone(),
        );
        let texture = adopted
            .import_texture("host color", self.color.clone())
            .expect("import host texture");
        let device = Arc::new(Device::Wgpu(adopted));
        let mut renderer = Renderer::builder()
            .with_device(device)
            .with_feature(UnlitMeshPack::new())
            .build()
            .expect("renderer");
        let target = renderer
            .import_texture(texture, Access::COLOR_ATTACHMENT, Access::COPY_SRC)
            .expect("target");
        (renderer, target)
    }
}

fn render_mode_c() -> Vec<u8> {
    let mut renderer = Renderer::builder()
        .with_clear_color(Color::rgba(0.0, 0.0, 1.0, 1.0))
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba8Unorm)
        .expect("target");
    renderer.render_frame(&frame(target)).expect("frame");

    let device = renderer.device();
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: u64::from(SIZE * SIZE * 4),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("buffer");
    let mut list = CommandList::new();
    list.copy_texture_to_buffer(
        TextureCopy::whole(renderer.target_texture(target).expect("texture")),
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

#[test]
fn mode_b_on_the_host_device_matches_mode_c() {
    let host = Host::new();
    host.queue.submit([host.clear(SCENE_BACKGROUND).finish()]);
    let (mut renderer, target) = host.adopt_into_renderer();
    renderer
        .render_frame(&frame(target))
        .expect("pathon renders into the host texture");
    let mode_b = host.read();

    let mode_c = render_mode_c();
    assert_ne!(
        pixel(&mode_c, SIZE / 2, SIZE / 2),
        SCENE_BACKGROUND,
        "the scene draws something"
    );
    assert!(
        mode_b == mode_c,
        "mode B must be pixel-identical to mode C on the same backend"
    );
}

#[test]
fn mode_a_draws_on_top_of_the_host_image_in_the_host_encoder() {
    let host = Host::new();
    let (mut renderer, target) = host.adopt_into_renderer();
    let commands = renderer.record_frame(&frame(target)).expect("record");

    let mut encoder = host.clear(HOST_IMAGE);
    let wgpu_device = renderer.device().as_wgpu().expect("wgpu backend");
    wgpu_device
        .encode_into(commands, &mut encoder)
        .expect("encode into the host encoder");
    host.queue.submit([encoder.finish()]);
    let pixels = host.read();

    assert_eq!(pixel(&pixels, 0, 0), HOST_IMAGE, "pathon keeps what the host rendered");
    assert_ne!(
        pixel(&pixels, SIZE / 2, SIZE / 2),
        HOST_IMAGE,
        "pathon's pass drew on top"
    );
}

fn counter() -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    (Arc::clone(&count), count)
}

#[test]
fn adopting_keeps_the_host_error_handler() {
    let host = Host::new();
    let (errors, counted) = counter();
    host.device.on_uncaptured_error(Arc::new(move |_: wgpu::Error| {
        counted.fetch_add(1, Ordering::SeqCst);
    }));
    let _pathon = host.adopt_into_renderer();

    drop(host.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("invalid on purpose"),
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::MAP_WRITE,
        mapped_at_creation: false,
    }));
    assert_eq!(errors.load(Ordering::SeqCst), 1);
}

#[test]
fn adopting_keeps_the_host_device_lost_callback() {
    let host = Host::new();
    let (losses, counted) = counter();
    host.device.set_device_lost_callback(move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    let _pathon = host.adopt_into_renderer();

    host.device.destroy();
    let _ = host.device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    assert_eq!(losses.load(Ordering::SeqCst), 1);
}
