// IEEE 754 binary16 conversions with round to nearest even, like the GPU does on store.

const LARGEST_FINITE: f32 = 65504.0;

#[cfg(test)]
pub(crate) fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let mantissa = u32::from(bits & 0x3ff);
    let pattern = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, _) => {
            let mut exponent = 113;
            let mut mantissa = mantissa;
            while mantissa & 0x400 == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            sign | (exponent << 23) | ((mantissa & 0x3ff) << 13)
        }
        (31, _) => sign | 0x7f80_0000 | (mantissa << 13),
        _ => sign | ((exponent + 112) << 23) | (mantissa << 13),
    };
    f32::from_bits(pattern)
}

/// Radiance is never negative and a stored infinity would spread through every filter, so the value is saturated
/// into `0..=65504` first; `value` has to be finite.
pub(crate) fn radiance_to_f16(value: f32) -> u16 {
    let bits = value.clamp(0.0, LARGEST_FINITE).to_bits();
    let exponent = i32::try_from((bits >> 23) & 0xff).unwrap_or(0);
    let mantissa = bits & 0x7f_ffff;
    let half_exponent = exponent - 127 + 15;
    let (magnitude, dropped_bits) = if half_exponent <= 0 {
        if half_exponent < -10 {
            return 0;
        }
        (mantissa | 0x80_0000, u32::try_from(14 - half_exponent).unwrap_or(24))
    } else {
        ((u32::try_from(half_exponent).unwrap_or(0) << 23) | mantissa, 13)
    };
    let kept = magnitude >> dropped_bits;
    let remainder = magnitude & ((1 << dropped_bits) - 1);
    let halfway = 1 << (dropped_bits - 1);
    let rounded = if remainder > halfway || (remainder == halfway && kept & 1 == 1) {
        kept + 1
    } else {
        kept
    };
    u16::try_from(rounded).unwrap_or(0x7bff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_representable_values() {
        for value in [0.0, 1.0, 0.5, 3.25, 1024.0, 65504.0, 0.000_061_035_156] {
            assert_eq!(f16_to_f32(radiance_to_f16(value)), value, "{value}");
        }
    }

    #[test]
    fn saturates_instead_of_overflowing_and_drops_negatives() {
        assert_eq!(f16_to_f32(radiance_to_f16(1e9)), 65504.0);
        assert_eq!(f16_to_f32(radiance_to_f16(-3.0)), 0.0);
    }

    #[test]
    fn rounds_to_nearest_even() {
        // 1 + 2^-11 is exactly halfway between 1 and 1 + 2^-10 and rounds to the even mantissa (1.0).
        assert_eq!(f16_to_f32(radiance_to_f16(1.0 + 2.0_f32.powi(-11))), 1.0);
        assert_eq!(
            f16_to_f32(radiance_to_f16(1.0 + 3.0 * 2.0_f32.powi(-11))),
            1.0 + 2.0_f32.powi(-9)
        );
    }
}
