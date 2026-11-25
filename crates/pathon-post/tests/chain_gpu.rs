#![cfg(feature = "gpu-tests")]
#![expect(
    clippy::cast_possible_truncation,
    reason = "test images are small and their coordinates fit every integer type"
)]

use std::sync::{Arc, Mutex};

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, BufferCopy, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment,
    ColorTargetState, DeviceApi, Extent3d, Format, OverrideValue, PhysicalSize, PrimitiveState, RenderPassDesc,
    RenderPipelineDesc, RenderPipelineId, RhiError, ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess,
    TexelLayout, TextureCopy,
};
use pathon_api::{Camera, RecordCtx, RenderFeature, SetupCtx, Slot};
use pathon_core::{FrameInput, RenderError, Renderer, TargetId, ViewInput};
use pathon_golden::{GoldenImage, Tolerance, announce_adapter, assert_golden, gpu_or_skip, read_texture};
use pathon_post::reference::{self, HistogramRange};
use pathon_post::{
    AutoExposure, BLOOM, BloomSettings, ExposureMode, ExposureSettings, PostPack, PostSettings, TonemapSettings,
    Tonemapper,
};

const ROW_ALIGNMENT: u32 = 256;

// ---- test support: a source that writes `hdr` from the CPU and probes that copy any slot back ----

const SOURCE_WGSL: &str = r"
override WIDTH: u32 = 1u;
@group(0) @binding(0) var<storage, read> pixels: array<vec4<f32>>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return pixels[u32(position.y) * WIDTH + u32(position.x)];
}
";

#[derive(Clone, Debug, Default)]
struct SharedBuffer(Arc<Mutex<Option<BufferId>>>);

impl SharedBuffer {
    fn set(&self, buffer: BufferId) {
        *self.0.lock().expect("mutex") = Some(buffer);
    }

    fn get(&self) -> BufferId {
        self.0.lock().expect("mutex").expect("the feature ran setup")
    }
}

#[derive(Debug)]
struct HdrSource {
    size: (u32, u32),
    pixels: Vec<[f32; 4]>,
    buffer: SharedBuffer,
    objects: Option<(BindGroupLayoutId, RenderPipelineId)>,
}

impl RenderFeature for HdrSource {
    fn name(&self) -> &'static str {
        "hdr source"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let buffer = device.create_buffer(&BufferDesc {
            label: "hdr source pixels",
            size: u64::from(self.size.0 * self.size.1) * 16,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        })?;
        device.write_buffer(buffer, 0, bytemuck::cast_slice(&self.pixels))?;
        self.buffer.set(buffer);
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "hdr source",
            wgsl: SOURCE_WGSL,
        })?;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "hdr source",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::StorageBuffer {
                    access: StorageAccess::ReadOnly,
                },
                count: None,
            }],
        })?;
        let pipeline = device.create_render_pipeline(&RenderPipelineDesc {
            label: "hdr source",
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
                constants: vec![("WIDTH", OverrideValue::from(self.size.0))],
            }),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba16Float)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.objects = Some((layout, pipeline));
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let (layout, pipeline) = self.objects.expect("setup ran");
        let buffer = self.buffer.get();
        let hdr = ctx
            .create_slot_texture(Slot::HDR, "synthetic hdr")
            .expect("hdr has a format");
        let mut pass = ctx.graph().add_pass(PassDesc::new("hdr source", Stage::Opaque));
        let hdr = pass.write(hdr, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(hdr)?;
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "hdr source",
                layout,
                entries: vec![BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer {
                        buffer,
                        offset: 0,
                        size: None,
                    },
                }],
            })?;
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "hdr source",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
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

#[derive(Debug)]
struct Probe {
    slot: Slot,
    size: (u32, u32),
    texel_bytes: u32,
    buffer: SharedBuffer,
}

impl Probe {
    fn padded_row(&self) -> u32 {
        (self.size.0 * self.texel_bytes).next_multiple_of(ROW_ALIGNMENT)
    }
}

impl RenderFeature for Probe {
    fn name(&self) -> &'static str {
        "probe"
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
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("probe", Stage::Overlay).side_effect());
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

fn half_to_f32(bits: u16) -> f32 {
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

struct Chain {
    renderer: Renderer,
    target: TargetId,
    source: SharedBuffer,
    exposure_probe: (SharedBuffer, Probe),
    bloom_probe: (SharedBuffer, Probe),
}

fn gradient(width: u32, height: u32, value: impl Fn(u32, u32) -> [f32; 3]) -> Vec<[f32; 4]> {
    (0..width * height)
        .map(|index| {
            let [r, g, b] = value(index % width, index / width);
            [r, g, b, 1.0]
        })
        .collect()
}

fn uniform(width: u32, height: u32, value: f32) -> Vec<[f32; 4]> {
    gradient(width, height, |_, _| [value; 3])
}

fn chain(
    test: &str,
    size: (u32, u32),
    pixels: Vec<[f32; 4]>,
    settings: PostSettings,
    target_format: Format,
) -> Option<Chain> {
    let source = SharedBuffer::default();
    let exposure_buffer = SharedBuffer::default();
    let bloom_buffer = SharedBuffer::default();
    let bloom_size = (size.0.div_ceil(2), size.1.div_ceil(2));
    let built = Renderer::builder()
        .with_feature(HdrSource {
            size,
            pixels,
            buffer: source.clone(),
            objects: None,
        })
        .with_pack(PostPack::with_settings(settings))
        .with_feature(Probe {
            slot: Slot::EXPOSURE,
            size: (1, 1),
            texel_bytes: 4,
            buffer: exposure_buffer.clone(),
        })
        .with_feature(Probe {
            slot: BLOOM,
            size: bloom_size,
            texel_bytes: 8,
            buffer: bloom_buffer.clone(),
        })
        .build();
    let mut renderer = gpu_or_skip(test, built, |error| {
        matches!(error, RenderError::Rhi(RhiError::NoAdapter(_)))
    })?;
    announce_adapter(test, renderer.device().caps());
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(size.0, size.1), target_format)
        .expect("target");
    let probe = |slot, size, texel_bytes, buffer: &SharedBuffer| {
        (
            buffer.clone(),
            Probe {
                slot,
                size,
                texel_bytes,
                buffer: buffer.clone(),
            },
        )
    };
    Some(Chain {
        renderer,
        target,
        source,
        exposure_probe: probe(Slot::EXPOSURE, (1, 1), 4, &exposure_buffer),
        bloom_probe: probe(BLOOM, bloom_size, 8, &bloom_buffer),
    })
}

impl Chain {
    fn render(&mut self, delta_seconds: f32) {
        let input = FrameInput {
            views: vec![ViewInput {
                target: self.target,
                camera: Camera::default(),
            }],
            delta_seconds,
            ..FrameInput::default()
        };
        let report = self.renderer.render_frame(&input).expect("frame");
        let errors: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == pathon_api::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    fn set_pixels(&self, pixels: &[[f32; 4]]) {
        self.renderer
            .device()
            .write_buffer(self.source.get(), 0, bytemuck::cast_slice(pixels))
            .expect("upload");
    }

    fn exposure(&self) -> f32 {
        let bytes = self
            .renderer
            .device()
            .read_buffer(self.exposure_probe.0.get())
            .expect("read");
        f32::from_le_bytes(bytes[..4].try_into().expect("four bytes"))
    }

    fn bloom(&self) -> Vec<[f32; 3]> {
        let (buffer, probe) = &self.bloom_probe;
        let bytes = self.renderer.device().read_buffer(buffer.get()).expect("read");
        let row = probe.padded_row() as usize;
        let mut texels = Vec::new();
        for y in 0..probe.size.1 as usize {
            for x in 0..probe.size.0 as usize {
                let at = y * row + x * 8;
                let channel =
                    |index: usize| half_to_f32(u16::from_le_bytes([bytes[at + index * 2], bytes[at + index * 2 + 1]]));
                texels.push([channel(0), channel(1), channel(2)]);
            }
        }
        texels
    }

    fn output(&self) -> GoldenImage {
        let texture = self.renderer.target_texture(self.target).expect("offscreen texture");
        read_texture(&**self.renderer.device(), texture, Access::SAMPLED).expect("readback")
    }

    fn output_pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let [red, green, blue, _] = self.output().pixel(x, y);
        [red, green, blue]
    }
}

fn manual(ev: f32) -> PostSettings {
    PostSettings {
        exposure: ExposureSettings {
            mode: ExposureMode::Manual { ev },
            compensation_ev: 0.0,
        },
        bloom: BloomSettings {
            enabled: false,
            ..BloomSettings::default()
        },
        ..PostSettings::default()
    }
}

fn close_bytes(actual: [u8; 3], expected: [u8; 3], tolerance: u8) -> bool {
    actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= tolerance)
}

// ---- tonemapping against the CPU reference ----

fn tonemap_samples() -> Vec<[f32; 3]> {
    let mut samples = vec![[0.0; 3], [0.18; 3], [1.0; 3], [4.0; 3], [100.0; 3]];
    for level in [0.002, 0.02, 0.1, 0.5, 2.0, 8.0] {
        samples.extend([
            [level, level * 0.5, level * 0.1],
            [level * 0.1, level, level * 0.3],
            [level * 0.2, level * 0.2, level],
        ]);
    }
    samples
}

fn check_tonemapper(operator: Tonemapper, format: Format, test: &str) {
    let samples = tonemap_samples();
    let width = samples.len() as u32;
    let pixels = samples.iter().map(|[r, g, b]| [*r, *g, *b, 1.0]).collect();
    let mut settings = manual(0.0);
    settings.tonemap = TonemapSettings {
        operator,
        ..TonemapSettings::default()
    };
    let Some(mut chain) = chain(test, (width, 1), pixels, settings, format) else {
        return;
    };

    chain.render(1.0 / 60.0);

    let image = chain.output();
    for (x, sample) in samples.iter().enumerate() {
        let at = x * 4;
        let actual: [u8; 3] = image.as_rgba8()[at..at + 3].try_into().expect("three bytes");
        let expected = reference::display_rgb8(operator, *sample);
        assert!(
            close_bytes(actual, expected, 2),
            "{operator:?} {format:?} {sample:?}: gpu {actual:?}, reference {expected:?}"
        );
    }
}

#[test]
fn aces_fitted_matches_the_cpu_reference() {
    check_tonemapper(
        Tonemapper::AcesFitted,
        Format::Rgba8Unorm,
        "aces_fitted_matches_the_cpu_reference",
    );
}

#[test]
fn pbr_neutral_matches_the_cpu_reference() {
    check_tonemapper(
        Tonemapper::PbrNeutral,
        Format::Rgba8Unorm,
        "pbr_neutral_matches_the_cpu_reference",
    );
}

#[test]
fn srgb_target_gets_the_same_encoding_from_the_hardware() {
    check_tonemapper(
        Tonemapper::AcesFitted,
        Format::Rgba8UnormSrgb,
        "srgb_target_gets_the_same_encoding_from_the_hardware",
    );
}

#[test]
fn bgra_target_keeps_the_channel_order() {
    check_tonemapper(
        Tonemapper::PbrNeutral,
        Format::Bgra8Unorm,
        "bgra_target_keeps_the_channel_order",
    );
}

// ---- exposure ----

#[test]
fn manual_exposure_is_two_to_the_minus_ev_with_compensation() {
    let mut settings = manual(2.0);
    settings.exposure.compensation_ev = 1.0;
    let Some(mut chain) = chain(
        "manual_exposure_is_two_to_the_minus_ev_with_compensation",
        (8, 8),
        uniform(8, 8, 0.5),
        settings,
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    chain.render(1.0 / 60.0);

    assert!((chain.exposure() - 0.5).abs() < 1.0e-6, "{}", chain.exposure());
}

fn auto_settings() -> PostSettings {
    let mut settings = PostSettings::default();
    settings.exposure.mode = ExposureMode::Auto(AutoExposure {
        to_brighter_seconds: 0.2,
        to_darker_seconds: 0.5,
        ..AutoExposure::default()
    });
    settings.bloom.enabled = false;
    settings
}

#[test]
fn auto_exposure_brings_a_grey_card_to_the_target_at_once() {
    let Some(mut chain) = chain(
        "auto_exposure_brings_a_grey_card_to_the_target_at_once",
        (32, 32),
        uniform(32, 32, 0.18),
        auto_settings(),
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    chain.render(1.0 / 60.0);

    let error_ev = chain.exposure().log2().abs();
    assert!(
        error_ev < 0.1,
        "a 0.18 card needs exposure 1, got {} ({error_ev} EV off)",
        chain.exposure()
    );
}

#[test]
fn auto_exposure_adapts_over_time_and_settles_on_the_target() {
    let dark = 0.18 / 16.0;
    let bright = 0.18 * 16.0;
    let Some(mut chain) = chain(
        "auto_exposure_adapts_over_time_and_settles_on_the_target",
        (32, 32),
        uniform(32, 32, dark),
        auto_settings(),
        Format::Rgba8Unorm,
    ) else {
        return;
    };
    let ExposureMode::Auto(auto) = auto_settings().exposure.mode else {
        unreachable!("auto settings");
    };
    let range = HistogramRange {
        min_ev: auto.min_ev,
        max_ev: auto.max_ev,
        low_percentile: auto.low_percentile,
        high_percentile: auto.high_percentile,
    };
    let delta = 1.0 / 60.0;

    chain.render(delta);
    let first_ev = chain.exposure().log2();
    assert!((first_ev - 4.0).abs() < 0.1, "dark scene wants +4 EV, got {first_ev}");

    chain.set_pixels(&uniform(32, 32, bright));
    let target_log2 =
        0.18_f32.log2() - reference::average_log2_luminance(&vec![[bright; 3]; 1024], range).expect("lit");
    let mut expected_log2 = first_ev;
    let brighter = reference::adaptation_coefficient(delta, auto.to_brighter_seconds);
    let darker = reference::adaptation_coefficient(delta, auto.to_darker_seconds);
    for frame in 0..180 {
        chain.render(delta);
        expected_log2 = reference::adapt_log2_exposure(Some(expected_log2), target_log2, brighter, darker);
        let actual = chain.exposure().log2();
        assert!(
            (actual - expected_log2).abs() < 0.05,
            "frame {frame}: gpu {actual} EV, model {expected_log2} EV"
        );
    }

    let settled = chain.exposure().log2();
    assert!(
        (settled - (-4.0)).abs() < 0.1,
        "bright scene wants -4 EV, got {settled}"
    );
}

#[test]
fn auto_exposure_does_not_move_without_elapsed_time() {
    let Some(mut chain) = chain(
        "auto_exposure_does_not_move_without_elapsed_time",
        (16, 16),
        uniform(16, 16, 0.18),
        auto_settings(),
        Format::Rgba8Unorm,
    ) else {
        return;
    };
    chain.render(1.0 / 60.0);
    let before = chain.exposure();
    chain.set_pixels(&uniform(16, 16, 10.0));

    chain.render(0.0);

    assert!((chain.exposure() - before).abs() < 1.0e-6);
}

// ---- bloom ----

// A 16x16 square in the middle of a black 64x64 frame.
fn blob_scene(value: f32) -> Vec<[f32; 4]> {
    gradient(64, 64, |x, y| {
        let inside = (24..40).contains(&x) && (24..40).contains(&y);
        [if inside { value } else { 0.0 }; 3]
    })
}

fn bloom_energy_settings(karis: bool) -> PostSettings {
    let mut settings = manual(0.0);
    settings.bloom = BloomSettings {
        enabled: true,
        threshold: 0.0,
        knee: 0.0,
        scatter: 0.6,
        levels: 6,
        karis_average: karis,
        ..BloomSettings::default()
    };
    settings
}

#[test]
fn bloom_keeps_the_mean_energy_of_its_input() {
    let value = 4.0;
    let Some(mut chain) = chain(
        "bloom_keeps_the_mean_energy_of_its_input",
        (64, 64),
        blob_scene(value),
        bloom_energy_settings(false),
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    chain.render(1.0 / 60.0);

    let bloom = chain.bloom();
    let input_mean = value * 256.0 / 4096.0;
    let bloom_mean = bloom.iter().map(|texel| texel[0]).sum::<f32>() / bloom.len() as f32;
    assert!(
        (bloom_mean / input_mean - 1.0).abs() < 0.05,
        "input mean {input_mean}, bloom mean {bloom_mean}"
    );
    let center = bloom[16 * 32 + 16][0];
    let corner = bloom[0][0];
    assert!(
        center > corner * 2.0,
        "glow is brightest at the source: centre {center}, corner {corner}"
    );
}

#[test]
fn bloom_with_firefly_suppression_never_adds_energy() {
    let Some(mut chain) = chain(
        "bloom_with_firefly_suppression_never_adds_energy",
        (64, 64),
        blob_scene(500.0),
        bloom_energy_settings(true),
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    chain.render(1.0 / 60.0);

    let bloom = chain.bloom();
    let mean = bloom.iter().map(|texel| texel[0]).sum::<f32>() / bloom.len() as f32;
    assert!(
        mean.is_finite() && mean > 0.0 && mean <= 500.0 * 256.0 / 4096.0 * 1.05,
        "{mean}"
    );
}

#[test]
fn bloom_threshold_keeps_dark_scenes_dark() {
    let mut settings = bloom_energy_settings(false);
    settings.bloom.threshold = 1.0;
    let Some(mut chain) = chain(
        "bloom_threshold_keeps_dark_scenes_dark",
        (64, 64),
        uniform(64, 64, 0.5),
        settings,
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    chain.render(1.0 / 60.0);

    assert!(chain.bloom().iter().all(|texel| texel[0] < 1.0e-6));
}

// ---- hostile input ----

fn hostile_scene() -> Vec<[f32; 4]> {
    let mut pixels = uniform(32, 32, 0.18);
    pixels[5 * 32 + 5] = [f32::NAN; 4];
    pixels[5 * 32 + 20] = [f32::INFINITY; 4];
    pixels[20 * 32 + 5] = [f32::NEG_INFINITY, -3.0, 0.0, 1.0];
    pixels[20 * 32 + 20] = [1.0e30; 4];
    pixels
}

#[test]
fn broken_pixels_stay_local_and_never_leak_nan_or_inf() {
    let Some(mut chain) = chain(
        "broken_pixels_stay_local_and_never_leak_nan_or_inf",
        (32, 32),
        hostile_scene(),
        manual(0.0),
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    chain.render(1.0 / 60.0);

    let clean = reference::display_rgb8(Tonemapper::AcesFitted, [0.18; 3]);
    assert_eq!(chain.output_pixel(5, 5), [0, 0, 0], "NaN becomes black");
    assert!(chain.output_pixel(20, 5).iter().all(|c| *c >= 250), "+Inf saturates");
    assert_eq!(chain.output_pixel(5, 20), [0, 0, 0], "-Inf and negatives become black");
    assert!(
        chain.output_pixel(20, 20).iter().all(|c| *c >= 250),
        "huge values saturate"
    );
    for (x, y) in [(4, 5), (6, 5), (5, 4), (5, 6), (21, 20), (10, 10)] {
        assert!(
            close_bytes(chain.output_pixel(x, y), clean, 1),
            "neighbour ({x}, {y}) of a broken pixel was touched"
        );
    }
}

#[test]
fn bloom_and_auto_exposure_survive_broken_pixels() {
    let mut settings = auto_settings();
    settings.bloom.enabled = true;
    settings.bloom.threshold = 0.0;
    let Some(mut chain) = chain(
        "bloom_and_auto_exposure_survive_broken_pixels",
        (32, 32),
        hostile_scene(),
        settings,
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    for _ in 0..4 {
        chain.render(1.0 / 60.0);
    }

    let exposure = chain.exposure();
    assert!(exposure.is_finite() && exposure > 0.0, "{exposure}");
    assert!(
        (exposure.log2()).abs() < 0.2,
        "broken pixels must not decide the exposure: {exposure}"
    );
    for texel in chain.bloom() {
        assert!(texel.iter().all(|c| c.is_finite() && *c >= 0.0), "{texel:?}");
    }
}

#[test]
fn all_black_and_all_infinite_frames_stay_finite() {
    for (name, value) in [("zero", 0.0), ("infinity", f32::INFINITY), ("nan", f32::NAN)] {
        let Some(mut chain) = chain(
            "all_black_and_all_infinite_frames_stay_finite",
            (16, 16),
            uniform(16, 16, value),
            auto_settings(),
            Format::Rgba8Unorm,
        ) else {
            return;
        };

        chain.render(1.0 / 60.0);
        chain.render(1.0 / 60.0);

        let exposure = chain.exposure();
        assert!(exposure.is_finite() && exposure > 0.0, "{name}: {exposure}");
    }
}

// ---- golden images through the whole chain ----

const GOLDEN_SIZE: (u32, u32) = (128, 96);

// Top: a ramp over 16 stops around middle grey. Middle: colour bars at 1x and (lower half) 8x. Bottom: a dark
// field with a very bright spot and a dimmer one, which is what the bloom is for.
fn golden_scene() -> Vec<[f32; 4]> {
    const BARS: [[f32; 3]; 8] = [
        [1.0, 1.0, 1.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.05, 0.05, 0.05],
    ];
    let (width, height) = GOLDEN_SIZE;
    gradient(width, height, |x, y| match y {
        0..32 => {
            let stops = x as f32 / (width - 1) as f32 * 16.0 - 8.0;
            [0.18 * stops.exp2(); 3]
        }
        32..64 => {
            let scale = if y < 48 { 1.0 } else { 8.0 };
            BARS[(x / 16) as usize].map(|channel| channel * scale)
        }
        _ => {
            let distance = |cx: f32, cy: f32| ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if distance(40.0, 80.0) < 3.0 {
                [300.0, 280.0, 240.0]
            } else if distance(100.0, 80.0) < 5.0 {
                [6.0, 3.0, 1.0]
            } else {
                [0.02; 3]
            }
        }
    })
}

fn golden_settings(operator: Tonemapper) -> PostSettings {
    let mut settings = manual(0.0);
    settings.bloom = BloomSettings {
        enabled: true,
        intensity: 0.3,
        threshold: 1.0,
        knee: 0.5,
        scatter: 0.7,
        levels: 5,
        karis_average: true,
    };
    settings.tonemap.operator = operator;
    settings
}

fn check_golden(name: &str, operator: Tonemapper) {
    let Some(mut chain) = chain(
        name,
        GOLDEN_SIZE,
        golden_scene(),
        golden_settings(operator),
        Format::Rgba8Unorm,
    ) else {
        return;
    };

    chain.render(1.0 / 60.0);

    assert_golden(name, &chain.output(), Tolerance::cross_backend());
}

#[test]
fn full_chain_with_aces_matches_its_golden_image() {
    check_golden("post_chain_aces", Tonemapper::AcesFitted);
}

#[test]
fn full_chain_with_pbr_neutral_matches_its_golden_image() {
    check_golden("post_chain_neutral", Tonemapper::PbrNeutral);
}
