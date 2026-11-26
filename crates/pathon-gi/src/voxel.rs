use pathon_api::glam::{UVec3, Vec3};
use pathon_scene::Aabb;

/// The grid of voxels the scene is reduced to: cell `(x, y, z)` is number `x + nx * (y + ny * z)`, `size` metres on a
/// side, its corner nearest the origin at `origin + (x, y, z) * size`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VoxelLayout {
    pub origin: Vec3,
    pub size: f32,
    pub dims: UVec3,
}

/// Bytes of the voxel buffers per cell: albedo, emission, the radiance of six faces and a place in the list of solid
/// cells, a word each.
pub const BYTES_PER_CELL: u64 = 4 * (2 + 6 + 1);

impl VoxelLayout {
    /// A grid with a cell of at least `size` metres that covers `bounds` plus one cell of margin on each side and has
    /// at most `max_cells` cells (the cell grows until it fits). `None` for a box that is not finite.
    pub fn fit(bounds: Aabb, size: f32, max_cells: u32) -> Option<Self> {
        if !(bounds.is_bounded() && size.is_finite() && size > 0.0) {
            return None;
        }
        let mut size = size;
        loop {
            let margin = Vec3::splat(size);
            let origin = bounds.min - margin;
            let extent = bounds.max - bounds.min + margin * 2.0;
            let dims = (extent / size).ceil().as_uvec3().max(UVec3::ONE);
            let cells = u64::from(dims.x) * u64::from(dims.y) * u64::from(dims.z);
            if cells <= u64::from(max_cells) {
                return Some(Self { origin, size, dims });
            }
            size *= 1.05;
        }
    }

    pub fn cell_count(&self) -> u32 {
        self.dims.x * self.dims.y * self.dims.z
    }

    pub fn index(&self, cell: UVec3) -> u32 {
        cell.x + self.dims.x * (cell.y + self.dims.y * cell.z)
    }

    pub fn cell_of(&self, position: Vec3) -> Option<UVec3> {
        let cell = ((position - self.origin) / self.size).floor();
        (cell.cmpge(Vec3::ZERO).all() && cell.cmplt(self.dims.as_vec3()).all()).then(|| cell.as_uvec3())
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::new(self.origin, self.origin + self.dims.as_vec3() * self.size)
    }
}

/// The bytes of WGSL `VoxelGrid`, 32 bytes in a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VoxelGridUniforms {
    origin_size: [f32; 4],
    dims: [u32; 4],
}

impl VoxelGridUniforms {
    pub(crate) fn new(layout: &VoxelLayout) -> Self {
        Self {
            origin_size: [layout.origin.x, layout.origin.y, layout.origin.z, layout.size],
            dims: [layout.dims.x, layout.dims.y, layout.dims.z, layout.cell_count()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_covers_the_box_with_a_margin() {
        let bounds = Aabb::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 2.0, 3.0));

        let layout = VoxelLayout::fit(bounds, 0.5, 1_000_000).expect("a finite box");

        let covered = layout.bounds();
        assert!(covered.min.cmple(bounds.min - Vec3::splat(0.5)).all());
        assert!(covered.max.cmpge(bounds.max + Vec3::splat(0.5)).all());
        assert_eq!(layout.dims, UVec3::new(10, 6, 8));
    }

    #[test]
    fn the_cell_grows_to_fit_the_budget() {
        let bounds = Aabb::new(Vec3::ZERO, Vec3::new(20.0, 10.0, 20.0));

        let layout = VoxelLayout::fit(bounds, 0.05, 100_000).expect("a finite box");

        assert!(layout.cell_count() <= 100_000);
        assert!(layout.size > 0.05);
    }

    #[test]
    fn positions_find_their_cell() {
        let layout = VoxelLayout {
            origin: Vec3::new(-1.0, 0.0, 0.0),
            size: 0.5,
            dims: UVec3::new(4, 4, 4),
        };

        assert_eq!(layout.cell_of(Vec3::new(-0.9, 0.1, 1.9)), Some(UVec3::new(0, 0, 3)));
        assert_eq!(layout.cell_of(Vec3::new(-1.1, 0.1, 0.1)), None);
        assert_eq!(layout.cell_of(Vec3::new(1.1, 0.1, 0.1)), None);
    }

    #[test]
    fn the_uniform_block_has_the_size_the_shader_expects() {
        assert_eq!(size_of::<VoxelGridUniforms>(), 32);
    }
}
