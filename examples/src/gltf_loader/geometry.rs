// glTF primitive -> the standard vertex layout of pathon-mesh (positions; normal, tangent, uv). Attributes a file
// leaves out are derived: smooth normals from the faces, tangents from the UV gradients.

use pathon::api::glam::{Vec2, Vec3, Vec4};

use super::{CpuMesh, SurfaceVertex};

fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut sums = vec![Vec3::ZERO; positions.len()];
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|index| Vec3::from(positions[index as usize]));
        // Not normalized: the cross product's length is twice the face area, so larger faces weigh more.
        let face = (b - a).cross(c - a);
        for &index in triangle {
            sums[index as usize] += face;
        }
    }
    sums.into_iter()
        .map(|sum| sum.try_normalize().unwrap_or(Vec3::Y).to_array())
        .collect()
}

// Any unit vector perpendicular to the normal, for meshes without UVs.
fn arbitrary_tangent(normal: Vec3) -> Vec4 {
    let axis = if normal.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    normal.cross(axis).normalize_or(Vec3::X).extend(1.0)
}

// Per-vertex accumulation of the triangle tangents (Lengyel), orthonormalized against the normal. glTF's +V points
// down the image while the normal map's green points up, so the bitangent is -dP/dv.
fn tangents(positions: &[[f32; 3]], normals: &[[f32; 3]], uvs: &[[f32; 2]], indices: &[u32]) -> Vec<[f32; 4]> {
    let mut tangent_sums = vec![Vec3::ZERO; positions.len()];
    let mut bitangent_sums = vec![Vec3::ZERO; positions.len()];
    for triangle in indices.as_chunks::<3>().0 {
        let [i0, i1, i2] = triangle.map(|index| index as usize);
        let (e1, e2) = (
            Vec3::from(positions[i1]) - Vec3::from(positions[i0]),
            Vec3::from(positions[i2]) - Vec3::from(positions[i0]),
        );
        let (d1, d2) = (
            Vec2::from(uvs[i1]) - Vec2::from(uvs[i0]),
            Vec2::from(uvs[i2]) - Vec2::from(uvs[i0]),
        );
        let determinant = d1.x * d2.y - d2.x * d1.y;
        if determinant.abs() < f32::EPSILON {
            continue;
        }
        let tangent = (e1 * d2.y - e2 * d1.y) / determinant;
        let bitangent = -((e2 * d1.x - e1 * d2.x) / determinant);
        for index in [i0, i1, i2] {
            tangent_sums[index] += tangent;
            bitangent_sums[index] += bitangent;
        }
    }
    (0..positions.len())
        .map(|index| {
            let normal = Vec3::from(normals[index]);
            let tangent = tangent_sums[index] - normal * normal.dot(tangent_sums[index]);
            match tangent.try_normalize() {
                Some(tangent) => {
                    let handedness = if normal.cross(tangent).dot(bitangent_sums[index]) < 0.0 {
                        -1.0
                    } else {
                        1.0
                    };
                    tangent.extend(handedness).to_array()
                }
                None => arbitrary_tangent(normal).to_array(),
            }
        })
        .collect()
}

// `None` for anything but a triangle list with positions, which a standard-layout mesh cannot draw.
pub(super) fn read_primitive(
    primitive: &gltf::Primitive<'_>,
    buffers: &[Vec<u8>],
    name: String,
    material: usize,
) -> Option<CpuMesh> {
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        return None;
    }
    let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
    let positions: Vec<[f32; 3]> = reader.read_positions()?.collect();
    let indices: Vec<u32> = match reader.read_indices() {
        Some(indices) => indices.into_u32().collect(),
        None => (0..u32::try_from(positions.len()).ok()?).collect(),
    };
    if indices.is_empty()
        || !indices.len().is_multiple_of(3)
        || indices.iter().any(|&index| index as usize >= positions.len())
    {
        return None;
    }
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(normals) => normals.collect(),
        None => smooth_normals(&positions, &indices),
    };
    let uvs: Option<Vec<[f32; 2]>> = reader.read_tex_coords(0).map(|uvs| uvs.into_f32().collect());
    let tangents: Vec<[f32; 4]> = match (reader.read_tangents(), &uvs) {
        (Some(tangents), _) => tangents.collect(),
        (None, Some(uvs)) => tangents(&positions, &normals, uvs, &indices),
        (None, None) => normals
            .iter()
            .map(|&normal| arbitrary_tangent(Vec3::from(normal)).to_array())
            .collect(),
    };
    let uvs = uvs.unwrap_or_else(|| vec![[0.0; 2]; positions.len()]);
    let surface = normals
        .iter()
        .zip(&tangents)
        .zip(&uvs)
        .map(|((&normal, &tangent), &uv)| SurfaceVertex { normal, tangent, uv })
        .collect();
    Some(CpuMesh {
        name,
        positions,
        surface,
        indices,
        material,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUAD: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

    #[test]
    fn missing_normals_follow_the_winding() {
        let normals = smooth_normals(&QUAD, &QUAD_INDICES);
        assert!(
            normals
                .iter()
                .all(|normal| normal.iter().zip([0.0, 0.0, 1.0]).all(|(a, b)| (a - b).abs() < 1e-6))
        );
    }

    #[test]
    fn tangent_follows_increasing_u_and_handedness_follows_v() {
        // V grows downwards in glTF: the quad's top edge (y = 1) has the smaller v.
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let normals = [[0.0, 0.0, 1.0]; 4];
        let tangents = tangents(&QUAD, &normals, &uvs, &QUAD_INDICES);
        for tangent in tangents {
            assert!(
                tangent
                    .iter()
                    .zip([1.0, 0.0, 0.0, 1.0])
                    .all(|(a, b)| (a - b).abs() < 1e-6),
                "{tangent:?}"
            );
        }
    }

    #[test]
    fn mirrored_uvs_flip_the_handedness() {
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let normals = [[0.0, 0.0, 1.0]; 4];
        let tangents = tangents(&QUAD, &normals, &uvs, &QUAD_INDICES);
        assert!(tangents.iter().all(|tangent| (tangent[3] + 1.0).abs() < 1e-6));
    }
}
