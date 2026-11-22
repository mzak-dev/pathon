//! CPU mip chain with the same arithmetic as the compute shader: the reference the GPU output is tested against and
//! the path used when the device cannot write the format from compute.

use pathon_api::rhi::{Extent3d, Format};

use crate::half::{f16_to_f32, f32_to_f16};
use crate::mipmap::MipMode;

type Texel = [f32; 4];

/// Mips 1.. of a texture, each holding all layers one after another like the input.
///
/// Only `Rgba8Unorm` and `Rgba16Float` are averaged; `None` for other formats.
pub fn generate_chain(
    format: Format,
    mode: MipMode,
    width: u32,
    height: u32,
    layers: u32,
    level0: &[u8],
) -> Option<Vec<Vec<u8>>> {
    let codec = Codec::of(format)?;
    let size = Extent3d::new_2d(width, height);
    let mut previous = level0.to_vec();
    let mut chain = Vec::new();
    for level in 1..size.max_mip_count() {
        let (source, target) = (size.mip_level(level - 1), size.mip_level(level));
        previous = downsample_layers(codec, mode, source, target, layers, &previous);
        chain.push(previous.clone());
    }
    Some(chain)
}

#[derive(Clone, Copy)]
enum Codec {
    Rgba8,
    Rgba16f,
}

impl Codec {
    fn of(format: Format) -> Option<Self> {
        match format {
            Format::Rgba8Unorm => Some(Self::Rgba8),
            Format::Rgba16Float => Some(Self::Rgba16f),
            _ => None,
        }
    }

    fn texel_bytes(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgba16f => 8,
        }
    }

    fn read(self, bytes: &[u8]) -> Texel {
        match self {
            Self::Rgba8 => std::array::from_fn(|channel| f32::from(bytes[channel]) / 255.0),
            Self::Rgba16f => std::array::from_fn(|channel| {
                f16_to_f32(u16::from_le_bytes([bytes[channel * 2], bytes[channel * 2 + 1]]))
            }),
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 before the cast, like the unorm store of the GPU"
    )]
    fn write(self, texel: Texel, bytes: &mut Vec<u8>) {
        match self {
            Self::Rgba8 => bytes.extend(texel.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)),
            Self::Rgba16f => bytes.extend(texel.iter().flat_map(|value| f32_to_f16(*value).to_le_bytes())),
        }
    }
}

fn downsample_layers(
    codec: Codec,
    mode: MipMode,
    source: Extent3d,
    target: Extent3d,
    layers: u32,
    previous: &[u8],
) -> Vec<u8> {
    let source_layer_bytes = (source.width * source.height) as usize * codec.texel_bytes();
    let mut next = Vec::with_capacity((target.width * target.height * layers) as usize * codec.texel_bytes());
    for layer in previous.chunks_exact(source_layer_bytes).take(layers as usize) {
        for y in 0..target.height {
            for x in 0..target.width {
                let mean = footprint_mean(codec, mode, layer, source, target, (x, y));
                codec.write(encode(mean, mode), &mut next);
            }
        }
    }
    next
}

// Each target texel averages the source texels its area covers; for halving that is the usual 2x2 box, and an odd
// size widens the boxes so that no source row or column is dropped.
fn footprint_mean(
    codec: Codec,
    mode: MipMode,
    layer: &[u8],
    source: Extent3d,
    target: Extent3d,
    (x, y): (u32, u32),
) -> Texel {
    let range = |index: u32, source_size: u32, target_size: u32| {
        let first = index * source_size / target_size;
        let end = ((index + 1) * source_size).div_ceil(target_size);
        first..end
    };
    let (columns, rows) = (
        range(x, source.width, target.width),
        range(y, source.height, target.height),
    );
    let count = (columns.len() * rows.len()) as f32;
    let mut sum = [0.0_f32; 4];
    for row in rows {
        for column in columns.clone() {
            let at = (row * source.width + column) as usize * codec.texel_bytes();
            let texel = decode(codec.read(&layer[at..at + codec.texel_bytes()]), mode);
            sum.iter_mut().zip(texel).for_each(|(sum, value)| *sum += value);
        }
    }
    sum.map(|value| value / count)
}

fn decode(texel: Texel, mode: MipMode) -> Texel {
    let [red, green, blue, alpha] = texel;
    match mode {
        MipMode::Linear => texel,
        MipMode::Srgb => [srgb_to_linear(red), srgb_to_linear(green), srgb_to_linear(blue), alpha],
        MipMode::Normal => [red * 2.0 - 1.0, green * 2.0 - 1.0, blue * 2.0 - 1.0, alpha],
    }
}

fn encode(mean: Texel, mode: MipMode) -> Texel {
    let [red, green, blue, alpha] = mean;
    match mode {
        MipMode::Linear => mean,
        MipMode::Srgb => [linear_to_srgb(red), linear_to_srgb(green), linear_to_srgb(blue), alpha],
        MipMode::Normal => {
            let length = (red * red + green * green + blue * blue).sqrt();
            let direction = if length < NORMAL_EPSILON {
                [0.0, 0.0, 1.0]
            } else {
                [red / length, green / length, blue / length]
            };
            let [x, y, z] = direction.map(|component| component * 0.5 + 0.5);
            [x, y, z, alpha]
        }
    }
}

// Averaged normals that cancel out have no direction; the shader falls back to straight up as well.
pub(crate) const NORMAL_EPSILON: f32 = 1.0e-5;

pub(crate) fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub(crate) fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba8_chain(mode: MipMode, size: u32, texels: &[[u8; 4]]) -> Vec<Vec<u8>> {
        let level0: Vec<u8> = texels.iter().flatten().copied().collect();
        generate_chain(Format::Rgba8Unorm, mode, size, size, 1, &level0).expect("rgba8")
    }

    #[test]
    fn linear_mips_are_plain_averages() {
        let chain = rgba8_chain(
            MipMode::Linear,
            2,
            &[[0, 0, 0, 255], [100, 0, 0, 255], [0, 200, 0, 255], [0, 0, 255, 255]],
        );
        assert_eq!(chain, vec![vec![25, 50, 64, 255]]);
    }

    #[test]
    fn srgb_mips_average_in_linear_light() {
        let black_white = [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 255],
        ];
        let chain = rgba8_chain(MipMode::Srgb, 2, &black_white);
        assert_eq!(
            chain[0],
            [188, 188, 188, 255],
            "half linear light is 188 in sRGB, not 128"
        );
    }

    #[test]
    fn normal_mips_are_unit_length_vectors() {
        let slanted_left = [64, 128, 230, 255];
        let slanted_right = [192, 128, 230, 255];
        let chain = rgba8_chain(
            MipMode::Normal,
            2,
            &[slanted_left, slanted_right, slanted_left, slanted_right],
        );
        // Opposite tilts cancel to the straight-up normal (0.5, 0.5, 1.0 in the encoding).
        assert_eq!(chain[0], [128, 128, 255, 255]);
    }

    #[test]
    fn odd_sizes_keep_every_row_in_some_footprint() {
        let level0: Vec<u8> = (0..5 * 5)
            .flat_map(|texel| [u8::try_from(texel * 10).expect("small"), 0, 0, 255])
            .collect();
        let chain = generate_chain(Format::Rgba8Unorm, MipMode::Linear, 5, 5, 1, &level0).expect("rgba8");
        assert_eq!(chain.len(), 2, "5 -> 2 -> 1");
        assert_eq!(chain[0].len(), 2 * 2 * 4);
        assert_eq!(chain[1].len(), 4);
    }

    #[test]
    fn each_layer_is_filtered_alone() {
        let layer_a = [10, 10, 10, 255].repeat(4);
        let layer_b = [200, 200, 200, 255].repeat(4);
        let level0 = [layer_a, layer_b].concat();
        let chain = generate_chain(Format::Rgba8Unorm, MipMode::Linear, 2, 2, 2, &level0).expect("rgba8");
        assert_eq!(chain[0], [10, 10, 10, 255, 200, 200, 200, 255]);
    }

    #[test]
    fn half_float_mips_average_values_above_one() {
        let texel = |value: f32| f32_to_f16(value).to_le_bytes();
        let level0: Vec<u8> = [0.0_f32, 4.0, 8.0, 12.0]
            .iter()
            .flat_map(|value| [texel(*value), texel(*value), texel(*value), texel(1.0)].concat())
            .collect();
        let chain = generate_chain(Format::Rgba16Float, MipMode::Linear, 2, 2, 1, &level0).expect("rgba16f");
        assert_eq!(f16_to_f32(u16::from_le_bytes([chain[0][0], chain[0][1]])), 6.0);
    }

    #[test]
    fn other_formats_are_not_averaged() {
        assert!(generate_chain(Format::R8Unorm, MipMode::Linear, 2, 2, 1, &[0; 4]).is_none());
    }
}
