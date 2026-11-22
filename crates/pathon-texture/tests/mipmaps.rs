// Mips written by the compute shader against the CPU reference, for every colour space and the formats the shader
// supports. The null device runs the whole command list through RHI validation (barriers, states, bindings); with
// `gpu-tests` the same scenarios run on a real device and the texels are compared.

#[path = "support/readback.rs"]
mod readback;

use pathon_rhi::{Access, Backend, Device, DeviceApi, Format, TextureUsage};
use pathon_texture::reference::generate_chain;
use pathon_texture::{ColorSpace, MipMode, TextureLoader, TextureSource, Variant};
use readback::read_mip;

macro_rules! on_every_device {
    ($scenario:ident) => {
        mod $scenario {
            use super::*;

            #[test]
            fn on_null() {
                super::$scenario(&Device::null());
            }

            #[cfg(feature = "gpu-tests")]
            #[test]
            fn on_wgpu() {
                let built = Device::build(&pathon_rhi::DeviceRequest {
                    label: "texture mips",
                    ..Default::default()
                });
                let name = concat!(stringify!($scenario), "::on_wgpu");
                let Some(device) = pathon_golden::gpu_or_skip(name, built, |error| {
                    matches!(error, pathon_rhi::RhiError::NoAdapter(_))
                }) else {
                    return;
                };
                pathon_golden::announce_adapter(name, device.caps());
                super::$scenario(&device);
            }
        }
    };
}

fn noise(seed: u32, count: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[1]
        })
        .collect()
}

// Loads `level0` (all layers), then compares every generated mip with the reference chain.
fn check_chain(device: &Device, format: Format, mode: MipMode, size: (u32, u32), layers: u32, level0: &[u8]) {
    let color_space = match mode {
        MipMode::Srgb => ColorSpace::Srgb,
        MipMode::Normal => ColorSpace::Normal,
        _ => ColorSpace::Linear,
    };
    let source = TextureSource::new("mip test", size.0, size.1, color_space)
        .with_layers(layers)
        .with_extra_usage(TextureUsage::COPY_SRC)
        .with_variant(Variant::new(format, vec![level0]));
    let expected = generate_chain(format, mode, size.0, size.1, layers, level0).expect("reference");
    for mut loader in [TextureLoader::new(), TextureLoader::new().with_cpu_mips()] {
        let texture = loader
            .load(device, &source)
            .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
        assert_eq!(texture.desc.mip_level_count as usize, expected.len() + 1);

        if device.caps().backend != Backend::Null {
            let texel_bytes = format.block_size_bytes().expect("size") as usize;
            for (level, expected_level) in (1_u32..).zip(&expected) {
                let mip = texture.desc.size.mip_level(level);
                let layer_bytes = (mip.width * mip.height) as usize * texel_bytes;
                for layer in 0..layers {
                    let actual = read_mip(device, texture.id, Access::SAMPLED, level, layer);
                    let start = layer as usize * layer_bytes;
                    let what = format!("mip {level} layer {layer}");
                    assert_close(format, &actual, &expected_level[start..start + layer_bytes], &what);
                }
            }
        }
        loader.release(device);
    }
}

fn assert_close(format: Format, actual: &[u8], expected: &[u8], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: size");
    match format {
        Format::Rgba8Unorm => {
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert!(
                    actual.abs_diff(*expected) <= 1,
                    "{what}: byte {index} is {actual}, reference {expected}"
                );
            }
        }
        Format::Rgba16Float => {
            let half = |bytes: &[u8]| -> Vec<f32> {
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| half_to_f32(u16::from_le_bytes(*pair)))
                    .collect()
            };
            for (index, (actual, expected)) in half(actual).iter().zip(half(expected)).enumerate() {
                let allowed = 2.0e-3 * expected.abs().max(1.0);
                assert!(
                    (actual - expected).abs() <= allowed,
                    "{what}: value {index} is {actual}, reference {expected}"
                );
            }
        }
        other => panic!("no comparison for {other:?}"),
    }
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2.0_f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + mantissa / 1024.0) * 2.0_f32.powi(exponent - 15),
    }
}

fn linear_rgba8_mips_match_the_reference(device: &Device) {
    check_chain(
        device,
        Format::Rgba8Unorm,
        MipMode::Linear,
        (16, 16),
        1,
        &noise(1, 16 * 16 * 4),
    );
}
on_every_device!(linear_rgba8_mips_match_the_reference);

fn srgb_mips_match_the_reference(device: &Device) {
    check_chain(
        device,
        Format::Rgba8Unorm,
        MipMode::Srgb,
        (32, 16),
        1,
        &noise(2, 32 * 16 * 4),
    );
}
on_every_device!(srgb_mips_match_the_reference);

// Black and white texels average to half the light, which is 188 in sRGB; averaging the encoded values would give 128.
fn srgb_checkerboard_averages_in_linear_light(device: &Device) {
    let level0: Vec<u8> = (0..8 * 8)
        .flat_map(|index| {
            let white = (index % 8 + index / 8) % 2 == 0;
            let value = if white { 255 } else { 0 };
            [value, value, value, 255]
        })
        .collect();
    let source = TextureSource::new("checker", 8, 8, ColorSpace::Srgb)
        .with_extra_usage(TextureUsage::COPY_SRC)
        .with_variant(Variant::rgba8(vec![&level0]));
    let mut loader = TextureLoader::new();
    let texture = loader.load(device, &source).expect("load");
    assert_eq!(
        texture.sampling_format(),
        Format::Rgba8UnormSrgb,
        "sampled through the sRGB view"
    );
    assert_eq!(
        texture.desc.format,
        Format::Rgba8Unorm,
        "stored linear so that compute can write it"
    );
    if device.caps().backend != Backend::Null {
        let mip1 = read_mip(device, texture.id, Access::SAMPLED, 1, 0);
        for texel in mip1.as_chunks::<4>().0 {
            assert!(texel[..3].iter().all(|channel| channel.abs_diff(188) <= 1), "{texel:?}");
            assert_eq!(texel[3], 255);
        }
    }
    loader.release(device);
}
on_every_device!(srgb_checkerboard_averages_in_linear_light);

// Tilted unit normals, encoded as n * 0.5 + 0.5.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "components are within -1..=1, so the encoding is within 0..=255"
)]
fn tilted_normals(seed: u32, texels: usize) -> Vec<u8> {
    noise(seed, texels * 2)
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|pair| {
            let (x, y) = (
                (f32::from(pair[0]) / 255.0 - 0.5) * 1.2,
                (f32::from(pair[1]) / 255.0 - 0.5) * 1.2,
            );
            let z = (1.0 - x * x - y * y).sqrt();
            let encode = |component: f32| (component * 127.5 + 127.5).round() as u8;
            [encode(x), encode(y), encode(z), 255]
        })
        .collect()
}

fn normal_map_mips_match_the_reference_and_stay_unit_length(device: &Device) {
    let level0 = tilted_normals(3, 16 * 16);
    check_chain(device, Format::Rgba8Unorm, MipMode::Normal, (16, 16), 1, &level0);

    if device.caps().backend != Backend::Null {
        let source = TextureSource::new("normals", 16, 16, ColorSpace::Normal)
            .with_extra_usage(TextureUsage::COPY_SRC)
            .with_variant(Variant::rgba8(vec![&level0]));
        let mut loader = TextureLoader::new();
        let texture = loader.load(device, &source).expect("load");
        assert_eq!(
            texture.sampling_format(),
            Format::Rgba8Unorm,
            "normals are data: no sRGB view"
        );
        for level in 1..texture.desc.mip_level_count {
            for texel in read_mip(device, texture.id, Access::SAMPLED, level, 0)
                .as_chunks::<4>()
                .0
            {
                let [x, y, z] = [0, 1, 2].map(|channel| f32::from(texel[channel]) / 127.5 - 1.0);
                let length = (x * x + y * y + z * z).sqrt();
                assert!((length - 1.0).abs() < 0.03, "mip {level}: |n| = {length}");
            }
        }
        loader.release(device);
    }
}
on_every_device!(normal_map_mips_match_the_reference_and_stay_unit_length);

fn rgba16f_mips_match_the_reference(device: &Device) {
    // Halves between 0.25 and 16, built from their bits: above 1, so that nothing is clamped.
    let level0: Vec<u8> = noise(4, 16 * 8 * 4)
        .iter()
        .flat_map(|byte| {
            let (exponent, mantissa) = (13 + u16::from(*byte % 7), u16::from(*byte) * 4);
            ((exponent << 10) | mantissa).to_le_bytes()
        })
        .collect();
    check_chain(device, Format::Rgba16Float, MipMode::Linear, (16, 8), 1, &level0);
}
on_every_device!(rgba16f_mips_match_the_reference);

fn odd_sizes_keep_every_texel_in_some_footprint(device: &Device) {
    check_chain(
        device,
        Format::Rgba8Unorm,
        MipMode::Linear,
        (13, 7),
        1,
        &noise(5, 13 * 7 * 4),
    );
    check_chain(
        device,
        Format::Rgba8Unorm,
        MipMode::Srgb,
        (5, 9),
        1,
        &noise(6, 5 * 9 * 4),
    );
}
on_every_device!(odd_sizes_keep_every_texel_in_some_footprint);

fn every_layer_is_filtered_on_its_own(device: &Device) {
    let layers: Vec<u8> = [[40, 80, 120, 255], [200, 100, 50, 255], [0, 0, 0, 255]]
        .iter()
        .flat_map(|texel| texel.repeat(8 * 8))
        .collect();
    check_chain(device, Format::Rgba8Unorm, MipMode::Linear, (8, 8), 3, &layers);
}
on_every_device!(every_layer_is_filtered_on_its_own);

fn a_complete_chain_is_used_as_given(device: &Device) {
    let (top, middle, bottom) = ([10_u8; 4 * 4 * 4], [20_u8; 2 * 2 * 4], [30_u8; 4]);
    let source = TextureSource::new("given", 4, 4, ColorSpace::Linear)
        .with_extra_usage(TextureUsage::COPY_SRC)
        .with_variant(Variant::rgba8(vec![&top, &middle, &bottom]));
    let mut loader = TextureLoader::new();
    let texture = loader.load(device, &source).expect("load");
    assert_eq!(texture.desc.mip_level_count, 3);
    if device.caps().backend != Backend::Null {
        assert_eq!(read_mip(device, texture.id, Access::SAMPLED, 1, 0), middle);
        assert_eq!(read_mip(device, texture.id, Access::SAMPLED, 2, 0), bottom);
    }
}
on_every_device!(a_complete_chain_is_used_as_given);
