use std::f32::consts::TAU;

use pathon_api::glam::{Mat4, Quat, Vec3};
use pathon_mesh::{IndexData, MeshData};

use crate::reference::{Influences, RestVertex};

const LENGTH: f32 = 2.0;
const ELBOW: f32 = 1.0;
const RADIUS: f32 = 0.2;
// Weights blend over this stretch of the arm around the elbow.
const BLEND_START: f32 = 0.6;
const BLEND_END: f32 = 1.4;

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255"
)]
fn unorm8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// An arm of two bones along +X: a cylinder from `x = 0` to `x = 2` with the elbow at `x = 1`. Bone 0 is the upper
/// arm, bone 1 the forearm; weights blend between them around the elbow. It is the content of the tests and the
/// example, the way `SyntheticSky` is in `pathon-ibl`; the layout is `VertexLayout::skinned()`.
#[derive(Clone, Debug)]
pub struct SampleArm {
    positions: Vec<[f32; 3]>,
    surface: Vec<[f32; 9]>,
    skin: Vec<[u32; 3]>,
    indices: Vec<u32>,
    influences: Vec<Influences>,
    rest: Vec<RestVertex>,
}

impl SampleArm {
    /// `rings` cross sections of `sides` vertices each (at least 3 and 2).
    pub fn new(rings: u32, sides: u32) -> Self {
        let (rings, sides) = (rings.max(2), sides.max(3));
        let mut arm = Self {
            positions: Vec::new(),
            surface: Vec::new(),
            skin: Vec::new(),
            indices: Vec::new(),
            influences: Vec::new(),
            rest: Vec::new(),
        };
        for ring in 0..rings {
            let x = LENGTH * ring as f32 / (rings - 1) as f32;
            let blend = ((x - BLEND_START) / (BLEND_END - BLEND_START)).clamp(0.0, 1.0);
            let blend = blend * blend * (3.0 - 2.0 * blend);
            let influences = Influences {
                joints: [0, 1, 0, 0],
                weights: [unorm8(1.0 - blend), unorm8(blend), 0, 0],
            };
            for side in 0..sides {
                let angle = TAU * side as f32 / sides as f32;
                let (sin, cos) = angle.sin_cos();
                let vertex = RestVertex {
                    position: [x, RADIUS * cos, RADIUS * sin],
                    normal: [0.0, cos, sin],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [x / LENGTH, side as f32 / sides as f32],
                };
                arm.positions.push(vertex.position);
                arm.surface.push([
                    vertex.normal[0],
                    vertex.normal[1],
                    vertex.normal[2],
                    vertex.tangent[0],
                    vertex.tangent[1],
                    vertex.tangent[2],
                    vertex.tangent[3],
                    vertex.uv[0],
                    vertex.uv[1],
                ]);
                let [low, high] = influences.packed_joints();
                arm.skin.push([low, high, influences.packed_weights()]);
                arm.influences.push(influences);
                arm.rest.push(vertex);
            }
        }
        for ring in 0..rings - 1 {
            for side in 0..sides {
                let next = (side + 1) % sides;
                let (a, b) = (ring * sides + side, ring * sides + next);
                let (c, d) = (a + sides, b + sides);
                arm.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        arm
    }

    pub fn vertex_count(&self) -> u32 {
        u32::try_from(self.positions.len()).expect("a sample arm is small")
    }

    pub fn rest(&self) -> &[RestVertex] {
        &self.rest
    }

    pub fn influences(&self) -> &[Influences] {
        &self.influences
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Streams in the order of `VertexLayout::skinned()`.
    pub fn mesh_data(&self, label: &'static str) -> MeshData<'_> {
        MeshData::new(
            label,
            vec![
                bytemuck::cast_slice(&self.positions),
                bytemuck::cast_slice(&self.surface),
                bytemuck::cast_slice(&self.skin),
            ],
            IndexData::U32(&self.indices),
        )
    }

    /// Skinning matrices with the forearm bent by `elbow_angle` radians around Z; the upper arm stays at rest.
    pub fn pose(elbow_angle: f32) -> [Mat4; 2] {
        let elbow = Vec3::X * ELBOW;
        let joint = Mat4::from_translation(elbow) * Mat4::from_quat(Quat::from_rotation_z(elbow_angle));
        [Mat4::IDENTITY, joint * Mat4::from_translation(-elbow)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arm_is_a_closed_ring_strip_with_normalized_weights() {
        let arm = SampleArm::new(9, 8);
        assert_eq!(arm.vertex_count(), 72);
        assert_eq!(arm.indices().len(), 8 * 8 * 6);
        assert!(arm.indices().iter().all(|index| *index < arm.vertex_count()));
        for influences in arm.influences() {
            let sum: u32 = influences.weights.iter().map(|weight| u32::from(*weight)).sum();
            assert!((254..=256).contains(&sum), "{sum}");
        }
    }

    #[test]
    fn the_rest_pose_leaves_the_arm_where_it_is() {
        let [upper, forearm] = SampleArm::pose(0.0);
        assert_eq!(upper, Mat4::IDENTITY);
        assert!(forearm.abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }
}
