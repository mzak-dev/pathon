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

/// Samples along each side of an area light at most, and the spacing the samples aim for in metres.
const MAX_AREA_SAMPLES_PER_SIDE: u32 = 12;
const AREA_SAMPLE_SPACING: f32 = 0.7;
const COSINE_EMITTER: f32 = 3.0;
const TWO_SIDED_COSINE_EMITTER: f32 = 4.0;

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a length in metres over the spacing, clamped to 1..=12 right after"
)]
fn samples_along(length: f32) -> u32 {
    ((length / AREA_SAMPLE_SPACING).ceil() as u32).clamp(1, MAX_AREA_SAMPLES_PER_SIDE)
}

impl GiLight {
    /// A rectangle that emits like a Lambertian surface, as a grid of small emitters that each shine with the cosine
    /// of the angle to the rectangle's axis: seen from a voxel, the sum is the rectangle (the shadow rays of the
    /// voxels cannot integrate over a shape, so they sample it).
    fn area_samples(light: &Light, width: f32, height: f32, two_sided: bool, out: &mut Vec<Self>) {
        let (columns, rows) = (samples_along(width), samples_along(height));
        let luminance = light.luminance_nits().unwrap_or(0.0);
        let color = light.color * luminance * (width * height / (columns * rows) as f32);
        let (_, range) = positioned(light);
        let axis = |axis: Vec3| axis.normalize_or_zero();
        let (x_axis, y_axis) = (
            axis(Vec3::from(light.transform.matrix3.x_axis)),
            axis(Vec3::from(light.transform.matrix3.y_axis)),
        );
        let kind = if two_sided {
            TWO_SIDED_COSINE_EMITTER
        } else {
            COSINE_EMITTER
        };
        for row in 0..rows {
            for column in 0..columns {
                let along_x = ((column as f32 + 0.5) / columns as f32 - 0.5) * width;
                let along_y = ((row as f32 + 0.5) / rows as f32 - 0.5) * height;
                let position = light.position() + x_axis * along_x + y_axis * along_y;
                out.push(Self {
                    position_range: [position.x, position.y, position.z, range],
                    direction_kind: with_kind(light.direction(), kind),
                    color_intensity: [color.x, color.y, color.z, 0.0],
                    spot: [0.0, 0.0, 1.0 / (range * range), 0.0],
                });
            }
        }
    }
}

/// The lights of the world that shine, with each area light as a grid of emitters; a light kind this version does not
/// know is left out.
pub fn pack_lights(world: &RenderWorld) -> Vec<GiLight> {
    let mut packed = Vec::new();
    for (_, light) in world.lights().iter() {
        match light.kind {
            LightKind::Area {
                width,
                height,
                two_sided,
            } => GiLight::area_samples(light, width, height, two_sided, &mut packed),
            _ => packed.extend(GiLight::from_light(light)),
        }
    }
    packed
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
    fn an_area_light_becomes_a_grid_of_emitters_that_carries_its_power() {
        let light = Light::area(Affine3A::IDENTITY, 1000.0, 3.0, 1.0);

        let mut packed = Vec::new();
        GiLight::area_samples(&light, 3.0, 1.0, false, &mut packed);

        assert_eq!(packed.len(), 5 * 2, "0.7 m apart along each side");
        // Each emitter shines with I(0) = L * dA; together they make L * A = P / pi on the axis.
        let on_axis: f32 = packed.iter().map(|sample| sample.color_intensity[0]).sum();
        assert!((on_axis - 1000.0 / std::f32::consts::PI).abs() < 0.1, "{on_axis}");
        assert!(
            packed
                .iter()
                .all(|sample| (sample.direction_kind[3] - COSINE_EMITTER).abs() < 1.0e-6)
        );
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
