// What the host changes between frames; it reaches the packs through `Renderer::shared_mut`.

/// Which lighting the viewer draws with: the stack of pathon packs, or the one-feature stand-in of M4-10 for
/// comparison.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LightingMode {
    #[default]
    Pbr,
    Simple,
}

/// Blender renders the classroom at exposure 0 with lights converted at 683 lm/W (docs/scenes.md), so 1/683 is its
/// exposure. The viewer starts one stop above: the tonemapper takes the mid-tones down where Blender's Standard
/// transform does not.
pub const DEFAULT_EXPOSURE: f32 = 2.0 / 683.0;
const EXPOSURE_STEP_STOPS: f32 = 0.5;
/// Blender's radiance is the pathon radiance divided by 683 lm/W, so this exposure makes the two equal
/// (docs/porownanie-obrazu.md).
pub const REFERENCE_EXPOSURE: f32 = 1.0 / 683.0;
/// Scale of an environment picture in W/m2/sr (Blender) to the photometric units of the lights.
const REFERENCE_ENVIRONMENT_INTENSITY: f32 = 683.0;

/// How the Cycles reference of the classroom is lit around the lights of the file: the black world of the
/// `lights-only` variant, or `Garage.hdr` as both light and backdrop (`ibl`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReferenceLighting {
    LightsOnly,
    Ibl,
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one key of the viewer; an enum per key would only rename it"
)]
pub struct ViewerSettings {
    /// Linear multiplier of the scene radiance before the tonemapper. With automatic exposure it is only the
    /// compensation: `log2(exposure / DEFAULT_EXPOSURE)` stops on top of what the histogram chose.
    pub exposure: f32,
    pub auto_exposure: bool,
    pub bloom: bool,
    /// Constant stand-in for image-based lighting in `--simple` mode: illuminance in lux from above and below.
    pub ambient_sky_lux: f32,
    pub ambient_ground_lux: f32,
    /// Emission in glTF files from Blender is in watts, lights are in lumens: this brings the two to one scale.
    pub emissive_scale: f32,
    pub sun_shadows: bool,
    /// The point and spot lights of the scene compete for shadow maps (`ShadowSettings::max_local_lights` of them).
    pub local_shadows: bool,
    /// Area lights get a shadow too, from their centre as if they were point lights (an approximation).
    pub area_shadows: bool,
    /// `ShadowSettings::max_local_lights`: how many lights have a shadow map at once.
    pub local_shadow_lights: u32,
    /// Draws every shadow map of the local lights again every frame, to time the frame without the cache.
    pub redraw_local_shadows: bool,
    /// Scale of the environment as light (`PbrSettings::environment_intensity`): the units of the HDR picture are
    /// not lux, and nothing here tells the room from the outdoors, so this is the one knob of the skylight.
    pub environment_intensity: f32,
    /// Scale of the environment as the backdrop seen through the windows.
    pub sky_intensity: f32,
    /// Tests every light in every pixel instead of the clusters, to compare the two.
    pub brute_force_lights: bool,
}

impl Default for ViewerSettings {
    fn default() -> Self {
        Self {
            exposure: DEFAULT_EXPOSURE,
            auto_exposure: false,
            bloom: true,
            ambient_sky_lux: 400.0,
            ambient_ground_lux: 150.0,
            emissive_scale: 683.0,
            sun_shadows: true,
            local_shadows: true,
            area_shadows: false,
            local_shadow_lights: 8,
            redraw_local_shadows: false,
            environment_intensity: 300.0,
            sky_intensity: 6000.0,
            brute_force_lights: false,
        }
    }
}

impl ViewerSettings {
    pub fn adjust_exposure(&mut self, stops: f32) {
        self.exposure *= (stops * EXPOSURE_STEP_STOPS).exp2();
    }

    /// The `[` and `]` keys: the ambient of `--simple` and the environment of the pbr stack by the same factor.
    pub fn adjust_ambient(&mut self, stops: f32) {
        let factor = (stops * EXPOSURE_STEP_STOPS).exp2();
        self.ambient_sky_lux *= factor;
        self.ambient_ground_lux *= factor;
        self.environment_intensity *= factor;
    }

    /// The settings that make the picture comparable with the Cycles reference: exposure 1/683, no automatic
    /// exposure, no bloom, and an environment of the reference's strength (none for `LightsOnly`).
    pub fn reference_like(lighting: ReferenceLighting) -> Self {
        let environment = match lighting {
            ReferenceLighting::LightsOnly => 0.0,
            ReferenceLighting::Ibl => REFERENCE_ENVIRONMENT_INTENSITY,
        };
        Self {
            exposure: REFERENCE_EXPOSURE,
            auto_exposure: false,
            bloom: false,
            environment_intensity: environment,
            sky_intensity: environment,
            ..Self::default()
        }
    }

    /// Stops of `exposure` above the default: the compensation of automatic exposure.
    pub fn exposure_stops(&self) -> f32 {
        (self.exposure / DEFAULT_EXPOSURE).log2()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_settings_match_blender_exposure_and_the_environment_of_the_variant() {
        let lights_only = ViewerSettings::reference_like(ReferenceLighting::LightsOnly);
        assert!((lights_only.exposure * 683.0 - 1.0).abs() < 1e-6);
        assert!(!lights_only.bloom && !lights_only.auto_exposure);
        assert_eq!(
            (lights_only.environment_intensity, lights_only.sky_intensity),
            (0.0, 0.0)
        );
        let ibl = ViewerSettings::reference_like(ReferenceLighting::Ibl);
        assert_eq!((ibl.environment_intensity, ibl.sky_intensity), (683.0, 683.0));
    }

    #[test]
    fn two_steps_up_is_one_stop() {
        let mut settings = ViewerSettings::default();
        settings.adjust_exposure(1.0);
        settings.adjust_exposure(1.0);
        assert!((settings.exposure / DEFAULT_EXPOSURE - 2.0).abs() < 1e-5);
        assert!((settings.exposure_stops() - 1.0).abs() < 1e-5);
    }
}
