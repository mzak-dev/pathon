use pathon_api::glam::{UVec3, Vec3};
use pathon_scene::Aabb;

/// Interior texels per side of a probe's irradiance tile, and the tile with its border (WGSL `IRRADIANCE_*`).
pub const IRRADIANCE_RES: u32 = 8;
pub const IRRADIANCE_TILE: u32 = IRRADIANCE_RES + 2;
/// The same for the distance moments.
pub const VISIBILITY_RES: u32 = 16;
pub const VISIBILITY_TILE: u32 = VISIBILITY_RES + 2;

const MAX_ATLAS_WIDTH: u32 = 4096;

/// Where the probes are and how the atlases hold them: a regular grid of `counts` probes `spacing` metres apart, probe
/// `(x, y, z)` being number `x + nx * (y + ny * z)` and tile number `i` of the atlas (row `i / tiles_per_row`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ProbeLayout {
    pub origin: Vec3,
    pub spacing: f32,
    pub counts: UVec3,
    pub tiles_per_row: u32,
}

impl ProbeLayout {
    /// A grid that covers `bounds`, with at most `max_probes` probes: the spacing grows until they fit. `None` for a
    /// box that is not finite or has no volume.
    pub fn fit(bounds: Aabb, spacing: f32, max_probes: u32) -> Option<Self> {
        if !bounds.is_bounded() || !(spacing.is_finite() && spacing > 0.0) {
            return None;
        }
        let extent = (bounds.max - bounds.min).max(Vec3::splat(1.0e-3));
        let mut spacing = spacing;
        let counts = loop {
            let counts = (extent / spacing).ceil().as_uvec3() + UVec3::ONE;
            let counts = counts.max(UVec3::splat(2));
            let total = u64::from(counts.x) * u64::from(counts.y) * u64::from(counts.z);
            if total <= u64::from(max_probes.max(8)) {
                break counts;
            }
            spacing *= 1.05;
        };
        let covered = (counts - UVec3::ONE).as_vec3() * spacing;
        let origin = bounds.center() - covered * 0.5;
        let total = counts.x * counts.y * counts.z;
        Some(Self {
            origin,
            spacing,
            counts,
            tiles_per_row: total.min(MAX_ATLAS_WIDTH / VISIBILITY_TILE),
        })
    }

    pub fn probe_count(&self) -> u32 {
        self.counts.x * self.counts.y * self.counts.z
    }

    pub fn rows(&self) -> u32 {
        self.probe_count().div_ceil(self.tiles_per_row)
    }

    /// Width and height of the irradiance atlas in texels.
    pub fn irradiance_size(&self) -> (u32, u32) {
        (self.tiles_per_row * IRRADIANCE_TILE, self.rows() * IRRADIANCE_TILE)
    }

    pub fn visibility_size(&self) -> (u32, u32) {
        (self.tiles_per_row * VISIBILITY_TILE, self.rows() * VISIBILITY_TILE)
    }

    pub fn index(&self, cell: UVec3) -> u32 {
        cell.x + self.counts.x * (cell.y + self.counts.y * cell.z)
    }

    pub fn cell(&self, index: u32) -> UVec3 {
        UVec3::new(
            index % self.counts.x,
            (index / self.counts.x) % self.counts.y,
            index / (self.counts.x * self.counts.y),
        )
    }

    pub fn position(&self, cell: UVec3) -> Vec3 {
        self.origin + cell.as_vec3() * self.spacing
    }

    /// The tile of probe `index` in the atlas: (column, row).
    pub fn tile(&self, index: u32) -> (u32, u32) {
        (index % self.tiles_per_row, index / self.tiles_per_row)
    }

    /// The box the probes span.
    pub fn bounds(&self) -> Aabb {
        Aabb::new(
            self.origin,
            self.origin + (self.counts - UVec3::ONE).as_vec3() * self.spacing,
        )
    }
}

/// The bytes of WGSL `gi::common::GiVolume`, 80 bytes in a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GiVolumeUniforms {
    origin_spacing: [f32; 4],
    counts: [u32; 4],
    atlas: [f32; 4],
    bias: [f32; 4],
    tuning: [f32; 4],
}

impl GiVolumeUniforms {
    pub(crate) fn new(layout: &ProbeLayout, settings: &crate::GiSettings) -> Self {
        let (irradiance_width, irradiance_height) = layout.irradiance_size();
        let (visibility_width, visibility_height) = layout.visibility_size();
        Self {
            origin_spacing: [layout.origin.x, layout.origin.y, layout.origin.z, layout.spacing],
            counts: [layout.counts.x, layout.counts.y, layout.counts.z, layout.tiles_per_row],
            atlas: [
                irradiance_width as f32,
                irradiance_height as f32,
                visibility_width as f32,
                visibility_height as f32,
            ],
            bias: [
                settings.normal_bias,
                settings.view_bias,
                settings.max_ray_distance,
                settings.indirect_intensity,
            ],
            tuning: [settings.specular_occlusion, 1.0, 0.0, 0.0],
        }
    }

    pub fn origin(&self) -> Vec3 {
        Vec3::new(self.origin_spacing[0], self.origin_spacing[1], self.origin_spacing[2])
    }

    pub fn spacing(&self) -> f32 {
        self.origin_spacing[3]
    }

    pub fn counts(&self) -> UVec3 {
        UVec3::new(self.counts[0], self.counts[1], self.counts[2])
    }

    /// Width and height of the irradiance atlas in texels.
    pub fn irradiance_size(&self) -> (u32, u32) {
        (self.atlas[0] as u32, self.atlas[1] as u32)
    }

    pub fn visibility_size(&self) -> (u32, u32) {
        (self.atlas[2] as u32, self.atlas[3] as u32)
    }

    pub fn tiles_per_row(&self) -> u32 {
        self.counts[3]
    }

    pub fn max_distance(&self) -> f32 {
        self.bias[2]
    }

    pub fn specular_occlusion(&self) -> f32 {
        self.tuning[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> Aabb {
        Aabb::new(Vec3::new(-5.0, 0.0, -4.0), Vec3::new(5.0, 3.0, 4.0))
    }

    #[test]
    fn the_probes_cover_the_box_and_centre_on_it() {
        let layout = ProbeLayout::fit(room(), 0.5, 100_000).expect("a finite box");

        assert_eq!(layout.counts, UVec3::new(21, 7, 17));
        let span = layout.bounds();
        assert!(span.min.cmple(room().min).all() && span.max.cmpge(room().max).all());
        assert!((span.center() - room().center()).abs().max_element() < 1.0e-4);
    }

    #[test]
    fn the_spacing_grows_until_the_budget_holds() {
        let layout = ProbeLayout::fit(room(), 0.1, 2000).expect("a finite box");

        assert!(layout.probe_count() <= 2000);
        assert!(layout.spacing > 0.1);
    }

    #[test]
    fn indices_and_cells_are_inverse() {
        let layout = ProbeLayout::fit(room(), 1.0, 4096).expect("a finite box");

        for index in [0, 1, layout.counts.x, layout.probe_count() - 1] {
            assert_eq!(layout.index(layout.cell(index)), index);
        }
    }

    #[test]
    fn the_atlas_holds_every_tile_and_stays_within_the_texture_limit() {
        let layout = ProbeLayout::fit(room(), 0.25, 8000).expect("a finite box");

        let (width, height) = layout.visibility_size();
        assert!(width <= MAX_ATLAS_WIDTH);
        assert!(layout.tiles_per_row * layout.rows() >= layout.probe_count());
        assert_eq!(height, layout.rows() * VISIBILITY_TILE);
    }

    #[test]
    fn an_unbounded_box_has_no_layout() {
        assert_eq!(ProbeLayout::fit(Aabb::UNBOUNDED, 0.5, 100), None);
    }

    #[test]
    fn the_uniform_block_has_the_size_the_shader_expects() {
        assert_eq!(size_of::<GiVolumeUniforms>(), 80);
    }
}
