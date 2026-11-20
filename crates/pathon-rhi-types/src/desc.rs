use bitflags::bitflags;

use crate::format::{Format, FormatKind};
use crate::id::{BufferId, TextureId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Extent3d {
    pub width: u32,
    pub height: u32,
    pub depth_or_layers: u32,
}

impl Extent3d {
    pub const fn new_2d(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            depth_or_layers: 1,
        }
    }

    #[must_use]
    pub fn mip_level(self, level: u32) -> Self {
        Self {
            width: (self.width >> level).max(1),
            height: (self.height >> level).max(1),
            ..self
        }
    }

    pub fn max_mip_count(self) -> u32 {
        32 - self.width.max(self.height).max(1).leading_zeros()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Origin3d {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct TextureUsage: u32 {
        const COPY_SRC = 1 << 0;
        const COPY_DST = 1 << 1;
        const SAMPLED = 1 << 2;
        const STORAGE = 1 << 3;
        const RENDER_TARGET = 1 << 4;
    }
}

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct BufferUsage: u32 {
        const COPY_SRC = 1 << 0;
        const COPY_DST = 1 << 1;
        const VERTEX = 1 << 2;
        const INDEX = 1 << 3;
        const UNIFORM = 1 << 4;
        const STORAGE = 1 << 5;
        const INDIRECT = 1 << 6;
        const MAP_READ = 1 << 7;
        const QUERY_RESOLVE = 1 << 8;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextureDimension {
    D1,
    D2,
    D3,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextureDesc {
    pub label: &'static str,
    pub size: Extent3d,
    pub mip_level_count: u32,
    pub sample_count: u32,
    pub dimension: TextureDimension,
    pub format: Format,
    pub usage: TextureUsage,
}

impl TextureDesc {
    pub fn new_2d(label: &'static str, width: u32, height: u32, format: Format, usage: TextureUsage) -> Self {
        Self {
            label,
            size: Extent3d::new_2d(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage,
        }
    }

    #[must_use]
    pub fn with_mips(self, mip_level_count: u32) -> Self {
        Self {
            mip_level_count,
            ..self
        }
    }

    pub fn array_layers(&self) -> u32 {
        match self.dimension {
            TextureDimension::D3 => 1,
            TextureDimension::D1 | TextureDimension::D2 => self.size.depth_or_layers,
        }
    }

    // Two descriptions describe interchangeable GPU memory when they differ only by label.
    pub fn same_allocation(&self, other: &Self) -> bool {
        Self { label: "", ..*self } == Self { label: "", ..*other }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextureViewDimension {
    D1,
    D2,
    D2Array,
    Cube,
    CubeArray,
    D3,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextureAspect {
    #[default]
    All,
    DepthOnly,
    StencilOnly,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct TextureViewDesc {
    pub format: Option<Format>,
    pub dimension: Option<TextureViewDimension>,
    pub aspect: TextureAspect,
    pub base_mip: u32,
    pub mip_count: Option<u32>,
    pub base_layer: u32,
    pub layer_count: Option<u32>,
}

impl TextureViewDesc {
    pub fn single_mip(level: u32) -> Self {
        Self {
            base_mip: level,
            mip_count: Some(1),
            ..Self::default()
        }
    }

    pub fn depth_only(format: Format) -> Self {
        let aspect = if format.kind() == FormatKind::Color {
            TextureAspect::All
        } else {
            TextureAspect::DepthOnly
        };
        Self {
            aspect,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BufferDesc {
    pub label: &'static str,
    pub size: u64,
    pub usage: BufferUsage,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum AddressMode {
    #[default]
    ClampToEdge,
    Repeat,
    MirrorRepeat,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum FilterMode {
    #[default]
    Nearest,
    Linear,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CompareFunction {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SamplerDesc {
    pub label: &'static str,
    pub address_mode: AddressMode,
    pub mag_filter: FilterMode,
    pub min_filter: FilterMode,
    pub mipmap_filter: FilterMode,
    pub lod_min_clamp: f32,
    pub lod_max_clamp: f32,
    pub compare: Option<CompareFunction>,
    pub anisotropy_clamp: u16,
}

impl SamplerDesc {
    pub const LINEAR_CLAMP: Self = Self {
        label: "linear clamp",
        address_mode: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: FilterMode::Linear,
        lod_min_clamp: 0.0,
        lod_max_clamp: 32.0,
        compare: None,
        anisotropy_clamp: 1,
    };

    pub const NEAREST_CLAMP: Self = Self {
        label: "nearest clamp",
        mag_filter: FilterMode::Nearest,
        min_filter: FilterMode::Nearest,
        mipmap_filter: FilterMode::Nearest,
        ..Self::LINEAR_CLAMP
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct QuerySetDesc {
    pub label: &'static str,
    pub count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TexelLayout {
    pub offset: u64,
    pub bytes_per_row: u32,
    pub rows_per_image: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BufferCopy {
    pub buffer: BufferId,
    pub layout: TexelLayout,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextureCopy {
    pub texture: TextureId,
    pub mip: u32,
    pub origin: Origin3d,
    pub aspect: TextureAspect,
}

impl TextureCopy {
    pub fn whole(texture: TextureId) -> Self {
        Self {
            texture,
            mip: 0,
            origin: Origin3d::default(),
            aspect: TextureAspect::All,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_ends_at_one_pixel() {
        let size = Extent3d::new_2d(1024, 256);
        assert_eq!(size.max_mip_count(), 11);
        assert_eq!(size.mip_level(10), Extent3d::new_2d(1, 1));
    }

    #[test]
    fn labels_do_not_affect_allocation_identity() {
        let a = TextureDesc::new_2d("a", 4, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED);
        let b = TextureDesc { label: "b", ..a };
        assert!(a.same_allocation(&b));
        assert!(!a.same_allocation(&TextureDesc {
            format: Format::R8Unorm,
            ..a
        }));
    }
}
