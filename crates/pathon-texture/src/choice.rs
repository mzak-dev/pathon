use pathon_api::Diagnostic;
use pathon_api::rhi::{Caps, Compression, Features, Format, FormatCaps};

use crate::error::TextureError;
use crate::source::{ColorSpace, TextureSource};

/// Which variant of a source to upload and how the GPU texture is typed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FormatChoice {
    /// Index into `TextureSource::variants`.
    pub variant: usize,
    /// Format of the allocation.
    pub storage: Format,
    /// Format of the views materials sample through: `storage`, or its sRGB twin for 8-bit colour, which is stored
    /// linear so that compute can write it.
    pub sampling: Format,
    /// A variant the source offered with a more compact encoding exists, but this device cannot sample it.
    pub fallback: bool,
}

const SAMPLEABLE: FormatCaps = FormatCaps::SAMPLED.union(FormatCaps::FILTERABLE);

pub(crate) fn choose_format(caps: &Caps, source: &TextureSource<'_>) -> Result<FormatChoice, TextureError> {
    let max = caps.limits.max_texture_dimension_2d;
    if source.width > max || source.height > max {
        return Err(TextureError::TooLarge {
            label: source.label,
            width: source.width,
            height: source.height,
            max,
        });
    }
    let mut best_offered = None;
    let mut best_usable: Option<(Rank, FormatChoice)> = None;
    for (index, variant) in source.variants.iter().enumerate() {
        let Some((storage, sampling)) = formats_for(variant.format(), source.color_space) else {
            continue;
        };
        let rank = rank_of(variant.format(), index);
        best_offered = Some(best_offered.map_or(rank, |best: Rank| best.min(rank)));
        let usable = is_sampleable(caps, storage) && is_sampleable(caps, sampling);
        if usable && best_usable.is_none_or(|(best, _)| rank < best) {
            let choice = FormatChoice {
                variant: index,
                storage,
                sampling,
                fallback: false,
            };
            best_usable = Some((rank, choice));
        }
    }
    match (best_usable, best_offered) {
        (Some((rank, choice)), Some(offered)) => Ok(FormatChoice {
            fallback: rank.tier > offered.tier,
            ..choice
        }),
        _ => Err(TextureError::NoSupportedVariant {
            label: source.label,
            offered: source
                .variants
                .iter()
                .map(|variant| format!("{:?}", variant.format()))
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

impl FormatChoice {
    /// A note for the log when the device sampled a less compact variant than the source offered (X0006).
    pub fn fallback_diagnostic(&self, source: &TextureSource<'_>) -> Option<Diagnostic> {
        self.fallback.then(|| {
            Diagnostic::warning(
                "X0006",
                format!(
                    "texture '{}' uses the fallback {:?}: the device cannot sample the more compact variants",
                    source.label, self.storage
                ),
            )
            .with_hint("expected on GPUs without BCn; ship ASTC or ETC2 variants to keep memory down there")
        })
    }
}

// Lower is better: compact encodings first, the order the source lists variants in breaks ties.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Rank {
    tier: u8,
    declared: usize,
}

fn rank_of(format: Format, declared: usize) -> Rank {
    let tier = match format.compression() {
        Some(Compression::Bc) => 0,
        Some(Compression::Astc) => 1,
        Some(Compression::Etc2) => 2,
        None => 3,
    };
    Rank { tier, declared }
}

// (storage, sampling) for a variant in the source's colour space; `None` when the colour space has no such format.
fn formats_for(linear: Format, color_space: ColorSpace) -> Option<(Format, Format)> {
    if color_space != ColorSpace::Srgb {
        return Some((linear, linear));
    }
    let srgb = srgb_twin(linear)?;
    Some(if linear.compression().is_some() {
        (srgb, srgb)
    } else {
        (linear, srgb)
    })
}

fn srgb_twin(linear: Format) -> Option<Format> {
    Some(match linear {
        Format::Rgba8Unorm => Format::Rgba8UnormSrgb,
        Format::Bgra8Unorm => Format::Bgra8UnormSrgb,
        Format::Bc1RgbaUnorm => Format::Bc1RgbaUnormSrgb,
        Format::Bc3RgbaUnorm => Format::Bc3RgbaUnormSrgb,
        Format::Bc7RgbaUnorm => Format::Bc7RgbaUnormSrgb,
        Format::Astc4x4Unorm => Format::Astc4x4UnormSrgb,
        _ => return None,
    })
}

fn is_sampleable(caps: &Caps, format: Format) -> bool {
    let feature = match format.compression() {
        Some(Compression::Bc) => Features::TEXTURE_COMPRESSION_BC,
        Some(Compression::Etc2) => Features::TEXTURE_COMPRESSION_ETC2,
        Some(Compression::Astc) => Features::TEXTURE_COMPRESSION_ASTC,
        None => Features::empty(),
    };
    caps.has(feature) && caps.format(format).contains(SAMPLEABLE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Variant;
    use crate::test_caps::mock_caps;

    const BC: [Format; 5] = [
        Format::Bc1RgbaUnorm,
        Format::Bc1RgbaUnormSrgb,
        Format::Bc7RgbaUnorm,
        Format::Bc7RgbaUnormSrgb,
        Format::Bc3RgbaUnorm,
    ];
    const ASTC: [Format; 2] = [Format::Astc4x4Unorm, Format::Astc4x4UnormSrgb];
    const ETC2: [Format; 2] = [Format::Etc2Rgb8Unorm, Format::Etc2Rgba8Unorm];
    fn caps(features: Features, compressed: &[&[Format]], max_dimension: u32) -> Caps {
        mock_caps(features, compressed, max_dimension, SAMPLEABLE)
    }

    fn desktop() -> Caps {
        caps(Features::TEXTURE_COMPRESSION_BC, &[&BC], 16384)
    }

    // Apple GPUs sample ASTC and ETC2 but not BCn.
    fn ios_like() -> Caps {
        let features = Features::TEXTURE_COMPRESSION_ASTC | Features::TEXTURE_COMPRESSION_ETC2;
        caps(features, &[&ASTC, &ETC2], 16384)
    }

    // A browser GPU that exposes no compressed format at all.
    fn web() -> Caps {
        caps(Features::empty(), &[], 8192)
    }

    fn source(color_space: ColorSpace, formats: &[Format]) -> TextureSource<'static> {
        formats
            .iter()
            .fold(TextureSource::new("t", 64, 64, color_space), |source, format| {
                source.with_variant(Variant::new(*format, vec![]))
            })
    }

    const OFFER: [Format; 4] = [
        Format::Bc7RgbaUnorm,
        Format::Astc4x4Unorm,
        Format::Etc2Rgba8Unorm,
        Format::Rgba8Unorm,
    ];

    #[test]
    fn choice_table() {
        use ColorSpace::{Linear, Normal, Srgb};
        use Format::{
            Astc4x4Unorm, Astc4x4UnormSrgb, Bc1RgbaUnorm, Bc7RgbaUnorm, Bc7RgbaUnormSrgb, Etc2Rgba8Unorm, Rgba8Unorm,
            Rgba8UnormSrgb, Rgba16Float,
        };
        struct Case {
            name: &'static str,
            device: Caps,
            color_space: ColorSpace,
            offered: Vec<Format>,
            expected: FormatChoice,
        }
        let case = |name, device, color_space, offered: &[Format], variant, storage, sampling, fallback| Case {
            name,
            device,
            color_space,
            offered: offered.to_vec(),
            expected: FormatChoice {
                variant,
                storage,
                sampling,
                fallback,
            },
        };
        let table = [
            case(
                "desktop prefers BC",
                desktop(),
                Srgb,
                &OFFER,
                0,
                Bc7RgbaUnormSrgb,
                Bc7RgbaUnormSrgb,
                false,
            ),
            case(
                "desktop linear",
                desktop(),
                Linear,
                &OFFER,
                0,
                Bc7RgbaUnorm,
                Bc7RgbaUnorm,
                false,
            ),
            case(
                "desktop falls back to raw",
                desktop(),
                Srgb,
                &[Astc4x4Unorm, Rgba8Unorm],
                1,
                Rgba8Unorm,
                Rgba8UnormSrgb,
                true,
            ),
            case(
                "ios takes ASTC",
                ios_like(),
                Srgb,
                &OFFER,
                1,
                Astc4x4UnormSrgb,
                Astc4x4UnormSrgb,
                true,
            ),
            case(
                "ios linear takes ASTC",
                ios_like(),
                Normal,
                &OFFER,
                1,
                Astc4x4Unorm,
                Astc4x4Unorm,
                true,
            ),
            case(
                "ios takes ETC2 without ASTC",
                ios_like(),
                Linear,
                &[Bc7RgbaUnorm, Etc2Rgba8Unorm, Rgba8Unorm],
                1,
                Etc2Rgba8Unorm,
                Etc2Rgba8Unorm,
                true,
            ),
            case(
                "no sRGB ETC2, so raw",
                ios_like(),
                Srgb,
                &[Bc7RgbaUnorm, Etc2Rgba8Unorm, Rgba8Unorm],
                2,
                Rgba8Unorm,
                Rgba8UnormSrgb,
                true,
            ),
            case(
                "web without compression",
                web(),
                Srgb,
                &OFFER,
                3,
                Rgba8Unorm,
                Rgba8UnormSrgb,
                true,
            ),
            case(
                "raw only is no fallback",
                web(),
                Srgb,
                &[Rgba8Unorm],
                0,
                Rgba8Unorm,
                Rgba8UnormSrgb,
                false,
            ),
            case(
                "HDR stays 16F",
                web(),
                Linear,
                &[Rgba16Float],
                0,
                Rgba16Float,
                Rgba16Float,
                false,
            ),
            case(
                "BC outranks raw whatever the declaration order",
                desktop(),
                Linear,
                &[Rgba8Unorm, Bc1RgbaUnorm],
                1,
                Bc1RgbaUnorm,
                Bc1RgbaUnorm,
                false,
            ),
        ];
        for Case {
            name,
            device,
            color_space,
            offered,
            expected,
        } in table
        {
            let choice = choose_format(&device, &source(color_space, &offered))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(choice, expected, "{name}");
        }
    }

    #[test]
    fn a_missing_feature_bit_disables_a_format_the_matrix_lists() {
        let mut device = desktop();
        device.features = Features::empty();
        let choice = choose_format(&device, &source(ColorSpace::Linear, &OFFER)).expect("raw fallback");
        assert_eq!(choice.storage, Format::Rgba8Unorm);
    }

    #[test]
    fn nothing_sampleable_is_x0001() {
        for (device, offered) in [
            (ios_like(), vec![Format::Bc7RgbaUnorm]),
            (web(), vec![Format::Bc7RgbaUnorm, Format::Astc4x4Unorm]),
            (desktop(), vec![Format::Rgba32Float]),
        ] {
            let error = choose_format(&device, &source(ColorSpace::Linear, &offered)).expect_err("nothing usable");
            assert_eq!(error.code(), "X0001", "{offered:?}");
        }
        let error =
            choose_format(&desktop(), &source(ColorSpace::Srgb, &[Format::Rgba16Float])).expect_err("no sRGB 16F");
        assert_eq!(error.code(), "X0001");
    }

    #[test]
    fn a_texture_beyond_the_device_limit_is_x0003() {
        let big = TextureSource::new("big", 16384, 4, ColorSpace::Linear).with_variant(Variant::rgba8(vec![]));
        assert_eq!(choose_format(&web(), &big).expect_err("8192 max").code(), "X0003");
        assert!(choose_format(&desktop(), &big).is_ok());
    }

    #[test]
    fn only_a_fallback_has_a_diagnostic() {
        let offered = source(ColorSpace::Srgb, &OFFER);
        let on_web = choose_format(&web(), &offered).expect("raw");
        let note = on_web.fallback_diagnostic(&offered).expect("fallback");
        assert_eq!(note.code, "X0006");
        let on_desktop = choose_format(&desktop(), &offered).expect("bc7");
        assert!(on_desktop.fallback_diagnostic(&offered).is_none());
    }
}
