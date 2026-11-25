/// Sizes of the lookup tables and steps of the marches: fixed when the pack is created (they size pipelines and
/// textures), unlike [`crate::SkySettings`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SkyQuality {
    pub transmittance_size: (u32, u32),
    pub multiscatter_size: u32,
    pub sky_view_size: (u32, u32),
    pub transmittance_steps: u32,
    pub multiscatter_steps: u32,
    pub sky_view_steps: u32,
}

impl SkyQuality {
    /// The sizes of Hillaire's paper: the tables cost well under a millisecond on a desktop GPU.
    pub const DESKTOP: Self = Self {
        transmittance_size: (256, 64),
        multiscatter_size: 32,
        sky_view_size: (192, 108),
        transmittance_steps: 40,
        multiscatter_steps: 20,
        sky_view_steps: 32,
    };

    /// Web and mobile: a quarter of the texels and about half of the steps.
    pub const COMPACT: Self = Self {
        transmittance_size: (128, 32),
        multiscatter_size: 16,
        sky_view_size: (128, 72),
        transmittance_steps: 24,
        multiscatter_steps: 12,
        sky_view_steps: 20,
    };

    pub const MIN_TABLE_SIZE: u32 = 8;
    pub const MAX_TABLE_SIZE: u32 = 1024;
    pub const MAX_STEPS: u32 = 256;

    /// What is wrong with the numbers; empty for usable quality.
    pub fn problems(&self) -> Vec<String> {
        let size_ok = |size: u32| (Self::MIN_TABLE_SIZE..=Self::MAX_TABLE_SIZE).contains(&size);
        let steps_ok = |steps: u32| (1..=Self::MAX_STEPS).contains(&steps);
        let mut problems = Vec::new();
        let sizes = [
            self.transmittance_size.0,
            self.transmittance_size.1,
            self.multiscatter_size,
            self.sky_view_size.0,
            self.sky_view_size.1,
        ];
        if !sizes.into_iter().all(size_ok) {
            problems.push(format!(
                "table sizes must be in {}..={}",
                Self::MIN_TABLE_SIZE,
                Self::MAX_TABLE_SIZE
            ));
        }
        if ![self.transmittance_steps, self.multiscatter_steps, self.sky_view_steps]
            .into_iter()
            .all(steps_ok)
        {
            problems.push(format!("march steps must be in 1..={}", Self::MAX_STEPS));
        }
        problems
    }
}

impl Default for SkyQuality {
    fn default() -> Self {
        Self::DESKTOP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_are_valid() {
        assert_eq!(SkyQuality::DESKTOP.problems(), Vec::<String>::new());
        assert_eq!(SkyQuality::COMPACT.problems(), Vec::<String>::new());
    }

    #[test]
    fn a_zero_size_is_a_problem() {
        let quality = SkyQuality {
            multiscatter_size: 0,
            ..SkyQuality::DESKTOP
        };
        assert_eq!(quality.problems().len(), 1);
    }
}
