// A Cornell box built in code (K12): red and green side walls, white back wall, floor and ceiling, two white boxes and
// a square area light in the ceiling. Everything that reaches the walls in the shadow of the boxes arrives by bounces,
// so it is the picture that shows what `pathon-gi` adds. The golden test and the `gi_cornell` example use it.

use pathon::api::glam::{Affine3A, Quat, Vec3};
use pathon_material::StandardMaterial;
use pathon_scene::Light;

use crate::gltf_loader::{CpuInstance, CpuMaterial, CpuMesh, LoadedScene, SurfaceVertex};

/// Edge of the room in metres.
pub const ROOM: f32 = 4.0;

fn matte(name: &str, color: [f32; 3]) -> CpuMaterial {
    CpuMaterial {
        name: name.into(),
        standard: StandardMaterial {
            base_color: [color[0], color[1], color[2], 1.0],
            metallic: 0.0,
            roughness: 0.9,
            double_sided: true,
            ..StandardMaterial::default()
        },
        textures: Default::default(),
    }
}

/// A quad `[-1, 1]^2` in the XY plane facing +Z.
fn quad(name: &str, material: usize) -> CpuMesh {
    let corner = |x: f32, y: f32| {
        (
            [x, y, 0.0],
            SurfaceVertex {
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [x * 0.5 + 0.5, 0.5 - y * 0.5],
            },
        )
    };
    let corners = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    CpuMesh {
        name: name.into(),
        positions: corners.iter().map(|(position, _)| *position).collect(),
        surface: corners.iter().map(|(_, surface)| *surface).collect(),
        indices: vec![0, 1, 2, 0, 2, 3],
        material,
    }
}

/// A unit cube `[-0.5, 0.5]^3` with flat faces.
fn cube(name: &str, material: usize) -> CpuMesh {
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::NEG_Z),
        (Vec3::NEG_X, Vec3::Y, Vec3::Z),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::NEG_Z, Vec3::X),
        (Vec3::Z, Vec3::Y, Vec3::X),
        (Vec3::NEG_Z, Vec3::Y, Vec3::NEG_X),
    ];
    let (mut positions, mut surface, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for (normal, up, right) in faces {
        let base = u32::try_from(positions.len()).expect("a few vertices");
        for (u, v) in [(-1.0_f32, -1.0_f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let point = normal * 0.5 + right * (u * 0.5) + up * (v * 0.5);
            positions.push(point.to_array());
            surface.push(SurfaceVertex {
                normal: normal.to_array(),
                tangent: [right.x, right.y, right.z, 1.0],
                uv: [u * 0.5 + 0.5, 0.5 - v * 0.5],
            });
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    CpuMesh {
        name: name.into(),
        positions,
        surface,
        indices,
        material,
    }
}

/// The scene: the room spans `x, z` in `[-2, 2]` and `y` in `[0, 4]`; the side towards `+z`, where the camera
/// stands, is open, as in the original box.
pub fn cornell_scene() -> LoadedScene {
    let half = ROOM / 2.0;
    let materials = vec![
        matte("white", [0.72, 0.72, 0.72]),
        matte("red", [0.68, 0.06, 0.05]),
        matte("green", [0.12, 0.45, 0.15]),
    ];
    let meshes = vec![
        quad("wall white", 0),
        quad("wall red", 1),
        quad("wall green", 2),
        cube("box white", 0),
    ];
    let wall = |mesh: usize, center: Vec3, rotation: Quat| CpuInstance {
        mesh,
        transform: Affine3A::from_scale_rotation_translation(Vec3::new(half, half, 1.0), rotation, center),
    };
    let block = |center: Vec3, size: Vec3, turn: f32| CpuInstance {
        mesh: 3,
        transform: Affine3A::from_scale_rotation_translation(size, Quat::from_rotation_y(turn), center),
    };
    let instances = vec![
        // Back, floor and ceiling in white; the left wall is red and the right one green.
        wall(0, Vec3::new(0.0, half, -half), Quat::IDENTITY),
        wall(
            0,
            Vec3::new(0.0, 0.0, 0.0),
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        ),
        wall(
            0,
            Vec3::new(0.0, ROOM, 0.0),
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        ),
        wall(
            1,
            Vec3::new(-half, half, 0.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        ),
        wall(
            2,
            Vec3::new(half, half, 0.0),
            Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
        ),
        block(Vec3::new(-0.7, 0.6, -0.6), Vec3::new(1.2, 1.2, 1.2), 0.35),
        block(Vec3::new(0.75, 1.2, -1.0), Vec3::new(1.2, 2.4, 1.2), -0.3),
    ];
    // A 1.2 m square in the ceiling that faces down: the local -Z of the light is `-Y` in the world.
    let lamp = Light::area(
        Affine3A::from_rotation_translation(
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            Vec3::new(0.0, ROOM - 0.02, 0.0),
        ),
        6.0e4,
        1.2,
        1.2,
    );
    LoadedScene {
        meshes,
        materials,
        instances,
        lights: vec![lamp],
        ..LoadedScene::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_room_has_six_walls_and_two_boxes_under_one_light() {
        let scene = cornell_scene();

        assert_eq!(scene.instances.len(), 7);
        assert_eq!(scene.lights.len(), 1);
        let (min, max) = scene.bounds().expect("geometry");
        assert!(
            (max - min - Vec3::splat(ROOM)).abs().max_element() < 1.0e-4,
            "{min} {max}"
        );
    }
}
