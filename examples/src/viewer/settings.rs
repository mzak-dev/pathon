// What the host changes between frames; it reaches the pack through `Renderer::shared_mut`.

/// Blender renders the classroom at exposure 0 with lights converted at 683 lm/W (docs/scenes.md), so 1/683 is its
/// exposure. The viewer starts one stop above: ACES takes the mid-tones down where Blender's Standard transform does not.
pub const DEFAULT_EXPOSURE: f32 = 2.0 / 683.0;
const EXPOSURE_STEP_STOPS: f32 = 0.5;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewerSettings {
    /// Linear multiplier of the scene radiance before the tonemapper.
    pub exposure: f32,
    /// Constant stand-in for image-based lighting (IBL arrives with M4-06): illuminance in lux from above and below.
    pub ambient_sky_lux: f32,
    pub ambient_ground_lux: f32,
    /// Emission in glTF files from Blender is in watts, lights are in lumens: this brings the two to one scale.
    pub emissive_scale: f32,
    pub sun_shadows: bool,
}

impl Default for ViewerSettings {
    fn default() -> Self {
        Self {
            exposure: DEFAULT_EXPOSURE,
            ambient_sky_lux: 400.0,
            ambient_ground_lux: 150.0,
            emissive_scale: 683.0,
            sun_shadows: true,
        }
    }
}

impl ViewerSettings {
    pub fn adjust_exposure(&mut self, stops: f32) {
        self.exposure *= (stops * EXPOSURE_STEP_STOPS).exp2();
    }

    pub fn adjust_ambient(&mut self, stops: f32) {
        let factor = (stops * EXPOSURE_STEP_STOPS).exp2();
        self.ambient_sky_lux *= factor;
        self.ambient_ground_lux *= factor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_steps_up_is_one_stop() {
        let mut settings = ViewerSettings::default();
        settings.adjust_exposure(1.0);
        settings.adjust_exposure(1.0);
        assert!((settings.exposure / DEFAULT_EXPOSURE - 2.0).abs() < 1e-5);
    }
}
