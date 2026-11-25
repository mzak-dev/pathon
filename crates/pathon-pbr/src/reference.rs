//! CPU references the GPU tests compare with: written from the definitions, in `f64` where precision matters, and not
//! sharing code with the shaders.

use pathon_api::glam::{DMat4, DVec2, DVec3, DVec4, Mat4, Vec2, Vec3};
use pathon_api::rhi::PhysicalSize;

use crate::cluster::{ClusterGrid, LAST_SLICE_END};

fn sign_not_zero(value: f32) -> f32 {
    if value >= 0.0 { 1.0 } else { -1.0 }
}

/// Octahedral encoding of a unit vector (the `normals` slot).
pub fn oct_encode(normal: Vec3) -> Vec2 {
    let projected = normal.truncate() / (normal.x.abs() + normal.y.abs() + normal.z.abs());
    if normal.z < 0.0 {
        Vec2::new(
            (1.0 - projected.y.abs()) * sign_not_zero(projected.x),
            (1.0 - projected.x.abs()) * sign_not_zero(projected.y),
        )
    } else {
        projected
    }
}

pub fn oct_decode(encoded: Vec2) -> Vec3 {
    let mut normal = Vec3::new(encoded.x, encoded.y, 1.0 - encoded.x.abs() - encoded.y.abs());
    if normal.z < 0.0 {
        normal = Vec3::new(
            (1.0 - encoded.y.abs()) * sign_not_zero(encoded.x),
            (1.0 - encoded.x.abs()) * sign_not_zero(encoded.y),
            normal.z,
        );
    }
    normal.normalize()
}

/// The pixel a world point is at, y down, for a view-projection matrix and a render size.
pub fn project_to_pixel(view_projection: Mat4, point: Vec3, render_size: PhysicalSize) -> Vec2 {
    let clip = view_projection * point.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    Vec2::new(
        (ndc.x * 0.5 + 0.5) * render_size.width as f32,
        (0.5 - ndc.y * 0.5) * render_size.height as f32,
    )
}

/// Expected `motion` of a world point that was at `previous_point` one frame ago: current pixel + motion = previous.
pub fn motion_of(
    view_projection: Mat4,
    previous_view_projection: Mat4,
    point: Vec3,
    previous_point: Vec3,
    render_size: PhysicalSize,
) -> Vec2 {
    project_to_pixel(previous_view_projection, previous_point, render_size)
        - project_to_pixel(view_projection, point, render_size)
}

/// One mip of the depth pyramid.
#[derive(Clone, PartialEq, Debug)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<f32>,
}

impl Level {
    pub fn at(&self, x: u32, y: u32) -> f32 {
        self.texels[(y * self.width + x) as usize]
    }
}

/// The Hi-Z chain of a reverse-Z depth image: every mip keeps the minimum (the farthest depth) of the texels below
/// it; an odd source size makes the last texel cover three rows or columns.
pub fn depth_pyramid(width: u32, height: u32, depth: &[f32]) -> Vec<Level> {
    let mut levels = vec![Level {
        width,
        height,
        texels: depth.to_vec(),
    }];
    while levels.last().is_some_and(|level| level.width > 1 || level.height > 1) {
        let source = levels.last().expect("at least one level");
        let (target_width, target_height) = ((source.width / 2).max(1), (source.height / 2).max(1));
        let mut texels = Vec::with_capacity((target_width * target_height) as usize);
        for y in 0..target_height {
            for x in 0..target_width {
                let span_x = if x + 1 == target_width && source.width % 2 == 1 { 3 } else { 2 };
                let span_y = if y + 1 == target_height && source.height % 2 == 1 { 3 } else { 2 };
                let mut farthest = 1.0_f32;
                for dy in 0..span_y {
                    for dx in 0..span_x {
                        let (sx, sy) = ((2 * x + dx).min(source.width - 1), (2 * y + dy).min(source.height - 1));
                        farthest = farthest.min(source.at(sx, sy));
                    }
                }
                texels.push(farthest);
            }
        }
        levels.push(Level {
            width: target_width,
            height: target_height,
            texels,
        });
    }
    levels
}

/// A point or spot light as the cluster assignment sees it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SphereLight {
    pub position: Vec3,
    pub range: f32,
}

/// The lights a cluster must have and may have: a light whose sphere clearly touches the cluster is in `must`, one
/// within `tolerance` of the boundary may be on either side of float rounding and is only in `may`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ClusterExpectation {
    pub must: Vec<u32>,
    pub may: Vec<u32>,
}

fn unproject(inverse: DMat4, ndc: DVec2, z: f64) -> DVec3 {
    let point = inverse * DVec4::new(ndc.x, ndc.y, z, 1.0);
    point.truncate() / point.w
}

fn ray_at_depth(inverse: DMat4, ndc: DVec2, depth: f64) -> DVec3 {
    let near_point = unproject(inverse, ndc, 1.0);
    let middle_point = unproject(inverse, ndc, 0.5);
    near_point + (middle_point - near_point) * ((-depth - near_point.z) / (middle_point.z - near_point.z))
}

/// The view-space box of cluster `(x, y, slice)`, from the definition of the grid.
pub fn cluster_bounds(
    grid: &ClusterGrid,
    projection: Mat4,
    render_size: PhysicalSize,
    (x, y, slice): (u32, u32, u32),
) -> (DVec3, DVec3) {
    let inverse = DMat4::from_cols_array_2d(&projection.to_cols_array_2d().map(|column| column.map(f64::from))).inverse();
    let (width, height) = (f64::from(render_size.width), f64::from(render_size.height));
    let tile = f64::from(grid.tile_size);
    let left = f64::from(x) * tile;
    let right = ((f64::from(x) + 1.0) * tile).min(width);
    let top = f64::from(y) * tile;
    let bottom = ((f64::from(y) + 1.0) * tile).min(height);
    let (near, far) = (f64::from(grid.near), f64::from(grid.far));
    let start = |slice: u32| {
        if slice == 0 {
            0.0
        } else {
            near * (far / near).powf(f64::from(slice) / f64::from(grid.slices))
        }
    };
    let (near_depth, far_depth) = (
        start(slice),
        if slice + 1 == grid.slices {
            f64::from(LAST_SLICE_END)
        } else {
            start(slice + 1)
        },
    );
    let mut low = DVec3::splat(f64::MAX);
    let mut high = DVec3::splat(f64::MIN);
    for depth in [near_depth, far_depth] {
        for (px, py) in [(left, top), (right, top), (left, bottom), (right, bottom)] {
            let ndc = DVec2::new(px / width * 2.0 - 1.0, 1.0 - py / height * 2.0);
            let point = ray_at_depth(inverse, ndc, depth);
            low = low.min(point);
            high = high.max(point);
        }
    }
    (low, high)
}

/// Brute force: every cluster against every light.
pub fn assign_lights(
    grid: &ClusterGrid,
    projection: Mat4,
    world_to_view: Mat4,
    render_size: PhysicalSize,
    lights: &[SphereLight],
    tolerance: f64,
) -> Vec<ClusterExpectation> {
    let centers: Vec<DVec3> = lights
        .iter()
        .map(|light| {
            let view = world_to_view * light.position.extend(1.0);
            DVec3::new(f64::from(view.x), f64::from(view.y), f64::from(view.z))
        })
        .collect();
    let mut clusters = Vec::with_capacity(grid.count() as usize);
    for slice in 0..grid.slices {
        for y in 0..grid.tiles_y {
            for x in 0..grid.tiles_x {
                let (low, high) = cluster_bounds(grid, projection, render_size, (x, y, slice));
                let mut expectation = ClusterExpectation::default();
                for (index, (center, light)) in centers.iter().zip(lights).enumerate() {
                    let nearest = center.clamp(low, high);
                    let margin = f64::from(light.range) - (*center - nearest).length();
                    let index = u32::try_from(index).expect("fewer than 2^32 lights");
                    if margin > tolerance {
                        expectation.must.push(index);
                        expectation.may.push(index);
                    } else if margin > -tolerance {
                        expectation.may.push(index);
                    }
                }
                clusters.push(expectation);
            }
        }
    }
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octahedral_encoding_round_trips_the_unit_sphere() {
        for index in 0..2000 {
            let t = index as f32 / 2000.0;
            let (azimuth, z) = (t * 97.0, 2.0 * ((t * 13.0).fract()) - 1.0);
            let radius = (1.0 - z * z).sqrt();
            let normal = Vec3::new(radius * azimuth.cos(), radius * azimuth.sin(), z);

            let decoded = oct_decode(oct_encode(normal));

            assert!((decoded - normal).length() < 1e-5, "{normal} -> {decoded}");
        }
        assert!((oct_decode(oct_encode(Vec3::NEG_Z)) - Vec3::NEG_Z).length() < 1e-6);
    }

    #[test]
    fn pyramid_of_odd_sizes_keeps_every_texel() {
        let depth: Vec<f32> = (0..15).map(|index| 0.1 + 0.05 * index as f32).collect();

        let levels = depth_pyramid(5, 3, &depth);

        let sizes: Vec<_> = levels.iter().map(|level| (level.width, level.height)).collect();
        assert_eq!(sizes, [(5, 3), (2, 1), (1, 1)]);
        assert_eq!(levels[1].at(0, 0), 0.1, "texels (0..2, 0..2)... the farthest of the 2x2 block");
        assert_eq!(levels[1].at(1, 0), 0.1 + 0.05 * 2.0, "covers columns 2, 3 and 4, rows 0 and 1 and 2");
        assert_eq!(levels[2].at(0, 0), 0.1);
    }
}
