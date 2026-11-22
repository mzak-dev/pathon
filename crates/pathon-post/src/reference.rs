//! CPU twins of the shader maths: the tests compare the GPU against them, and they document the formulas.

use crate::settings::Tonemapper;

pub const HISTOGRAM_BINS: usize = 256;
pub const MAX_FINITE_HALF: f32 = 65504.0;
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// NaN and negative values become 0, `+Inf` the largest half float: one broken texel must not spread through
/// the bloom or the histogram.
pub fn sanitize_channel(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, MAX_FINITE_HALF)
    }
}

pub fn sanitize(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(sanitize_channel)
}

pub fn luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

pub fn srgb_oetf(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

fn multiply(rows: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    rows.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

/// Hill's fit of the ACES RRT and ODT; the `1 / 0.6` is the usual exposure bias that puts middle grey where
/// the real transform puts it.
pub fn aces_fitted(rgb: [f32; 3]) -> [f32; 3] {
    const INPUT: [[f32; 3]; 3] = [
        [0.597_19, 0.354_58, 0.048_23],
        [0.076_00, 0.908_34, 0.015_66],
        [0.028_40, 0.133_83, 0.837_77],
    ];
    const OUTPUT: [[f32; 3]; 3] = [
        [1.604_75, -0.531_08, -0.073_67],
        [-0.102_08, 1.108_13, -0.006_05],
        [-0.003_27, -0.072_76, 1.076_02],
    ];
    let fit = |v: f32| {
        let a = v * (v + 0.024_578_6) - 0.000_090_537;
        let b = v * (0.983_729 * v + 0.432_951) + 0.238_081;
        a / b
    };
    let biased = rgb.map(|channel| channel / 0.6);
    multiply(OUTPUT, multiply(INPUT, biased).map(fit)).map(|channel| channel.clamp(0.0, 1.0))
}

/// Khronos PBR Neutral: identity (minus a small toe offset) up to the shoulder, then a smooth highlight
/// compression that desaturates towards white.
pub fn pbr_neutral(rgb: [f32; 3]) -> [f32; 3] {
    const START_COMPRESSION: f32 = 0.8 - 0.04;
    const DESATURATION: f32 = 0.15;
    let lowest = rgb[0].min(rgb[1]).min(rgb[2]);
    let offset = if lowest < 0.08 {
        lowest - 6.25 * lowest * lowest
    } else {
        0.04
    };
    let shifted = rgb.map(|channel| channel - offset);
    let peak = shifted[0].max(shifted[1]).max(shifted[2]);
    if peak < START_COMPRESSION {
        return shifted;
    }
    let d = 1.0 - START_COMPRESSION;
    let new_peak = 1.0 - d * d / (peak + d - START_COMPRESSION);
    let scaled = shifted.map(|channel| channel * new_peak / peak);
    let blend = 1.0 - 1.0 / (DESATURATION * (peak - new_peak) + 1.0);
    scaled.map(|channel| channel + (new_peak - channel) * blend)
}

pub fn tonemap(operator: Tonemapper, rgb: [f32; 3]) -> [f32; 3] {
    match operator {
        Tonemapper::AcesFitted => aces_fitted(rgb),
        Tonemapper::PbrNeutral => pbr_neutral(rgb),
    }
}

/// What the tonemap pass writes to an 8-bit `Unorm` target for exposed, bloomed scene colour.
pub fn display_rgb8(operator: Tonemapper, rgb: [f32; 3]) -> [u8; 3] {
    tonemap(operator, sanitize(rgb)).map(|channel| {
        let encoded = srgb_oetf(channel.clamp(0.0, 1.0)) * 255.0;
        #[expect(clippy::cast_possible_truncation, reason = "rounded and clamped to 0..=255")]
        #[expect(clippy::cast_sign_loss, reason = "the value is clamped to be non-negative")]
        let byte = encoded.round().clamp(0.0, 255.0) as u8;
        byte
    })
}

pub fn manual_multiplier(ev: f32, compensation_ev: f32) -> f32 {
    (compensation_ev - ev).exp2()
}

/// Share of the remaining distance covered in `delta_seconds` by an exponential approach with the given time
/// constant; no time passed means no movement, a zero time constant means an instant jump.
pub fn adaptation_coefficient(delta_seconds: f32, time_constant_seconds: f32) -> f32 {
    if delta_seconds <= 0.0 {
        0.0
    } else if time_constant_seconds <= 0.0 {
        1.0
    } else {
        1.0 - (-delta_seconds / time_constant_seconds).exp()
    }
}

/// Auto-exposure works on `log2(exposure)`, so brightening and darkening are symmetric in stops.
pub fn adapt_log2_exposure(
    previous_log2: Option<f32>,
    target_log2: f32,
    brighter_coefficient: f32,
    darker_coefficient: f32,
) -> f32 {
    let Some(previous) = previous_log2 else {
        return target_log2;
    };
    let coefficient = if target_log2 < previous {
        brighter_coefficient
    } else {
        darker_coefficient
    };
    previous + (target_log2 - previous) * coefficient
}

/// Parameters of the luminance histogram: `HISTOGRAM_BINS` bins over `[min_ev, max_ev]` (bin 0 holds what is
/// darker and is ignored), then percentile clipping.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HistogramRange {
    pub min_ev: f32,
    pub max_ev: f32,
    pub low_percentile: f32,
    pub high_percentile: f32,
}

fn histogram_bin(luminance: f32, range: HistogramRange) -> usize {
    if luminance <= 0.0 {
        return 0;
    }
    let position = (luminance.log2() - range.min_ev) / (range.max_ev - range.min_ev);
    if position < 0.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the position is non-negative and clamped below"
    )]
    #[expect(clippy::cast_sign_loss, reason = "the position is non-negative")]
    let bin = (position * (HISTOGRAM_BINS - 1) as f32) as usize;
    1 + bin.min(HISTOGRAM_BINS - 2)
}

/// Average `log2` luminance as the histogram pass computes it; `None` when nothing is brighter than `min_ev`.
pub fn average_log2_luminance(pixels: &[[f32; 3]], range: HistogramRange) -> Option<f32> {
    let mut counts = [0_u32; HISTOGRAM_BINS];
    for pixel in pixels {
        counts[histogram_bin(luminance(sanitize(*pixel)), range)] += 1;
    }
    let total = counts[1..].iter().map(|count| *count as f32).sum::<f32>();
    let (low, high) = (total * range.low_percentile, total * range.high_percentile);
    let (mut cumulative, mut weighted, mut weight) = (0.0_f32, 0.0_f32, 0.0_f32);
    for (bin, count) in counts.iter().enumerate().skip(1) {
        let start = cumulative;
        cumulative += *count as f32;
        let inside = (cumulative.min(high) - start.max(low)).max(0.0);
        let center =
            range.min_ev + ((bin - 1) as f32 + 0.5) / (HISTOGRAM_BINS - 1) as f32 * (range.max_ev - range.min_ev);
        weighted += inside * center;
        weight += inside;
    }
    (weight > 0.0).then(|| weighted / weight)
}

/// Soft threshold of the bloom prefilter on the brightest channel of exposed colour.
pub fn bloom_prefilter(rgb: [f32; 3], threshold: f32, knee: f32) -> [f32; 3] {
    let brightness = rgb[0].max(rgb[1]).max(rgb[2]);
    let soft = (brightness - threshold + knee).clamp(0.0, 2.0 * knee);
    let soft = soft * soft / (4.0 * knee + 1.0e-5);
    let contribution = soft.max(brightness - threshold) / brightness.max(1.0e-5);
    rgb.map(|channel| channel * contribution)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: HistogramRange = HistogramRange {
        min_ev: -9.0,
        max_ev: 13.0,
        low_percentile: 0.1,
        high_percentile: 0.95,
    };

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn srgb_oetf_matches_known_points() {
        assert!(close(srgb_oetf(0.0), 0.0, 1.0e-6));
        assert!(close(srgb_oetf(1.0), 1.0, 1.0e-6));
        assert!(close(srgb_oetf(0.18), 0.461_356, 1.0e-4));
        assert!(close(srgb_oetf(0.002), 0.025_84, 1.0e-5));
    }

    #[test]
    fn both_operators_map_zero_to_black_and_stay_in_range() {
        for operator in [Tonemapper::AcesFitted, Tonemapper::PbrNeutral] {
            assert!(
                tonemap(operator, [0.0; 3]).iter().all(|c| c.abs() < 0.01),
                "{operator:?}"
            );
            for exposure in [0.01, 0.18, 1.0, 16.0, 1000.0] {
                let mapped = tonemap(operator, [exposure, exposure * 0.5, exposure * 0.1]);
                assert!(
                    mapped.iter().all(|c| (0.0..=1.0 + 1.0e-4).contains(c)),
                    "{operator:?} {mapped:?}"
                );
            }
        }
    }

    #[test]
    fn aces_fitted_maps_middle_grey_near_half_display_brightness() {
        // Hill's fit by hand for a neutral v = 0.18 / 0.6: 0.0973 / 0.4565 = 0.213, which sRGB encodes as 0.505.
        let mapped = aces_fitted([0.18; 3]);

        assert!(close(mapped[0], 0.213, 0.005), "{mapped:?}");
        assert!(close(mapped[0], mapped[2], 0.02), "neutral stays neutral: {mapped:?}");
    }

    #[test]
    fn pbr_neutral_is_nearly_identity_below_the_shoulder() {
        let mapped = pbr_neutral([0.5, 0.4, 0.3]);

        assert!(close(mapped[0], 0.46, 1.0e-4), "{mapped:?}");
        assert!(close(mapped[2], 0.26, 1.0e-4), "{mapped:?}");
    }

    #[test]
    fn pbr_neutral_compresses_highlights_towards_white() {
        let mapped = pbr_neutral([100.0, 100.0, 100.0]);

        assert!(mapped.iter().all(|c| *c > 0.97 && *c <= 1.0), "{mapped:?}");
    }

    #[test]
    fn sanitize_removes_nan_and_clamps_infinity() {
        let broken = sanitize([f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
        let ordinary = sanitize([-1.0, 2.0, 0.0]);

        assert!(broken[0].abs() < 1.0e-9 && broken[2].abs() < 1.0e-9);
        assert!(close(broken[1], MAX_FINITE_HALF, 1.0e-3));
        assert!(ordinary[0].abs() < 1.0e-9 && close(ordinary[1], 2.0, 1.0e-9));
    }

    #[test]
    fn histogram_average_of_a_grey_card_is_within_a_bin() {
        let pixels = vec![[0.18; 3]; 4096];

        let average = average_log2_luminance(&pixels, RANGE).expect("lit pixels");

        assert!(close(average, 0.18_f32.log2(), 0.05), "{average}");
    }

    #[test]
    fn histogram_ignores_black_and_broken_pixels() {
        let mut pixels = vec![[0.0; 3]; 1000];
        pixels.extend(vec![[f32::NAN; 3]; 10]);

        assert_eq!(average_log2_luminance(&pixels, RANGE), None);
    }

    #[test]
    fn percentile_clipping_drops_a_few_very_bright_pixels() {
        let mut pixels = vec![[0.18; 3]; 990];
        pixels.extend(vec![[5000.0; 3]; 10]);

        let average = average_log2_luminance(&pixels, RANGE).expect("lit pixels");

        assert!(close(average, 0.18_f32.log2(), 0.05), "{average}");
    }

    #[test]
    fn adaptation_moves_a_share_of_the_distance_and_is_symmetric_in_stops() {
        let coefficient = adaptation_coefficient(1.0, 1.0);
        assert!(close(coefficient, 0.632_1, 1.0e-3));
        assert!(close(
            adapt_log2_exposure(Some(0.0), 4.0, 1.0, coefficient),
            4.0 * coefficient,
            1.0e-6
        ));
        assert!(close(
            adapt_log2_exposure(Some(0.0), -4.0, coefficient, 1.0),
            -4.0 * coefficient,
            1.0e-6
        ));
        assert!(close(adapt_log2_exposure(None, 3.0, 0.0, 0.0), 3.0, 1.0e-6));
        assert!(close(adaptation_coefficient(0.0, 1.0), 0.0, 0.0));
        assert!(close(adaptation_coefficient(0.1, 0.0), 1.0, 0.0));
    }

    #[test]
    fn prefilter_passes_everything_without_a_threshold_and_cuts_below_it() {
        let colour = [0.4, 0.2, 0.1];

        assert!(
            bloom_prefilter(colour, 0.0, 0.0)
                .iter()
                .zip(colour)
                .all(|(a, b)| close(*a, b, 1.0e-6))
        );
        assert!(bloom_prefilter(colour, 1.0, 0.0).iter().all(|c| c.abs() < 1.0e-9));
        assert!(bloom_prefilter([3.0, 0.0, 0.0], 1.0, 0.0)[0] > 1.9);
    }
}
