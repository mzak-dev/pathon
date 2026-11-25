use pathon_api::glam::Vec3;
use pathon_api::{Camera, Shared};

use crate::SkySettings;
use crate::reference::{self, Planet, SkyConditions};

/// The sun as the rest of the renderer sees it: published in `Shared` every frame by the sky, so lighting and shadows
/// follow the sky without depending on its crate's internals. Plain data; clouds and the atmosphere are already in it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SunLight {
    /// Unit vector from the scene towards the sun, world space.
    pub direction_to_sun: Vec3,
    /// Illuminance in lux on a surface facing the sun, per linear RGB channel: the sun above the atmosphere, dimmed
    /// and reddened by its path through it, and by the cloud cover. Zero while the planet hides the sun.
    pub illuminance_lux: [f32; 3],
    /// Radius of the disc as seen from the camera, radians.
    pub angular_radius: f32,
}

impl SunLight {
    /// Single number for a light that has one intensity: the luminance of [`Self::illuminance_lux`].
    pub fn luminous_illuminance(&self) -> f32 {
        let [red, green, blue] = self.illuminance_lux;
        0.2126 * red + 0.7152 * green + 0.0722 * blue
    }
}

/// The settings of the application (or the defaults), repaired, and what had to be repaired.
pub(crate) fn resolve_settings(shared: &Shared) -> (SkySettings, Vec<String>) {
    shared.get::<SkySettings>().cloned().unwrap_or_default().sanitized()
}

/// Everything about the sky that depends on the settings and the camera but not on a table.
#[derive(Clone, Debug)]
pub(crate) struct SkyFrame {
    pub settings: SkySettings,
    pub conditions: SkyConditions,
    pub sun_light: SunLight,
    pub overcast_zenith: [f32; 3],
}

#[expect(clippy::cast_possible_truncation, reason = "the GPU works in f32")]
fn narrow(value: [f64; 3]) -> [f32; 3] {
    value.map(|channel| channel as f32)
}

impl SkyFrame {
    /// `settings` must already be sanitised.
    pub(crate) fn new(settings: &SkySettings, camera: &Camera) -> Self {
        let planet = Planet::new(&settings.atmosphere);
        let sun = settings.sun_direction;
        let conditions = SkyConditions {
            camera_radius_km: f64::from(settings.camera_radius_km(camera.position.y)),
            sun_direction: [f64::from(sun.x), f64::from(sun.y), f64::from(sun.z)],
            sun_illuminance: std::array::from_fn(|channel| {
                f64::from(settings.sun_illuminance_lux) * f64::from(settings.sun_tint[channel])
            }),
            cloud_coverage: f64::from(settings.cloud_coverage),
        };
        let direct = reference::sun_illuminance_at_camera(&planet, &conditions);
        let uncovered = 1.0 - conditions.cloud_coverage;
        Self {
            sun_light: SunLight {
                direction_to_sun: sun,
                illuminance_lux: narrow(reference::scale(direct, uncovered)),
                angular_radius: settings.sun_angular_radius,
            },
            overcast_zenith: narrow(reference::overcast_zenith_radiance(&planet, &conditions)),
            settings: settings.clone(),
            conditions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(elevation_degrees: f32, coverage: f32) -> SkyFrame {
        let settings = SkySettings {
            cloud_coverage: coverage,
            ..SkySettings::default().with_sun(elevation_degrees, 0.0)
        };
        SkyFrame::new(&settings.sanitized().0, &Camera::default())
    }

    #[test]
    fn the_noon_sun_is_nearly_white_and_the_low_sun_is_red() {
        let noon = frame(80.0, 0.0).sun_light.illuminance_lux;
        let low = frame(3.0, 0.0).sun_light.illuminance_lux;
        assert!(noon[0] > 90_000.0 && noon[2] / noon[0] > 0.7, "{noon:?}");
        assert!(low[2] / low[0] < 0.4 && low[0] > low[2], "{low:?}");
    }

    #[test]
    fn a_sun_under_the_horizon_gives_no_light() {
        assert_eq!(frame(-5.0, 0.0).sun_light.illuminance_lux, [0.0; 3]);
        assert_eq!(frame(-5.0, 0.5).overcast_zenith, [0.0; 3]);
    }

    #[test]
    fn clouds_take_the_direct_light_and_leave_a_diffuse_sky() {
        let clear = frame(45.0, 0.0);
        let cloudy = frame(45.0, 0.75);
        assert!((cloudy.sun_light.illuminance_lux[1] / clear.sun_light.illuminance_lux[1] - 0.25).abs() < 1e-3);
        assert!(cloudy.overcast_zenith[1] > 1000.0);
    }
}
