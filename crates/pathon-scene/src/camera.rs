use pathon_api::glam::{Affine3A, Mat4, Vec2, Vec3, Vec4};
use pathon_api::{Camera, ViewCtx};

use crate::error::SceneError;
use crate::layer::LayerMask;

// Right-handed, Y up, camera looks down -Z, reverse-Z depth in 0..1 (docs/slots.md): the near plane maps to 1, the
// far plane (or infinity) to 0. The aspect ratio comes from the view that renders the camera, not from the camera.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub enum Projection {
    Perspective {
        vertical_fov: f32,
        near: f32,
        // `None` is an infinite far plane.
        far: Option<f32>,
    },
    Orthographic {
        // Visible height in metres; the width follows from the aspect ratio.
        height: f32,
        near: f32,
        far: f32,
    },
}

impl Projection {
    pub fn perspective(vertical_fov: f32, near: f32) -> Self {
        Self::Perspective {
            vertical_fov,
            near,
            far: None,
        }
    }

    pub fn orthographic(height: f32, near: f32, far: f32) -> Self {
        Self::Orthographic { height, near, far }
    }

    pub fn validate(&self) -> Result<(), SceneError> {
        let reason = match *self {
            Self::Perspective {
                vertical_fov,
                near,
                far,
            } => {
                if !(vertical_fov.is_finite() && vertical_fov > 0.0 && vertical_fov < std::f32::consts::PI) {
                    Some("vertical fov must be between 0 and pi")
                } else if !(near.is_finite() && near > 0.0) {
                    Some("near plane must be positive")
                } else if far.is_some_and(|far| !(far.is_finite() && far > near)) {
                    Some("far plane must be finite and beyond the near plane")
                } else {
                    None
                }
            }
            Self::Orthographic { height, near, far } => {
                if !(height.is_finite() && height > 0.0) {
                    Some("orthographic height must be positive")
                } else if !(near.is_finite() && far.is_finite() && far > near) {
                    Some("far plane must be finite and beyond the near plane")
                } else {
                    None
                }
            }
        };
        reason.map_or(Ok(()), |reason| Err(SceneError::InvalidCamera { reason }))
    }

    // View space to clip space, unjittered. `aspect_ratio` is width / height of the render target.
    pub fn matrix(&self, aspect_ratio: f32) -> Mat4 {
        match *self {
            Self::Perspective {
                vertical_fov,
                near,
                far,
            } => {
                let focal = 1.0 / (vertical_fov * 0.5).tan();
                // clip.z = a * view.z + b, clip.w = -view.z; near -> 1, far -> 0 (b = near for an infinite far plane).
                let (a, b) = match far {
                    Some(far) => (near / (far - near), near * far / (far - near)),
                    None => (0.0, near),
                };
                Mat4::from_cols(
                    Vec4::new(focal / aspect_ratio, 0.0, 0.0, 0.0),
                    Vec4::new(0.0, focal, 0.0, 0.0),
                    Vec4::new(0.0, 0.0, a, -1.0),
                    Vec4::new(0.0, 0.0, b, 0.0),
                )
            }
            Self::Orthographic { height, near, far } => {
                let range = far - near;
                Mat4::from_cols(
                    Vec4::new(2.0 / (height * aspect_ratio), 0.0, 0.0, 0.0),
                    Vec4::new(0.0, 2.0 / height, 0.0, 0.0),
                    Vec4::new(0.0, 0.0, 1.0 / range, 0.0),
                    Vec4::new(0.0, 0.0, far / range, 1.0),
                )
            }
        }
    }
}

// What the host writes for a camera: where it stands and how it projects.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct CameraDesc {
    // Camera to world.
    pub transform: Affine3A,
    pub projection: Projection,
    // Instances and lights on these layers are visible to the camera.
    pub layers: LayerMask,
}

impl CameraDesc {
    pub fn new(transform: Affine3A, projection: Projection) -> Self {
        Self {
            transform,
            projection,
            layers: LayerMask::ALL,
        }
    }

    pub fn look_at(position: Vec3, target: Vec3, projection: Projection) -> Self {
        let world_to_view = pathon_api::glam::camera::rh::view::look_at_mat4(position, target, Vec3::Y);
        Self::new(Affine3A::from_mat4(world_to_view.inverse()), projection)
    }

    #[must_use]
    pub fn with_layers(self, layers: LayerMask) -> Self {
        Self { layers, ..self }
    }

    pub(crate) fn validate(&self) -> Result<(), SceneError> {
        if !self.transform.is_finite() {
            return Err(SceneError::NotFinite {
                what: "camera transform",
            });
        }
        self.projection.validate()
    }
}

// A camera as the render world stores it: the description plus last extract's pose and projection, for the
// previous view-projection of motion vectors and temporal reprojection.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct CameraData {
    pub transform: Affine3A,
    pub previous_transform: Affine3A,
    pub projection: Projection,
    pub previous_projection: Projection,
    pub layers: LayerMask,
}

impl CameraData {
    pub(crate) fn new(desc: &CameraDesc) -> Self {
        Self {
            transform: desc.transform,
            previous_transform: desc.transform,
            projection: desc.projection,
            previous_projection: desc.projection,
            layers: desc.layers,
        }
    }

    // Overwrites the description and keeps the previous pose. Returns whether anything changed.
    pub(crate) fn apply(&mut self, desc: &CameraDesc) -> bool {
        let changed =
            self.transform != desc.transform || self.projection != desc.projection || self.layers != desc.layers;
        self.transform = desc.transform;
        self.projection = desc.projection;
        self.layers = desc.layers;
        changed
    }

    // Start of an extract: last frame's pose becomes the previous one. Returns whether anything changed.
    pub(crate) fn settle(&mut self) -> bool {
        let changed = self.previous_transform != self.transform || self.previous_projection != self.projection;
        self.previous_transform = self.transform;
        self.previous_projection = self.projection;
        changed
    }

    pub fn position(&self) -> Vec3 {
        Vec3::from(self.transform.translation)
    }

    pub fn world_to_view(&self) -> Mat4 {
        Mat4::from(self.transform.inverse())
    }

    pub fn previous_world_to_view(&self) -> Mat4 {
        Mat4::from(self.previous_transform.inverse())
    }

    pub fn view_projection(&self, aspect_ratio: f32) -> Mat4 {
        self.projection.matrix(aspect_ratio) * self.world_to_view()
    }

    // Unjittered, like motion vectors (docs/slots.md).
    pub fn previous_view_projection(&self, aspect_ratio: f32) -> Mat4 {
        self.previous_projection.matrix(aspect_ratio) * self.previous_world_to_view()
    }

    // The camera for rasterising `view`: aspect from the render size, projection shifted by the view's jitter.
    pub fn view_camera(&self, view: &ViewCtx) -> Camera {
        let aspect_ratio = aspect_ratio(view);
        Camera {
            world_to_view: self.world_to_view(),
            projection: jittered(self.projection.matrix(aspect_ratio), view),
            position: self.position(),
        }
    }
}

fn aspect_ratio(view: &ViewCtx) -> f32 {
    view.render_size.width as f32 / view.render_size.height.max(1) as f32
}

// `ViewCtx::jitter` is a sub-pixel offset in render-target pixels, y down. It becomes a clip-space shift that scales
// with `w`, so it is the same translation for perspective and orthographic projections.
fn jittered(projection: Mat4, view: &ViewCtx) -> Mat4 {
    let width = view.render_size.width.max(1) as f32;
    let height = view.render_size.height.max(1) as f32;
    let shift = Vec2::new(2.0 * view.jitter.x / width, -2.0 * view.jitter.y / height);
    Mat4::from_translation(shift.extend(0.0)) * projection
}

#[cfg(test)]
mod tests {
    use pathon_api::rhi::PhysicalSize;
    use pathon_api::{ViewId, glam};

    use super::*;

    fn ndc(matrix: Mat4, point: Vec3) -> Vec3 {
        let clip = matrix * point.extend(1.0);
        clip.truncate() / clip.w
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }

    #[test]
    fn infinite_reverse_z_maps_near_to_one_and_distance_to_zero() {
        let matrix = Projection::perspective(std::f32::consts::FRAC_PI_2, 0.1).matrix(1.0);

        assert_near(ndc(matrix, Vec3::new(0.0, 0.0, -0.1)).z, 1.0);
        assert_near(ndc(matrix, Vec3::new(0.0, 0.0, -1.0)).z, 0.1);
        assert!(ndc(matrix, Vec3::new(0.0, 0.0, -1.0e6)).z < 1.0e-6);
    }

    #[test]
    fn finite_reverse_z_maps_near_to_one_and_far_to_zero() {
        let projection = Projection::Perspective {
            vertical_fov: std::f32::consts::FRAC_PI_2,
            near: 0.5,
            far: Some(100.0),
        };
        let matrix = projection.matrix(1.0);

        assert_near(ndc(matrix, Vec3::new(0.0, 0.0, -0.5)).z, 1.0);
        assert_near(ndc(matrix, Vec3::new(0.0, 0.0, -100.0)).z, 0.0);
        assert!(ndc(matrix, Vec3::new(0.0, 0.0, -10.0)).z > ndc(matrix, Vec3::new(0.0, 0.0, -20.0)).z);
    }

    #[test]
    fn perspective_agrees_with_the_core_camera() {
        let desc = CameraDesc::look_at(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 0.05),
        );
        let data = CameraData::new(&desc);
        let core = Camera::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, std::f32::consts::FRAC_PI_3, 1.5);

        let ours = data.view_projection(1.5);
        let theirs = core.view_projection();

        assert!(ours.abs_diff_eq(theirs, 1e-5), "{ours:?} vs {theirs:?}");
    }

    #[test]
    fn fov_and_aspect_place_the_frustum_edges_at_ndc_one() {
        let matrix = Projection::perspective(std::f32::consts::FRAC_PI_2, 0.1).matrix(2.0);

        let top = ndc(matrix, Vec3::new(0.0, 5.0, -5.0));
        let right = ndc(matrix, Vec3::new(10.0, 0.0, -5.0));

        assert_near(top.y, 1.0);
        assert_near(right.x, 1.0);
    }

    #[test]
    fn orthographic_reverse_z_maps_near_to_one_and_far_to_zero() {
        let matrix = Projection::orthographic(10.0, 1.0, 11.0).matrix(2.0);

        assert_near(ndc(matrix, Vec3::new(0.0, 0.0, -1.0)).z, 1.0);
        assert_near(ndc(matrix, Vec3::new(0.0, 0.0, -11.0)).z, 0.0);
        assert_near(ndc(matrix, Vec3::new(0.0, 0.0, -6.0)).z, 0.5);
        assert_near(ndc(matrix, Vec3::new(10.0, 5.0, -6.0)).x, 1.0);
        assert_near(ndc(matrix, Vec3::new(10.0, 5.0, -6.0)).y, 1.0);
    }

    #[test]
    fn look_at_puts_the_target_on_the_view_axis() {
        let desc = CameraDesc::look_at(
            Vec3::new(3.0, 2.0, 5.0),
            Vec3::new(1.0, 0.0, 0.0),
            Projection::perspective(1.0, 0.1),
        );
        let data = CameraData::new(&desc);

        let target_in_view = data.world_to_view().transform_point3(Vec3::new(1.0, 0.0, 0.0));

        assert_near(target_in_view.x, 0.0);
        assert_near(target_in_view.y, 0.0);
        assert!(target_in_view.z < 0.0);
        assert!((data.position() - Vec3::new(3.0, 2.0, 5.0)).length() < 1e-5);
    }

    #[test]
    fn jitter_shifts_the_image_by_the_requested_pixels() {
        let desc = CameraDesc::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Projection::perspective(1.0, 0.1));
        let data = CameraData::new(&desc);
        let mut view = ViewCtx::new(ViewId(0), PhysicalSize::new(200, 100), Camera::default());
        let plain = data.view_camera(&view);
        view.jitter = glam::Vec2::new(0.5, -0.25);
        let shifted = data.view_camera(&view);

        let point = Vec3::new(0.3, 0.2, 0.0);
        let before = ndc(plain.view_projection(), point);
        let after = ndc(shifted.view_projection(), point);

        // Half a pixel of 200 is 0.5 * 2 / 200 in NDC; a quarter pixel up (negative y jitter) of 100 is +0.005.
        assert_near(after.x - before.x, 0.005);
        assert_near(after.y - before.y, 0.005);
        assert_near(after.z, before.z);
    }

    #[test]
    fn jitter_is_the_same_shift_for_an_orthographic_camera() {
        let desc = CameraDesc::new(Affine3A::IDENTITY, Projection::orthographic(4.0, 0.1, 10.0));
        let data = CameraData::new(&desc);
        let mut view = ViewCtx::new(ViewId(0), PhysicalSize::new(100, 100), Camera::default());
        view.jitter = glam::Vec2::new(1.0, 0.0);

        let shifted = ndc(data.view_camera(&view).view_projection(), Vec3::new(0.0, 0.0, -5.0));

        assert_near(shifted.x, 0.02);
    }

    #[test]
    fn the_previous_view_projection_lags_one_settle_behind() {
        let mut data = CameraData::new(&CameraDesc::new(Affine3A::IDENTITY, Projection::perspective(1.0, 0.1)));
        let moved = CameraDesc::new(Affine3A::from_translation(Vec3::X), Projection::perspective(1.0, 0.1));

        assert!(data.apply(&moved));
        assert_eq!(data.previous_transform, Affine3A::IDENTITY);
        assert!(data.settle());
        assert_eq!(data.previous_transform, moved.transform);
        assert!(!data.settle());
    }

    #[test]
    fn invalid_projections_are_rejected() {
        let bad = [
            Projection::perspective(0.0, 0.1),
            Projection::perspective(std::f32::consts::PI, 0.1),
            Projection::perspective(1.0, 0.0),
            Projection::perspective(1.0, f32::NAN),
            Projection::Perspective {
                vertical_fov: 1.0,
                near: 1.0,
                far: Some(0.5),
            },
            Projection::orthographic(0.0, 0.1, 1.0),
            Projection::orthographic(1.0, 2.0, 1.0),
        ];

        for projection in bad {
            assert!(
                matches!(projection.validate(), Err(SceneError::InvalidCamera { .. })),
                "{projection:?}"
            );
        }
        assert!(Projection::perspective(1.0, 0.1).validate().is_ok());
    }
}
