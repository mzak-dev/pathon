/// How the camera's depth range is cut into cascades. The first cascade starts at the camera's near plane and the last
/// ends at `ShadowSettings::max_distance` (or the camera's far plane when that is nearer).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SplitScheme {
    /// Practical split scheme: `lambda` blends uniform (0) and logarithmic (1) splits (Zhang et al., 2006).
    Practical { lambda: f32 },
    /// Split positions as fractions of the shadowed depth range, `0 < a < b < c < 1`; only the first
    /// `cascade_count - 1` are read. A typical choice is `[0.1, 0.25, 0.5]`.
    Fractions([f32; 3]),
}

impl Default for SplitScheme {
    fn default() -> Self {
        Self::Practical { lambda: 0.6 }
    }
}

/// How a cascade's orthographic box is fitted around its slice of the camera frustum.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CascadeFit {
    /// Bounding sphere of the slice, snapped to the texel grid. The box does not change size when the camera turns,
    /// and a shadow edge does not swim when it moves: the right choice for a moving camera.
    #[default]
    Stable,
    /// Tight box around the slice in light space. Up to about twice the resolution of `Stable` on a wide, flat slice,
    /// but edges shimmer when the camera moves; for still images and slow cameras.
    Focused,
}

/// Softness of the shadow edge. Every filter reads a hardware comparison sampler, so each tap is already a bilinear
/// 2x2 percentage-closer filter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ShadowFilter {
    /// One tap: hard, aliased edges.
    Hard,
    /// Four taps that add up to a box filter three texels wide: smooth, no noise.
    #[default]
    Pcf3,
    /// Sixteen taps of a Poisson disc of radius 2.5 texels, rotated per pixel by interleaved gradient noise when
    /// `rotate_filter` is set: the softest edge, the noise is high-frequency and hides under TAA.
    Pcf5,
}

impl ShadowFilter {
    pub(crate) const fn shader_value(self) -> u32 {
        match self {
            Self::Hard => 0,
            Self::Pcf3 => 1,
            Self::Pcf5 => 2,
        }
    }
}

/// Everything the application can tune; lives in `Shared`, so it can change between frames. Values outside the
/// documented ranges are repaired and reported once as `D0001`.
#[derive(Clone, PartialEq, Debug)]
pub struct ShadowSettings {
    /// 1 to [`ShadowSettings::MAX_CASCADES`].
    pub cascade_count: u32,
    /// Side of every cascade's depth map, 64 to 8192 (and within the device's texture limit).
    pub resolution: u32,
    /// Distance from the camera, in metres, after which there are no shadows; shadows fade out over the last
    /// `fade_fraction` of it.
    pub max_distance: f32,
    pub splits: SplitScheme,
    pub fit: CascadeFit,
    pub filter: ShadowFilter,
    /// Rotates the `Pcf5` disc per pixel; off gives a clean pattern, on trades banding for noise.
    pub rotate_filter: bool,
    /// Casters are moved this far (metres) away from the light before they are rendered.
    pub constant_bias: f32,
    /// Receivers are moved along their normal by `sin(theta) * normal_bias` texels of their cascade, where `theta` is
    /// the angle between the normal and the light (1.0 is right for `Pcf3`).
    pub normal_bias: f32,
    /// Hardware polygon offset of the caster pass: a constant in depth steps and a multiple of the polygon's depth
    /// slope.
    pub polygon_offset_constant: f32,
    pub polygon_offset_slope: f32,
    /// Share of a cascade's depth range, counted from its far end, over which it blends into the next cascade.
    pub blend_fraction: f32,
    /// Share of `max_distance`, counted from its far end, over which shadows fade to light.
    pub fade_fraction: f32,
    /// How far (metres) in front of a cascade a caster without known bounds may stand and still shadow it.
    pub unbounded_caster_distance: f32,
    /// Draw the casters of a cascade with one `multi_draw_indexed_indirect` per mesh buffer when the device allows
    /// first-instance indirect draws; `false` always draws one by one.
    pub use_indirect_draws: bool,
}

impl ShadowSettings {
    pub const MAX_CASCADES: u32 = 4;
    pub const MIN_RESOLUTION: u32 = 64;
    pub const MAX_RESOLUTION: u32 = 8192;

    /// Four 2048 cascades over 150 m.
    pub const DESKTOP: Self = Self {
        cascade_count: 4,
        resolution: 2048,
        max_distance: 150.0,
        splits: SplitScheme::Practical { lambda: 0.6 },
        fit: CascadeFit::Stable,
        filter: ShadowFilter::Pcf3,
        rotate_filter: true,
        constant_bias: 0.002,
        normal_bias: 1.0,
        polygon_offset_constant: 1.0,
        polygon_offset_slope: 1.5,
        blend_fraction: 0.1,
        fade_fraction: 0.1,
        unbounded_caster_distance: 200.0,
        use_indirect_draws: true,
    };

    /// Three 1024 cascades over 80 m, for the Web profile and mobile GPUs.
    pub const COMPACT: Self = Self {
        cascade_count: 3,
        resolution: 1024,
        max_distance: 80.0,
        ..Self::DESKTOP
    };

    /// Clamps every value into its range and lists what was wrong; NaN becomes the default.
    pub(crate) fn sanitized(&self, max_texture_size: u32, max_layers: u32) -> (Self, Vec<String>) {
        let mut repaired = self.clone();
        let mut problems = Vec::new();
        let defaults = Self::DESKTOP;

        let cascades = self.cascade_count.clamp(1, Self::MAX_CASCADES.min(max_layers.max(1)));
        if cascades != self.cascade_count {
            problems.push(format!("cascade_count {} -> {cascades}", self.cascade_count));
            repaired.cascade_count = cascades;
        }
        let limit = Self::MAX_RESOLUTION.min(max_texture_size.max(Self::MIN_RESOLUTION));
        let resolution = self.resolution.clamp(Self::MIN_RESOLUTION, limit);
        if resolution != self.resolution {
            problems.push(format!("resolution {} -> {resolution}", self.resolution));
            repaired.resolution = resolution;
        }
        let mut float = |name: &str, value: f32, default: f32, range: (f32, f32)| {
            let fixed = if value.is_finite() {
                value.clamp(range.0, range.1)
            } else {
                default
            };
            if fixed.to_bits() != value.to_bits() {
                problems.push(format!("{name} {value} -> {fixed}"));
            }
            fixed
        };
        repaired.max_distance = float(
            "max_distance",
            self.max_distance,
            defaults.max_distance,
            (0.5, 100_000.0),
        );
        repaired.constant_bias = float("constant_bias", self.constant_bias, defaults.constant_bias, (0.0, 10.0));
        repaired.normal_bias = float("normal_bias", self.normal_bias, defaults.normal_bias, (0.0, 16.0));
        repaired.polygon_offset_constant = float(
            "polygon_offset_constant",
            self.polygon_offset_constant,
            defaults.polygon_offset_constant,
            (0.0, 1.0e6),
        );
        repaired.polygon_offset_slope = float(
            "polygon_offset_slope",
            self.polygon_offset_slope,
            defaults.polygon_offset_slope,
            (0.0, 1000.0),
        );
        repaired.blend_fraction = float(
            "blend_fraction",
            self.blend_fraction,
            defaults.blend_fraction,
            (0.0, 0.5),
        );
        repaired.fade_fraction = float("fade_fraction", self.fade_fraction, defaults.fade_fraction, (0.0, 1.0));
        repaired.unbounded_caster_distance = float(
            "unbounded_caster_distance",
            self.unbounded_caster_distance,
            defaults.unbounded_caster_distance,
            (0.0, 1.0e6),
        );
        repaired.splits = match self.splits {
            SplitScheme::Practical { lambda } => SplitScheme::Practical {
                lambda: float("splits.lambda", lambda, 0.6, (0.0, 1.0)),
            },
            SplitScheme::Fractions(fractions) => {
                let fallback = [0.125, 0.25, 0.5];
                let mut fixed = fractions;
                for (fraction, default) in fixed.iter_mut().zip(fallback) {
                    *fraction = float("splits.fractions", *fraction, default, (0.001, 0.999));
                }
                if !(fixed[0] < fixed[1] && fixed[1] < fixed[2]) {
                    problems.push(format!("splits.fractions {fractions:?} -> {fallback:?}"));
                    fixed = fallback;
                }
                SplitScheme::Fractions(fixed)
            }
        };
        (repaired, problems)
    }
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self::DESKTOP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_are_inside_every_range() {
        for preset in [ShadowSettings::DESKTOP, ShadowSettings::COMPACT] {
            let (repaired, problems) = preset.sanitized(8192, 256);
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(repaired, preset);
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_and_listed() {
        let settings = ShadowSettings {
            cascade_count: 9,
            resolution: 3,
            max_distance: f32::NAN,
            normal_bias: -1.0,
            splits: SplitScheme::Fractions([0.5, 0.2, 0.9]),
            ..ShadowSettings::DESKTOP
        };

        let (repaired, problems) = settings.sanitized(4096, 256);

        assert_eq!(repaired.cascade_count, 4);
        assert_eq!(repaired.resolution, ShadowSettings::MIN_RESOLUTION);
        assert_eq!(
            repaired.max_distance.to_bits(),
            ShadowSettings::DESKTOP.max_distance.to_bits()
        );
        assert_eq!(repaired.normal_bias.to_bits(), 0.0_f32.to_bits());
        assert_eq!(repaired.splits, SplitScheme::Fractions([0.125, 0.25, 0.5]));
        assert_eq!(problems.len(), 5, "{problems:?}");
    }

    #[test]
    fn the_device_limits_cap_cascades_and_resolution() {
        let (repaired, problems) = ShadowSettings::DESKTOP.sanitized(1024, 2);

        assert_eq!((repaired.cascade_count, repaired.resolution), (2, 1024));
        assert_eq!(problems.len(), 2);
    }
}
