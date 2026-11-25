#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test images are small and their coordinates fit every integer type"
)]

use std::sync::{Arc, Mutex};

use pathon_api::glam::{Vec3, Vec4};
use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, Backend, BindGroupLayoutDesc, BufferCopy, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment,
    ColorTargetState, DepthAttachment, DepthState, DeviceApi, DeviceRequest, Extent3d, Format, PhysicalSize,
    PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError, ShaderModuleDesc, ShaderStage,
    TexelLayout, TextureCopy,
};
use pathon_api::{Camera, RecordCtx, RenderFeature, SetupCtx, Severity, Slot};
use pathon_core::{FrameInput, FrameReport, RenderError, Renderer, TargetId, ViewInput};
use pathon_golden::{GoldenImage, announce_adapter, gpu_or_skip};
use pathon_ibl::IblPack;
use pathon_sky::{MULTISCATTER_LUT, SKY_VIEW_LUT, SkyPack, SkyQuality, SkySettings, TRANSMITTANCE_LUT};

const ROW_ALIGNMENT: u32 = 256;

#[derive(Clone, Debug, Default)]
pub struct SharedBuffer(Arc<Mutex<Option<BufferId>>>);

impl SharedBuffer {
    fn set(&self, buffer: BufferId) {
        *self.0.lock().expect("mutex") = Some(buffer);
    }

    fn get(&self) -> BufferId {
        self.0.lock().expect("mutex").expect("the feature ran setup")
    }
}

/// Copies a slot to a buffer the test reads after the frame.
#[derive(Debug)]
struct Probe {
    slot: Slot,
    order: i32,
    size: (u32, u32),
    buffer: SharedBuffer,
}

impl Probe {
    fn padded_row(&self) -> u32 {
        (self.size.0 * 8).next_multiple_of(ROW_ALIGNMENT)
    }
}

impl RenderFeature for Probe {
    fn name(&self) -> &'static str {
        self.slot.name()
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let buffer = ctx.device().create_buffer(&BufferDesc {
            label: "probe",
            size: u64::from(self.padded_row() * self.size.1),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })?;
        self.buffer.set(buffer);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(handle) = ctx.slot(self.slot) else {
            return Ok(());
        };
        let (buffer, size, bytes_per_row) = (self.buffer.get(), self.size, self.padded_row());
        let mut pass = ctx.graph().add_pass(
            PassDesc::new(self.slot.name(), Stage::Overlay)
                .priority(self.order)
                .side_effect(),
        );
        let handle = pass.read(handle, Access::COPY_SRC);
        pass.record(move |pass| {
            let texture = pass.texture(handle);
            pass.commands().copy_texture_to_buffer(
                TextureCopy::whole(texture),
                BufferCopy {
                    buffer,
                    layout: TexelLayout {
                        offset: 0,
                        bytes_per_row,
                        rows_per_image: size.1,
                    },
                },
                Extent3d::new_2d(size.0, size.1),
            );
            Ok(())
        });
        Ok(())
    }
}

const WALL_WGSL: &str = r"
@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // The left half of the screen as two triangles, at depth 0.5.
    let x = array<f32, 6>(-1.0, 0.0, -1.0, -1.0, 0.0, 0.0);
    let y = array<f32, 6>(-1.0, -1.0, 1.0, 1.0, -1.0, 1.0);
    return vec4<f32>(x[index], y[index], 0.5, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(0.25, 0.25, 0.25, 1.0);
}
";

/// Geometry for the depth test of the sky: a grey wall over the left half of the picture, writing `hdr` and `depth`.
#[derive(Debug, Default)]
struct Wall {
    pipeline: Option<RenderPipelineId>,
}

impl RenderFeature for Wall {
    fn name(&self) -> &'static str {
        "wall"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "wall",
            wgsl: WALL_WGSL,
        })?;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "wall",
            entries: vec![],
        })?;
        self.pipeline = Some(device.create_render_pipeline(&RenderPipelineDesc {
            label: "wall",
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
            color_targets: vec![ColorTargetState::opaque(Format::Rgba16Float)],
            primitive: PrimitiveState::default(),
            depth: Some(DepthState::reverse_z(Format::Depth32Float)),
            sample_count: 1,
            immediate_size: 0,
        })?);
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let pipeline = self.pipeline.expect("setup ran");
        let hdr = ctx
            .create_slot_texture(Slot::HDR, "wall hdr")
            .expect("hdr has a format");
        let depth = ctx
            .create_slot_texture(Slot::DEPTH, "wall depth")
            .expect("depth has a format");
        let mut pass = ctx.graph().add_pass(PassDesc::new("wall", Stage::Opaque));
        let hdr = pass.write(hdr, Access::COLOR_ATTACHMENT);
        let depth = pass.write(depth, Access::DEPTH_WRITE);
        pass.record(move |pass| {
            let (hdr_view, depth_view) = (pass.view(hdr)?, pass.view(depth)?);
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "wall",
                color: vec![ColorAttachment::clear(hdr_view, Color::BLACK)],
                depth: Some(DepthAttachment::clear_reverse_z(depth_view)),
                timestamps: None,
            });
            render.set_pipeline(pipeline);
            render.draw(0..6, 0..1);
            Ok(())
        });
        ctx.set_slot(Slot::HDR, hdr);
        ctx.set_slot(Slot::DEPTH, depth);
        Ok(())
    }
}

pub struct Options {
    pub size: (u32, u32),
    pub quality: SkyQuality,
    pub settings: SkySettings,
    pub with_ibl: bool,
    pub with_wall: bool,
    pub null_device: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            size: (128, 96),
            quality: SkyQuality::DESKTOP,
            settings: SkySettings::default(),
            with_ibl: false,
            with_wall: false,
            null_device: false,
        }
    }
}

pub struct Harness {
    pub renderer: Renderer,
    target: TargetId,
    size: (u32, u32),
    quality: SkyQuality,
    probes: Vec<(Slot, (u32, u32), SharedBuffer)>,
    pub time_seconds: f64,
}

impl Harness {
    /// `None` (the test is skipped) when the machine has no GPU adapter.
    pub fn new(test: &str, options: Options) -> Option<Self> {
        let probed = [
            (Slot::HDR, options.size),
            (TRANSMITTANCE_LUT, options.quality.transmittance_size),
            (
                MULTISCATTER_LUT,
                (options.quality.multiscatter_size, options.quality.multiscatter_size),
            ),
            (SKY_VIEW_LUT, options.quality.sky_view_size),
        ];
        let probes: Vec<_> = probed
            .into_iter()
            .map(|(slot, size)| (slot, size, SharedBuffer::default()))
            .collect();
        let mut builder = Renderer::builder();
        if options.null_device {
            builder = builder.with_device_request(DeviceRequest {
                backend: Some(Backend::Null),
                ..DeviceRequest::default()
            });
        }
        if options.with_wall {
            builder = builder.with_feature(Wall::default());
        }
        if options.with_ibl {
            builder = builder.with_pack(IblPack::new());
        }
        builder = builder.with_pack(
            SkyPack::new()
                .with_settings(options.settings)
                .with_quality(options.quality),
        );
        for (order, (slot, size, buffer)) in (0..).zip(&probes) {
            builder = builder.with_feature(Probe {
                slot: *slot,
                order,
                size: *size,
                buffer: buffer.clone(),
            });
        }
        let mut renderer = if options.null_device {
            builder.build().expect("renderer on the null device")
        } else {
            let renderer = gpu_or_skip(test, builder.build(), |error| {
                matches!(error, RenderError::Rhi(RhiError::NoAdapter(_)))
            })?;
            announce_adapter(test, renderer.device().caps());
            renderer
        };
        let target = renderer
            .add_offscreen_target(PhysicalSize::new(options.size.0, options.size.1), Format::Rgba8Unorm)
            .expect("target");
        Some(Self {
            renderer,
            target,
            size: options.size,
            quality: options.quality,
            probes,
            time_seconds: 0.0,
        })
    }

    pub fn set_settings(&mut self, settings: SkySettings) {
        self.renderer.shared_mut().insert(settings);
    }

    /// Renders one frame and asserts that nothing reported an error.
    pub fn render(&mut self, camera: Camera) -> FrameReport {
        let input = FrameInput {
            views: vec![ViewInput {
                target: self.target,
                camera,
            }],
            time_seconds: self.time_seconds,
            delta_seconds: 1.0 / 60.0,
        };
        let report = self.renderer.render_frame(&input).expect("frame");
        let errors: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        report
    }

    pub fn camera(&self) -> Camera {
        camera_looking(0.0, 0.0, self.size)
    }

    /// The texels of a probed slot, row by row, as RGBA.
    pub fn texels(&self, slot: Slot) -> (Vec<[f32; 4]>, (u32, u32)) {
        let (_, size, buffer) = self
            .probes
            .iter()
            .find(|(probed, _, _)| *probed == slot)
            .expect("probed slot");
        let bytes = self.renderer.device().read_buffer(buffer.get()).expect("read");
        let row = (size.0 * 8).next_multiple_of(ROW_ALIGNMENT) as usize;
        let mut texels = Vec::with_capacity((size.0 * size.1) as usize);
        for y in 0..size.1 as usize {
            for x in 0..size.0 as usize {
                let at = y * row + x * 8;
                let channel =
                    |index: usize| f16_to_f32(u16::from_le_bytes([bytes[at + index * 2], bytes[at + index * 2 + 1]]));
                texels.push([channel(0), channel(1), channel(2), channel(3)]);
            }
        }
        (texels, *size)
    }

    pub fn hdr(&self) -> Vec<[f32; 4]> {
        self.texels(Slot::HDR).0
    }

    pub fn quality(&self) -> SkyQuality {
        self.quality
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * fraction * 2.0_f32.powi(-24),
        31 if fraction == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent - 15),
    }
}

/// A camera 1.7 m above the ground looking towards `-Z` turned by `yaw_degrees` (positive to the right) and tilted up
/// by `pitch_degrees`, with a 70 degree vertical field of view.
pub fn camera_looking(yaw_degrees: f32, pitch_degrees: f32, size: (u32, u32)) -> Camera {
    let (yaw, pitch) = (yaw_degrees.to_radians(), pitch_degrees.to_radians());
    let direction = Vec3::new(pitch.cos() * yaw.sin(), pitch.sin(), -pitch.cos() * yaw.cos());
    let position = Vec3::new(0.0, 1.7, 0.0);
    Camera::look_at(
        position,
        position + direction,
        70.0_f32.to_radians(),
        size.0 as f32 / size.1 as f32,
    )
}

/// World direction of the centre of a pixel, as the sky shader computes it.
pub fn pixel_direction(camera: &Camera, size: (u32, u32), x: u32, y: u32) -> [f64; 3] {
    let inverse = camera.view_projection().inverse();
    let ndc_x = (x as f32 + 0.5) / size.0 as f32 * 2.0 - 1.0;
    let ndc_y = 1.0 - (y as f32 + 0.5) / size.1 as f32 * 2.0;
    let unproject = |depth: f32| {
        let point = inverse * Vec4::new(ndc_x, ndc_y, depth, 1.0);
        point.truncate() / point.w
    };
    let direction = (unproject(0.5) - unproject(1.0)).normalize();
    [f64::from(direction.x), f64::from(direction.y), f64::from(direction.z)]
}

/// Display value of a linear radiance: exposure for `ev100`, a Reinhard curve and the sRGB transfer.
pub fn display(radiance: [f32; 3], ev100: f32) -> [u8; 3] {
    let exposure = 1.0 / (1.2 * ev100.exp2());
    radiance.map(|value| {
        let mapped = (value * exposure).max(0.0);
        let mapped = mapped / (1.0 + mapped);
        let encoded = if mapped <= 0.003_130_8 {
            mapped * 12.92
        } else {
            1.055 * mapped.powf(1.0 / 2.4) - 0.055
        };
        (encoded.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    })
}

pub fn golden_image(texels: &[[f32; 4]], size: (u32, u32), ev100: f32) -> GoldenImage {
    let mut rgba = Vec::with_capacity(texels.len() * 4);
    for texel in texels {
        let [red, green, blue] = display([texel[0], texel[1], texel[2]], ev100);
        rgba.extend([red, green, blue, 255]);
    }
    GoldenImage::from_rgba8(size.0, size.1, rgba).expect("image")
}

pub fn all_finite(texels: &[[f32; 4]]) -> bool {
    texels.iter().all(|texel| texel.iter().all(|value| value.is_finite()))
}

pub fn radiance(texels: &[[f32; 4]], size: (u32, u32), x: u32, y: u32) -> [f64; 3] {
    let texel = texels[(y * size.0 + x) as usize];
    [f64::from(texel[0]), f64::from(texel[1]), f64::from(texel[2])]
}

/// One mip of one layer of an `Rgba16Float` texture as RGBA texels; the subresource ends in the state it started in.
pub fn read_rgba16f(
    device: &dyn DeviceApi,
    texture: pathon_api::rhi::TextureId,
    state: Access,
    mip: u32,
    layer: u32,
) -> ((u32, u32), Vec<[f32; 4]>) {
    use pathon_api::rhi::{CommandList, Origin3d, SubresourceRange, TextureAspect};

    let desc = device.texture_desc(texture).expect("texture");
    let size = desc.size.mip_level(mip);
    let packed_row = size.width * 8;
    let padded_row = packed_row.next_multiple_of(ROW_ALIGNMENT);
    let buffer = device
        .create_buffer(&BufferDesc {
            label: "mip readback",
            size: u64::from(padded_row * size.height),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("buffer");
    let range = SubresourceRange {
        base_mip: mip,
        mip_count: 1,
        base_layer: layer,
        layer_count: 1,
    };
    let mut list = CommandList::new();
    list.texture_access(texture, range, state, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy {
            texture,
            mip,
            origin: Origin3d { x: 0, y: 0, z: layer },
            aspect: TextureAspect::All,
        },
        BufferCopy {
            buffer,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: padded_row,
                rows_per_image: size.height,
            },
        },
        Extent3d::new_2d(size.width, size.height),
    );
    list.texture_access(texture, range, Access::COPY_SRC, state);
    device.submit(list).expect("submit");
    let padded = device.read_buffer(buffer).expect("read");
    let mut texels = Vec::new();
    for row in padded.chunks(padded_row as usize).take(size.height as usize) {
        for texel in row[..packed_row as usize].as_chunks::<8>().0 {
            let channel = |index: usize| f16_to_f32(u16::from_le_bytes([texel[index * 2], texel[index * 2 + 1]]));
            texels.push([channel(0), channel(1), channel(2), channel(3)]);
        }
    }
    ((size.width, size.height), texels)
}
