//! Reader for KTX2 containers (<https://registry.khronos.org/KTX/specs/2.0/ktxspec.v2.html>), without supercompression.

use pathon_api::rhi::Format;

use crate::error::TextureError;
use crate::source::{ColorSpace, TextureSource, Variant};

const IDENTIFIER: [u8; 12] = [0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a];
const HEADER_BYTES: usize = 80;
const LEVEL_INDEX_ENTRY_BYTES: usize = 24;
const NO_SUPERCOMPRESSION: u32 = 0;
const CUBE_FACES: u32 = 6;
// 32 mips cover u32 dimensions; the layer cap is far above any device limit and keeps arithmetic in range on wasm32.
const MAX_LEVELS: u32 = 32;
const MAX_LAYERS: u32 = 1 << 16;

/// One image of a KTX2 file: the encoding it was stored in and its mip levels, borrowed from the file bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ktx2Image<'data> {
    pub format: Format,
    /// The `vkFormat` was an sRGB one.
    pub srgb: bool,
    pub width: u32,
    pub height: u32,
    /// Array layers times faces; six for a cube map.
    pub layers: u32,
    /// The file stores only the base level and asks the reader to generate the rest (`levelCount` 0).
    pub wants_mip_generation: bool,
    pub levels: Vec<&'data [u8]>,
}

impl<'data> Ktx2Image<'data> {
    pub fn parse(bytes: &'data [u8]) -> Result<Self, TextureError> {
        let mut reader = Reader { bytes };
        if reader.take(IDENTIFIER.len())? != IDENTIFIER {
            return Err(error("not a KTX2 file (bad identifier)"));
        }
        let vk_format = reader.u32()?;
        let _type_size = reader.u32()?;
        let (width, height, depth) = (reader.u32()?, reader.u32()?, reader.u32()?);
        let (layer_count, face_count, level_count) = (reader.u32()?, reader.u32()?, reader.u32()?);
        let supercompression = reader.u32()?;
        // Skip the data format descriptor and key-value ranges and the supercompression global data.
        reader.take(HEADER_BYTES - IDENTIFIER.len() - 9 * 4)?;

        if supercompression != NO_SUPERCOMPRESSION {
            return Err(error(format!(
                "supercompression scheme {supercompression} is not supported (only uncompressed level data)"
            )));
        }
        if width == 0 || height == 0 || depth > 1 {
            return Err(error(format!("{width}x{height}x{depth} is not a 2D texture")));
        }
        if face_count != 1 && face_count != CUBE_FACES {
            return Err(error(format!("{face_count} faces")));
        }
        let (format, srgb) = format_from_vk(vk_format)?;
        if level_count > MAX_LEVELS {
            return Err(error(format!("{level_count} levels (at most {MAX_LEVELS})")));
        }
        let layers = layer_count
            .max(1)
            .checked_mul(face_count)
            .filter(|&layers| layers <= MAX_LAYERS)
            .ok_or_else(|| {
                error(format!(
                    "{layer_count} layers of {face_count} faces (at most {MAX_LAYERS})"
                ))
            })?;
        let stored_levels = level_count.max(1) as usize;
        let index = reader
            .take(stored_levels * LEVEL_INDEX_ENTRY_BYTES)
            .map_err(|_| error("level index is cut off"))?;
        let mut levels = Vec::with_capacity(stored_levels);
        for entry in index.as_chunks::<LEVEL_INDEX_ENTRY_BYTES>().0 {
            let (offset, length) = (u64_at(entry, 0), u64_at(entry, 8));
            let range = usize::try_from(offset)
                .ok()
                .zip(usize::try_from(length).ok())
                .and_then(|(offset, length)| Some(offset..offset.checked_add(length)?))
                .ok_or_else(|| error("level range overflows"))?;
            levels.push(bytes.get(range).ok_or_else(|| error("level data is cut off"))?);
        }
        Ok(Self {
            format,
            srgb,
            width,
            height,
            layers,
            wants_mip_generation: level_count == 0,
            levels,
        })
    }

    pub fn color_space(&self) -> ColorSpace {
        if self.srgb {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        }
    }

    pub fn variant(&self) -> Variant<'data> {
        Variant::new(self.format, self.levels.clone())
    }

    /// A source with this image as its only variant; add fallback variants with `with_variant`, or change the colour
    /// space to `Normal` for a normal map (KTX2 cannot tell).
    pub fn source(&self, label: &'static str) -> TextureSource<'data> {
        let source = TextureSource::new(label, self.width, self.height, self.color_space())
            .with_layers(self.layers)
            .with_variant(self.variant());
        if self.wants_mip_generation || self.levels.len() == 1 {
            source
        } else {
            source.without_mip_generation()
        }
    }
}

struct Reader<'data> {
    bytes: &'data [u8],
}

impl<'data> Reader<'data> {
    fn take(&mut self, count: usize) -> Result<&'data [u8], TextureError> {
        let (head, tail) = self
            .bytes
            .split_at_checked(count)
            .ok_or_else(|| error("file is cut off in the header"))?;
        self.bytes = tail;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, TextureError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

fn error(reason: impl Into<String>) -> TextureError {
    TextureError::Container { reason: reason.into() }
}

// `VkFormat` values from the Vulkan registry; the second element says the format is the sRGB flavour.
fn format_from_vk(vk_format: u32) -> Result<(Format, bool), TextureError> {
    Ok(match vk_format {
        9 => (Format::R8Unorm, false),
        16 => (Format::Rg8Unorm, false),
        37 => (Format::Rgba8Unorm, false),
        43 => (Format::Rgba8Unorm, true),
        76 => (Format::R16Float, false),
        83 => (Format::Rg16Float, false),
        97 => (Format::Rgba16Float, false),
        100 => (Format::R32Float, false),
        131 | 133 => (Format::Bc1RgbaUnorm, false),
        132 | 134 => (Format::Bc1RgbaUnorm, true),
        137 => (Format::Bc3RgbaUnorm, false),
        138 => (Format::Bc3RgbaUnorm, true),
        139 => (Format::Bc4RUnorm, false),
        141 => (Format::Bc5RgUnorm, false),
        143 => (Format::Bc6hRgbUfloat, false),
        145 => (Format::Bc7RgbaUnorm, false),
        146 => (Format::Bc7RgbaUnorm, true),
        147 => (Format::Etc2Rgb8Unorm, false),
        151 => (Format::Etc2Rgba8Unorm, false),
        157 => (Format::Astc4x4Unorm, false),
        158 => (Format::Astc4x4Unorm, true),
        other => return Err(error(format!("vkFormat {other} has no matching texture format"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK_RGBA8_SRGB: u32 = 43;
    const VK_BC7_UNORM: u32 = 145;

    // A file with `levels` stored from the base level down; `data` holds each level's bytes.
    fn file(
        vk_format: u32,
        size: (u32, u32),
        faces: u32,
        supercompression: u32,
        level_count: u32,
        data: &[&[u8]],
    ) -> Vec<u8> {
        let mut bytes = IDENTIFIER.to_vec();
        for word in [vk_format, 1, size.0, size.1, 0, 0, faces, level_count, supercompression] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend([0; HEADER_BYTES - 48]);
        let first_level = (HEADER_BYTES + data.len() * LEVEL_INDEX_ENTRY_BYTES) as u64;
        let mut offset = first_level;
        for level in data {
            for word in [offset, level.len() as u64, level.len() as u64] {
                bytes.extend(word.to_le_bytes());
            }
            offset += level.len() as u64;
        }
        for level in data {
            bytes.extend(*level);
        }
        bytes
    }

    #[test]
    fn reads_an_srgb_chain() {
        let (base, tail) = ([1_u8; 4 * 4 * 4], [2_u8; 2 * 2 * 4]);
        let bytes = file(VK_RGBA8_SRGB, (4, 4), 1, 0, 2, &[&base, &tail]);
        let image = Ktx2Image::parse(&bytes).expect("valid");
        assert_eq!(
            (image.format, image.srgb, image.width, image.layers),
            (Format::Rgba8Unorm, true, 4, 1)
        );
        assert_eq!(image.levels, vec![&base[..], &tail[..]]);
        let source = image.source("t");
        assert_eq!(source.color_space, ColorSpace::Srgb);
        assert!(!source.generate_mips, "a full chain is used as it is");
        assert!(source.validate().is_ok());
    }

    #[test]
    fn level_count_zero_asks_for_generation() {
        let base = [0_u8; 4 * 4 * 4];
        let bytes = file(VK_RGBA8_SRGB, (4, 4), 1, 0, 0, &[&base]);
        let image = Ktx2Image::parse(&bytes).expect("valid");
        assert!(image.wants_mip_generation);
        assert!(image.source("t").generate_mips);
    }

    #[test]
    fn cube_maps_have_six_layers() {
        let base = [0_u8; 6 * 16];
        let bytes = file(VK_BC7_UNORM, (4, 4), 6, 0, 1, &[&base]);
        let image = Ktx2Image::parse(&bytes).expect("valid");
        assert_eq!((image.format, image.layers), (Format::Bc7RgbaUnorm, 6));
        assert!(image.source("cube").validate().is_ok());
    }

    #[test]
    fn absurd_level_and_layer_counts_are_rejected_not_overflowed() {
        let base = [0_u8; 64];
        let good = file(VK_RGBA8_SRGB, (4, 4), 6, 0, 1, &[&base]);
        let with_word = |offset: usize, value: u32| {
            let mut bytes = good.clone();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            bytes
        };
        let (layers_at, levels_at) = (IDENTIFIER.len() + 5 * 4, IDENTIFIER.len() + 7 * 4);
        for bytes in [with_word(levels_at, u32::MAX), with_word(layers_at, u32::MAX)] {
            assert!(matches!(Ktx2Image::parse(&bytes), Err(TextureError::Container { .. })));
        }
    }

    #[test]
    fn broken_files_are_x0004() {
        let base = [0_u8; 64];
        let good = file(VK_RGBA8_SRGB, (4, 4), 1, 0, 1, &[&base]);
        let mut bad_magic = good.clone();
        bad_magic[1] = 0;
        let cases = [
            ("magic", bad_magic),
            ("header cut", good[..40].to_vec()),
            ("data cut", good[..good.len() - 1].to_vec()),
            ("zstd", file(VK_RGBA8_SRGB, (4, 4), 1, 2, 1, &[&base])),
            ("basis lz", file(VK_RGBA8_SRGB, (4, 4), 1, 1, 1, &[&base])),
            ("unknown format", file(1, (4, 4), 1, 0, 1, &[&base])),
            ("three faces", file(VK_RGBA8_SRGB, (4, 4), 3, 0, 1, &[&base])),
            ("empty", file(VK_RGBA8_SRGB, (0, 4), 1, 0, 1, &[&base])),
        ];
        for (name, bytes) in cases {
            let error = Ktx2Image::parse(&bytes).expect_err(name);
            assert_eq!(error.code(), "X0004", "{name}: {error}");
        }
    }
}
