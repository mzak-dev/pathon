use pathon_api::glam::Vec3;
use pathon_scene::{Light, LightKind, RenderWorld};

/// A light as the GI shaders read it (WGSL `GpuLight`, the layout of `pbr::lights::GpuLight`): 64 bytes. Lux for a
/// directional light, candela for the others, so the voxels are lit in the units the surfaces are.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GiLight {
    /// xyz: position, w: range in metres (0 for a directional light).
    pub position_range: [f32; 4],
    /// Directional: unit vector to the light. Spot: unit vector the light shines along. w: kind (0, 1, 2).
    pub direction_kind: [f32; 4],
    pub color_intensity: [f32; 4],
    /// x, y: spot angle scale and offset, z: 1 / range^2.
    pub spot: [f32; 4],
}

const MIN_RANGE: f32 = 1.0e-3;

fn with_kind(direction: Vec3, kind: f32) -> [f32; 4] {
    [direction.x, direction.y, direction.z, kind]
}

fn positioned(light: &Light) -> (Vec3, f32) {
    let range = light.influence_radius().unwrap_or(MIN_RANGE).max(MIN_RANGE);
    (light.position(), range)
}

impl GiLight {
    pub fn from_light(light: &Light) -> Option<Self> {
        let intensity = light
            .illuminance_lux()
            .or_else(|| light.luminous_intensity_candela())
            .unwrap_or(0.0);
        let color = light.color * intensity;
        let color_intensity = [color.x, color.y, color.z, 0.0];
        match light.kind {
            LightKind::Directional => Some(Self {
                position_range: [0.0; 4],
                direction_kind: with_kind(-light.direction(), 0.0),
                color_intensity,
                spot: [0.0; 4],
            }),
            LightKind::Point => {
                let (position, range) = positioned(light);
                Some(Self {
                    position_range: [position.x, position.y, position.z, range],
                    direction_kind: with_kind(Vec3::NEG_Z, 1.0),
                    color_intensity,
                    spot: [0.0, 0.0, 1.0 / (range * range), 0.0],
                })
            }
            LightKind::Spot {
                inner_angle,
                outer_angle,
            } => {
                let (position, range) = positioned(light);
                let scale = 1.0 / (inner_angle.cos() - outer_angle.cos()).max(1.0e-4);
                let offset = -outer_angle.cos() * scale;
                Some(Self {
                    position_range: [position.x, position.y, position.z, range],
                    direction_kind: with_kind(light.direction(), 2.0),
                    color_intensity,
                    spot: [scale, offset, 1.0 / (range * range), 0.0],
                })
            }
            _ => None,
        }
    }
}

/// The lights of the world that shine: a light kind this version does not know is left out.
pub fn pack_lights(world: &RenderWorld) -> Vec<GiLight> {
    world
        .lights()
        .iter()
        .filter_map(|(_, light)| GiLight::from_light(light))
        .collect()
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::{Affine3A, Quat};

    use super::*;

    #[test]
    fn a_directional_light_points_to_the_light() {
        let down = Affine3A::from_quat(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2));

        let packed = GiLight::from_light(&Light::directional(down, 3.0)).expect("known kind");

        assert!((packed.direction_kind[1] - 1.0).abs() < 1.0e-6);
        assert_eq!(packed.color_intensity[0], 3.0);
    }

    #[test]
    fn a_point_light_is_candela_with_a_range() {
        let light = Light::point(Affine3A::from_translation(Vec3::new(0.0, 2.0, 0.0)), 1000.0).with_range(5.0);

        let packed = GiLight::from_light(&light).expect("known kind");

        let candela = 1000.0 / (4.0 * std::f32::consts::PI);
        assert!((packed.color_intensity[0] - candela).abs() < 1.0e-3);
        assert_eq!(packed.position_range[3], 5.0);
        assert!((packed.spot[2] - 0.04).abs() < 1.0e-6);
    }
}
