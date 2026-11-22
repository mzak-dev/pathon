use pathon_api::rhi::{Extent3d, Format, FormatKind, TextureUsage};

use crate::error::TextureError;

/// What the texels mean; decides how mips are averaged and which sampling format an 8-bit texture gets.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ColorSpace {
    /// Colour stored sRGB-encoded: averaged in linear light, sampled through an sRGB view.
    Srgb,
    /// Data and linear colour: averaged as stored.
    Linear,
    /// Tangent-space normals stored as `n * 0.5 + 0.5`: averaged as vectors and renormalized.
    Normal,
}

/// The same image in one encoding, as mip levels from the largest down.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Variant<'data> {
    format: Format,
    levels: Vec<&'data [u8]>,
}

impl<'data> Variant<'data> {
    /// `format` may be given as either of an sRGB pair: the source's [`ColorSpace`] decides which one is used.
    /// Every level holds all layers one after another, tightly packed in blocks.
    pub fn new(format: Format, levels: Vec<&'data [u8]>) -> Self {
        Self {
            format: format.linear_variant(),
            levels,
        }
    }

    pub fn rgba8(levels: Vec<&'data [u8]>) -> Self {
        Self::new(Format::Rgba8Unorm, levels)
    }

    pub fn rgba16f(levels: Vec<&'data [u8]>) -> Self {
        Self::new(Format::Rgba16Float, levels)
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn levels(&self) -> &[&'data [u8]] {
        &self.levels
    }
}

/// An image offered in one or more encodings; [`TextureLoader`](crate::TextureLoader) picks the best one the device
/// can sample.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct TextureSource<'data> {
    pub label: &'static str,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub color_space: ColorSpace,
    pub generate_mips: bool,
    /// Usage on top of what sampling and loading need, e.g. `COPY_SRC` to read the texture back.
    pub extra_usage: TextureUsage,
    pub variants: Vec<Variant<'data>>,
}

impl<'data> TextureSource<'data> {
    pub fn new(label: &'static str, width: u32, height: u32, color_space: ColorSpace) -> Self {
        Self {
            label,
            width,
            height,
            layers: 1,
            color_space,
            generate_mips: true,
            extra_usage: TextureUsage::empty(),
            variants: Vec::new(),
        }
    }

    /// Six layers make a cube map (faces in the order +X, -X, +Y, -Y, +Z, -Z).
    #[must_use]
    pub fn with_layers(self, layers: u32) -> Self {
        Self { layers, ..self }
    }

    #[must_use]
    pub fn with_variant(mut self, variant: Variant<'data>) -> Self {
        self.variants.push(variant);
        self
    }

    /// Uses single-level variants as they are instead of generating the rest of the chain.
    #[must_use]
    pub fn without_mip_generation(self) -> Self {
        Self {
            generate_mips: false,
            ..self
        }
    }

    #[must_use]
    pub fn with_extra_usage(self, extra_usage: TextureUsage) -> Self {
        Self { extra_usage, ..self }
    }

    pub(crate) fn invalid(&self, reason: impl Into<String>) -> TextureError {
        TextureError::InvalidSource {
            label: self.label,
            reason: reason.into(),
        }
    }

    pub fn validate(&self) -> Result<(), TextureError> {
        if self.width == 0 || self.height == 0 || self.layers == 0 {
            return Err(self.invalid(format!(
                "{}x{} with {} layers has no texels",
                self.width, self.height, self.layers
            )));
        }
        if self.variants.is_empty() {
            return Err(self.invalid("no variants"));
        }
        self.variants
            .iter()
            .try_for_each(|variant| self.validate_variant(variant))
    }

    fn validate_variant(&self, variant: &Variant<'_>) -> Result<(), TextureError> {
        let format = variant.format;
        if format.kind() != FormatKind::Color {
            return Err(self.invalid(format!("{format:?} is not a colour format")));
        }
        let (block_width, block_height) = format.block_dimensions();
        if !self.width.is_multiple_of(block_width) || !self.height.is_multiple_of(block_height) {
            return Err(self.invalid(format!(
                "{format:?} needs a size divisible by {block_width}x{block_height}, got {}x{}",
                self.width, self.height
            )));
        }
        let max_levels = Extent3d::new_2d(self.width, self.height).max_mip_count();
        if variant.levels.is_empty() || variant.levels.len() > max_levels as usize {
            return Err(self.invalid(format!(
                "{format:?} has {} mip levels, a {}x{} texture has 1..={max_levels}",
                variant.levels.len(),
                self.width,
                self.height
            )));
        }
        for (level, bytes) in (0_u32..).zip(&variant.levels) {
            let expected = level_byte_len(format, self.width, self.height, level, self.layers);
            if Some(bytes.len() as u64) != expected {
                return Err(self.invalid(format!(
                    "{format:?} mip {level} has {} bytes, expected {expected:?}",
                    bytes.len()
                )));
            }
        }
        Ok(())
    }
}

pub(crate) fn blocks_in_level(format: Format, width: u32, height: u32, level: u32) -> (u32, u32) {
    let size = Extent3d::new_2d(width, height).mip_level(level);
    let (block_width, block_height) = format.block_dimensions();
    (size.width.div_ceil(block_width), size.height.div_ceil(block_height))
}

pub(crate) fn level_byte_len(format: Format, width: u32, height: u32, level: u32, layers: u32) -> Option<u64> {
    let block_bytes = u64::from(format.block_size_bytes()?);
    let (blocks_x, blocks_y) = blocks_in_level(format, width, height, level);
    Some(u64::from(blocks_x) * u64::from(blocks_y) * block_bytes * u64::from(layers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba8_source(levels: Vec<&[u8]>) -> TextureSource<'_> {
        TextureSource::new("t", 4, 4, ColorSpace::Srgb).with_variant(Variant::rgba8(levels))
    }

    #[test]
    fn variants_are_normalized_to_the_linear_twin() {
        assert_eq!(
            Variant::new(Format::Bc7RgbaUnormSrgb, vec![]).format(),
            Format::Bc7RgbaUnorm
        );
    }

    #[test]
    fn level_sizes_follow_blocks_and_layers() {
        assert_eq!(level_byte_len(Format::Rgba8Unorm, 8, 4, 1, 1), Some(4 * 2 * 4));
        assert_eq!(level_byte_len(Format::Bc7RgbaUnorm, 8, 8, 1, 1), Some(16));
        assert_eq!(
            level_byte_len(Format::Bc7RgbaUnorm, 8, 8, 2, 6),
            Some(16 * 6),
            "1x1 still takes a block"
        );
        assert_eq!(level_byte_len(Format::Depth24PlusStencil8, 8, 8, 0, 1), None);
    }

    #[test]
    fn a_wrong_level_length_is_x0002() {
        let bytes = [0_u8; 63];
        let error = rgba8_source(vec![&bytes]).validate().expect_err("one byte short");
        assert_eq!(error.code(), "X0002");
    }

    #[test]
    fn a_complete_chain_validates() {
        let (top, middle, bottom) = ([0_u8; 64], [0_u8; 16], [0_u8; 4]);
        assert!(rgba8_source(vec![&top, &middle, &bottom]).validate().is_ok());
        assert!(rgba8_source(vec![&top, &middle, &bottom, &bottom]).validate().is_err());
    }

    #[test]
    fn compressed_variants_need_block_aligned_sizes() {
        let bytes = [0_u8; 16];
        let source = TextureSource::new("t", 6, 4, ColorSpace::Linear)
            .with_variant(Variant::new(Format::Bc7RgbaUnorm, vec![&bytes]));
        assert_eq!(source.validate().expect_err("6 is not a multiple of 4").code(), "X0002");
    }
}
