use pathon_api::glam::{Affine3A, Vec3};

// Axis-aligned box. `UNBOUNDED` is the "unknown" value: culling must never reject it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const UNBOUNDED: Self = Self {
        min: Vec3::NEG_INFINITY,
        max: Vec3::INFINITY,
    };

    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn is_bounded(&self) -> bool {
        self.min.is_finite() && self.max.is_finite()
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extents(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    // Tight box of the transformed box (Arvo's method: only the absolute value of the linear part matters).
    #[must_use]
    pub fn transformed(&self, transform: &Affine3A) -> Self {
        if !self.is_bounded() {
            return Self::UNBOUNDED;
        }
        let center = Vec3::from(transform.transform_point3a(self.center().into()));
        let linear = transform.matrix3;
        let half = self.half_extents();
        let extent =
            Vec3::from(linear.x_axis.abs() * half.x + linear.y_axis.abs() * half.y + linear.z_axis.abs() * half.z);
        Self {
            min: center - extent,
            max: center + extent,
        }
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Self::UNBOUNDED
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::Quat;

    use super::*;

    #[test]
    fn translation_moves_the_box() {
        let unit = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));

        let moved = unit.transformed(&Affine3A::from_translation(Vec3::new(10.0, 0.0, 0.0)));

        assert_eq!(moved, Aabb::new(Vec3::new(9.0, -1.0, -1.0), Vec3::new(11.0, 1.0, 1.0)));
    }

    #[test]
    fn a_quarter_turn_swaps_the_extents() {
        let flat = Aabb::new(Vec3::new(-2.0, -1.0, -1.0), Vec3::new(2.0, 1.0, 1.0));

        let turned = flat.transformed(&Affine3A::from_quat(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)));

        assert!((turned.half_extents() - Vec3::new(1.0, 2.0, 1.0)).abs().max_element() < 1e-5);
    }

    #[test]
    fn unbounded_stays_unbounded() {
        let moved = Aabb::UNBOUNDED.transformed(&Affine3A::from_scale(Vec3::splat(2.0)));

        assert!(!moved.is_bounded());
    }
}
