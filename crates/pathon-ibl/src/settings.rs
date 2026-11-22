use crate::IblError;

/// Resolutions and sample counts of one build. Every size is a power of two.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IblSettings {
    /// Face size of the prefiltered cube (mip 0, the sharp environment) and of the scratch cube it is made from.
    pub cube_size: u32,
    /// Face size of the irradiance cube: it holds only a very smooth function.
    pub irradiance_size: u32,
    /// GGX samples per prefiltered texel.
    pub prefilter_samples: u32,
    /// Side of the square BRDF lookup table.
    pub lut_size: u32,
    /// GGX samples per texel of the lookup table.
    pub lut_samples: u32,
}

impl IblSettings {
    pub const MIN_CUBE_SIZE: u32 = 16;
    pub const MIN_MAP_SIZE: u32 = 4;
    pub const MAX_CUBE_SIZE: u32 = 2048;
    pub const MAX_SAMPLES: u32 = 16384;

    /// Desktop profile: a 256 cube has six roughness levels and keeps the sun of a 2k picture sharp.
    pub const DESKTOP: Self = Self {
        cube_size: 256,
        irradiance_size: 32,
        prefilter_samples: 1024,
        lut_size: 128,
        lut_samples: 1024,
    };

    /// Web and mobile: the same shaders at a quarter of the texels and a quarter of the samples.
    pub const COMPACT: Self = Self {
        cube_size: 128,
        irradiance_size: 16,
        prefilter_samples: 256,
        lut_size: 64,
        lut_samples: 512,
    };

    /// Mips of the prefiltered cube down to 8 x 8 texels per face: smaller ones blur away the direction of rough
    /// reflections (ADR-0037).
    pub fn prefiltered_levels(&self) -> u32 {
        self.cube_size.ilog2() - 2
    }

    /// Mip levels of the scratch cube the prefilter reads: down to a single texel.
    pub fn source_levels(&self) -> u32 {
        self.cube_size.ilog2() + 1
    }

    pub fn validate(&self) -> Result<(), IblError> {
        let invalid = |reason: String| IblError::InvalidSettings { reason };
        for (name, size, smallest) in [
            ("cube_size", self.cube_size, Self::MIN_CUBE_SIZE),
            ("irradiance_size", self.irradiance_size, Self::MIN_MAP_SIZE),
            ("lut_size", self.lut_size, Self::MIN_MAP_SIZE),
        ] {
            if !size.is_power_of_two() || !(smallest..=Self::MAX_CUBE_SIZE).contains(&size) {
                return Err(invalid(format!(
                    "{name} is {size}, expected a power of two in {smallest}..={}",
                    Self::MAX_CUBE_SIZE
                )));
            }
        }
        for (name, samples) in [
            ("prefilter_samples", self.prefilter_samples),
            ("lut_samples", self.lut_samples),
        ] {
            if !(1..=Self::MAX_SAMPLES).contains(&samples) {
                return Err(invalid(format!(
                    "{name} is {samples}, expected 1..={}",
                    Self::MAX_SAMPLES
                )));
            }
        }
        Ok(())
    }
}

impl Default for IblSettings {
    fn default() -> Self {
        Self::DESKTOP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_are_valid() {
        assert_eq!(IblSettings::DESKTOP.validate(), Ok(()));
        assert_eq!(IblSettings::COMPACT.validate(), Ok(()));
    }

    #[test]
    fn levels_run_down_to_eight_texels_per_face() {
        assert_eq!(IblSettings::DESKTOP.prefiltered_levels(), 6);
        assert_eq!(IblSettings::COMPACT.prefiltered_levels(), 5);
        assert_eq!(IblSettings::DESKTOP.source_levels(), 9);
    }

    #[test]
    fn a_size_that_is_not_a_power_of_two_is_i0001() {
        let settings = IblSettings {
            cube_size: 200,
            ..IblSettings::DESKTOP
        };
        assert_eq!(settings.validate().expect_err("200").code(), "I0001");
        let settings = IblSettings {
            prefilter_samples: 0,
            ..IblSettings::DESKTOP
        };
        assert_eq!(settings.validate().expect_err("no samples").code(), "I0001");
    }
}
