/// Centre and half width of the ozone layer, whose absorption density is a tent over altitude (Hillaire 2020).
pub const OZONE_CENTER_KM: f64 = 25.0;
pub const OZONE_HALF_WIDTH_KM: f64 = 15.0;

/// Participating media of a planet: Rayleigh and Mie scattering plus ozone absorption. Lengths are kilometres and
/// coefficients per kilometre, as in the paper, so f32 keeps its precision.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Atmosphere {
    pub bottom_radius_km: f32,
    pub top_radius_km: f32,
    pub rayleigh_scattering: [f32; 3],
    pub rayleigh_scale_height_km: f32,
    pub mie_scattering: [f32; 3],
    pub mie_extinction: [f32; 3],
    pub mie_scale_height_km: f32,
    /// Henyey-Greenstein-like asymmetry of the Mie phase function (Cornette-Shanks), `-1..1`.
    pub mie_anisotropy: f32,
    pub ozone_absorption: [f32; 3],
}

impl Atmosphere {
    /// Earth at sea level: coefficients from Hillaire's reference parameters.
    pub const EARTH: Self = Self {
        bottom_radius_km: 6360.0,
        top_radius_km: 6460.0,
        rayleigh_scattering: [5.802e-3, 13.558e-3, 33.1e-3],
        rayleigh_scale_height_km: 8.0,
        mie_scattering: [3.996e-3; 3],
        mie_extinction: [4.44e-3; 3],
        mie_scale_height_km: 1.2,
        mie_anisotropy: 0.8,
        ozone_absorption: [0.650e-3, 1.881e-3, 0.085e-3],
    };

    pub fn thickness_km(&self) -> f32 {
        self.top_radius_km - self.bottom_radius_km
    }

    /// What is wrong with the numbers, one sentence each; empty for a usable atmosphere.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let finite_non_negative = |values: &[f32; 3]| values.iter().all(|value| value.is_finite() && *value >= 0.0);
        let radii_ok = self.bottom_radius_km.is_finite()
            && self.top_radius_km.is_finite()
            && self.bottom_radius_km >= 1.0
            && self.top_radius_km > self.bottom_radius_km
            && self.thickness_km() <= self.bottom_radius_km;
        if !radii_ok {
            problems.push(format!(
                "radii {} and {} km are not a thin atmosphere over a planet",
                self.bottom_radius_km, self.top_radius_km
            ));
        }
        for (name, values) in [
            ("rayleigh_scattering", &self.rayleigh_scattering),
            ("mie_scattering", &self.mie_scattering),
            ("mie_extinction", &self.mie_extinction),
            ("ozone_absorption", &self.ozone_absorption),
        ] {
            if !finite_non_negative(values) {
                problems.push(format!("{name} has a negative or non-finite coefficient"));
            }
        }
        if self
            .mie_scattering
            .iter()
            .zip(self.mie_extinction)
            .any(|(scattering, extinction)| scattering > &extinction)
        {
            problems.push("mie_extinction is smaller than mie_scattering".into());
        }
        for (name, height) in [
            ("rayleigh_scale_height_km", self.rayleigh_scale_height_km),
            ("mie_scale_height_km", self.mie_scale_height_km),
        ] {
            if !(height.is_finite() && height > 0.0) {
                problems.push(format!("{name} is {height}, expected a positive height"));
            }
        }
        if !(self.mie_anisotropy.is_finite() && self.mie_anisotropy.abs() <= 0.99) {
            problems.push(format!(
                "mie_anisotropy is {}, expected -0.99..=0.99",
                self.mie_anisotropy
            ));
        }
        problems
    }
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self::EARTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earth_is_a_usable_atmosphere() {
        assert_eq!(Atmosphere::EARTH.problems(), Vec::<String>::new());
    }

    #[test]
    fn broken_numbers_are_listed() {
        let atmosphere = Atmosphere {
            top_radius_km: 6000.0,
            mie_anisotropy: 1.5,
            rayleigh_scale_height_km: 0.0,
            ..Atmosphere::EARTH
        };
        assert_eq!(atmosphere.problems().len(), 3);
    }
}
