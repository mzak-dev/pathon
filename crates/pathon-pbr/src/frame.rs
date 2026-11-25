use pathon_api::glam::{Mat4, Vec2, Vec3};
use pathon_api::rhi::PhysicalSize;
use pathon_api::{Camera, ViewCtx};

use crate::cluster::ClusterGrid;
use crate::settings::LightAssignment;

/// `pbr::common::Frame`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameUniforms {
    pub view_projection: [[f32; 4]; 4],
    pub unjittered_view_projection: [[f32; 4]; 4],
    pub previous_view_projection: [[f32; 4]; 4],
    pub world_to_view: [[f32; 4]; 4],
    pub camera_position: [f32; 4],
    pub render_size: [f32; 2],
    pub environment_intensity: f32,
    pub padding: f32,
    pub cluster_dims: [u32; 4],
    pub cluster_depth: [f32; 4],
    pub light_counts: [u32; 4],
}

/// The matrices of one view: the camera of the host with this frame's jitter applied, plus the unjittered ones that
/// motion vectors need.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewMatrices {
    pub world_to_view: Mat4,
    /// Jittered: what rasterisation and the cluster grid use.
    pub projection: Mat4,
    pub unjittered_projection: Mat4,
    pub position: Vec3,
}

impl ViewMatrices {
    pub fn new(camera: &Camera, jitter: Vec2, render_size: PhysicalSize) -> Self {
        let width = render_size.width.max(1) as f32;
        let height = render_size.height.max(1) as f32;
        // The jitter is a sub-pixel shift in render-target pixels, y down: in clip space it is the same translation,
        // scaled by w, for perspective and orthographic projections.
        let shift = Vec2::new(2.0 * jitter.x / width, -2.0 * jitter.y / height);
        Self {
            world_to_view: camera.world_to_view,
            projection: Mat4::from_translation(shift.extend(0.0)) * camera.projection,
            unjittered_projection: camera.projection,
            position: camera.position,
        }
    }

    pub fn from_view(view: &ViewCtx) -> Self {
        Self::new(&view.camera, view.jitter, view.render_size)
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection * self.world_to_view
    }

    pub fn unjittered_view_projection(&self) -> Mat4 {
        self.unjittered_projection * self.world_to_view
    }
}

#[derive(Debug)]
pub struct FrameInputs<'inputs> {
    pub matrices: ViewMatrices,
    pub previous_view_projection: Mat4,
    pub render_size: PhysicalSize,
    pub grid: &'inputs ClusterGrid,
    pub directional_lights: u32,
    pub total_lights: u32,
    pub assignment: LightAssignment,
    pub environment_intensity: f32,
}

impl FrameUniforms {
    pub fn new(inputs: &FrameInputs<'_>) -> Self {
        let matrices = &inputs.matrices;
        let position = matrices.position;
        let grid = inputs.grid;
        Self {
            view_projection: matrices.view_projection().to_cols_array_2d(),
            unjittered_view_projection: matrices.unjittered_view_projection().to_cols_array_2d(),
            previous_view_projection: inputs.previous_view_projection.to_cols_array_2d(),
            world_to_view: matrices.world_to_view.to_cols_array_2d(),
            camera_position: [position.x, position.y, position.z, 1.0],
            render_size: [inputs.render_size.width as f32, inputs.render_size.height as f32],
            environment_intensity: inputs.environment_intensity,
            padding: 0.0,
            cluster_dims: [grid.tiles_x, grid.tiles_y, grid.slices, grid.tile_size],
            cluster_depth: [grid.near, grid.far, grid.slices_per_ln(), 0.0],
            light_counts: [
                inputs.directional_lights,
                inputs.total_lights,
                0,
                u32::from(inputs.assignment == LightAssignment::BruteForce),
            ],
        }
    }
}

/// The previous frame's unjittered view-projection per view: the motion vectors of the camera. A frame that is
/// recorded again (the same `ViewCtx::frame`) keeps the matrix of the frame before it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewHistory {
    frame: u64,
    current: Mat4,
    previous: Mat4,
}

impl ViewHistory {
    pub(crate) fn first(frame: u64, current: Mat4) -> Self {
        Self {
            frame,
            current,
            previous: current,
        }
    }

    /// Records this frame's matrix and returns the one of the previous committed frame.
    pub(crate) fn advance(&mut self, frame: u64, current: Mat4) -> Mat4 {
        if frame != self.frame {
            self.previous = self.current;
            self.frame = frame;
        }
        self.current = current;
        self.previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_moves_the_projection_but_not_the_unjittered_one() {
        let camera = Camera::default();
        let size = PhysicalSize::new(100, 50);

        let still = ViewMatrices::new(&camera, Vec2::ZERO, size);
        let jittered = ViewMatrices::new(&camera, Vec2::new(0.5, 0.25), size);

        assert_eq!(still.projection, jittered.unjittered_projection);
        assert_eq!(jittered.unjittered_projection, still.unjittered_projection);
        let point = Vec3::ZERO.extend(1.0);
        let (a, b) = (still.view_projection() * point, jittered.view_projection() * point);
        assert!((b.x / b.w - a.x / a.w - 2.0 * 0.5 / 100.0).abs() < 1e-6);
        assert!((b.y / b.w - a.y / a.w + 2.0 * 0.25 / 50.0).abs() < 1e-6);
    }

    #[test]
    fn history_keeps_the_previous_frames_matrix_across_re_records() {
        let (a, b, c) = (
            Mat4::from_scale(Vec3::ONE),
            Mat4::from_scale(Vec3::splat(2.0)),
            Mat4::from_scale(Vec3::splat(3.0)),
        );
        let mut history = ViewHistory::first(0, a);

        assert_eq!(history.advance(0, a), a);
        assert_eq!(history.advance(1, b), a);
        assert_eq!(history.advance(1, b), a, "the same frame recorded again");
        assert_eq!(history.advance(2, c), b);
    }
}
