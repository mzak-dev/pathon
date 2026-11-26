use pathon_api::glam::Vec3;
use pathon_scene::{Light, LightKind};

/// A light as the shaders read it (`pbr::lights::GpuLight`, 96 bytes).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLight {
    /// xyz: position, w: range in metres (0 for a directional light).
    pub position_range: [f32; 4],
    /// Directional: unit vector to the light. Spot: unit vector the light shines along. w: kind.
    pub direction_kind: [f32; 4],
    /// rgb: tint times lux (directional), candela (point, spot) or luminance in nits (area).
    pub color_intensity: [f32; 4],
    /// x, y: spot angle scale and offset, z: 1 / range^2.
    pub spot: [f32; 4],
    /// Area light: xyz half of the width side in world space, w: 1 for a two-sided light. Zero for other lights.
    pub area_half_width: [f32; 4],
    /// Area light: xyz half of the height side in world space. Zero for other lights.
    pub area_half_height: [f32; 4],
}

impl GpuLight {
    pub const KIND_DIRECTIONAL: f32 = 0.0;
    pub const KIND_POINT: f32 = 1.0;
    pub const KIND_SPOT: f32 = 2.0;
    pub const KIND_AREA: f32 = 3.0;

    /// Smallest range the shaders accept; a zero range would divide by zero.
    pub const MIN_RANGE: f32 = 1.0e-3;

    pub fn from_light(light: &Light) -> Self {
        let color = light.color * light.intensity_for_shader();
        let color_intensity = [color.x, color.y, color.z, 0.0];
        match light.kind {
            LightKind::Directional => Self {
                position_range: [0.0; 4],
                direction_kind: direction_with(-light.direction(), Self::KIND_DIRECTIONAL),
                color_intensity,
                ..Self::default()
            },
            LightKind::Point => {
                let (position, range) = positioned(light);
                Self {
                    position_range: [position.x, position.y, position.z, range],
                    direction_kind: direction_with(Vec3::NEG_Z, Self::KIND_POINT),
                    color_intensity,
                    spot: [0.0, 0.0, 1.0 / (range * range), 0.0],
                    ..Self::default()
                }
            }
            LightKind::Spot {
                inner_angle,
                outer_angle,
            } => {
                let (position, range) = positioned(light);
                // glTF KHR_lights_punctual: the cone window is a ramp between the cosines of the two half angles.
                let scale = 1.0 / (inner_angle.cos() - outer_angle.cos()).max(1.0e-4);
                let offset = -outer_angle.cos() * scale;
                Self {
                    position_range: [position.x, position.y, position.z, range],
                    direction_kind: direction_with(light.direction(), Self::KIND_SPOT),
                    color_intensity,
                    spot: [scale, offset, 1.0 / (range * range), 0.0],
                    ..Self::default()
                }
            }
            LightKind::Area {
                width,
                height,
                two_sided,
            } => Self::area(light, width, height, two_sided),
            _ => Self::zeroed_directional(),
        }
    }

    // The radiance of the rectangle (nits) times the tint is what the shader integrates over the solid angle it covers.
    fn area(light: &Light, width: f32, height: f32, two_sided: bool) -> Self {
        let (position, range) = positioned(light);
        let radiance = light.color * light.luminance_nits().unwrap_or(0.0);
        let side = |axis: Vec3, length: f32| axis.normalize_or_zero() * (0.5 * length);
        let half_width = side(Vec3::from(light.transform.matrix3.x_axis), width);
        let half_height = side(Vec3::from(light.transform.matrix3.y_axis), height);
        Self {
            position_range: [position.x, position.y, position.z, range],
            direction_kind: direction_with(light.direction(), Self::KIND_AREA),
            color_intensity: [radiance.x, radiance.y, radiance.z, 0.0],
            spot: [0.0, 0.0, 1.0 / (range * range), 0.0],
            area_half_width: [half_width.x, half_width.y, half_width.z, f32::from(u8::from(two_sided))],
            area_half_height: [half_height.x, half_height.y, half_height.z, 0.0],
        }
    }

    // A light kind this version does not know shines with no light.
    fn zeroed_directional() -> Self {
        Self {
            direction_kind: [0.0, 1.0, 0.0, Self::KIND_DIRECTIONAL],
            ..Self::default()
        }
    }

    pub fn is_directional(&self) -> bool {
        self.direction_kind[3] == Self::KIND_DIRECTIONAL
    }

    pub fn is_area(&self) -> bool {
        self.direction_kind[3] == Self::KIND_AREA
    }
}

fn direction_with(direction: Vec3, kind: f32) -> [f32; 4] {
    [direction.x, direction.y, direction.z, kind]
}

fn positioned(light: &Light) -> (Vec3, f32) {
    let range = light
        .influence_radius()
        .unwrap_or(GpuLight::MIN_RANGE)
        .max(GpuLight::MIN_RANGE);
    (light.position(), range)
}

trait Photometric {
    fn intensity_for_shader(&self) -> f32;
}

impl Photometric for Light {
    // Lux for a directional light, candela for the others: what the BRDF multiplies by cos(theta) and the falloff.
    fn intensity_for_shader(&self) -> f32 {
        self.illuminance_lux()
            .or_else(|| self.luminous_intensity_candela())
            .unwrap_or(0.0)
    }
}

/// The lights of a scene as one array: directional lights first, then point and spot lights.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PackedLights {
    pub lights: Vec<GpuLight>,
    pub directional: u32,
}

impl PackedLights {
    pub fn from_lights<'light>(lights: impl IntoIterator<Item = &'light Light>) -> Self {
        let (directional, clustered): (Vec<GpuLight>, Vec<GpuLight>) = lights
            .into_iter()
            .map(GpuLight::from_light)
            .partition(GpuLight::is_directional);
        let count = u32::try_from(directional.len()).expect("fewer than 2^32 lights");
        Self {
            lights: directional.into_iter().chain(clustered).collect(),
            directional: count,
        }
    }

    pub fn clustered(&self) -> &[GpuLight] {
        &self.lights[self.directional as usize..]
    }

    /// Whether the colour pass needs the permutation with the area light code and the LTC table.
    pub fn has_area(&self) -> bool {
        self.lights.iter().any(GpuLight::is_area)
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::{Affine3A, Quat};

    use super::*;

    #[test]
    fn directional_lights_come_first_and_point_to_the_light() {
        let down = Affine3A::from_quat(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2));
        let lights = [
            Light::point(Affine3A::from_translation(Vec3::new(0.0, 2.0, 0.0)), 1000.0),
            Light::directional(down, 3.0),
        ];

        let packed = PackedLights::from_lights(&lights);

        assert_eq!(packed.directional, 1);
        assert!(packed.lights[0].is_directional());
        assert!(
            (packed.lights[0].direction_kind[1] - 1.0).abs() < 1e-6,
            "to the light = up"
        );
        assert_eq!(packed.lights[0].color_intensity[0], 3.0, "lux");
        assert_eq!(packed.clustered().len(), 1);
        let candela = 1000.0 / (4.0 * std::f32::consts::PI);
        assert!((packed.lights[1].color_intensity[0] - candela).abs() < 1e-3);
    }

    #[test]
    fn spot_cone_window_is_one_inside_and_zero_outside() {
        let light = Light::spot(Affine3A::IDENTITY, 800.0, 0.2, 0.5);
        let gpu = GpuLight::from_light(&light);
        let window = |angle: f32| (angle.cos() * gpu.spot[0] + gpu.spot[1]).clamp(0.0, 1.0);

        assert!((window(0.2) - 1.0).abs() < 1e-5);
        assert!(window(0.5).abs() < 1e-5);
        assert!(window(0.7) < 1e-9);
        assert!((gpu.spot[2] - 1.0 / (gpu.position_range[3] * gpu.position_range[3])).abs() < 1e-9);
    }
}
