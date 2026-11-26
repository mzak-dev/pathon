// CPU-side layouts of the viewer's buffers (they mirror `forward.wesl`) and the code that fills them from a snapshot of
// the render world.

use bytemuck::{Pod, Zeroable};
use pathon::api::glam::{self, Affine3A, Mat4, Vec3};
use pathon_material::{AlphaKind, MaterialId, MaterialTable, MaterialVariant, flag_bits};
use pathon_mesh::MeshId;
use pathon_scene::{Light, LightKind, RenderWorld};

pub(super) const MAX_LIGHTS: usize = 64;
pub(super) const SHADOW_MAP_SIZE: u32 = 4096;

const KIND_DIRECTIONAL: f32 = 0.0;
const KIND_POINT: f32 = 1.0;
const KIND_SPOT: f32 = 2.0;
// Never zero: a spot whose inner and outer cones coincide would divide by it.
const MIN_CONE_SPAN: f32 = 1e-4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct FrameUniform {
    pub view_projection: [[f32; 4]; 4],
    pub sun_view_projection: [[f32; 4]; 4],
    pub camera_position: [f32; 4],
    pub params: [f32; 4],
    pub counts: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct InstanceData {
    pub model: [[f32; 4]; 4],
    pub normal_matrix: [[f32; 4]; 4],
    pub material: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct LightData {
    pub position_range: [f32; 4],
    pub direction_kind: [f32; 4],
    pub color_intensity: [f32; 4],
    pub spot: [f32; 4],
}

/// Which pipeline draws an instance: the alpha kind picks the shader permutation and the blend state, the culling
/// follows the material's `doubleSided`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) struct PipelineKey {
    pub alpha: AlphaKind,
    pub double_sided: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DrawCall {
    pub key: PipelineKey,
    pub material: u32,
    pub mesh: MeshId,
    /// Position in the instance buffer, which `@builtin(instance_index)` addresses.
    pub instance: u32,
    pub center: Vec3,
}

#[derive(Debug, Default)]
pub(super) struct SceneDraws {
    pub instances: Vec<InstanceData>,
    /// Opaque and masked, ordered by pipeline and material to keep state changes low.
    pub solid: Vec<DrawCall>,
    /// Blended; the order is per frame, back to front.
    pub blended: Vec<DrawCall>,
    /// The merged variant of every alpha kind that occurs: one shader permutation covers all its materials.
    pub variants: Vec<(AlphaKind, MaterialVariant)>,
}

fn instance_data(transform: Affine3A, material: u32) -> InstanceData {
    let linear = transform.matrix3;
    let determinant = linear.determinant();
    // Normals follow the inverse transpose; a singular matrix gets the plain linear part instead of NaNs.
    let normal = if determinant.abs() > f32::EPSILON {
        linear.inverse().transpose()
    } else {
        linear
    };
    InstanceData {
        model: Mat4::from(transform).to_cols_array_2d(),
        normal_matrix: Mat4::from_mat3a(normal).to_cols_array_2d(),
        material: [material, u32::from(determinant < 0.0), 0, 0],
    }
}

/// Reads the instances of the snapshot into draw calls; an instance whose material the table does not know is skipped.
pub(super) fn build_draws(world: &RenderWorld, materials: &MaterialTable, material_ids: &[MaterialId]) -> SceneDraws {
    let instances = world.instances();
    let mut draws = SceneDraws::default();
    for slot in instances.iter_alive() {
        let Some(&material_id) = material_ids.get(instances.materials()[slot].raw().index() as usize) else {
            continue;
        };
        let (Some(variant), Some(data)) = (materials.variant(material_id), materials.get(material_id)) else {
            continue;
        };
        let transform = instances.transforms()[slot];
        let instance = u32::try_from(draws.instances.len()).expect("fewer than u32::MAX instances");
        draws.instances.push(instance_data(transform, material_id.index()));
        let center = instances
            .world_bounds(slot)
            .map_or(Vec3::from(transform.translation), |bounds| bounds.center());
        let call = DrawCall {
            key: PipelineKey {
                alpha: variant.alpha,
                double_sided: data.flags & flag_bits::DOUBLE_SIDED != 0,
            },
            material: material_id.index(),
            mesh: instances.meshes()[slot].to_id(),
            instance,
            center,
        };
        match draws.variants.iter_mut().find(|(alpha, _)| *alpha == variant.alpha) {
            Some((_, merged)) => *merged = merged.merge(variant).unwrap_or(*merged),
            None => draws.variants.push((variant.alpha, variant)),
        }
        if variant.alpha == AlphaKind::Transparent {
            draws.blended.push(call);
        } else {
            draws.solid.push(call);
        }
    }
    draws
        .solid
        .sort_by_key(|call| (call.key.alpha as u8, call.key.double_sided, call.material));
    draws
}

pub(super) fn light_data(light: &Light) -> LightData {
    let direction = light.direction();
    let color = light.color;
    let (kind, intensity, spot) = match light.kind {
        LightKind::Directional => (KIND_DIRECTIONAL, light.intensity, [0.0, 0.0, 0.0, 0.0]),
        LightKind::Point => (
            KIND_POINT,
            light.luminous_intensity_candela().unwrap_or_default(),
            [0.0; 4],
        ),
        LightKind::Spot {
            inner_angle,
            outer_angle,
        } => {
            let (cos_inner, cos_outer) = (inner_angle.cos(), outer_angle.cos());
            (
                KIND_SPOT,
                light.luminous_intensity_candela().unwrap_or_default(),
                [cos_outer, 1.0 / (cos_inner - cos_outer).max(MIN_CONE_SPAN), 0.0, 0.0],
            )
        }
        // The `--simple` lighting has no rectangles: an area light is the hemisphere spot the exporter wrote for it
        // (power / 2 pi candela, cone of 76 to 90 degrees), which is what pathon drew before K05.
        LightKind::Area { .. } => {
            let (cos_inner, cos_outer) = (1.3343_f32.cos(), 1.5698_f32.cos());
            (
                KIND_SPOT,
                light.intensity / (2.0 * std::f32::consts::PI),
                [cos_outer, 1.0 / (cos_inner - cos_outer).max(MIN_CONE_SPAN), 0.0, 0.0],
            )
        }
        // `LightKind` is non-exhaustive: an unknown kind contributes nothing.
        _ => (KIND_POINT, 0.0, [0.0; 4]),
    };
    let position = light.position();
    LightData {
        position_range: [position.x, position.y, position.z, light.range.unwrap_or(0.0)],
        direction_kind: [direction.x, direction.y, direction.z, kind],
        color_intensity: [color.x * intensity, color.y * intensity, color.z * intensity, 0.0],
        spot,
    }
}

/// The lights of the snapshot (at most [`MAX_LIGHTS`]) and the index of the first directional one, the sun.
pub(super) fn build_lights(world: &RenderWorld) -> (Vec<LightData>, Option<usize>) {
    let mut lights = Vec::new();
    let mut sun = None;
    for (_, light) in world.lights().iter().take(MAX_LIGHTS) {
        if sun.is_none() && matches!(light.kind, LightKind::Directional) {
            sun = Some(lights.len());
        }
        lights.push(light_data(light));
    }
    (lights, sun)
}

/// An orthographic reverse-Z projection that holds the whole box in the view of a light shining along `direction`.
pub(super) fn sun_view_projection(direction: Vec3, bounds: (Vec3, Vec3)) -> Mat4 {
    let (min, max) = bounds;
    let centre = (min + max) * 0.5;
    let radius = ((max - min).length() * 0.5).max(1.0);
    let up = if direction.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    let view = glam::camera::rh::view::look_to_mat4(centre - direction * radius * 2.0, direction, up);
    let corners = (0..8).map(|corner| {
        let pick = |bit: u32| if corner & bit == 0 { 0.0 } else { 1.0 };
        view.transform_point3(min + (max - min) * Vec3::new(pick(1), pick(2), pick(4)))
    });
    let (low, high) = corners.fold((Vec3::MAX, Vec3::MIN), |(low, high), corner| {
        (low.min(corner), high.max(corner))
    });
    // View space looks down -Z: the box lies between `-high.z` (near) and `-low.z` (far).
    let projection =
        glam::camera::rh::proj::directx::orthographic(low.x, high.x, low.y, high.y, -high.z - 1.0, -low.z + 1.0);
    // Depth 0..1 near-to-far becomes far-to-near, the reverse-Z of the slot contract.
    Mat4::from_translation(Vec3::Z) * Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0)) * projection * view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_matrix_maps_the_box_into_the_unit_depth_range_with_near_at_one() {
        let bounds = (Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 3.0, 5.0));
        let direction = Vec3::new(-0.3, -0.8, -0.5).normalize();
        let matrix = sun_view_projection(direction, bounds);

        let toward_sun = matrix.project_point3(Vec3::new(0.0, 1.5, 0.0) - direction * 4.0);
        let away_from_sun = matrix.project_point3(Vec3::new(0.0, 1.5, 0.0) + direction * 4.0);

        assert!((0.0..=1.0).contains(&toward_sun.z) && (0.0..=1.0).contains(&away_from_sun.z));
        assert!(toward_sun.z > away_from_sun.z, "reverse-Z: nearer to the sun is larger");
    }

    #[test]
    fn mirrored_transforms_are_flagged_and_keep_outward_normals() {
        let mirrored = Affine3A::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let data = instance_data(mirrored, 7);
        assert_eq!(data.material, [7, 1, 0, 0]);
        assert_eq!(data.normal_matrix[0][0], -1.0);
    }

    #[test]
    fn spot_lights_carry_their_cone() {
        let light = Light::spot(Affine3A::IDENTITY, 400.0, 0.2, 0.5);
        let data = light_data(&light);
        assert!((data.direction_kind[3] - KIND_SPOT).abs() < f32::EPSILON);
        assert!((data.spot[0] - 0.5_f32.cos()).abs() < 1e-6);
        assert!((data.color_intensity[0] - 400.0 / (4.0 * std::f32::consts::PI)).abs() < 1e-3);
    }
}
