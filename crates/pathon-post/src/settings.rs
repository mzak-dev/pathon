/// Everything the application can tune. Lives in `Shared`, so it can change between frames
/// (`Renderer::shared_mut().get_mut::<PostSettings>()`).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PostSettings {
    pub exposure: ExposureSettings,
    pub bloom: BloomSettings,
    pub tonemap: TonemapSettings,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct ExposureSettings {
    pub mode: ExposureMode,
    /// Stops added on top of the mode: positive is brighter.
    pub compensation_ev: f32,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ExposureMode {
    Auto(AutoExposure),
    /// The multiplier is `2^-ev`: `ev = 0` leaves `hdr` unchanged and each stop halves it.
    Manual {
        ev: f32,
    },
}

impl Default for ExposureMode {
    fn default() -> Self {
        Self::Auto(AutoExposure::default())
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct AutoExposure {
    /// Darkest and brightest average luminance (log2 of the `hdr` value) the histogram tells apart.
    pub min_ev: f32,
    pub max_ev: f32,
    /// Fractions of the histogram cut off at both ends before averaging, so a few very dark or very bright
    /// pixels do not decide the exposure.
    pub low_percentile: f32,
    pub high_percentile: f32,
    /// Display value the average luminance is brought to (0.18 is middle grey).
    pub target_luminance: f32,
    /// Time constants of the adaptation in seconds; zero adapts at once. The scene getting brighter lowers the
    /// exposure, and the eye does that faster than the opposite.
    pub to_brighter_seconds: f32,
    pub to_darker_seconds: f32,
}

impl Default for AutoExposure {
    fn default() -> Self {
        Self {
            min_ev: -9.0,
            max_ev: 13.0,
            low_percentile: 0.1,
            high_percentile: 0.95,
            target_luminance: 0.18,
            to_brighter_seconds: 0.3,
            to_darker_seconds: 1.0,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct BloomSettings {
    pub enabled: bool,
    /// Share of the filtered bright energy added to the image.
    pub intensity: f32,
    /// Exposed brightness below which nothing glows, and the width of the soft transition above it.
    pub threshold: f32,
    pub knee: f32,
    /// How much of each wider level reaches the result: 0 keeps only the sharpest glow, 1 only the widest.
    pub scatter: f32,
    /// Levels of the pyramid, each half the size of the previous one; more levels give a wider glow.
    pub levels: u32,
    /// Weighs the first downsample by `1 / (1 + luma)` so a single very bright pixel does not flicker.
    pub karis_average: bool,
}

impl BloomSettings {
    pub const MAX_LEVELS: u32 = 8;
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            intensity: 0.15,
            threshold: 1.0,
            knee: 0.5,
            scatter: 0.7,
            levels: 6,
            karis_average: true,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct TonemapSettings {
    pub operator: Tonemapper,
    pub output: OutputTransfer,
}

/// Curves from exposed scene-linear colour to display-linear `[0, 1]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub enum Tonemapper {
    /// ACES fitted (Hill): filmic, with the hue shifts of the real ACES output transform.
    #[default]
    AcesFitted,
    /// Khronos PBR Neutral: keeps base colours of materials below the shoulder, compresses only highlights.
    PbrNeutral,
    /// No curve and no upper clamp: the exposed scene colour as it is, for float targets that a tool reads back
    /// (image comparison). On an 8-bit target it clips at 1.
    Linear,
}

impl Tonemapper {
    pub(crate) const fn shader_constant(self) -> u32 {
        match self {
            Self::AcesFitted => 0,
            Self::PbrNeutral => 1,
            Self::Linear => 2,
        }
    }
}

/// How display-linear colour becomes the numbers of the target.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
#[non_exhaustive]
pub enum OutputTransfer {
    /// SDR: sRGB OETF, applied by the `*Srgb` view of the target or in the shader for other 8-bit formats.
    #[default]
    Srgb,
    /// HDR10: PQ with `peak_nits` mapped to the top of the range. Not implemented yet (M4-16, Q0002).
    Pq { peak_nits: f32 },
    /// scRGB: linear, 1.0 is `paper_white_nits`. Not implemented yet (M4-16, Q0002).
    ScRgb { paper_white_nits: f32 },
}

impl OutputTransfer {
    pub(crate) const fn is_implemented(self) -> bool {
        matches!(self, Self::Srgb)
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::Pq { .. } => "PQ",
            Self::ScRgb { .. } => "scRGB",
        }
    }
}

fn fix(problems: &mut Vec<String>, name: &str, value: &mut f32, min: f32, max: f32, fallback: f32) {
    let fixed = if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    };
    if fixed.to_bits() != value.to_bits() {
        problems.push(format!("{name} = {value} is outside [{min}, {max}], using {fixed}"));
        *value = fixed;
    }
}

impl PostSettings {
    /// Puts every value in its valid range and says what had to change.
    #[must_use]
    pub fn sanitized(mut self) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        fix(
            &mut problems,
            "exposure.compensation_ev",
            &mut self.exposure.compensation_ev,
            -16.0,
            16.0,
            0.0,
        );
        match &mut self.exposure.mode {
            ExposureMode::Manual { ev } => fix(&mut problems, "exposure.manual ev", ev, -32.0, 32.0, 0.0),
            ExposureMode::Auto(auto) => {
                fix(&mut problems, "auto.min_ev", &mut auto.min_ev, -24.0, 24.0, -9.0);
                fix(&mut problems, "auto.max_ev", &mut auto.max_ev, -24.0, 24.0, 13.0);
                if auto.max_ev < auto.min_ev + 1.0 {
                    problems.push(format!(
                        "auto.max_ev {} is not above min_ev {}",
                        auto.max_ev, auto.min_ev
                    ));
                    auto.max_ev = auto.min_ev + 1.0;
                }
                fix(
                    &mut problems,
                    "auto.low_percentile",
                    &mut auto.low_percentile,
                    0.0,
                    0.9,
                    0.1,
                );
                fix(
                    &mut problems,
                    "auto.high_percentile",
                    &mut auto.high_percentile,
                    0.1,
                    1.0,
                    0.95,
                );
                if auto.high_percentile <= auto.low_percentile {
                    problems.push("auto.high_percentile must exceed low_percentile".to_owned());
                    auto.low_percentile = 0.0;
                    auto.high_percentile = 1.0;
                }
                fix(
                    &mut problems,
                    "auto.target_luminance",
                    &mut auto.target_luminance,
                    1.0e-3,
                    1.0,
                    0.18,
                );
                fix(
                    &mut problems,
                    "auto.to_brighter_seconds",
                    &mut auto.to_brighter_seconds,
                    0.0,
                    60.0,
                    0.3,
                );
                fix(
                    &mut problems,
                    "auto.to_darker_seconds",
                    &mut auto.to_darker_seconds,
                    0.0,
                    60.0,
                    1.0,
                );
            }
        }
        let bloom = &mut self.bloom;
        fix(&mut problems, "bloom.intensity", &mut bloom.intensity, 0.0, 4.0, 0.15);
        fix(&mut problems, "bloom.threshold", &mut bloom.threshold, 0.0, 1.0e4, 1.0);
        fix(&mut problems, "bloom.knee", &mut bloom.knee, 0.0, 1.0e4, 0.5);
        fix(&mut problems, "bloom.scatter", &mut bloom.scatter, 0.0, 1.0, 0.7);
        let levels = bloom.levels.clamp(1, BloomSettings::MAX_LEVELS);
        if levels != bloom.levels {
            problems.push(format!(
                "bloom.levels = {} is outside [1, {}], using {levels}",
                bloom.levels,
                BloomSettings::MAX_LEVELS
            ));
            bloom.levels = levels;
        }
        (self, problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_already_valid() {
        let (settings, problems) = PostSettings::default().sanitized();

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(settings, PostSettings::default());
    }

    #[test]
    fn non_finite_and_out_of_range_values_are_replaced_and_reported() {
        let mut settings = PostSettings::default();
        settings.bloom.intensity = f32::NAN;
        settings.bloom.levels = 99;
        settings.exposure.compensation_ev = 1000.0;

        let (fixed, problems) = settings.sanitized();

        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(fixed.bloom.intensity.is_finite());
        assert_eq!(fixed.bloom.levels, BloomSettings::MAX_LEVELS);
        assert!((fixed.exposure.compensation_ev - 16.0).abs() < f32::EPSILON);
    }

    #[test]
    fn inverted_histogram_range_is_repaired() {
        let mut settings = PostSettings::default();
        settings.exposure.mode = ExposureMode::Auto(AutoExposure {
            min_ev: 5.0,
            max_ev: 2.0,
            ..AutoExposure::default()
        });

        let (fixed, problems) = settings.sanitized();

        let ExposureMode::Auto(auto) = fixed.exposure.mode else {
            panic!("mode stays auto");
        };
        assert!(auto.max_ev > auto.min_ev);
        assert_eq!(problems.len(), 1);
    }
}
