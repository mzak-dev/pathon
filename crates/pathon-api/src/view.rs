use glam::{Mat4, Vec2, Vec3};
use pathon_rhi_types::PhysicalSize;

use crate::slot::Slot;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewId(pub u64);

// Right-handed, Y up, metres; projection uses reverse-Z (docs/slots.md).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera {
    pub world_to_view: Mat4,
    pub projection: Mat4,
    pub position: Vec3,
}

impl Camera {
    pub fn look_at(position: Vec3, target: Vec3, vertical_fov: f32, aspect_ratio: f32) -> Self {
        Self {
            world_to_view: glam::camera::rh::view::look_at_mat4(position, target, Vec3::Y),
            // wgpu clip space matches the DirectX convention: y up, depth 0..1.
            projection: glam::camera::rh::proj::directx::perspective_infinite_reverse(vertical_fov, aspect_ratio, 0.05),
            position,
        }
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection * self.world_to_view
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, std::f32::consts::FRAC_PI_3, 1.0)
    }
}

// Per-view, per-frame state. The host supplies time (core never reads a clock); core owns `frame` and `record_epoch`.
// The TemporalResolve role sets render size, jitter and mip bias in `prepare_view`, everyone else only reads them.
#[derive(Clone, PartialEq, Debug)]
pub struct ViewCtx {
    pub view: ViewId,
    pub output_size: PhysicalSize,
    pub render_size: PhysicalSize,
    pub jitter: Vec2,
    pub mip_bias: f32,
    // Frames committed so far: the clock `DeferredRelease` runs on. Equal for every view of one frame.
    pub frame: u64,
    // Grows with every record attempt, abandoned ones included: per-record scratch (rings) restarts when it changes.
    pub record_epoch: u64,
    pub time_seconds: f64,
    pub delta_seconds: f32,
    pub camera: Camera,
    pub(crate) requested: Vec<Slot>,
}

impl ViewCtx {
    pub fn new(view: ViewId, output_size: PhysicalSize, camera: Camera) -> Self {
        Self {
            view,
            output_size,
            render_size: output_size,
            jitter: Vec2::ZERO,
            mip_bias: 0.0,
            frame: 0,
            record_epoch: 0,
            time_seconds: 0.0,
            delta_seconds: 0.0,
            camera,
            requested: Vec::new(),
        }
    }

    pub fn is_requested(&self, slot: Slot) -> bool {
        self.requested.contains(&slot)
    }
}
