use std::ops::RangeInclusive;

use pathon_api::glam::Vec3;
use pathon_ibl::IblSettings;

use crate::Atmosphere;

/// Where the height of the camera above the ground comes from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CameraHeight {
    /// `Camera::position.y` plus an offset (the height of the scene's ground over the planet surface).
    FromCamera {
        ground_offset_meters: f32,
    },
    Fixed {
        meters: f32,
    },
}

/// When the sky is baked into an environment for image-based lighting (ADR-0045). The prefilter is the expensive part,
/// so a new environment is made only after a change above these thresholds and not more often than the interval.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SkyIblSettings {
    pub enabled: bool,
    /// Width of the equirectangular picture; the height is half of it.
    pub image_width: u32,
    pub ibl: IblSettings,
    pub min_sun_angle_degrees: f32,
    /// Relative change of the sun illuminance, and absolute change of cloud coverage and ground albedo.
    pub min_relative_change: f32,
    pub min_altitude_change_meters: f32,
    /// Measured on `ViewCtx::time_seconds`; `0` rebuilds on every change above the thresholds.
    pub min_interval_seconds: f32,
}

impl SkyIblSettings {
    pub const MIN_IMAGE_WIDTH: u32 = 16;
    pub const MAX_IMAGE_WIDTH: u32 = 1024;
}

impl Default for SkyIblSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            image_width: 128,
            ibl: IblSettings::COMPACT,
            min_sun_angle_degrees: 1.0,
            min_relative_change: 0.05,
            min_altitude_change_meters: 100.0,
            min_interval_seconds: 0.5,
        }
    }
}

/// The sky of one renderer, read from `Shared` every frame, so the application changes it live.
#[derive(Clone, PartialEq, Debug)]
pub struct SkySettings {
    /// World space, Y up; the direction from the scene towards the sun. Need not be normalised.
    pub sun_direction: Vec3,
    /// Illuminance of a surface facing the sun above the atmosphere, lux. About 120 000 lux for the real sun.
    pub sun_illuminance_lux: f32,
    /// Linear colour the sun illuminance is multiplied by; white is the real sun.
    pub sun_tint: [f32; 3],
    /// Radius of the sun disc as seen from the planet, radians (0.0047 for the real sun).
    pub sun_angular_radius: f32,
    pub draw_sun_disc: bool,
    /// The disc is brighter than a binary16 `hdr` slot holds, so its radiance is capped here (nits).
    pub sun_disc_max_radiance: f32,
    /// `0` is a clear sky, `1` a uniform overcast; between them the two skies are blended (ADR-0045).
    pub cloud_coverage: f32,
    pub ground_albedo: [f32; 3],
    pub camera_height: CameraHeight,
    pub atmosphere: Atmosphere,
    pub ibl: SkyIblSettings,
}

impl SkySettings {
    pub const MAX_ILLUMINANCE_LUX: f32 = 1.0e6;
    pub const MAX_DISC_RADIANCE: f32 = 60_000.0;
    pub const MIN_CAMERA_ALTITUDE_KM: f32 = 0.001;

    /// The sun at `elevation_degrees` over the horizon in the direction of `azimuth_degrees` (0 = -Z, 90 = +X).
    #[must_use]
    pub fn with_sun(mut self, elevation_degrees: f32, azimuth_degrees: f32) -> Self {
        let (elevation, azimuth) = (elevation_degrees.to_radians(), azimuth_degrees.to_radians());
        self.sun_direction = Vec3::new(
            elevation.cos() * azimuth.sin(),
            elevation.sin(),
            -elevation.cos() * azimuth.cos(),
        );
        self
    }

    /// The settings with every value inside its range, and what had to be repaired.
    pub fn sanitized(&self) -> (Self, Vec<String>) {
        let mut repaired = self.clone();
        let mut problems = Vec::new();
        let default = Self::default();

        if self.sun_direction.is_finite() && self.sun_direction.length_squared() > 1e-12 {
            repaired.sun_direction = self.sun_direction.normalize();
        } else {
            problems.push(format!("sun_direction {} is not a direction", self.sun_direction));
            repaired.sun_direction = default.sun_direction;
        }
        repaired.sun_illuminance_lux = clamp_or(
            self.sun_illuminance_lux,
            0.0..=Self::MAX_ILLUMINANCE_LUX,
            default.sun_illuminance_lux,
            "sun_illuminance_lux",
            &mut problems,
        );
        for (channel, name) in ["sun_tint.r", "sun_tint.g", "sun_tint.b"].into_iter().enumerate() {
            repaired.sun_tint[channel] = clamp_or(self.sun_tint[channel], 0.0..=16.0, 1.0, name, &mut problems);
        }
        repaired.sun_angular_radius = clamp_or(
            self.sun_angular_radius,
            1.0e-4..=0.05,
            default.sun_angular_radius,
            "sun_angular_radius",
            &mut problems,
        );
        repaired.sun_disc_max_radiance = clamp_or(
            self.sun_disc_max_radiance,
            1.0..=Self::MAX_DISC_RADIANCE,
            default.sun_disc_max_radiance,
            "sun_disc_max_radiance",
            &mut problems,
        );
        repaired.cloud_coverage = clamp_or(self.cloud_coverage, 0.0..=1.0, 0.0, "cloud_coverage", &mut problems);
        for (channel, name) in ["ground_albedo.r", "ground_albedo.g", "ground_albedo.b"]
            .into_iter()
            .enumerate()
        {
            repaired.ground_albedo[channel] =
                clamp_or(self.ground_albedo[channel], 0.0..=1.0, 0.3, name, &mut problems);
        }
        repaired.camera_height = match self.camera_height {
            CameraHeight::Fixed { meters } => CameraHeight::Fixed {
                meters: clamp_or(meters, 0.0..=1.0e5, 0.0, "camera_height", &mut problems),
            },
            CameraHeight::FromCamera { ground_offset_meters } => CameraHeight::FromCamera {
                ground_offset_meters: clamp_or(
                    ground_offset_meters,
                    -1.0e4..=1.0e5,
                    0.0,
                    "ground_offset_meters",
                    &mut problems,
                ),
            },
        };
        let atmosphere_problems = self.atmosphere.problems();
        if !atmosphere_problems.is_empty() {
            problems.extend(
                atmosphere_problems
                    .into_iter()
                    .map(|problem| format!("atmosphere: {problem}")),
            );
            repaired.atmosphere = Atmosphere::EARTH;
        }
        repaired.ibl = self.ibl.sanitized(&mut problems);
        (repaired, problems)
    }

    /// Height of the camera over the ground before it is limited to the atmosphere, metres.
    pub fn camera_altitude_meters(&self, camera_position_y: f32) -> f32 {
        let meters = match self.camera_height {
            CameraHeight::FromCamera { ground_offset_meters } => camera_position_y + ground_offset_meters,
            CameraHeight::Fixed { meters } => meters,
        };
        if meters.is_finite() { meters } else { 0.0 }
    }

    /// Distance of the camera from the planet's centre, kilometres, with the camera kept inside the atmosphere.
    pub fn camera_radius_km(&self, camera_position_y: f32) -> f32 {
        let altitude = (self.camera_altitude_meters(camera_position_y) / 1000.0).clamp(
            Self::MIN_CAMERA_ALTITUDE_KM,
            self.atmosphere.thickness_km() - Self::MIN_CAMERA_ALTITUDE_KM,
        );
        self.atmosphere.bottom_radius_km + altitude
    }
}

impl SkyIblSettings {
    fn sanitized(&self, problems: &mut Vec<String>) -> Self {
        let mut repaired = *self;
        let default = Self::default();
        let width = self.image_width.clamp(Self::MIN_IMAGE_WIDTH, Self::MAX_IMAGE_WIDTH) & !1;
        if width != self.image_width {
            problems.push(format!(
                "ibl.image_width is {}, expected an even value in {}..={}",
                self.image_width,
                Self::MIN_IMAGE_WIDTH,
                Self::MAX_IMAGE_WIDTH
            ));
            repaired.image_width = width;
        }
        if let Err(error) = self.ibl.validate() {
            problems.push(format!("ibl.ibl: {error}"));
            repaired.ibl = default.ibl;
        }
        repaired.min_sun_angle_degrees = clamp_or(
            self.min_sun_angle_degrees,
            0.0..=90.0,
            default.min_sun_angle_degrees,
            "ibl.min_sun_angle_degrees",
            problems,
        );
        repaired.min_relative_change = clamp_or(
            self.min_relative_change,
            0.0..=1.0,
            default.min_relative_change,
            "ibl.min_relative_change",
            problems,
        );
        repaired.min_altitude_change_meters = clamp_or(
            self.min_altitude_change_meters,
            0.0..=1.0e5,
            default.min_altitude_change_meters,
            "ibl.min_altitude_change_meters",
            problems,
        );
        repaired.min_interval_seconds = clamp_or(
            self.min_interval_seconds,
            0.0..=3600.0,
            default.min_interval_seconds,
            "ibl.min_interval_seconds",
            problems,
        );
        repaired
    }
}

impl Default for SkySettings {
    fn default() -> Self {
        Self {
            sun_direction: Vec3::Y,
            sun_illuminance_lux: 120_000.0,
            sun_tint: [1.0; 3],
            sun_angular_radius: 0.004_65,
            draw_sun_disc: true,
            sun_disc_max_radiance: 50_000.0,
            cloud_coverage: 0.0,
            ground_albedo: [0.3; 3],
            camera_height: CameraHeight::FromCamera {
                ground_offset_meters: 0.0,
            },
            atmosphere: Atmosphere::EARTH,
            ibl: SkyIblSettings::default(),
        }
        .with_sun(45.0, 0.0)
    }
}

// NaN takes the fallback, a value outside the range is clamped; either one is a problem to report.
fn clamp_or(value: f32, range: RangeInclusive<f32>, fallback: f32, name: &str, problems: &mut Vec<String>) -> f32 {
    if !value.is_finite() {
        problems.push(format!("{name} is {value}, using {fallback}"));
        return fallback;
    }
    let clamped = value.clamp(*range.start(), *range.end());
    if clamped.to_bits() != value.to_bits() {
        problems.push(format!("{name} is {value}, clamped to {clamped}"));
    }
    clamped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_need_no_repair() {
        let (repaired, problems) = SkySettings::default().sanitized();
        assert_eq!(problems, Vec::<String>::new());
        assert!((repaired.sun_direction.length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn broken_values_are_repaired_and_listed() {
        let settings = SkySettings {
            sun_direction: Vec3::ZERO,
            cloud_coverage: 3.0,
            sun_illuminance_lux: f32::NAN,
            ground_albedo: [0.2, -1.0, 0.2],
            ..SkySettings::default()
        };
        let (repaired, problems) = settings.sanitized();
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!((repaired.cloud_coverage - 1.0).abs() < f32::EPSILON);
        assert!(repaired.ground_albedo[1].abs() < f32::EPSILON);
        assert!(repaired.sun_illuminance_lux.is_finite());
    }

    #[test]
    fn the_sun_goes_where_elevation_and_azimuth_say() {
        let north = SkySettings::default().with_sun(30.0, 0.0).sun_direction;
        assert!((north.y - 0.5).abs() < 1e-6 && north.z < 0.0 && north.x.abs() < 1e-6);
        let east = SkySettings::default().with_sun(0.0, 90.0).sun_direction;
        assert!((east.x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_camera_stays_inside_the_atmosphere() {
        let settings = SkySettings::default();
        let top = settings.atmosphere.top_radius_km;
        assert!(settings.camera_radius_km(1.0e9) < top);
        assert!(settings.camera_radius_km(-50.0) > settings.atmosphere.bottom_radius_km);
        assert!(settings.camera_radius_km(f32::NAN) > settings.atmosphere.bottom_radius_km);
    }
}
