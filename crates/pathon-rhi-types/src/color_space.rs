use bitflags::bitflags;

use crate::format::Format;

// Mirrors wgpu's `SurfaceColorSpace`; the names say what the signal is, docs/crates/pathon-rhi-types.md what each
// backend can present (ADR-0034).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub enum SurfaceColorSpace {
    // Never HDR: resolves to `Srgb`, whatever the display and the format are.
    #[default]
    Auto,
    Srgb,
    DisplayP3,
    // scRGB: BT.709, linear, values above 1.0 are brighter than SDR white. Rgba16Float.
    ExtendedSrgbLinear,
    ExtendedSrgb,
    ExtendedDisplayP3,
    // BT.2020 primaries, PQ transfer, 0..1 is 0..10000 nits. Rgb10a2Unorm.
    Hdr10St2084,
    // BT.2020 primaries, HLG transfer. Rgb10a2Unorm.
    Hlg,
}

impl SurfaceColorSpace {
    pub const ALL: &'static [Self] = &[
        Self::Auto,
        Self::Srgb,
        Self::DisplayP3,
        Self::ExtendedSrgbLinear,
        Self::ExtendedSrgb,
        Self::ExtendedDisplayP3,
        Self::Hdr10St2084,
        Self::Hlg,
    ];

    pub const fn is_hdr(self) -> bool {
        matches!(
            self,
            Self::ExtendedSrgbLinear | Self::ExtendedSrgb | Self::ExtendedDisplayP3 | Self::Hdr10St2084 | Self::Hlg
        )
    }

    // `None` for `Auto`: it names no space of its own.
    pub const fn flag(self) -> Option<SurfaceColorSpaces> {
        match self {
            Self::Auto => None,
            Self::Srgb => Some(SurfaceColorSpaces::SRGB),
            Self::DisplayP3 => Some(SurfaceColorSpaces::DISPLAY_P3),
            Self::ExtendedSrgbLinear => Some(SurfaceColorSpaces::EXTENDED_SRGB_LINEAR),
            Self::ExtendedSrgb => Some(SurfaceColorSpaces::EXTENDED_SRGB),
            Self::ExtendedDisplayP3 => Some(SurfaceColorSpaces::EXTENDED_DISPLAY_P3),
            Self::Hdr10St2084 => Some(SurfaceColorSpaces::HDR10_ST2084),
            Self::Hlg => Some(SurfaceColorSpaces::HLG),
        }
    }

    // The format the space is meant for, tried before the surface's own preference order.
    pub const fn preferred_format(self) -> Option<Format> {
        match self {
            Self::ExtendedSrgbLinear | Self::ExtendedSrgb | Self::ExtendedDisplayP3 => Some(Format::Rgba16Float),
            Self::Hdr10St2084 | Self::Hlg => Some(Format::Rgb10a2Unorm),
            Self::Auto | Self::Srgb | Self::DisplayP3 => None,
        }
    }
}

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct SurfaceColorSpaces: u32 {
        const SRGB = 1 << 0;
        const DISPLAY_P3 = 1 << 1;
        const EXTENDED_SRGB_LINEAR = 1 << 2;
        const EXTENDED_SRGB = 1 << 3;
        const EXTENDED_DISPLAY_P3 = 1 << 4;
        const HDR10_ST2084 = 1 << 5;
        const HLG = 1 << 6;
    }
}

impl SurfaceColorSpaces {
    pub fn supports(self, space: SurfaceColorSpace) -> bool {
        space.flag().is_some_and(|flag| self.contains(flag))
    }

    pub fn spaces(self) -> impl Iterator<Item = SurfaceColorSpace> {
        SurfaceColorSpace::ALL
            .iter()
            .copied()
            .filter(move |space| self.supports(*space))
    }

    pub fn has_hdr(self) -> bool {
        self.spaces().any(SurfaceColorSpace::is_hdr)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SurfaceFormatCaps {
    pub format: Format,
    pub color_spaces: SurfaceColorSpaces,
}

// What the platform says about the display behind a surface. `None` is unknown, never zero and never SDR; the
// numbers are the OS's claim about the panel (advisory).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct DisplayHdr {
    pub peak_nits: Option<f32>,
    pub full_frame_nits: Option<f32>,
    pub sdr_white_nits: Option<f32>,
    // Linear multiplier of SDR white the display can drive now (1.0: none); what a tonemapper scales highlights by.
    pub headroom: Option<f32>,
}

// Surface formats in the surface's order of preference, each with the colour spaces it can be presented in.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SurfaceCaps {
    pub formats: Vec<SurfaceFormatCaps>,
    pub display: DisplayHdr,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SurfaceChoice {
    pub format: Format,
    // Never `Auto`.
    pub color_space: SurfaceColorSpace,
    // An explicit space the surface does not offer was replaced by `Srgb`.
    pub fell_back: bool,
}

impl SurfaceCaps {
    pub fn color_spaces_of(&self, format: Format) -> SurfaceColorSpaces {
        self.formats
            .iter()
            .filter(|caps| caps.format == format)
            .fold(SurfaceColorSpaces::empty(), |all, caps| all | caps.color_spaces)
    }

    pub fn color_spaces(&self) -> SurfaceColorSpaces {
        self.formats
            .iter()
            .fold(SurfaceColorSpaces::empty(), |all, caps| all | caps.color_spaces)
    }

    pub fn formats_with(&self, space: SurfaceColorSpace) -> impl Iterator<Item = Format> {
        self.formats
            .iter()
            .filter(move |caps| caps.color_spaces.supports(space))
            .map(|caps| caps.format)
    }

    // `Auto` is always `Srgb`: moving to HDR changes how the application must encode its output, so it is opt-in.
    // An explicit space the surface lacks is not an error: the caller shows `fell_back` as a diagnostic.
    pub fn resolve_color_space(&self, requested: SurfaceColorSpace) -> (SurfaceColorSpace, bool) {
        match requested {
            SurfaceColorSpace::Auto | SurfaceColorSpace::Srgb => (SurfaceColorSpace::Srgb, false),
            explicit if self.color_spaces().supports(explicit) => (explicit, false),
            _ => (SurfaceColorSpace::Srgb, true),
        }
    }

    // The format a space is presented in: the application's choice if the space allows it, then the space's own
    // format, then the surface's first (most preferred) one. SDR prefers sRGB formats, so rendering is gamma-correct.
    pub fn choose_format(&self, space: SurfaceColorSpace, wanted: Option<Format>) -> Option<Format> {
        let first = self.formats_with(space).next()?;
        let offered = |format: Format| self.color_spaces_of(format).supports(space);
        wanted
            .filter(|format| offered(*format))
            .or_else(|| space.preferred_format().filter(|format| offered(*format)))
            .or_else(|| {
                (!space.is_hdr())
                    .then(|| self.formats_with(space).find(|format| format.is_srgb()))
                    .flatten()
            })
            .or(Some(first))
    }

    pub fn resolve(&self, wanted_format: Option<Format>, requested: SurfaceColorSpace) -> Option<SurfaceChoice> {
        let (color_space, fell_back) = self.resolve_color_space(requested);
        let format = self.choose_format(color_space, wanted_format)?;
        Some(SurfaceChoice {
            format,
            color_space,
            fell_back,
        })
    }
}

// What a configured surface presents.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SurfaceOutput {
    pub format: Format,
    pub color_space: SurfaceColorSpace,
    pub display: DisplayHdr,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sdr_only() -> SurfaceCaps {
        SurfaceCaps {
            formats: vec![
                SurfaceFormatCaps {
                    format: Format::Bgra8Unorm,
                    color_spaces: SurfaceColorSpaces::SRGB,
                },
                SurfaceFormatCaps {
                    format: Format::Bgra8UnormSrgb,
                    color_spaces: SurfaceColorSpaces::SRGB,
                },
            ],
            display: DisplayHdr::default(),
        }
    }

    fn with_hdr() -> SurfaceCaps {
        let mut caps = sdr_only();
        caps.formats.push(SurfaceFormatCaps {
            format: Format::Rgba16Float,
            color_spaces: SurfaceColorSpaces::SRGB | SurfaceColorSpaces::EXTENDED_SRGB_LINEAR,
        });
        caps.formats.push(SurfaceFormatCaps {
            format: Format::Rgb10a2Unorm,
            color_spaces: SurfaceColorSpaces::SRGB | SurfaceColorSpaces::HDR10_ST2084,
        });
        caps
    }

    #[test]
    fn auto_never_resolves_to_hdr() {
        for caps in [sdr_only(), with_hdr()] {
            for wanted in [None, Some(Format::Rgba16Float), Some(Format::Rgb10a2Unorm)] {
                let choice = caps.resolve(wanted, SurfaceColorSpace::Auto).expect("a format");
                assert_eq!(choice.color_space, SurfaceColorSpace::Srgb);
                assert!(!choice.fell_back);
            }
        }
    }

    #[test]
    fn auto_with_a_float_format_stays_srgb_even_where_hdr_is_offered() {
        let choice = with_hdr()
            .resolve(Some(Format::Rgba16Float), SurfaceColorSpace::Auto)
            .expect("a format");
        assert_eq!(choice.format, Format::Rgba16Float);
        assert!(!choice.color_space.is_hdr());
    }

    #[test]
    fn auto_prefers_an_srgb_format() {
        let choice = sdr_only().resolve(None, SurfaceColorSpace::Auto).expect("a format");
        assert_eq!(choice.format, Format::Bgra8UnormSrgb);
    }

    #[test]
    fn an_offered_hdr_space_picks_its_own_format() {
        let caps = with_hdr();
        let scrgb = caps
            .resolve(Some(Format::Bgra8Unorm), SurfaceColorSpace::ExtendedSrgbLinear)
            .expect("a format");
        assert_eq!(
            (scrgb.format, scrgb.color_space, scrgb.fell_back),
            (Format::Rgba16Float, SurfaceColorSpace::ExtendedSrgbLinear, false)
        );
        let hdr10 = caps.resolve(None, SurfaceColorSpace::Hdr10St2084).expect("a format");
        assert_eq!((hdr10.format, hdr10.fell_back), (Format::Rgb10a2Unorm, false));
    }

    #[test]
    fn an_unoffered_hdr_space_falls_back_to_srgb() {
        let choice = sdr_only()
            .resolve(None, SurfaceColorSpace::ExtendedSrgbLinear)
            .expect("a format");
        assert_eq!(choice.color_space, SurfaceColorSpace::Srgb);
        assert!(choice.fell_back);
        let hlg = with_hdr().resolve(None, SurfaceColorSpace::Hlg).expect("a format");
        assert!(hlg.fell_back);
    }

    #[test]
    fn a_surface_without_formats_resolves_to_nothing() {
        assert_eq!(SurfaceCaps::default().resolve(None, SurfaceColorSpace::Auto), None);
    }

    #[test]
    fn only_the_extended_pq_and_hlg_spaces_are_hdr() {
        let hdr: Vec<_> = SurfaceColorSpace::ALL.iter().filter(|space| space.is_hdr()).collect();
        assert_eq!(hdr.len(), 5);
        assert!(!SurfaceColorSpace::Auto.is_hdr());
        assert!(!SurfaceColorSpace::Srgb.is_hdr());
        assert!(!SurfaceColorSpace::DisplayP3.is_hdr());
    }

    #[test]
    fn flags_round_trip_through_spaces() {
        for space in SurfaceColorSpace::ALL
            .iter()
            .filter(|space| **space != SurfaceColorSpace::Auto)
        {
            let flags = space.flag().expect("a flag");
            assert_eq!(flags.spaces().collect::<Vec<_>>(), vec![*space]);
        }
        assert!(SurfaceColorSpaces::empty().spaces().next().is_none());
        assert!(with_hdr().color_spaces().has_hdr());
        assert!(!sdr_only().color_spaces().has_hdr());
    }
}
