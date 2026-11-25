// The node tree of the default scene, flattened: every mesh node becomes instances with a world matrix, lights and
// cameras get a world pose without scale.

use std::collections::HashMap;

use pathon::api::glam::{Affine3A, Mat4, Vec3};
use pathon_scene::{Light, Projection};

use super::{CpuInstance, SceneCamera};

const MAX_SPOT_ANGLE: f32 = std::f32::consts::FRAC_PI_2;
const MIN_SPOT_ANGLE: f32 = 0.001;
const FOUR_PI: f32 = 4.0 * std::f32::consts::PI;

// Which meshes of `LoadedScene::meshes` one glTF mesh became, one per primitive that could be read.
pub(super) type PrimitiveMeshes = HashMap<usize, Vec<usize>>;

#[derive(Default)]
pub(super) struct Flattened {
    pub instances: Vec<CpuInstance>,
    pub lights: Vec<Light>,
    pub cameras: Vec<SceneCamera>,
}

// Lights and cameras ignore node scale: a scaled parent must not stretch a cone or a frustum.
fn pose(world: Mat4) -> Affine3A {
    let (_, rotation, translation) = world.to_scale_rotation_translation();
    Affine3A::from_rotation_translation(rotation, translation)
}

fn light_from(light: &gltf::khr_lights_punctual::Light<'_>, pose: Affine3A) -> Light {
    let color = Vec3::from(light.color());
    let intensity = light.intensity().max(0.0);
    let converted = match light.kind() {
        gltf::khr_lights_punctual::Kind::Directional => Light::directional(pose, intensity),
        // glTF gives candela; the render world stores lumens (candela * 4 pi), see docs/scenes.md.
        gltf::khr_lights_punctual::Kind::Point => Light::point(pose, intensity * FOUR_PI),
        gltf::khr_lights_punctual::Kind::Spot {
            inner_cone_angle,
            outer_cone_angle,
        } => {
            let outer = outer_cone_angle.clamp(MIN_SPOT_ANGLE, MAX_SPOT_ANGLE);
            let inner = inner_cone_angle.clamp(MIN_SPOT_ANGLE, outer);
            Light::spot(pose, intensity * FOUR_PI, inner, outer)
        }
    };
    let converted = converted.with_color(color);
    match light.range() {
        Some(range) if range > 0.0 => converted.with_range(range),
        _ => converted,
    }
}

fn camera_from(node: &gltf::Node<'_>, camera: &gltf::Camera<'_>, pose: Affine3A) -> Option<SceneCamera> {
    let projection = match camera.projection() {
        gltf::camera::Projection::Perspective(perspective) => {
            Projection::perspective(perspective.yfov(), perspective.znear())
        }
        gltf::camera::Projection::Orthographic(orthographic) => {
            Projection::orthographic(2.0 * orthographic.ymag(), orthographic.znear(), orthographic.zfar())
        }
    };
    projection.validate().ok()?;
    // The manifest names the objects of the source scene, which the exporter keeps as node names.
    let name = node.name().or_else(|| camera.name()).unwrap_or("camera").to_owned();
    Some(SceneCamera {
        name,
        transform: pose,
        projection,
    })
}

pub(super) fn flatten(document: &gltf::Document, primitives: &PrimitiveMeshes) -> Flattened {
    let mut flattened = Flattened::default();
    let scene = document.default_scene().or_else(|| document.scenes().next());
    let Some(scene) = scene else {
        return flattened;
    };
    // A stack popped from the end: children are pushed in reverse so the walk visits the file's order.
    let mut pending: Vec<(gltf::Node<'_>, Mat4)> = scene.nodes().map(|node| (node, Mat4::IDENTITY)).collect();
    pending.reverse();
    while let Some((node, parent)) = pending.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            let transform = Affine3A::from_mat4(world);
            for &mesh_index in primitives.get(&mesh.index()).into_iter().flatten() {
                flattened.instances.push(CpuInstance {
                    mesh: mesh_index,
                    transform,
                });
            }
        }
        if let Some(light) = node.light() {
            flattened.lights.push(light_from(&light, pose(world)));
        }
        if let Some(camera) = node.camera()
            && let Some(camera) = camera_from(&node, &camera, pose(world))
        {
            flattened.cameras.push(camera);
        }
        let children_start = pending.len();
        pending.extend(node.children().map(|child| (child, world)));
        pending[children_start..].reverse();
    }
    flattened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_drops_scale() {
        use pathon::api::glam::Quat;
        let world = Mat4::from_scale_rotation_translation(Vec3::splat(3.0), Quat::IDENTITY, Vec3::new(1.0, 2.0, 3.0));
        let pose = pose(world);
        assert_eq!(Vec3::from(pose.translation), Vec3::new(1.0, 2.0, 3.0));
        assert!((pose.matrix3.x_axis.length() - 1.0).abs() < 1e-6);
    }
}
