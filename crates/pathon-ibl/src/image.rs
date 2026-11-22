use crate::IblError;
use crate::half::radiance_to_f16;
use crate::reference::equirect_direction;

/// An equirectangular HDR picture in memory, kept as binary16 RGBA (radiance beyond 65504 saturates). Mapping of
/// directions: [`crate::reference::equirect_uv`]. Reading a file is the host's job.
#[derive(Clone, PartialEq, Debug)]
pub struct EquirectImage {
    width: u32,
    height: u32,
    texels: Vec<u16>,
}

impl EquirectImage {
    /// `rgb` holds `width * height * 3` linear radiance values, row by row.
    pub fn from_rgb_f32(width: u32, height: u32, rgb: &[f32]) -> Result<Self, IblError> {
        Self::from_channels(width, height, rgb, 3)
    }

    /// `rgba` holds `width * height * 4` values; alpha is ignored.
    pub fn from_rgba_f32(width: u32, height: u32, rgba: &[f32]) -> Result<Self, IblError> {
        Self::from_channels(width, height, rgba, 4)
    }

    /// `rgba` holds `width * height * 4` binary16 values (as bits); alpha is ignored.
    pub fn from_rgba_f16(width: u32, height: u32, rgba: Vec<u16>) -> Result<Self, IblError> {
        Self::check_length(width, height, rgba.len(), 4)?;
        let mut texels = rgba;
        for texel in texels.as_chunks_mut::<4>().0 {
            let finite = texel[..3].iter().all(|bits| bits & 0x7c00 != 0x7c00);
            if !finite {
                return Err(IblError::InvalidImage {
                    reason: "a texel is infinite or NaN".into(),
                });
            }
            // A negative value is a sign bit away from zero: radiance has no negative lobe.
            texel[..3]
                .iter_mut()
                .filter(|bits| **bits & 0x8000 != 0)
                .for_each(|bits| *bits = 0);
            texel[3] = 0x3c00;
        }
        Ok(Self { width, height, texels })
    }

    /// Evaluates `radiance` at the direction of every texel centre: a procedural sky without a file.
    pub fn from_fn(width: u32, height: u32, radiance: impl Fn([f32; 3]) -> [f32; 3]) -> Result<Self, IblError> {
        Self::check_dimensions(width, height)?;
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                let u = (f64::from(x) + 0.5) / f64::from(width);
                let v = (f64::from(y) + 0.5) / f64::from(height);
                #[expect(clippy::cast_possible_truncation, reason = "unit direction components fit f32")]
                let direction = equirect_direction(u, v).map(|axis| axis as f32);
                rgb.extend(radiance(direction));
            }
        }
        Self::from_rgb_f32(width, height, &rgb)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn texels(&self) -> &[u16] {
        &self.texels
    }

    fn from_channels(width: u32, height: u32, values: &[f32], channels: usize) -> Result<Self, IblError> {
        Self::check_length(width, height, values.len(), channels)?;
        if values.iter().any(|value| !value.is_finite()) {
            return Err(IblError::InvalidImage {
                reason: "a texel is infinite or NaN".into(),
            });
        }
        let mut texels = Vec::with_capacity(values.len() / channels * 4);
        for texel in values.chunks_exact(channels) {
            texels.extend(texel[..3].iter().copied().map(radiance_to_f16));
            texels.push(0x3c00);
        }
        Ok(Self { width, height, texels })
    }

    fn check_dimensions(width: u32, height: u32) -> Result<(), IblError> {
        if width == 0 || height == 0 {
            return Err(IblError::InvalidImage {
                reason: format!("{width}x{height} has no texels"),
            });
        }
        Ok(())
    }

    fn check_length(width: u32, height: u32, length: usize, channels: usize) -> Result<(), IblError> {
        Self::check_dimensions(width, height)?;
        let expected = width as usize * height as usize * channels;
        if length != expected {
            return Err(IblError::InvalidImage {
                reason: format!("{width}x{height} with {channels} channels needs {expected} values, got {length}"),
            });
        }
        Ok(())
    }
}

/// A procedural sky: a gradient from the horizon to the zenith, a dark ground and one bright sun disc.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SyntheticSky {
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    pub ground: [f32; 3],
    /// Unit vector from the origin to the sun.
    pub sun_direction: [f32; 3],
    pub sun_angular_radius: f32,
    pub sun_radiance: [f32; 3],
}

impl Default for SyntheticSky {
    fn default() -> Self {
        let sun = [0.45_f32, 0.55, -0.70];
        let length = (sun[0] * sun[0] + sun[1] * sun[1] + sun[2] * sun[2]).sqrt();
        Self {
            zenith: [0.15, 0.35, 0.85],
            horizon: [0.85, 0.9, 1.0],
            ground: [0.12, 0.1, 0.08],
            sun_direction: sun.map(|axis| axis / length),
            sun_angular_radius: 6.0_f32.to_radians(),
            sun_radiance: [40.0, 36.0, 28.0],
        }
    }
}

impl SyntheticSky {
    /// A constant environment of `radiance`: the white furnace of an energy test.
    pub fn furnace(radiance: f32) -> Self {
        Self {
            zenith: [radiance; 3],
            horizon: [radiance; 3],
            ground: [radiance; 3],
            sun_radiance: [radiance; 3],
            ..Self::default()
        }
    }

    pub fn radiance(&self, direction: [f32; 3]) -> [f32; 3] {
        let up = direction[1].clamp(-1.0, 1.0);
        let sun_cosine = direction
            .iter()
            .zip(self.sun_direction)
            .map(|(axis, sun)| axis * sun)
            .sum::<f32>();
        if sun_cosine >= self.sun_angular_radius.cos() {
            return self.sun_radiance;
        }
        let blend = |from: [f32; 3], to: [f32; 3], t: f32| {
            std::array::from_fn(|channel| from[channel] + (to[channel] - from[channel]) * t)
        };
        if up >= 0.0 {
            blend(self.horizon, self.zenith, up.sqrt())
        } else {
            blend(self.horizon, self.ground, (-up * 4.0).min(1.0))
        }
    }

    pub fn image(&self, width: u32, height: u32) -> Result<EquirectImage, IblError> {
        EquirectImage::from_fn(width, height, |direction| self.radiance(direction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_with_the_wrong_length_is_i0002() {
        let error = EquirectImage::from_rgb_f32(4, 2, &[0.0; 23]).expect_err("one value short");
        assert_eq!(error.code(), "I0002");
    }

    #[test]
    fn non_finite_radiance_is_i0002() {
        let mut rgb = vec![0.5_f32; 4 * 2 * 3];
        rgb[7] = f32::NAN;
        assert_eq!(
            EquirectImage::from_rgb_f32(4, 2, &rgb).expect_err("NaN").code(),
            "I0002"
        );
        let mut half = vec![0x3c00_u16; 4 * 2 * 4];
        half[5] = 0x7c00;
        assert_eq!(
            EquirectImage::from_rgba_f16(4, 2, half).expect_err("infinity").code(),
            "I0002"
        );
    }

    #[test]
    fn rgb_input_gets_an_opaque_alpha_and_saturated_radiance() {
        let image = EquirectImage::from_rgb_f32(1, 1, &[1.0, 1e9, -2.0]).expect("image");
        assert_eq!(image.texels(), &[0x3c00, 0x7bff, 0, 0x3c00]);
    }

    #[test]
    fn the_sun_is_where_the_sky_says_it_is() {
        let sky = SyntheticSky::default();
        assert_eq!(sky.radiance(sky.sun_direction), sky.sun_radiance);
        assert!(sky.radiance([0.0, 1.0, 0.0])[0] < 1.0);
    }
}
