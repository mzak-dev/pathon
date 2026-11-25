use pathon_api::glam::Mat4;
use pathon_api::rhi::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, CommandList, ComputePassDesc, ComputePipelineId, DeviceApi, PhysicalSize, RhiError,
    ShaderStages, StorageAccess,
};
use pathon_shader::ShaderError;

use crate::settings::ClusterSettings;
use crate::shaders::PbrShaders;

/// Where the last depth slice ends in the cluster bounds (metres): it must reach further than any light.
pub const LAST_SLICE_END: f32 = 1.0e5;
const WORKGROUP_SIZE: u32 = 64;

/// The cluster grid of a view: `tiles_x` by `tiles_y` pixel tiles by `slices` exponential depth slices. Cluster
/// `(x, y, z)` has index `(z * tiles_y + y) * tiles_x + x`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ClusterGrid {
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub slices: u32,
    pub tile_size: u32,
    pub near: f32,
    pub far: f32,
}

impl ClusterGrid {
    pub fn new(render_size: PhysicalSize, settings: &ClusterSettings) -> Self {
        Self {
            tiles_x: render_size.width.max(1).div_ceil(settings.tile_size),
            tiles_y: render_size.height.max(1).div_ceil(settings.tile_size),
            slices: settings.slices,
            tile_size: settings.tile_size,
            near: settings.near,
            far: settings.far,
        }
    }

    pub fn count(&self) -> u32 {
        self.tiles_x * self.tiles_y * self.slices
    }

    /// `slices / ln(far / near)`: what the shader multiplies `ln(depth / near)` by.
    pub fn slices_per_ln(&self) -> f32 {
        self.slices as f32 / (self.far / self.near).ln()
    }

    /// The depth slice of a view-space distance in front of the camera.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..slices before the cast"
    )]
    pub fn slice_of(&self, view_depth: f32) -> u32 {
        if view_depth <= self.near {
            return 0;
        }
        let slice = (view_depth / self.near).ln() * self.slices_per_ln();
        slice.clamp(0.0, (self.slices - 1) as f32) as u32
    }

    pub fn index(&self, x: u32, y: u32, slice: u32) -> u32 {
        (slice * self.tiles_y + y) * self.tiles_x + x
    }

    /// The cluster of the pixel at `(x, y)` (pixel coordinates, y down) at `view_depth`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "pixel coordinates inside the target are not negative and fit u32"
    )]
    pub fn index_of_pixel(&self, x: f32, y: f32, view_depth: f32) -> u32 {
        let tile = |coordinate: f32, tiles: u32| ((coordinate as u32) / self.tile_size).min(tiles - 1);
        self.index(tile(x, self.tiles_x), tile(y, self.tiles_y), self.slice_of(view_depth))
    }
}

/// `ClusterParams` of `cluster.wesl`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ClusterParams {
    pub inverse_projection: [[f32; 4]; 4],
    pub world_to_view: [[f32; 4]; 4],
    pub render_size: [f32; 2],
    pub padding: [f32; 2],
    pub dims: [u32; 4],
    pub depth: [f32; 4],
    pub lights: [u32; 4],
}

impl ClusterParams {
    /// `first_light` and `light_count` select the clustered lights of the light array; `projection` is the one the
    /// view is rasterised with (jitter included).
    pub fn new(
        grid: &ClusterGrid,
        projection: Mat4,
        world_to_view: Mat4,
        render_size: PhysicalSize,
        first_light: u32,
        light_count: u32,
        max_light_indices: u32,
    ) -> Self {
        Self {
            inverse_projection: projection.inverse().to_cols_array_2d(),
            world_to_view: world_to_view.to_cols_array_2d(),
            render_size: [render_size.width as f32, render_size.height as f32],
            padding: [0.0; 2],
            dims: [grid.tiles_x, grid.tiles_y, grid.slices, grid.tile_size],
            depth: [grid.near, grid.far, grid.slices_per_ln(), LAST_SLICE_END],
            lights: [first_light, light_count, max_light_indices, 0],
        }
    }
}

/// The buffers one assignment reads and writes.
#[derive(Clone, PartialEq, Debug)]
pub struct ClusterBuffers {
    pub params: BindingResource,
    pub lights: BindingResource,
    /// `grid.count()` pairs of `u32` (offset, count).
    pub ranges: BindingResource,
    /// `max_light_indices` words.
    pub indices: BindingResource,
    /// Two words: entries reserved, entries that did not fit.
    pub counters: BindingResource,
}

pub const COUNTERS_SIZE: u64 = 8;
pub const RANGE_SIZE: u64 = 8;

/// The compute pipelines that assign point and spot lights to clusters (`cluster.wesl`).
#[derive(Clone, Copy, Debug)]
pub struct ClusterPipeline {
    layout: BindGroupLayoutId,
    reset: ComputePipelineId,
    assign: ComputePipelineId,
}

impl ClusterPipeline {
    pub fn new(device: &dyn DeviceApi, shaders: &mut PbrShaders) -> Result<Self, ClusterError> {
        let entry = |binding: u32, ty: BindingType| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let storage = |access| BindingType::StorageBuffer { access };
        let layout = shaders.group_layout(
            device,
            &BindGroupLayoutDesc {
                label: "pbr clusters",
                entries: vec![
                    entry(0, BindingType::UniformBuffer),
                    entry(1, storage(StorageAccess::ReadOnly)),
                    entry(2, storage(StorageAccess::ReadWrite)),
                    entry(3, storage(StorageAccess::ReadWrite)),
                    entry(4, storage(StorageAccess::ReadWrite)),
                ],
            },
        )?;
        Ok(Self {
            layout,
            reset: shaders.compute_pipeline_with_layout(device, "cluster", "reset", "pbr cluster reset", layout)?,
            assign: shaders.compute_pipeline_with_layout(device, "cluster", "assign", "pbr cluster assign", layout)?,
        })
    }

    pub fn bind_group_desc(&self, buffers: &ClusterBuffers) -> BindGroupDesc {
        let entries = [
            &buffers.params,
            &buffers.lights,
            &buffers.ranges,
            &buffers.indices,
            &buffers.counters,
        ]
        .into_iter()
        .zip(0..)
        .map(|(resource, binding)| BindGroupEntry {
            binding,
            resource: resource.clone(),
        })
        .collect();
        BindGroupDesc {
            label: "pbr clusters",
            layout: self.layout,
            entries,
        }
    }

    /// One compute pass: clear the counters, then one thread per cluster.
    pub fn record(&self, commands: &mut CommandList, group: BindGroupId, grid: &ClusterGrid) {
        let mut pass = commands.compute_pass(ComputePassDesc {
            label: "pbr cluster lights",
            timestamps: None,
        });
        pass.set_bind_group(0, group);
        pass.set_pipeline(self.reset);
        pass.dispatch(1, 1, 1);
        pass.set_pipeline(self.assign);
        pass.dispatch(grid.count().div_ceil(WORKGROUP_SIZE), 1, 1);
    }
}

/// Why the cluster pipelines could not be made.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ClusterError {
    Shader(String),
    Rhi(RhiError),
}

impl From<ShaderError> for ClusterError {
    fn from(error: ShaderError) -> Self {
        Self::Shader(error.to_string())
    }
}

impl From<RhiError> for ClusterError {
    fn from(error: RhiError) -> Self {
        Self::Rhi(error)
    }
}

impl std::fmt::Display for ClusterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shader(message) => write!(f, "cluster shader: {message}"),
            Self::Rhi(error) => write!(f, "cluster pipeline: {error}"),
        }
    }
}

impl std::error::Error for ClusterError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> ClusterGrid {
        ClusterGrid::new(PhysicalSize::new(100, 50), &ClusterSettings::default())
    }

    #[test]
    fn the_grid_covers_the_render_size_with_whole_tiles() {
        let grid = grid();

        assert_eq!((grid.tiles_x, grid.tiles_y), (7, 4));
        assert_eq!(grid.count(), 7 * 4 * 24);
        assert_eq!(grid.index(6, 3, 23), grid.count() - 1);
    }

    #[test]
    fn depth_slices_are_exponential_and_clamped_at_both_ends() {
        let grid = grid();

        assert_eq!(grid.slice_of(0.0), 0);
        assert_eq!(grid.slice_of(0.05), 0);
        assert_eq!(grid.slice_of(grid.near * (grid.far / grid.near).powf(0.5 * 1.001)), 12);
        assert_eq!(grid.slice_of(grid.far), 23);
        assert_eq!(grid.slice_of(1.0e9), 23);
        let starts: Vec<u32> = (1..24)
            .map(|slice| grid.slice_of(grid.near * (grid.far / grid.near).powf((slice as f32 + 0.01) / 24.0)))
            .collect();
        assert_eq!(starts, (1..24).collect::<Vec<_>>());
    }
}
