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
    // A flat rectangle in the local XY plane, `width` along X and `height` along Y, emitting like a Lambertian surface
    // towards local -Z (and towards +Z too when `two_sided`). `intensity` is its total luminous power (ADR-0048).
    Area {
        width: f32,
        height: f32,
        two_sided: bool,
    },
}

// Physical units: `intensity` is illuminance in lux for a directional light and luminous power in lumens for a
// point, spot or area light. `color` is a linear RGB tint (1.0 = no tint), not a brightness. The light shines along the
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

    // Rectangle of `width` x `height` metres emitting on one side (towards local -Z). `luminous_power_lumens` is the
    // whole power: the luminance is `power / (pi * area)`, see `luminance_nits`.
    pub fn area(transform: Affine3A, luminous_power_lumens: f32, width: f32, height: f32) -> Self {
        Self::new(
            LightKind::Area {
                width,
                height,
                two_sided: false,
            },
            transform,
            luminous_power_lumens,
        )
    }

    // An area light that emits from both faces; its power is split equally between them.
    #[must_use]
    pub fn with_two_sided(self) -> Self {
        match self.kind {
            LightKind::Area { width, height, .. } => Self {
                kind: LightKind::Area {
                    width,
                    height,
                    two_sided: true,
                },
                ..self
            },
            _ => self,
        }
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
    // cone shapes the light, it does not concentrate the power. For an area light it is the peak, on the axis: a
    // Lambertian emitter of power P has I(0) = P / pi (P / 2pi per face when two-sided).
    pub fn luminous_intensity_candela(&self) -> Option<f32> {
        match self.kind {
            LightKind::Point | LightKind::Spot { .. } => Some(self.intensity / (4.0 * std::f32::consts::PI)),
            LightKind::Area { two_sided, .. } => Some(self.intensity / (std::f32::consts::PI * faces(two_sided))),
            _ => None,
        }
    }

    // Luminance of an area light in nits (cd / m^2), the same on every point and in every direction of an emitting
    // face: L = P / (pi * A) per face, so that the radiant exitance pi * L * A adds up to P.
    pub fn luminance_nits(&self) -> Option<f32> {
        match self.kind {
            LightKind::Area {
                width,
                height,
                two_sided,
            } => Some(self.intensity / (std::f32::consts::PI * faces(two_sided) * width * height)),
            _ => None,
        }
    }

    // Distance from the centre of an area light to its farthest corner.
    pub fn area_circumradius(&self) -> Option<f32> {
        match self.kind {
            LightKind::Area { width, height, .. } => Some(0.5 * width.hypot(height)),
            _ => None,
        }
    }

    pub fn illuminance_lux(&self) -> Option<f32> {
        matches!(self.kind, LightKind::Directional).then_some(self.intensity)
    }

    // Distance at which a point or spot light drops to `AUTO_RANGE_CUTOFF_LUX`, or the explicit range. Directional
    // lights reach everywhere (`None`). An area light never lights a point with more than its peak intensity over the
    // squared distance to the rectangle (form factor <= A / (pi d^2)), so its radius is that distance plus the one
    // from the centre to the farthest corner.
    pub fn influence_radius(&self) -> Option<f32> {
        let candela = self.luminous_intensity_candela()?;
        Some(self.range.unwrap_or_else(|| {
            (candela * self.color.max_element() / AUTO_RANGE_CUTOFF_LUX).sqrt() + self.area_circumradius().unwrap_or(0.0)
        }))
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
        } else if let LightKind::Area { width, height, .. } = self.kind
            && !(width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0)
        {
            Some("area light width and height must be positive")
        } else {
            None
        };
        reason.map_or(Ok(()), |reason| Err(SceneError::InvalidLight { reason }))
    }
}

fn faces(two_sided: bool) -> f32 {
    if two_sided { 2.0 } else { 1.0 }
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
    fn area_luminance_times_area_times_pi_is_the_power() {
        let light = Light::area(Affine3A::IDENTITY, 1000.0, 2.0, 0.5);

        let luminance = light.luminance_nits().expect("area light");

        assert!((luminance * std::f32::consts::PI * 1.0 - 1000.0).abs() < 1e-2);
        let candela = light.luminous_intensity_candela().expect("area light");
        assert!((candela - 1000.0 / std::f32::consts::PI).abs() < 1e-3, "I(0) = L * A = P / pi");
        let two_sided = light.with_two_sided();
        assert!((two_sided.luminance_nits().expect("area light") * 2.0 - luminance).abs() < 1e-3);
        assert_eq!(Light::point(Affine3A::IDENTITY, 1.0).luminance_nits(), None);
    }

    #[test]
    fn area_influence_radius_reaches_past_the_farthest_corner() {
        let light = Light::area(Affine3A::IDENTITY, 800.0, 3.0, 4.0);
        let candela = 800.0 / std::f32::consts::PI;

        let radius = light.influence_radius().expect("area light");

        assert!((radius - ((candela / AUTO_RANGE_CUTOFF_LUX).sqrt() + 2.5)).abs() < 1e-3);
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
            Light::area(at, 1.0, 0.0, 1.0),
            Light::area(at, 1.0, 1.0, f32::NAN),
        ];

        for light in bad {
            assert!(
                matches!(light.validate(), Err(SceneError::InvalidLight { .. })),
                "{light:?}"
            );
        }
        assert!(Light::spot(at, 1.0, 0.3, 0.5).validate().is_ok());
        assert!(Light::area(at, 1.0, 0.5, 2.0).validate().is_ok());
        assert!(matches!(
            Light::point(Affine3A::from_translation(Vec3::NAN), 1.0).validate(),
            Err(SceneError::NotFinite { .. })
        ));
    }
}
