// IEEE 754 binary16 conversions for the CPU mip reference; round to nearest even like the GPU does on store.

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

pub(crate) fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = u16::try_from((bits >> 16) & 0x8000).unwrap_or(0);
    let exponent = i32::try_from((bits >> 23) & 0xff).unwrap_or(0);
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        return sign | 0x7c00 | if mantissa == 0 { 0 } else { 0x200 };
    }
    let half_exponent = exponent - 127 + 15;
    if half_exponent >= 31 {
        return sign | 0x7c00;
    }
    let (magnitude, dropped_bits) = if half_exponent <= 0 {
        if half_exponent < -10 {
            return sign;
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
    // A carry out of the mantissa bumps the exponent, which is the correct rounding (and reaches infinity).
    sign | u16::try_from(rounded).unwrap_or(0x7c00)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_trip_through_f32() {
        for bits in (0..=u16::MAX).filter(|bits| (bits >> 10) & 0x1f != 31) {
            assert_eq!(f32_to_f16(f16_to_f32(bits)), bits, "{bits:#06x}");
        }
    }

    #[test]
    fn known_values() {
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(-2.0), 0xc000);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f32_to_f16(65504.0), 0x7bff);
        assert_eq!(f32_to_f16(1.0e6), 0x7c00, "overflow is infinity");
        assert_eq!(f16_to_f32(0x0001), 2.0_f32.powi(-24));
        assert!(f16_to_f32(0x7e00).is_nan());
    }

    #[test]
    fn ties_round_to_even() {
        // 1 + 2^-11 lies exactly between 1.0 and 1 + 2^-10.
        assert_eq!(f32_to_f16(1.0 + 2.0_f32.powi(-11)), 0x3c00);
        assert_eq!(f32_to_f16(1.0 + 3.0 * 2.0_f32.powi(-11)), 0x3c02);
    }
}
