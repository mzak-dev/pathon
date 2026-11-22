#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Format {
    R8Unorm,
    R8Uint,
    Rg8Unorm,
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Bgra8Unorm,
    Bgra8UnormSrgb,
    R16Float,
    R16Unorm,
    Rg16Float,
    Rgba16Float,
    Rgba16Unorm,
    R32Float,
    R32Uint,
    Rg32Float,
    Rg32Uint,
    Rgba32Float,
    Rgba32Uint,
    Rgb10a2Unorm,
    Rg11b10Ufloat,
    Depth16Unorm,
    Depth32Float,
    Depth24PlusStencil8,
    Depth32FloatStencil8,
    Bc1RgbaUnorm,
    Bc1RgbaUnormSrgb,
    Bc3RgbaUnorm,
    Bc3RgbaUnormSrgb,
    Bc4RUnorm,
    Bc5RgUnorm,
    Bc6hRgbUfloat,
    Bc7RgbaUnorm,
    Bc7RgbaUnormSrgb,
    Etc2Rgb8Unorm,
    Etc2Rgba8Unorm,
    Astc4x4Unorm,
    Astc4x4UnormSrgb,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FormatKind {
    Color,
    Depth,
    DepthStencil,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SampleKind {
    Float,
    UnfilterableFloat,
    Uint,
    Depth,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Compression {
    Bc,
    Etc2,
    Astc,
}

impl Format {
    pub const ALL: &[Self] = &[
        Self::R8Unorm,
        Self::R8Uint,
        Self::Rg8Unorm,
        Self::Rgba8Unorm,
        Self::Rgba8UnormSrgb,
        Self::Bgra8Unorm,
        Self::Bgra8UnormSrgb,
        Self::R16Float,
        Self::R16Unorm,
        Self::Rg16Float,
        Self::Rgba16Float,
        Self::Rgba16Unorm,
        Self::R32Float,
        Self::R32Uint,
        Self::Rg32Float,
        Self::Rg32Uint,
        Self::Rgba32Float,
        Self::Rgba32Uint,
        Self::Rgb10a2Unorm,
        Self::Rg11b10Ufloat,
        Self::Depth16Unorm,
        Self::Depth32Float,
        Self::Depth24PlusStencil8,
        Self::Depth32FloatStencil8,
        Self::Bc1RgbaUnorm,
        Self::Bc1RgbaUnormSrgb,
        Self::Bc3RgbaUnorm,
        Self::Bc3RgbaUnormSrgb,
        Self::Bc4RUnorm,
        Self::Bc5RgUnorm,
        Self::Bc6hRgbUfloat,
        Self::Bc7RgbaUnorm,
        Self::Bc7RgbaUnormSrgb,
        Self::Etc2Rgb8Unorm,
        Self::Etc2Rgba8Unorm,
        Self::Astc4x4Unorm,
        Self::Astc4x4UnormSrgb,
    ];

    pub fn kind(self) -> FormatKind {
        match self {
            Self::Depth16Unorm | Self::Depth32Float => FormatKind::Depth,
            Self::Depth24PlusStencil8 | Self::Depth32FloatStencil8 => FormatKind::DepthStencil,
            _ => FormatKind::Color,
        }
    }

    pub fn compression(self) -> Option<Compression> {
        match self {
            Self::Bc1RgbaUnorm
            | Self::Bc1RgbaUnormSrgb
            | Self::Bc3RgbaUnorm
            | Self::Bc3RgbaUnormSrgb
            | Self::Bc4RUnorm
            | Self::Bc5RgUnorm
            | Self::Bc6hRgbUfloat
            | Self::Bc7RgbaUnorm
            | Self::Bc7RgbaUnormSrgb => Some(Compression::Bc),
            Self::Etc2Rgb8Unorm | Self::Etc2Rgba8Unorm => Some(Compression::Etc2),
            Self::Astc4x4Unorm | Self::Astc4x4UnormSrgb => Some(Compression::Astc),
            _ => None,
        }
    }

    pub fn is_srgb(self) -> bool {
        matches!(
            self,
            Self::Rgba8UnormSrgb
                | Self::Bgra8UnormSrgb
                | Self::Bc1RgbaUnormSrgb
                | Self::Bc3RgbaUnormSrgb
                | Self::Bc7RgbaUnormSrgb
                | Self::Astc4x4UnormSrgb
        )
    }

    // The non-sRGB twin of an sRGB format; any other format is returned unchanged.
    #[must_use]
    pub fn linear_variant(self) -> Self {
        match self {
            Self::Rgba8UnormSrgb => Self::Rgba8Unorm,
            Self::Bgra8UnormSrgb => Self::Bgra8Unorm,
            Self::Bc1RgbaUnormSrgb => Self::Bc1RgbaUnorm,
            Self::Bc3RgbaUnormSrgb => Self::Bc3RgbaUnorm,
            Self::Bc7RgbaUnormSrgb => Self::Bc7RgbaUnorm,
            Self::Astc4x4UnormSrgb => Self::Astc4x4Unorm,
            other => other,
        }
    }

    // Views may reinterpret a texture only as a format of the same family: itself or its sRGB / non-sRGB twin.
    pub fn is_view_compatible_with(self, other: Self) -> bool {
        self.linear_variant() == other.linear_variant()
    }

    pub fn sample_kind(self) -> SampleKind {
        match self {
            Self::R8Uint | Self::R32Uint | Self::Rg32Uint | Self::Rgba32Uint => SampleKind::Uint,
            Self::R32Float | Self::Rg32Float | Self::Rgba32Float => SampleKind::UnfilterableFloat,
            _ if self.kind() != FormatKind::Color => SampleKind::Depth,
            _ => SampleKind::Float,
        }
    }

    pub fn block_dimensions(self) -> (u32, u32) {
        if self.compression().is_some() { (4, 4) } else { (1, 1) }
    }

    // Depth24Plus has no defined memory layout, so it cannot be copied to or from buffers.
    pub fn block_size_bytes(self) -> Option<u32> {
        let bytes = match self {
            Self::R8Unorm | Self::R8Uint => 1,
            Self::Rg8Unorm | Self::R16Float | Self::R16Unorm | Self::Depth16Unorm => 2,
            Self::Rgba8Unorm
            | Self::Rgba8UnormSrgb
            | Self::Bgra8Unorm
            | Self::Bgra8UnormSrgb
            | Self::Rg16Float
            | Self::R32Float
            | Self::R32Uint
            | Self::Rgb10a2Unorm
            | Self::Rg11b10Ufloat
            | Self::Depth32Float => 4,
            Self::Rgba16Float
            | Self::Rgba16Unorm
            | Self::Rg32Float
            | Self::Rg32Uint
            | Self::Bc1RgbaUnorm
            | Self::Bc1RgbaUnormSrgb
            | Self::Bc4RUnorm
            | Self::Etc2Rgb8Unorm => 8,
            Self::Rgba32Float
            | Self::Rgba32Uint
            | Self::Bc3RgbaUnorm
            | Self::Bc3RgbaUnormSrgb
            | Self::Bc5RgUnorm
            | Self::Bc6hRgbUfloat
            | Self::Bc7RgbaUnorm
            | Self::Bc7RgbaUnormSrgb
            | Self::Etc2Rgba8Unorm
            | Self::Astc4x4Unorm
            | Self::Astc4x4UnormSrgb => 16,
            Self::Depth24PlusStencil8 | Self::Depth32FloatStencil8 => return None,
        };
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_except_combined_depth_stencil_has_a_byte_size() {
        for format in Format::ALL {
            let copyable = format.kind() != FormatKind::DepthStencil;
            assert_eq!(format.block_size_bytes().is_some(), copyable, "{format:?}");
        }
    }

    #[test]
    fn views_may_switch_only_between_srgb_twins() {
        assert!(Format::Rgba8Unorm.is_view_compatible_with(Format::Rgba8UnormSrgb));
        assert!(Format::Bc7RgbaUnormSrgb.is_view_compatible_with(Format::Bc7RgbaUnorm));
        assert!(Format::R32Float.is_view_compatible_with(Format::R32Float));
        assert!(!Format::Rgba8Unorm.is_view_compatible_with(Format::Bgra8UnormSrgb));
        assert!(!Format::Rgba8Unorm.is_view_compatible_with(Format::R32Uint));
    }

    #[test]
    fn every_srgb_format_has_a_linear_twin_of_the_same_size() {
        for format in Format::ALL.iter().filter(|format| format.is_srgb()) {
            let linear = format.linear_variant();
            assert!(!linear.is_srgb(), "{format:?}");
            assert_eq!(linear.block_size_bytes(), format.block_size_bytes(), "{format:?}");
        }
    }

    #[test]
    fn compressed_formats_use_four_by_four_blocks() {
        assert_eq!(Format::Bc7RgbaUnorm.block_dimensions(), (4, 4));
        assert_eq!(Format::Rgba8Unorm.block_dimensions(), (1, 1));
    }
}
