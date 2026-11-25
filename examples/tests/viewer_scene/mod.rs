// A scene built in code for the viewer tests: a 2 x 2 m quad in the XY plane facing +Z, lit by a sun that shines
// along -Z.

#![allow(dead_code, unreachable_pub, reason = "each test crate uses a different part")]

use pathon::api::glam::Affine3A;
use pathon_examples::gltf_loader::{CpuInstance, CpuMaterial, CpuMesh, LoadedScene, SurfaceVertex};
use pathon_material::StandardMaterial;
use pathon_scene::Light;

pub const SIZE: u32 = 64;

pub fn red_quad_scene() -> LoadedScene {
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
    LoadedScene {
        meshes: vec![CpuMesh {
            name: "quad".into(),
            positions: corners.iter().map(|(position, _)| *position).collect(),
            surface: corners.iter().map(|(_, surface)| *surface).collect(),
            indices: vec![0, 1, 2, 0, 2, 3],
            material: 0,
        }],
        materials: vec![CpuMaterial {
            name: "red".into(),
            standard: StandardMaterial {
                base_color: [0.9, 0.02, 0.02, 1.0],
                metallic: 0.0,
                roughness: 0.7,
                ..StandardMaterial::default()
            },
            textures: Default::default(),
        }],
        instances: vec![CpuInstance {
            mesh: 0,
            transform: Affine3A::IDENTITY,
        }],
        lights: vec![Light::directional(Affine3A::IDENTITY, 600.0)],
        ..LoadedScene::default()
    }
}
