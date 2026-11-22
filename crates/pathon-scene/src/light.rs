use pathon_api::glam::{Affine3A, Vec3};

use crate::error::SceneError;
use crate::layer::LayerMask;

// Illuminance below which a point or spot light is considered not to reach further when it has no explicit range.
pub const AUTO_RANGE_CUTOFF_LUX: f32 = 0.02;

#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub enum LightKind {
    Directional,
    Point,
    Spot {
        // Full intensity inside the inner cone, falling to zero at the outer one. Half-angles in radians.
        inner_angle: f32,
        outer_angle: f32,
    },
}

// Physical units: `intensity` is illuminance in lux for a directional light and luminous power in lumens for a
// point or spot light. `color` is a linear RGB tint (1.0 = no tint), not a brightness. The light shines along the
// local -Z axis of `transform`, like a camera looks.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct Light {
    pub kind: LightKind,
    pub transform: Affine3A,
    pub color: Vec3,
    pub intensity: f32,
    // Metres; `None` derives it from the intensity (`influence_radius`).
    pub range: Option<f32>,
    pub casts_shadows: bool,
    pub layers: LayerMask,
}

impl Light {
    fn new(kind: LightKind, transform: Affine3A, intensity: f32) -> Self {
        Self {
            kind,
            transform,
            color: Vec3::ONE,
            intensity,
            range: None,
            casts_shadows: false,
            layers: LayerMask::ALL,
        }
    }

    // Sun-like light; 100 000 lux is a clear midday sun.
    pub fn directional(transform: Affine3A, illuminance_lux: f32) -> Self {
        Self::new(LightKind::Directional, transform, illuminance_lux)
    }

    pub fn point(transform: Affine3A, luminous_power_lumens: f32) -> Self {
        Self::new(LightKind::Point, transform, luminous_power_lumens)
    }

    pub fn spot(transform: Affine3A, luminous_power_lumens: f32, inner_angle: f32, outer_angle: f32) -> Self {
        Self::new(
            LightKind::Spot {
                inner_angle,
                outer_angle,
            },
            transform,
            luminous_power_lumens,
        )
    }

    #[must_use]
    pub fn with_color(self, color: Vec3) -> Self {
        Self { color, ..self }
    }

    #[must_use]
    pub fn with_range(self, range: f32) -> Self {
        Self {
            range: Some(range),
            ..self
        }
    }

    #[must_use]
    pub fn with_shadows(self) -> Self {
        Self {
            casts_shadows: true,
            ..self
        }
    }

    #[must_use]
    pub fn with_layers(self, layers: LayerMask) -> Self {
        Self { layers, ..self }
    }

    pub fn position(&self) -> Vec3 {
        Vec3::from(self.transform.translation)
    }

    // Unit vector the light shines along.
    pub fn direction(&self) -> Vec3 {
        (self.transform.matrix3 * Vec3::NEG_Z).normalize_or_zero()
    }

    // Candela of a point or spot light. Spots use the same lumens / 4pi as points (glTF KHR_lights_punctual): the
    // cone shapes the light, it does not concentrate the power.
    pub fn luminous_intensity_candela(&self) -> Option<f32> {
        match self.kind {
            LightKind::Directional => None,
            LightKind::Point | LightKind::Spot { .. } => Some(self.intensity / (4.0 * std::f32::consts::PI)),
        }
    }

    pub fn illuminance_lux(&self) -> Option<f32> {
        matches!(self.kind, LightKind::Directional).then_some(self.intensity)
    }

    // Distance at which a point or spot light drops to `AUTO_RANGE_CUTOFF_LUX`, or the explicit range. Directional
    // lights reach everywhere (`None`).
    pub fn influence_radius(&self) -> Option<f32> {
        let candela = self.luminous_intensity_candela()?;
        Some(
            self.range
                .unwrap_or_else(|| (candela * self.color.max_element() / AUTO_RANGE_CUTOFF_LUX).sqrt()),
        )
    }

    pub(crate) fn validate(&self) -> Result<(), SceneError> {
        let reason = if !self.transform.is_finite() {
            return Err(SceneError::NotFinite {
                what: "light transform",
            });
        } else if !(self.intensity.is_finite() && self.intensity >= 0.0) {
            Some("intensity must be finite and not negative")
        } else if !(self.color.is_finite() && self.color.min_element() >= 0.0) {
            Some("colour must be finite and not negative")
        } else if self.range.is_some_and(|range| !(range.is_finite() && range > 0.0)) {
            Some("range must be positive")
        } else if let LightKind::Spot {
            inner_angle,
            outer_angle,
        } = self.kind
            && !(inner_angle > 0.0 && inner_angle <= outer_angle && outer_angle <= std::f32::consts::FRAC_PI_2)
        {
            Some("spot angles must satisfy 0 < inner <= outer <= pi/2")
        } else {
            None
        };
        reason.map_or(Ok(()), |reason| Err(SceneError::InvalidLight { reason }))
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::Quat;

    use super::*;

    #[test]
    fn a_light_shines_along_local_minus_z() {
        let light = Light::directional(Affine3A::IDENTITY, 1.0e5);
        assert_eq!(light.direction(), Vec3::NEG_Z);

        let down = Light::directional(
            Affine3A::from_quat(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            1.0,
        );
        assert!((down.direction() - Vec3::NEG_Y).length() < 1e-6);
    }

    #[test]
    fn point_candela_is_lumens_over_four_pi() {
        let light = Light::point(Affine3A::IDENTITY, 800.0);

        let candela = light.luminous_intensity_candela().expect("point light");

        assert!((candela - 800.0 / (4.0 * std::f32::consts::PI)).abs() < 1e-4);
        assert_eq!(light.illuminance_lux(), None);
    }

    #[test]
    fn directional_light_has_lux_and_no_candela_or_radius() {
        let light = Light::directional(Affine3A::IDENTITY, 100_000.0);

        assert_eq!(light.illuminance_lux(), Some(100_000.0));
        assert_eq!(light.luminous_intensity_candela(), None);
        assert_eq!(light.influence_radius(), None);
    }

    #[test]
    fn influence_radius_follows_inverse_square_and_explicit_range_wins() {
        let light = Light::point(Affine3A::IDENTITY, 800.0);
        let candela = 800.0 / (4.0 * std::f32::consts::PI);
        let radius = light.influence_radius().expect("point light");

        assert!((candela / (radius * radius) - AUTO_RANGE_CUTOFF_LUX).abs() < 1e-4);
        assert_eq!(light.with_range(3.0).influence_radius(), Some(3.0));
    }

    #[test]
    fn invalid_lights_are_rejected() {
        let at = Affine3A::IDENTITY;
        let bad = [
            Light::point(at, -1.0),
            Light::point(at, f32::INFINITY),
            Light::point(at, 1.0).with_color(Vec3::new(1.0, -0.1, 1.0)),
            Light::point(at, 1.0).with_range(0.0),
            Light::spot(at, 1.0, 0.0, 0.5),
            Light::spot(at, 1.0, 0.6, 0.5),
            Light::spot(at, 1.0, 0.5, 2.0),
        ];

        for light in bad {
            assert!(
                matches!(light.validate(), Err(SceneError::InvalidLight { .. })),
                "{light:?}"
            );
        }
        assert!(Light::spot(at, 1.0, 0.3, 0.5).validate().is_ok());
        assert!(matches!(
            Light::point(Affine3A::from_translation(Vec3::NAN), 1.0).validate(),
            Err(SceneError::NotFinite { .. })
        ));
    }
}
