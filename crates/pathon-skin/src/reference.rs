//! The deformation on the CPU, in the order and arithmetic of `skin.wgsl`: morph targets on the rest pose, the blended
//! bone matrix, then normalization. Tests compare the GPU with it; so can an application checking its own data.

use pathon_api::glam::{Mat4, Vec3};

const MIN_LENGTH_SQUARED: f32 = 1e-20;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RestVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// xyz direction, w handedness.
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
}

/// The four bones and weights of a vertex as the `Joints` and `Weights` attributes carry them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Influences {
    pub joints: [u16; 4],
    /// `Unorm8x4`: 255 is a weight of one.
    pub weights: [u8; 4],
}

impl Influences {
    /// The `Uint32x2` of the `Joints` attribute: two `u16` per `u32`, the lower index first.
    pub fn packed_joints(self) -> [u32; 2] {
        let pair = |low: u16, high: u16| u32::from(low) | (u32::from(high) << 16);
        [
            pair(self.joints[0], self.joints[1]),
            pair(self.joints[2], self.joints[3]),
        ]
    }

    /// The `Unorm8x4` of the `Weights` attribute: the first weight in the lowest byte.
    pub fn packed_weights(self) -> u32 {
        u32::from_le_bytes(self.weights)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DeformedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
}

/// Weights are renormalized over the influences that point at an existing bone; a vertex with none keeps its rest pose.
pub fn blended_matrix(influences: Influences, bones: &[Mat4]) -> Mat4 {
    let mut sum = Mat4::ZERO;
    let mut total = 0.0_f32;
    for (joint, byte) in influences.joints.into_iter().zip(influences.weights) {
        let weight = f32::from(byte) / 255.0;
        if weight > 0.0
            && let Some(bone) = bones.get(usize::from(joint))
        {
            sum += *bone * weight;
            total += weight;
        }
    }
    if total <= 0.0 {
        Mat4::IDENTITY
    } else {
        sum * (1.0 / total)
    }
}

fn safe_normalize(direction: Vec3) -> Vec3 {
    let length_squared = direction.length_squared();
    if length_squared > MIN_LENGTH_SQUARED {
        direction * length_squared.sqrt().recip()
    } else {
        direction
    }
}

/// One morph target for one vertex: its weight and its position, normal and tangent deltas.
pub type MorphDelta = (f32, [[f32; 3]; 3]);

/// `skin` is the vertex's influences and the bones of its instance; `None` skips skinning (a morph-only mesh).
pub fn deform_vertex(rest: &RestVertex, skin: Option<(Influences, &[Mat4])>, morphs: &[MorphDelta]) -> DeformedVertex {
    let mut position = Vec3::from(rest.position);
    let mut normal = Vec3::from(rest.normal);
    let mut tangent = Vec3::new(rest.tangent[0], rest.tangent[1], rest.tangent[2]);
    for (weight, [delta_position, delta_normal, delta_tangent]) in morphs {
        if *weight != 0.0 {
            position += Vec3::from(*delta_position) * *weight;
            normal += Vec3::from(*delta_normal) * *weight;
            tangent += Vec3::from(*delta_tangent) * *weight;
        }
    }
    if let Some((influences, bones)) = skin {
        let matrix = blended_matrix(influences, bones);
        position = (matrix * position.extend(1.0)).truncate();
        normal = (matrix * normal.extend(0.0)).truncate();
        tangent = (matrix * tangent.extend(0.0)).truncate();
    }
    let normal = safe_normalize(normal);
    let tangent = safe_normalize(tangent);
    DeformedVertex {
        position: position.to_array(),
        normal: normal.to_array(),
        tangent: [tangent.x, tangent.y, tangent.z, rest.tangent[3]],
        uv: rest.uv,
    }
}

/// Previous pose for motion vectors, `current - previous`, in object space.
pub fn motion(current: [f32; 3], previous: [f32; 3]) -> [f32; 3] {
    (Vec3::from(current) - Vec3::from(previous)).to_array()
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::Quat;

    use super::*;

    fn rest() -> RestVertex {
        RestVertex {
            position: [1.0, 0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, -1.0],
            uv: [0.25, 0.75],
        }
    }

    fn one_bone(bone: u16) -> Influences {
        Influences {
            joints: [bone, 0, 0, 0],
            weights: [255, 0, 0, 0],
        }
    }

    #[test]
    fn a_quarter_turn_moves_position_and_normal_and_keeps_handedness_and_uv() {
        let turn = Mat4::from_quat(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        let deformed = deform_vertex(&rest(), Some((one_bone(0), &[turn])), &[]);
        assert!((Vec3::from(deformed.position) - Vec3::Y).length() < 1e-6);
        assert!((Vec3::from(deformed.normal) + Vec3::X).length() < 1e-6);
        assert_eq!((deformed.tangent[3], deformed.uv), (-1.0, [0.25, 0.75]));
    }

    #[test]
    fn weights_are_renormalized_and_bad_influences_never_make_nan() {
        let bones = [Mat4::from_translation(Vec3::X * 2.0)];
        // 100 + 100 of 255 is not a sum of one; the rounded sum must still land on the bone.
        let uneven = Influences {
            joints: [0, 0, 0, 0],
            weights: [100, 100, 0, 0],
        };
        assert!((blended_matrix(uneven, &bones) - bones[0]).abs_diff_eq(Mat4::ZERO, 1e-6));
        let nothing = Influences {
            joints: [0; 4],
            weights: [0; 4],
        };
        assert_eq!(blended_matrix(nothing, &bones), Mat4::IDENTITY);
        let stray = Influences {
            joints: [9, 0, 0, 0],
            weights: [200, 55, 0, 0],
        };
        assert!(blended_matrix(stray, &bones).abs_diff_eq(bones[0], 1e-6));
        for influences in [uneven, nothing, stray] {
            let deformed = deform_vertex(&rest(), Some((influences, &bones)), &[]);
            assert!(deformed.position.into_iter().chain(deformed.normal).all(f32::is_finite));
        }
    }

    #[test]
    fn morph_weights_interpolate_linearly() {
        let delta = [[0.0, 2.0, 0.0], [0.0; 3], [0.0; 3]];
        let half = deform_vertex(&rest(), None, &[(0.5, delta)]);
        assert_eq!(half.position, [1.0, 1.0, 0.0]);
    }

    #[test]
    fn packing_matches_the_vertex_attribute_layout() {
        let influences = Influences {
            joints: [1, 2, 3, 4],
            weights: [10, 20, 30, 40],
        };
        assert_eq!(influences.packed_joints(), [1 | (2 << 16), 3 | (4 << 16)]);
        assert_eq!(influences.packed_weights().to_le_bytes(), [10, 20, 30, 40]);
    }

    #[test]
    fn a_zero_normal_stays_zero() {
        let mut vertex = rest();
        vertex.normal = [0.0; 3];
        assert_eq!(deform_vertex(&vertex, None, &[]).normal, [0.0; 3]);
    }
}
