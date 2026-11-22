// The same textured scene through BC7 on a device that samples it and through the RGBA8 fallback on one that does not,
// both against one golden image (M4-13). The BC7 data is made here: a small mode 6 encoder (one subset, 7-bit RGBA
// endpoints with a shared p-bit, 4-bit indices) and a matching decoder that produces the RGBA8 variant, so the two
// variants carry exactly the same texels.

// The golden-image half of this file needs a device; without `gpu-tests` only the encoder tests build.
#![cfg_attr(not(feature = "gpu-tests"), allow(unused_imports, dead_code))]

use pathon_golden::{GoldenImage, Tolerance, announce_adapter, assert_golden, compare, gpu_or_skip};
use pathon_rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
    BufferCopy, BufferDesc, BufferUsage, Color, ColorAttachment, ColorTargetState, CommandList, Device, DeviceApi,
    DeviceRequest, Extent3d, Features, Format, NullDevice, PrimitiveState, RenderPassDesc, RenderPipelineDesc,
    RhiError, SamplerBindingType, SamplerDesc, ShaderModuleDesc, ShaderStage, ShaderStages, SubresourceRange,
    TexelLayout, TextureCopy, TextureDesc, TextureUsage, TextureViewDesc,
};
use pathon_texture::reference::generate_chain;
use pathon_texture::{ColorSpace, FormatChoice, MipMode, Texture, TextureLoader, TextureSource, Variant};

const SIZE: u32 = 32;
const SCENE_WIDTH: u32 = 128;
const SCENE_HEIGHT: u32 = 32;

const INDEX_WEIGHTS: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

type Rgba = [u8; 4];

struct BitWriter {
    bits: u128,
    written: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, count: u32) {
        self.bits |= u128::from(value) << self.written;
        self.written += count;
    }
}

struct Endpoint {
    high_bits: [u8; 4],
    p_bit: u8,
}

impl Endpoint {
    fn color(&self) -> [i32; 4] {
        self.high_bits.map(|bits| i32::from(bits) * 2 + i32::from(self.p_bit))
    }
}

fn distance(a: [i32; 4], b: [i32; 4]) -> i32 {
    a.iter().zip(b).map(|(a, b)| (a - b) * (a - b)).sum()
}

fn quantize(target: Rgba) -> Endpoint {
    let target = target.map(i32::from);
    (0..2)
        .map(|p_bit| {
            let high_bits =
                target.map(|value| u8::try_from(((value - p_bit + 1).max(0) / 2).min(127)).expect("7 bits"));
            Endpoint {
                high_bits,
                p_bit: u8::try_from(p_bit).expect("bit"),
            }
        })
        .min_by_key(|endpoint| distance(endpoint.color(), target))
        .expect("two candidates")
}

fn interpolate(start: [i32; 4], end: [i32; 4], weight: i32) -> [i32; 4] {
    std::array::from_fn(|channel| (start[channel] * (64 - weight) + end[channel] * weight + 32) >> 6)
}

fn encode_block(pixels: &[Rgba; 16]) -> [u8; 16] {
    let low: [i32; 4] = std::array::from_fn(|c| pixels.iter().map(|p| i32::from(p[c])).min().expect("pixels"));
    let high: [i32; 4] = std::array::from_fn(|c| pixels.iter().map(|p| i32::from(p[c])).max().expect("pixels"));
    let along_axis = |pixel: &Rgba| -> i32 {
        (0..4)
            .map(|c| (i32::from(pixel[c]) - low[c]) * (high[c] - low[c]))
            .sum()
    };
    let darkest = pixels.iter().min_by_key(|pixel| along_axis(pixel)).expect("pixels");
    let brightest = pixels.iter().max_by_key(|pixel| along_axis(pixel)).expect("pixels");
    let (mut first, mut second) = (quantize(*darkest), quantize(*brightest));

    let pick_index = |first: &Endpoint, second: &Endpoint, pixel: &Rgba| -> u8 {
        let target = pixel.map(i32::from);
        let best = (0..16)
            .min_by_key(|index| {
                distance(
                    interpolate(first.color(), second.color(), INDEX_WEIGHTS[*index]),
                    target,
                )
            })
            .expect("16 indices");
        u8::try_from(best).expect("4 bits")
    };
    let mut indices: Vec<u8> = pixels.iter().map(|pixel| pick_index(&first, &second, pixel)).collect();
    // The first index has no top bit: keep it below 8 by swapping the endpoints.
    if indices[0] >= 8 {
        std::mem::swap(&mut first, &mut second);
        for index in &mut indices {
            *index = 15 - *index;
        }
    }

    let mut writer = BitWriter { bits: 0, written: 0 };
    writer.put(1 << 6, 7);
    for channel in 0..4 {
        writer.put(u32::from(first.high_bits[channel]), 7);
        writer.put(u32::from(second.high_bits[channel]), 7);
    }
    writer.put(u32::from(first.p_bit), 1);
    writer.put(u32::from(second.p_bit), 1);
    for (position, index) in indices.iter().enumerate() {
        writer.put(u32::from(*index), if position == 0 { 3 } else { 4 });
    }
    assert_eq!(writer.written, 128);
    writer.bits.to_le_bytes()
}

fn decode_block(block: &[u8]) -> [Rgba; 16] {
    let bits = u128::from_le_bytes(block.try_into().expect("16 bytes"));
    let mut at = 0;
    let mut take = |count: u32| -> u32 {
        let value = u32::try_from((bits >> at) & ((1 << count) - 1)).expect("fits");
        at += count;
        value
    };
    assert_eq!(take(7), 1 << 6, "mode 6 block");
    let mut ends = [[0_u32; 4]; 2];
    for channel in 0..4 {
        for end in &mut ends {
            end[channel] = take(7);
        }
    }
    let p_bits = [take(1), take(1)];
    let color =
        |end: usize| -> [i32; 4] { ends[end].map(|bits| i32::try_from(bits * 2 + p_bits[end]).expect("8 bits")) };
    let (first, second) = (color(0), color(1));
    std::array::from_fn(|position| {
        let index = take(if position == 0 { 3 } else { 4 }) as usize;
        interpolate(first, second, INDEX_WEIGHTS[index]).map(|value| u8::try_from(value).expect("8 bits"))
    })
}

fn blocks(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(4), height.div_ceil(4))
}

fn encode_image(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (blocks_x, blocks_y) = blocks(width, height);
    let texel = |x: u32, y: u32| -> Rgba {
        let at = ((y.min(height - 1) * width + x.min(width - 1)) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    };
    let mut encoded = Vec::new();
    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let pixels: [Rgba; 16] = std::array::from_fn(|index| {
                let index = u32::try_from(index).expect("16 pixels");
                let (x, y) = (block_x * 4 + index % 4, block_y * 4 + index / 4);
                texel(x, y)
            });
            encoded.extend(encode_block(&pixels));
        }
    }
    encoded
}

fn decode_image(encoded: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (blocks_x, _) = blocks(width, height);
    let mut rgba = vec![0; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let block_at = ((y / 4) * blocks_x + x / 4) as usize * 16;
            let pixels = decode_block(&encoded[block_at..block_at + 16]);
            let at = ((y * width + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&pixels[((y % 4) * 4 + x % 4) as usize]);
        }
    }
    rgba
}

// Smooth ramps, a checker of two tints and a disc: gradients inside blocks and hard edges between them.
fn original_image() -> Vec<u8> {
    (0..SIZE * SIZE)
        .flat_map(|index| {
            let (x, y) = (index % SIZE, index / SIZE);
            let tint = if (x / 8 + y / 8) % 2 == 0 { 200 } else { 60 };
            let in_disc = x.abs_diff(16).pow(2) + y.abs_diff(16).pow(2) < 70;
            if in_disc {
                [250, 230, 40, 255]
            } else {
                [
                    u8::try_from(x * 8).expect("ramp"),
                    u8::try_from(y * 8).expect("ramp"),
                    tint,
                    255,
                ]
            }
        })
        .collect()
}

struct Levels {
    bc7: Vec<Vec<u8>>,
    rgba8: Vec<Vec<u8>>,
}

// Every mip of both variants comes from the decoded BC7 base, so the variants never differ by encoding error.
fn build_levels() -> Levels {
    let base = decode_image(&encode_image(&original_image(), SIZE, SIZE), SIZE, SIZE);
    let mut chain = vec![base.clone()];
    chain.extend(generate_chain(Format::Rgba8Unorm, MipMode::Srgb, SIZE, SIZE, 1, &base).expect("chain"));
    let mut levels = Levels {
        bc7: Vec::new(),
        rgba8: Vec::new(),
    };
    for (level, rgba) in chain.iter().enumerate() {
        let size = Extent3d::new_2d(SIZE, SIZE).mip_level(u32::try_from(level).expect("6 levels"));
        let encoded = encode_image(rgba, size.width, size.height);
        levels.rgba8.push(decode_image(&encoded, size.width, size.height));
        levels.bc7.push(encoded);
    }
    levels
}

fn scene_source(levels: &Levels) -> TextureSource<'_> {
    TextureSource::new("scene texture", SIZE, SIZE, ColorSpace::Srgb)
        .with_variant(Variant::new(
            Format::Bc7RgbaUnorm,
            levels.bc7.iter().map(Vec::as_slice).collect(),
        ))
        .with_variant(Variant::rgba8(levels.rgba8.iter().map(Vec::as_slice).collect()))
}

const SHADER: &str = "
@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Varying {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Varying(vec4<f32>(corner * 2.0 - vec2<f32>(1.0), 0.0, 1.0), vec2<f32>(corner.x, 1.0 - corner.y));
}

// Four columns show mips 0..3, each over the whole texture.
@fragment fn fs(in: Varying) -> @location(0) vec4<f32> {
    let column = min(u32(in.uv.x * 4.0), 3u);
    let local = vec2<f32>(fract(in.uv.x * 4.0), in.uv.y);
    return textureSampleLevel(scene_texture, scene_sampler, local, f32(column));
}
";

fn render_scene(device: &Device, texture: &Texture) -> GoldenImage {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "texture scene",
            wgsl: SHADER,
        })
        .expect("module");
    let entry = |binding, ty| BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty,
        count: None,
    };
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "texture scene",
            entries: vec![
                entry(0, BindingType::texture_2d()),
                entry(1, BindingType::Sampler(SamplerBindingType::Filtering)),
            ],
        })
        .expect("layout");
    let target_format = Format::Rgba8UnormSrgb;
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "texture scene",
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
            color_targets: vec![ColorTargetState::opaque(target_format)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })
        .expect("pipeline");
    let sampler = device.create_sampler(&SamplerDesc::LINEAR_CLAMP).expect("sampler");
    let view = texture.create_view(device).expect("texture view");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "texture scene",
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(sampler),
                },
            ],
        })
        .expect("bind group");

    // Rendering through an sRGB view of an RGBA8 target writes the colours as the texture stores them.
    let target = device
        .create_texture(
            &TextureDesc::new_2d(
                "scene target",
                SCENE_WIDTH,
                SCENE_HEIGHT,
                Format::Rgba8Unorm,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            )
            .with_view_formats(&[Format::Rgba8UnormSrgb]),
        )
        .expect("target");
    let target_view = device
        .create_texture_view(target, &TextureViewDesc::with_format(target_format))
        .expect("target view");
    let row_bytes = SCENE_WIDTH * 4;
    let readback = device
        .create_buffer(&BufferDesc {
            label: "scene readback",
            size: u64::from(row_bytes * SCENE_HEIGHT),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");

    let mut list = CommandList::new();
    list.texture_access(target, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "texture scene",
            color: vec![ColorAttachment::clear(target_view, Color::BLACK)],
            depth: None,
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.draw(0..3, 0..1);
    }
    list.texture_access(
        target,
        SubresourceRange::ALL,
        Access::COLOR_ATTACHMENT,
        Access::COPY_SRC,
    );
    list.copy_texture_to_buffer(
        TextureCopy::whole(target),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: row_bytes,
                rows_per_image: SCENE_HEIGHT,
            },
        },
        Extent3d::new_2d(SCENE_WIDTH, SCENE_HEIGHT),
    );
    device.submit(list).expect("submit");
    let rgba = device.read_buffer(readback).expect("read");
    GoldenImage::from_rgba8(SCENE_WIDTH, SCENE_HEIGHT, rgba).expect("image")
}

fn load(device: &Device, levels: &Levels) -> Texture {
    let source = scene_source(levels);
    let texture = TextureLoader::new()
        .load(device, &source)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    assert_eq!(texture.desc.mip_level_count, 6);
    texture
}

fn gpu(test: &str, request: &DeviceRequest) -> Option<Device> {
    let device = gpu_or_skip(test, Device::build(request), |error| {
        matches!(error, RhiError::NoAdapter(_))
    })?;
    announce_adapter(test, device.caps());
    Some(device)
}

#[test]
fn the_encoder_and_decoder_agree_on_a_flat_and_a_ramp_block() {
    // The p-bit is shared by all four channels, so a solid colour can be off by one in one of them.
    let flat = [[10, 20, 30, 254]; 16];
    assert_eq!(decode_block(&encode_block(&flat)), flat);
    let ramp: [Rgba; 16] = std::array::from_fn(|index| [u8::try_from(index * 16).expect("ramp"), 100, 50, 255]);
    let decoded = decode_block(&encode_block(&ramp));
    for (decoded, original) in decoded.iter().zip(ramp) {
        assert!(decoded[0].abs_diff(original[0]) <= 9, "{decoded:?} vs {original:?}");
    }
}

#[test]
fn bc7_levels_pass_rhi_validation_on_the_null_device() {
    let levels = build_levels();
    let bc7_device = Device::from(NullDevice::with_features(Features::TEXTURE_COMPRESSION_BC));
    let source = scene_source(&levels);
    let choice = TextureLoader::choose_format(bc7_device.caps(), &source).expect("choice");
    assert_eq!(choice.storage, Format::Bc7RgbaUnormSrgb);
    let texture = load(&bc7_device, &levels);
    assert_eq!(texture.desc.format, Format::Bc7RgbaUnormSrgb);
    assert_eq!(texture.desc.view_formats, &[] as &[Format]);

    let plain_device = Device::from(NullDevice::with_features(Features::empty()));
    let texture = load(&plain_device, &levels);
    assert_eq!(texture.desc.format, Format::Rgba8Unorm);
    assert_eq!(texture.desc.view_formats, &[Format::Rgba8UnormSrgb]);
    assert_eq!(texture.sampling_format(), Format::Rgba8UnormSrgb);
}

#[cfg(feature = "gpu-tests")]
#[test]
fn bc7_and_the_rgba8_fallback_render_the_same_scene() {
    let test = "bc7_and_the_rgba8_fallback_render_the_same_scene";
    let levels = build_levels();
    let source = scene_source(&levels);

    // No optional features: this device cannot sample BCn, whatever the GPU underneath is.
    let Some(fallback_device) = gpu(
        test,
        &DeviceRequest {
            label: "texture fallback",
            ..Default::default()
        },
    ) else {
        return;
    };
    let fallback_choice = TextureLoader::choose_format(fallback_device.caps(), &source).expect("choice");
    assert_eq!(
        fallback_choice,
        FormatChoice {
            variant: 1,
            storage: Format::Rgba8Unorm,
            sampling: Format::Rgba8UnormSrgb,
            fallback: true,
        }
    );
    let fallback_image = render_scene(&fallback_device, &load(&fallback_device, &levels));
    assert_golden("texture_bc7_mips", &fallback_image, Tolerance::cross_backend());

    let desktop_device = gpu(
        test,
        &DeviceRequest {
            label: "texture bc7",
            optional_features: Features::TEXTURE_COMPRESSION_BC,
            ..Default::default()
        },
    )
    .expect("the same adapter again");
    if !desktop_device.caps().has(Features::TEXTURE_COMPRESSION_BC) {
        eprintln!("SKIPPED the BC7 half of {test}: this GPU does not sample BCn");
        return;
    }
    let desktop_choice = TextureLoader::choose_format(desktop_device.caps(), &source).expect("choice");
    assert_eq!(
        desktop_choice,
        FormatChoice {
            variant: 0,
            storage: Format::Bc7RgbaUnormSrgb,
            sampling: Format::Bc7RgbaUnormSrgb,
            fallback: false,
        }
    );
    let bc7_image = render_scene(&desktop_device, &load(&desktop_device, &levels));
    assert_golden("texture_bc7_mips", &bc7_image, Tolerance::cross_backend());

    // The golden tolerance is loose for rasterizers; the two paths share one device class, so they must be near exact.
    let difference = compare(&fallback_image, &bc7_image, Tolerance::new(1, 0.0)).expect("same size");
    assert_eq!(difference.differing_pixels, 0, "{difference:?}");
}
