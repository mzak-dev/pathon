use pathon_api::Camera;
use pathon_api::glam::{Mat4, Vec3, Vec4};

/// The 12 edges of a cube whose corner `i` has x in bit 0, y in bit 1 and z in bit 2.
pub(crate) fn cuboid_edges(corners: &[Vec3; 8]) -> impl Iterator<Item = (Vec3, Vec3)> + '_ {
    (0..8_usize).flat_map(move |corner| {
        [1_usize, 2, 4]
            .into_iter()
            .filter(move |bit| corner & bit == 0)
            .map(move |bit| (corners[corner], corners[corner | bit]))
    })
}

pub(crate) fn aabb_corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|corner| {
        Vec3::new(
            if corner & 1 == 0 { min.x } else { max.x },
            if corner & 2 == 0 { min.y } else { max.y },
            if corner & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// Corners of the unit cube `[-0.5, 0.5]^3` moved by `transform`.
pub(crate) fn cuboid_corners(transform: Mat4) -> [Vec3; 8] {
    aabb_corners(Vec3::splat(-0.5), Vec3::splat(0.5)).map(|corner| transform.transform_point3(corner))
}

pub(crate) fn circle_points(center: Vec3, normal: Vec3, radius: f32, segments: u32) -> Vec<Vec3> {
    let Some(normal) = normal.try_normalize() else {
        return Vec::new();
    };
    let (tangent, bitangent) = normal.any_orthonormal_pair();
    (0..segments)
        .map(|index| {
            let angle = index as f32 / segments as f32 * std::f32::consts::TAU;
            center + (tangent * angle.cos() + bitangent * angle.sin()) * radius
        })
        .collect()
}

/// The shaft is not included: four barbs that start at the tip and sweep back along the shaft.
pub(crate) fn arrow_head(from: Vec3, to: Vec3) -> Option<[(Vec3, Vec3); 4]> {
    let shaft = to - from;
    let length = shaft.length();
    let direction = shaft.try_normalize()?;
    let head_length = length * 0.2;
    let (side, up) = direction.any_orthonormal_pair();
    let base = to - direction * head_length;
    let spread = head_length * 0.4;
    Some([side, -side, up, -up].map(|offset| (to, base + offset * spread)))
}

fn unproject(inverse_view_projection: Mat4, ndc: Vec3) -> Vec3 {
    let homogeneous = inverse_view_projection * Vec4::new(ndc.x, ndc.y, ndc.z, 1.0);
    homogeneous.truncate() / homogeneous.w
}

/// Near-plane and far-plane corners of a camera frustum, `far` metres from the camera along its view axis.
///
/// The camera projection is reverse-Z, so the near plane is at depth 1; the far plane is found by extending the ray of
/// each corner, which also works for a projection whose far plane is at infinity.
pub(crate) fn frustum_corners(camera: &Camera, far: f32) -> Option<([Vec3; 4], [Vec3; 4])> {
    let inverse = camera.view_projection().inverse();
    if !inverse.is_finite() {
        return None;
    }
    let view_depth = |point: Vec3| -camera.world_to_view.transform_point3(point).z;
    let ndc_corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let mut near = [Vec3::ZERO; 4];
    let mut far_corners = [Vec3::ZERO; 4];
    for (index, (x, y)) in ndc_corners.into_iter().enumerate() {
        let on_near = unproject(inverse, Vec3::new(x, y, 1.0));
        let further = unproject(inverse, Vec3::new(x, y, 0.5));
        let (near_depth, further_depth) = (view_depth(on_near), view_depth(further));
        if (further_depth - near_depth).abs() < 1e-6 {
            return None;
        }
        near[index] = on_near;
        far_corners[index] = on_near + (further - on_near) * ((far - near_depth) / (further_depth - near_depth));
    }
    Some((near, far_corners))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cuboid_has_twelve_distinct_edges() {
        let corners = aabb_corners(Vec3::ZERO, Vec3::ONE);
        let edges: Vec<_> = cuboid_edges(&corners).collect();
        assert_eq!(edges.len(), 12);
        for (start, end) in &edges {
            assert_eq!((*start - *end).length(), 1.0, "every edge is one side long");
        }
    }

    #[test]
    fn a_transformed_cuboid_moves_its_corners() {
        let corners = cuboid_corners(Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(2.0)));
        assert_eq!(corners[0], Vec3::new(9.0, -1.0, -1.0));
        assert_eq!(corners[7], Vec3::new(11.0, 1.0, 1.0));
    }

    #[test]
    fn circle_points_lie_on_the_circle_in_the_plane() {
        let center = Vec3::new(1.0, 2.0, 3.0);
        let normal = Vec3::new(0.0, 1.0, 1.0).normalize();
        let points = circle_points(center, normal, 2.0, 16);
        assert_eq!(points.len(), 16);
        for point in points {
            assert!(((point - center).length() - 2.0).abs() < 1e-5);
            assert!((point - center).dot(normal).abs() < 1e-5);
        }
        assert!(circle_points(center, Vec3::ZERO, 1.0, 8).is_empty());
    }

    #[test]
    fn the_arrow_head_sits_at_the_tip_and_points_back() {
        let barbs = arrow_head(Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0)).expect("an arrow with a length");
        for (tip, end) in barbs {
            assert_eq!(tip, Vec3::new(0.0, 0.0, -5.0));
            assert!(end.z > -5.0 && end.z < -3.9, "{end}");
        }
        assert!(arrow_head(Vec3::ONE, Vec3::ONE).is_none());
    }

    #[test]
    fn frustum_far_corners_are_at_the_requested_depth() {
        let camera = Camera::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, std::f32::consts::FRAC_PI_2, 2.0);
        let (near, far) = frustum_corners(&camera, 10.0).expect("a perspective camera has a frustum");
        for corner in far {
            let depth = -camera.world_to_view.transform_point3(corner).z;
            assert!((depth - 10.0).abs() < 1e-3, "{depth}");
        }
        for corner in near {
            let depth = -camera.world_to_view.transform_point3(corner).z;
            assert!((depth - 0.05).abs() < 1e-4, "{depth}");
        }
        // 90 degrees of vertical field of view: the half height equals the depth, the half width is twice that.
        assert!((far[2].y - 10.0).abs() < 1e-3 && (far[2].x - 20.0).abs() < 1e-3, "{:?}", far[2]);
    }
}
