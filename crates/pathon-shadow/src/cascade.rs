// The CPU side of cascaded shadow maps: where the camera's depth range is cut, and the orthographic box that covers each
// slice. Light space has axes (right, up, forward) with forward the direction the light travels, so `t` grows away
// from the light; the depth map stores reverse-Z, `(t_far - t) / (t_far - t_near)`, like every other depth in pathon.
//
// Splits are the practical scheme of Zhang et al. (2006). Stable fitting is Valient's (2008): a bounding sphere of the
// slice, so the box does not change size with the view direction, snapped to the texel grid so it does not swim with
// the position. The sphere is computed in camera space from the lens alone, so it is bit-identical from frame to frame.

use pathon_api::Camera;
use pathon_api::glam::{Mat4, Vec2, Vec3, Vec4};
use pathon_scene::Aabb;

use crate::settings::{CascadeFit, SplitScheme};

pub const MAX_CASCADES: usize = 4;

// Padding of the near end of the depth range, so a caster exactly on the boundary is not clipped.
const DEPTH_MARGIN: f32 = 1.0e-3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrustumKind {
    Perspective,
    Orthographic,
}

/// The part of a camera the cascades need, read back from its projection matrix (reverse-Z, x and y symmetric; jitter
/// is ignored, a few texels of it do not move a slice).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CameraFrustum {
    pub kind: FrustumKind,
    pub view_to_world: Mat4,
    /// Tangent of the half field of view per axis for a perspective camera, half the visible size in metres for an
    /// orthographic one.
    pub half_extent: Vec2,
    pub near: f32,
    /// Infinite for an infinite far plane.
    pub far: f32,
}

impl CameraFrustum {
    pub fn from_camera(camera: &Camera) -> Option<Self> {
        let projection = camera.projection;
        let (a, b) = (projection.z_axis.z, projection.w_axis.z);
        let (scale_x, scale_y) = (projection.x_axis.x, projection.y_axis.y);
        let symmetric = scale_x.is_finite() && scale_y.is_finite() && scale_x > 0.0 && scale_y > 0.0;
        if !symmetric || !camera.world_to_view.is_finite() {
            return None;
        }
        let half_extent = Vec2::new(1.0 / scale_x, 1.0 / scale_y);
        let (kind, near, far) = if (projection.z_axis.w + 1.0).abs() < 1.0e-4 && projection.w_axis.w.abs() < 1.0e-4 {
            // clip.z = a * view.z + b and clip.w = -view.z with the near plane at 1 and the far plane at 0.
            let far = if a.abs() < 1.0e-9 { f32::INFINITY } else { b / a };
            (FrustumKind::Perspective, b / (1.0 + a), far)
        } else if projection.z_axis.w.abs() < 1.0e-4 && (projection.w_axis.w - 1.0).abs() < 1.0e-4 && a > 0.0 {
            (FrustumKind::Orthographic, (b - 1.0) / a, b / a)
        } else {
            return None;
        };
        let usable = near.is_finite() && near > 0.0 && (far > near);
        usable.then(|| Self {
            kind,
            view_to_world: camera.world_to_view.inverse(),
            half_extent,
            near,
            far,
        })
    }

    /// The eight corners of the slice between two view distances, near face first.
    pub fn slice_corners(&self, near: f32, far: f32) -> [Vec3; 8] {
        let mut corners = [Vec3::ZERO; 8];
        for (index, corner) in corners.iter_mut().enumerate() {
            let distance = if index < 4 { near } else { far };
            let scale = match self.kind {
                FrustumKind::Perspective => self.half_extent * distance,
                FrustumKind::Orthographic => self.half_extent,
            };
            let x = if index & 1 == 0 { -scale.x } else { scale.x };
            let y = if index & 2 == 0 { -scale.y } else { scale.y };
            *corner = self.view_to_world.transform_point3(Vec3::new(x, y, -distance));
        }
        corners
    }

    /// Centre (world) and radius of the smallest sphere on the camera's axis that holds the slice. It depends only on
    /// the lens and the two distances, never on where the camera is or looks.
    pub fn slice_sphere(&self, near: f32, far: f32) -> (Vec3, f32) {
        let (distance, radius) = match self.kind {
            FrustumKind::Perspective => {
                let k = self.half_extent.length_squared();
                // Equal distance to the corners of both faces, unless that lies beyond the far face.
                let distance = (far.midpoint(near) * (k + 1.0)).min(far);
                let far_radius = (far * far * k + (far - distance).powi(2)).sqrt();
                let near_radius = (near * near * k + (distance - near).powi(2)).sqrt();
                (distance, far_radius.max(near_radius))
            }
            FrustumKind::Orthographic => {
                let half_depth = 0.5 * (far - near);
                (
                    far.midpoint(near),
                    (self.half_extent.length_squared() + half_depth * half_depth).sqrt(),
                )
            }
        };
        let centre = self.view_to_world.transform_point3(Vec3::new(0.0, 0.0, -distance));
        (centre, radius)
    }
}

/// `count + 1` view distances: the near plane, the `count - 1` splits and the far end of the shadowed range.
pub fn split_distances(near: f32, far: f32, count: usize, scheme: SplitScheme) -> Vec<f32> {
    let count = count.clamp(1, MAX_CASCADES);
    let mut distances = vec![near; count + 1];
    distances[count] = far;
    for cascade in 1..count {
        let fraction = cascade as f32 / count as f32;
        distances[cascade] = match scheme {
            SplitScheme::Practical { lambda } => {
                let logarithmic = near * (far / near).powf(fraction);
                let uniform = near + (far - near) * fraction;
                lambda * logarithmic + (1.0 - lambda) * uniform
            }
            SplitScheme::Fractions(fractions) => near + (far - near) * fractions[cascade - 1],
        };
    }
    distances
}

/// Orthonormal frame of the light: `forward` is the direction it shines along.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LightBasis {
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
}

impl LightBasis {
    pub fn new(direction: Vec3) -> Option<Self> {
        let forward = direction.try_normalize().filter(|forward| forward.is_finite())?;
        // The reference axis is Y unless the light points almost straight up or down, where the frame would spin.
        let reference = if forward.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
        let right = forward.cross(reference).normalize();
        Some(Self {
            right,
            up: right.cross(forward),
            forward,
        })
    }

    /// `(x, y, t)` of a world point.
    pub fn coordinates(&self, point: Vec3) -> Vec3 {
        Vec3::new(point.dot(self.right), point.dot(self.up), point.dot(self.forward))
    }

    /// The box of `aabb` in light space as `(centre, half extent)`.
    pub fn box_of(&self, aabb: &Aabb) -> (Vec3, Vec3) {
        let half = aabb.half_extents();
        (
            self.coordinates(aabb.center()),
            Vec3::new(
                self.right.abs().dot(half),
                self.up.abs().dot(half),
                self.forward.abs().dot(half),
            ),
        )
    }
}

/// One cascade: the slice of view depth it serves and the transforms of its depth map.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cascade {
    /// World to clip space of the light, x and y in -1..1 over the map, z in 1 (nearest to the light) to 0.
    pub view_projection: Mat4,
    /// The same with the casters moved `constant_bias` metres away from the light: what the depth pass renders with.
    pub caster_view_projection: Mat4,
    /// Metres per texel.
    pub texel_world_size: f32,
    pub near_depth: f32,
    pub far_depth: f32,
    /// Light-space `(x, y, t)` extent of the box, for culling.
    pub light_min: Vec3,
    pub light_max: Vec3,
}

impl Cascade {
    /// Whether the world box can put anything in this cascade's map.
    pub fn may_receive_from(&self, basis: &LightBasis, aabb: &Aabb) -> bool {
        if !aabb.is_bounded() {
            return true;
        }
        let (centre, half) = basis.box_of(aabb);
        let (low, high) = (centre - half, centre + half);
        low.x <= self.light_max.x
            && high.x >= self.light_min.x
            && low.y <= self.light_max.y
            && high.y >= self.light_min.y
            && high.z >= self.light_min.z
            && low.z <= self.light_max.z
    }

    /// Texture coordinates (0..1, y down) and the depth to compare against for a world point.
    pub fn project(&self, point: Vec3) -> Vec3 {
        let clip = self.view_projection * Vec4::new(point.x, point.y, point.z, 1.0);
        Vec3::new(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5, clip.z)
    }
}

fn light_matrix(basis: &LightBasis, centre: Vec2, half: Vec2, t_near: f32, t_far: f32, caster_shift: f32) -> Mat4 {
    let (scale_x, scale_y) = (1.0 / half.x, 1.0 / half.y);
    let depth_scale = 1.0 / (t_far - t_near);
    let rows = [
        (basis.right * scale_x, -centre.x * scale_x),
        (basis.up * scale_y, -centre.y * scale_y),
        (-basis.forward * depth_scale, (t_far - caster_shift) * depth_scale),
    ];
    Mat4::from_cols(
        Vec4::new(rows[0].0.x, rows[1].0.x, rows[2].0.x, 0.0),
        Vec4::new(rows[0].0.y, rows[1].0.y, rows[2].0.y, 0.0),
        Vec4::new(rows[0].0.z, rows[1].0.z, rows[2].0.z, 0.0),
        Vec4::new(rows[0].1, rows[1].1, rows[2].1, 1.0),
    )
}

/// What `plan_cascades` reads besides the camera and the light.
#[derive(Clone, Copy, Debug)]
pub struct CascadeRequest<'scene> {
    pub cascade_count: usize,
    pub resolution: u32,
    pub max_distance: f32,
    pub scheme: SplitScheme,
    pub fit: CascadeFit,
    pub constant_bias: f32,
    pub unbounded_caster_distance: f32,
    /// World boxes of every shadow caster.
    pub casters: &'scene [Aabb],
}

/// The cascades covering the camera's depth range from its near plane to `max_distance` without a gap; empty when
/// the camera has no usable range.
pub fn plan_cascades(frustum: &CameraFrustum, basis: &LightBasis, request: &CascadeRequest<'_>) -> Vec<Cascade> {
    let far = frustum.far.min(request.max_distance);
    if far <= frustum.near * 1.0001 {
        return Vec::new();
    }
    let splits = split_distances(frustum.near, far, request.cascade_count, request.scheme);
    splits
        .windows(2)
        .map(|range| fit_cascade(frustum, basis, request, range[0], range[1]))
        .collect()
}

fn fit_cascade(
    frustum: &CameraFrustum,
    basis: &LightBasis,
    request: &CascadeRequest<'_>,
    near: f32,
    far: f32,
) -> Cascade {
    let resolution = request.resolution as f32;
    // The box as a centre and half size, and the light-space depth the slice occupies.
    let (centre, half, t_low, t_high) = match request.fit {
        CascadeFit::Stable => {
            let (sphere_centre, radius) = frustum.slice_sphere(near, far);
            let centre = basis.coordinates(sphere_centre);
            let texel = 2.0 * radius / resolution;
            let snapped = Vec2::new((centre.x / texel).round() * texel, (centre.y / texel).round() * texel);
            (snapped, Vec2::splat(radius), centre.z - radius, centre.z + radius)
        }
        CascadeFit::Focused => {
            let mut low = Vec3::splat(f32::INFINITY);
            let mut high = Vec3::splat(f32::NEG_INFINITY);
            for corner in frustum.slice_corners(near, far) {
                let point = basis.coordinates(corner);
                low = low.min(point);
                high = high.max(point);
            }
            (
                (low.truncate() + high.truncate()) * 0.5,
                (high.truncate() - low.truncate()) * 0.5,
                low.z,
                high.z,
            )
        }
    };
    let (low, high) = (centre - half, centre + half);

    let mut t_near = t_low;
    for caster in request.casters {
        if !caster.is_bounded() {
            t_near = t_near.min(t_low - request.unbounded_caster_distance);
            continue;
        }
        let (centre, half) = basis.box_of(caster);
        let overlaps_map = centre.x + half.x >= low.x
            && centre.x - half.x <= high.x
            && centre.y + half.y >= low.y
            && centre.y - half.y <= high.y;
        // Casters beyond the slice cannot shadow it; the ones in front of it must be inside the depth range.
        if overlaps_map && centre.z - half.z <= t_high {
            t_near = t_near.min(centre.z - half.z);
        }
    }
    let t_near = t_near - DEPTH_MARGIN;

    let texel_world_size = 2.0 * half.max_element() / resolution;
    Cascade {
        view_projection: light_matrix(basis, centre, half, t_near, t_high, 0.0),
        caster_view_projection: light_matrix(basis, centre, half, t_near, t_high, request.constant_bias),
        texel_world_size,
        near_depth: near,
        far_depth: far,
        light_min: Vec3::new(low.x, low.y, t_near),
        light_max: Vec3::new(high.x, high.y, t_high),
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::{Quat, Vec3};

    use super::*;

    const RESOLUTION: u32 = 1024;

    fn camera_at(position: Vec3, yaw: f32) -> Camera {
        let target = position + Quat::from_rotation_y(yaw) * Vec3::new(0.0, -0.2, -1.0);
        Camera::look_at(position, target, 60.0_f32.to_radians(), 16.0 / 9.0)
    }

    fn request(casters: &[Aabb]) -> CascadeRequest<'_> {
        CascadeRequest {
            cascade_count: 4,
            resolution: RESOLUTION,
            max_distance: 120.0,
            scheme: SplitScheme::default(),
            fit: CascadeFit::Stable,
            constant_bias: 0.001,
            unbounded_caster_distance: 100.0,
            casters,
        }
    }

    fn sun() -> LightBasis {
        LightBasis::new(Vec3::new(-0.4, -1.0, -0.3)).expect("a direction")
    }

    // Deterministic points inside a slice: a lattice over the frustum with a different offset on every axis.
    fn points_in_slice(frustum: &CameraFrustum, near: f32, far: f32) -> Vec<Vec3> {
        let mut points = Vec::new();
        for depth in 0..=8 {
            let distance = near + (far - near) * depth as f32 / 8.0;
            let scale = match frustum.kind {
                FrustumKind::Perspective => frustum.half_extent * distance,
                FrustumKind::Orthographic => frustum.half_extent,
            };
            for x in -4..=4 {
                for y in -4..=4 {
                    let view = Vec3::new(scale.x * x as f32 / 4.0, scale.y * y as f32 / 4.0, -distance);
                    points.push(frustum.view_to_world.transform_point3(view));
                }
            }
        }
        points
    }

    #[test]
    fn the_frustum_is_read_back_from_the_projection() {
        let camera = Camera::look_at(Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO, 1.0, 2.0);

        let frustum = CameraFrustum::from_camera(&camera).expect("perspective");

        assert_eq!(frustum.kind, FrustumKind::Perspective);
        assert!((frustum.near - 0.05).abs() < 1.0e-5, "{}", frustum.near);
        assert!(frustum.far.is_infinite());
        assert!((frustum.half_extent.y - 0.5_f32.tan()).abs() < 1.0e-5);
        assert!((frustum.half_extent.x / frustum.half_extent.y - 2.0).abs() < 1.0e-5);
    }

    #[test]
    fn finite_and_orthographic_projections_are_recognised() {
        use pathon_scene::Projection;
        let view = Camera::default().world_to_view;
        let finite = Camera {
            projection: Projection::Perspective {
                vertical_fov: 1.0,
                near: 0.5,
                far: Some(80.0),
            }
            .matrix(1.5),
            world_to_view: view,
            position: Vec3::ZERO,
        };
        let ortho = Camera {
            projection: Projection::orthographic(10.0, 1.0, 50.0).matrix(1.5),
            ..finite
        };

        let finite = CameraFrustum::from_camera(&finite).expect("finite far plane");
        let ortho = CameraFrustum::from_camera(&ortho).expect("orthographic");

        assert!((finite.near - 0.5).abs() < 1.0e-4 && (finite.far - 80.0).abs() < 1.0e-2);
        assert_eq!(ortho.kind, FrustumKind::Orthographic);
        assert!((ortho.near - 1.0).abs() < 1.0e-4 && (ortho.far - 50.0).abs() < 1.0e-3);
        assert!((ortho.half_extent - Vec2::new(7.5, 5.0)).abs().max_element() < 1.0e-4);
    }

    #[test]
    fn a_projection_without_a_depth_range_is_refused() {
        let camera = Camera {
            projection: Mat4::ZERO,
            ..Camera::default()
        };

        assert_eq!(CameraFrustum::from_camera(&camera), None);
    }

    #[test]
    fn splits_start_at_near_end_at_far_and_grow() {
        for scheme in [
            SplitScheme::Practical { lambda: 0.0 },
            SplitScheme::Practical { lambda: 0.6 },
            SplitScheme::Practical { lambda: 1.0 },
            SplitScheme::Fractions([0.125, 0.25, 0.5]),
        ] {
            let splits = split_distances(0.1, 100.0, 4, scheme);

            assert_eq!((splits[0], splits[4]), (0.1, 100.0), "{scheme:?}");
            assert!(
                splits.windows(2).all(|pair| pair[0] < pair[1]),
                "{scheme:?}: {splits:?}"
            );
        }
    }

    #[test]
    fn uniform_and_logarithmic_splits_match_their_formulas() {
        let uniform = split_distances(1.0, 101.0, 4, SplitScheme::Practical { lambda: 0.0 });
        let logarithmic = split_distances(1.0, 16.0, 4, SplitScheme::Practical { lambda: 1.0 });

        assert!((uniform[1] - 26.0).abs() < 1.0e-4 && (uniform[2] - 51.0).abs() < 1.0e-4);
        assert!((logarithmic[1] - 2.0).abs() < 1.0e-4 && (logarithmic[2] - 4.0).abs() < 1.0e-4);
        assert!((logarithmic[3] - 8.0).abs() < 1.0e-3);
    }

    #[test]
    fn one_cascade_spans_the_whole_range() {
        let splits = split_distances(0.5, 40.0, 1, SplitScheme::default());

        assert_eq!(splits, [0.5, 40.0]);
    }

    #[test]
    fn the_light_basis_is_orthonormal_and_right_handed() {
        for direction in [
            Vec3::new(-0.4, -1.0, -0.3),
            Vec3::NEG_Y,
            Vec3::Y,
            Vec3::X,
            Vec3::new(0.0, -0.995, 0.1),
        ] {
            let basis = LightBasis::new(direction).expect("a direction");

            assert!((basis.right.length() - 1.0).abs() < 1.0e-5);
            assert!(basis.right.dot(basis.up).abs() < 1.0e-5);
            assert!(basis.up.dot(basis.forward).abs() < 1.0e-5);
            assert!(
                (basis.right.cross(basis.up) - -basis.forward).length() < 1.0e-4,
                "{direction}"
            );
        }
        assert_eq!(LightBasis::new(Vec3::ZERO), None);
        assert_eq!(LightBasis::new(Vec3::NAN), None);
    }

    #[test]
    fn cascades_cover_the_whole_range_without_gaps() {
        let casters = [Aabb::new(Vec3::new(-20.0, 0.0, -40.0), Vec3::new(20.0, 8.0, 10.0))];
        for fit in [CascadeFit::Stable, CascadeFit::Focused] {
            let camera = camera_at(Vec3::new(3.0, 1.7, 5.0), 0.7);
            let frustum = CameraFrustum::from_camera(&camera).expect("frustum");
            let cascades = plan_cascades(
                &frustum,
                &sun(),
                &CascadeRequest {
                    fit,
                    ..request(&casters)
                },
            );

            assert_eq!(cascades.len(), 4);
            assert_eq!(cascades[0].near_depth, frustum.near);
            assert_eq!(cascades[3].far_depth, 120.0);
            for pair in cascades.windows(2) {
                assert_eq!(pair[0].far_depth, pair[1].near_depth, "no gap between cascades");
            }
            for cascade in &cascades {
                for point in points_in_slice(&frustum, cascade.near_depth, cascade.far_depth) {
                    let uv = cascade.project(point);
                    let epsilon = 1.0e-3;
                    assert!(
                        (-epsilon..=1.0 + epsilon).contains(&uv.x)
                            && (-epsilon..=1.0 + epsilon).contains(&uv.y)
                            && (-epsilon..=1.0 + epsilon).contains(&uv.z),
                        "{fit:?} cascade {}-{}: {point} -> {uv}",
                        cascade.near_depth,
                        cascade.far_depth
                    );
                }
            }
        }
    }

    #[test]
    fn every_distance_belongs_to_exactly_one_cascade() {
        let camera = camera_at(Vec3::ZERO, 0.0);
        let frustum = CameraFrustum::from_camera(&camera).expect("frustum");
        let cascades = plan_cascades(&frustum, &sun(), &request(&[]));
        let splits: Vec<f32> = cascades.iter().skip(1).map(|cascade| cascade.near_depth).collect();

        for step in 0..=1000 {
            let depth = frustum.near + (120.0 - frustum.near) * step as f32 / 1000.0;
            // The shader's rule: the cascade is the number of splits the depth is beyond.
            let chosen = splits.iter().filter(|split| depth > **split).count();
            assert!(depth >= cascades[chosen].near_depth && depth <= cascades[chosen].far_depth);
        }
    }

    #[test]
    fn a_sphere_cascade_covers_an_orthographic_camera_too() {
        use pathon_scene::Projection;
        let camera = Camera {
            projection: Projection::orthographic(20.0, 0.5, 200.0).matrix(1.6),
            ..camera_at(Vec3::new(0.0, 10.0, 0.0), 0.3)
        };
        let frustum = CameraFrustum::from_camera(&camera).expect("orthographic");

        let cascades = plan_cascades(&frustum, &sun(), &request(&[]));

        assert_eq!(cascades.len(), 4);
        for cascade in &cascades {
            for point in points_in_slice(&frustum, cascade.near_depth, cascade.far_depth) {
                let uv = cascade.project(point);
                assert!(
                    (-1.0e-3..=1.001).contains(&uv.x) && (-1.0e-3..=1.001).contains(&uv.y),
                    "{uv}"
                );
            }
        }
    }

    #[test]
    fn nearer_to_the_light_means_a_larger_depth() {
        let camera = camera_at(Vec3::new(0.0, 1.7, 0.0), 0.0);
        let frustum = CameraFrustum::from_camera(&camera).expect("frustum");
        let basis = sun();
        let cascade = plan_cascades(&frustum, &basis, &request(&[]))[1];
        let (centre, _) = frustum.slice_sphere(cascade.near_depth, cascade.far_depth);

        let at_centre = cascade.project(centre);
        let towards_light = cascade.project(centre - basis.forward);
        let away = cascade.project(centre + basis.forward);

        assert!(towards_light.z > at_centre.z && at_centre.z > away.z);
        assert!((towards_light.z - at_centre.z - 1.0 / (cascade.light_max.z - cascade.light_min.z)).abs() < 1.0e-4);
        assert!(
            (away.x - at_centre.x).abs() < 1.0e-5,
            "moving along the light keeps the texel"
        );
    }

    #[test]
    fn casters_between_the_slice_and_the_light_stay_inside_the_depth_range() {
        let camera = camera_at(Vec3::new(0.0, 1.7, 0.0), 0.0);
        let frustum = CameraFrustum::from_camera(&camera).expect("frustum");
        let basis = sun();
        // A tall tower right above the camera and a caster that is far from every cascade.
        let tower = Aabb::new(Vec3::new(-1.0, 0.0, -9.0), Vec3::new(1.0, 60.0, -7.0));
        let away = Aabb::new(Vec3::new(900.0, 0.0, 900.0), Vec3::new(901.0, 1.0, 901.0));
        let without = plan_cascades(&frustum, &basis, &request(&[]));
        let with = plan_cascades(&frustum, &basis, &request(&[tower, away]));

        let (centre, half) = basis.box_of(&tower);
        let top = centre.z - half.z;
        assert!(with.iter().any(|cascade| cascade.light_min.z <= top));
        for (a, b) in with.iter().zip(&without) {
            assert!(a.light_min.z <= b.light_min.z, "casters never shrink the range");
            assert_eq!(a.light_max.z, b.light_max.z);
        }
        assert!(
            with[3].light_min.z >= without[3].light_min.z - 1.0e-3,
            "a caster outside the box does not widen it"
        );
    }

    #[test]
    fn a_caster_without_bounds_extends_every_cascade_by_the_configured_distance() {
        let camera = camera_at(Vec3::new(0.0, 1.7, 0.0), 0.0);
        let frustum = CameraFrustum::from_camera(&camera).expect("frustum");
        let without = plan_cascades(&frustum, &sun(), &request(&[]));
        let with = plan_cascades(&frustum, &sun(), &request(&[Aabb::UNBOUNDED]));

        for (a, b) in with.iter().zip(&without) {
            assert!((b.light_min.z - a.light_min.z - 100.0).abs() < 1.0e-2);
        }
    }

    // A shadow edge may not swim: moving or turning the camera moves the map by whole texels.
    #[test]
    fn stable_cascades_move_in_whole_texels() {
        let basis = sun();
        let probe = Vec3::new(3.3, 0.2, -11.7);
        let reference_camera = camera_at(Vec3::new(2.0, 1.7, 4.0), 0.4);
        let reference_frustum = CameraFrustum::from_camera(&reference_camera).expect("frustum");
        let reference = plan_cascades(&reference_frustum, &basis, &request(&[]));

        for (position, yaw) in [
            (Vec3::new(2.3, 1.7, 4.0), 0.4),
            (Vec3::new(2.0, 1.7, 3.1), 0.4),
            (Vec3::new(-15.37, 1.7, 22.9), 0.4),
            (Vec3::new(2.0, 1.7, 4.0), 1.9),
            (Vec3::new(40.0, 30.0, -4.0), -2.5),
        ] {
            let frustum = CameraFrustum::from_camera(&camera_at(position, yaw)).expect("frustum");
            let moved = plan_cascades(&frustum, &basis, &request(&[]));

            for (cascade, base) in moved.iter().zip(&reference) {
                assert_eq!(
                    cascade.texel_world_size, base.texel_world_size,
                    "the box keeps its size"
                );
                let texels = |cascade: &Cascade| cascade.project(probe).truncate() * RESOLUTION as f32;
                let shift = texels(cascade) - texels(base);
                // Large coordinates cost single-precision digits: a hundredth of a texel is still a whole texel.
                assert!(
                    (shift - shift.round()).abs().max_element() < 0.02,
                    "cascade {}-{} at {position} yaw {yaw}: shifted by {shift}",
                    cascade.near_depth,
                    cascade.far_depth
                );
            }
        }
    }

    #[test]
    fn focused_cascades_are_tighter_than_stable_ones() {
        let camera = camera_at(Vec3::new(0.0, 1.7, 0.0), 0.0);
        let frustum = CameraFrustum::from_camera(&camera).expect("frustum");
        let stable = plan_cascades(&frustum, &sun(), &request(&[]));
        let focused = plan_cascades(
            &frustum,
            &sun(),
            &CascadeRequest {
                fit: CascadeFit::Focused,
                ..request(&[])
            },
        );

        assert!(
            stable
                .iter()
                .zip(&focused)
                .all(|(s, f)| f.texel_world_size <= s.texel_world_size + 1.0e-6)
        );
        assert!(
            stable
                .iter()
                .zip(&focused)
                .any(|(s, f)| f.texel_world_size < 0.9 * s.texel_world_size)
        );
    }

    #[test]
    fn the_constant_bias_moves_casters_away_from_the_light_only_in_the_caster_matrix() {
        let camera = camera_at(Vec3::new(0.0, 1.7, 0.0), 0.0);
        let frustum = CameraFrustum::from_camera(&camera).expect("frustum");
        let basis = sun();
        let cascade = plan_cascades(&frustum, &basis, &request(&[]))[0];
        let point = frustum.slice_corners(0.5, 1.0)[0];

        let sampled = cascade.view_projection * point.extend(1.0);
        let rendered = cascade.caster_view_projection * point.extend(1.0);

        assert_eq!((sampled.x, sampled.y), (rendered.x, rendered.y));
        let range = cascade.light_max.z - cascade.light_min.z;
        assert!(
            (sampled.z - rendered.z - 0.001 / range).abs() < 1.0e-6,
            "farther means a smaller depth"
        );
    }

    #[test]
    fn a_camera_that_sees_nothing_in_range_plans_nothing() {
        let frustum = CameraFrustum::from_camera(&Camera::default()).expect("frustum");

        let cascades = plan_cascades(
            &frustum,
            &sun(),
            &CascadeRequest {
                max_distance: 0.04,
                ..request(&[])
            },
        );

        assert_eq!(cascades, []);
    }
}
