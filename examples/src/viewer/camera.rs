// The free camera of the viewer and the conversion of the file's cameras to `pathon_api::Camera`.

use pathon::api::Camera;
use pathon::api::glam::Vec3;
use pathon_scene::{CameraData, Projection};

const MAX_PITCH: f32 = 1.55;
const LOOK_RADIANS_PER_PIXEL: f32 = 0.0025;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct MoveInput {
    pub forward: f32,
    pub right: f32,
    pub up: f32,
    pub fast: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FlyCamera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub vertical_fov: f32,
    /// Metres per second.
    pub speed: f32,
}

impl FlyCamera {
    pub fn new(position: Vec3, forward: Vec3, vertical_fov: f32, speed: f32) -> Self {
        let forward = forward.normalize_or(Vec3::NEG_Z);
        Self {
            position,
            yaw: forward.x.atan2(-forward.z),
            pitch: forward.y.clamp(-1.0, 1.0).asin(),
            vertical_fov,
            speed,
        }
    }

    /// Takes the pose and the field of view of one of the file's cameras; the speed stays.
    pub fn jump_to(&mut self, camera: &CameraData) {
        let forward = camera.transform.matrix3 * Vec3::NEG_Z;
        let vertical_fov = match camera.projection {
            Projection::Perspective { vertical_fov, .. } => vertical_fov,
            _ => self.vertical_fov,
        };
        *self = Self::new(
            Vec3::from(camera.transform.translation),
            forward,
            vertical_fov,
            self.speed,
        );
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        )
    }

    pub fn look(&mut self, delta_x_pixels: f32, delta_y_pixels: f32) {
        self.yaw += delta_x_pixels * LOOK_RADIANS_PER_PIXEL;
        self.pitch = (self.pitch - delta_y_pixels * LOOK_RADIANS_PER_PIXEL).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn walk(&mut self, input: MoveInput, delta_seconds: f32) {
        let forward = self.forward();
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let direction = forward * input.forward + right * input.right + Vec3::Y * input.up;
        let speed = if input.fast { self.speed * 4.0 } else { self.speed };
        self.position += direction.normalize_or_zero() * speed * delta_seconds;
    }

    pub fn camera(&self, aspect_ratio: f32) -> Camera {
        Camera::look_at(
            self.position,
            self.position + self.forward(),
            self.vertical_fov,
            aspect_ratio,
        )
    }
}

/// Where the free camera starts when the file has no camera: outside the bounds, looking at their centre.
pub fn overview(bounds: (Vec3, Vec3), vertical_fov: f32) -> FlyCamera {
    let (min, max) = bounds;
    let centre = (min + max) * 0.5;
    let radius = ((max - min).length() * 0.5).max(1.0);
    let distance = radius / (vertical_fov * 0.5).sin();
    let position = centre + Vec3::new(0.0, 0.3, 1.0).normalize() * distance;
    FlyCamera::new(position, centre - position, vertical_fov, radius * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forward_vector_survives_the_yaw_pitch_round_trip() {
        let wanted = Vec3::new(0.3, -0.2, -0.9).normalize();
        let camera = FlyCamera::new(Vec3::ZERO, wanted, 1.0, 1.0);
        assert!((camera.forward() - wanted).length() < 1e-5);
    }

    #[test]
    fn walking_forward_moves_along_the_view_direction() {
        let mut camera = FlyCamera::new(Vec3::ZERO, Vec3::NEG_Z, 1.0, 2.0);
        camera.walk(
            MoveInput {
                forward: 1.0,
                ..MoveInput::default()
            },
            0.5,
        );
        assert!((camera.position - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5);
    }

    #[test]
    fn right_is_to_the_right_of_the_view() {
        let mut camera = FlyCamera::new(Vec3::ZERO, Vec3::NEG_Z, 1.0, 1.0);
        camera.walk(
            MoveInput {
                right: 1.0,
                ..MoveInput::default()
            },
            1.0,
        );
        assert!(camera.position.x > 0.9);
    }
}
