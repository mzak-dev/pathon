//! CPU model of the shading of rectangular area lights (ADR-0048): the analytic integrals the shader evaluates and the
//! numeric ones the tests compare them with. Everything is per unit luminance (1 nit), in `f64`.
//!
//! The geometry is Arvo's (1995, "Applications of Irradiance Tensors to the Simulation of Non-Lambertian Phenomena"):
//! the cosine-weighted solid angle of a spherical polygon is `1/2 sum theta_i (gamma_i . n)` over its edges, after
//! clipping it to the horizon of `n`.

use std::f64::consts::PI;

use pathon_api::glam::{DVec3, Vec3};
use pathon_scene::{Light, LightKind};

use crate::ltc;

/// A rectangle that emits as a Lambertian surface towards `normal` (and the opposite side when `two_sided`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RectLight {
    pub center: DVec3,
    pub half_width: DVec3,
    pub half_height: DVec3,
    pub normal: DVec3,
    pub two_sided: bool,
}

impl RectLight {
    /// `None` for a light that is not an area light.
    pub fn from_light(light: &Light) -> Option<Self> {
        let LightKind::Area {
            width,
            height,
            two_sided,
        } = light.kind
        else {
            return None;
        };
        let widen = |vector: Vec3| DVec3::new(f64::from(vector.x), f64::from(vector.y), f64::from(vector.z));
        let matrix = light.transform.matrix3;
        Some(Self {
            center: widen(light.position()),
            half_width: widen(Vec3::from(matrix.x_axis).normalize()) * f64::from(width) * 0.5,
            half_height: widen(Vec3::from(matrix.y_axis).normalize()) * f64::from(height) * 0.5,
            normal: widen(light.direction()),
            two_sided,
        })
    }

    pub fn area(&self) -> f64 {
        4.0 * self.half_width.cross(self.half_height).length()
    }

    pub fn corners(&self) -> [DVec3; 4] {
        let (u, v) = (self.half_width, self.half_height);
        [-u - v, u - v, u + v, -u + v].map(|offset| self.center + offset)
    }

    /// Whether `point` is on a side the light emits to.
    pub fn lights(&self, point: DVec3) -> bool {
        self.two_sided || (point - self.center).dot(self.normal) > 0.0
    }
}

/// `integral over the polygon clipped to the horizon of axis of (omega . axis) d omega`: the corners are relative to
/// the shaded point and need not be normalised.
pub fn cosine_integral(corners: &[DVec3], axis: DVec3) -> f64 {
    let mut clipped: Vec<DVec3> = Vec::with_capacity(corners.len() + 1);
    for (index, current) in corners.iter().enumerate() {
        let next = corners[(index + 1) % corners.len()];
        let (height_current, height_next) = (current.dot(axis), next.dot(axis));
        if height_current >= 0.0 {
            clipped.push(*current);
        }
        if (height_current >= 0.0) != (height_next >= 0.0) {
            let t = height_current / (height_current - height_next);
            clipped.push(*current + (next - *current) * t);
        }
    }
    if clipped.len() < 3 {
        return 0.0;
    }
    let directions: Vec<DVec3> = clipped.iter().map(|point| point.normalize()).collect();
    let mut sum = DVec3::ZERO;
    for (index, a) in directions.iter().enumerate() {
        let b = directions[(index + 1) % directions.len()];
        let cross = a.cross(b);
        let length = cross.length();
        if length > 1.0e-300 {
            sum += cross * (length.atan2(a.dot(b)) / length);
        }
    }
    (0.5 * sum.dot(axis)).abs()
}

/// Solid angle weighted by the cosine to `normal`, of the part of the light the point at `point` sees: the diffuse
/// irradiance per unit luminance, over pi (the Lambert BRDF is albedo / pi).
pub fn diffuse_cosine_integral(light: &RectLight, point: DVec3, normal: DVec3) -> f64 {
    if !light.lights(point) {
        return 0.0;
    }
    let corners = light.corners().map(|corner| corner - point);
    cosine_integral(&corners, normal)
}

/// The same by quadrature: `n x n` midpoints of the rectangle.
pub fn diffuse_cosine_integral_numeric(light: &RectLight, point: DVec3, normal: DVec3, n: usize) -> f64 {
    if !light.lights(point) {
        return 0.0;
    }
    let mut sum = 0.0;
    for_each_sample(light, n, |position, weight| {
        let to_light = position - point;
        let distance_square = to_light.length_squared();
        let direction = to_light / distance_square.sqrt();
        let emitted = (-direction).dot(light.normal).abs();
        sum += normal.dot(direction).max(0.0) * emitted * weight / distance_square;
    });
    sum
}

fn for_each_sample(light: &RectLight, n: usize, mut visit: impl FnMut(DVec3, f64)) {
    let weight = light.area() / (n * n) as f64;
    for i in 0..n {
        for j in 0..n {
            let (s, t) = (
                2.0 * (i as f64 + 0.5) / n as f64 - 1.0,
                2.0 * (j as f64 + 0.5) / n as f64 - 1.0,
            );
            visit(light.center + light.half_width * s + light.half_height * t, weight);
        }
    }
}

/// The specular of the standard material, GGX with height-correlated Smith and Schlick Fresnel, times `n.l`.
pub fn ggx_specular_cosine(normal: DVec3, view: DVec3, light: DVec3, roughness: f64, f0: f64, f90: f64) -> f64 {
    let n_dot_l = normal.dot(light);
    let n_dot_v = normal.dot(view).abs().max(1.0e-4);
    if n_dot_l <= 0.0 {
        return 0.0;
    }
    let half = (light + view).normalize();
    let alpha = roughness * roughness;
    let a2 = alpha * alpha;
    let n_dot_h = normal.dot(half).clamp(0.0, 1.0);
    let f = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    let distribution = a2 / (PI * f * f);
    let ggx_v = n_dot_l * (n_dot_v * n_dot_v * (1.0 - a2) + a2).sqrt();
    let ggx_l = n_dot_v * (n_dot_l * n_dot_l * (1.0 - a2) + a2).sqrt();
    let visibility = 0.5 / (ggx_v + ggx_l);
    let fresnel = f0 + (f90 - f0) * (1.0 - view.dot(half).clamp(0.0, 1.0)).powi(5);
    fresnel * distribution * visibility * n_dot_l
}

/// Specular radiance per unit luminance by quadrature over the rectangle (`n x n` midpoints).
pub fn specular_numeric(
    light: &RectLight,
    point: DVec3,
    normal: DVec3,
    view: DVec3,
    roughness: f64,
    (f0, f90): (f64, f64),
    n: usize,
) -> f64 {
    if !light.lights(point) {
        return 0.0;
    }
    let mut sum = 0.0;
    for_each_sample(light, n, |position, weight| {
        let to_light = position - point;
        let distance_square = to_light.length_squared();
        let direction = to_light / distance_square.sqrt();
        let emitted = (-direction).dot(light.normal).abs();
        sum += ggx_specular_cosine(normal, view, direction, roughness, f0, f90) * emitted * weight / distance_square;
    });
    sum
}

/// Specular radiance per unit luminance with the LTC table: the lobe integrated over the rectangle, times the lobe's
/// Fresnel-weighted albedo.
pub fn specular_ltc(
    light: &RectLight,
    point: DVec3,
    normal: DVec3,
    view: DVec3,
    roughness: f64,
    (f0, f90): (f64, f64),
) -> f64 {
    let corners = light.corners().map(|corner| corner - point);
    if !light.lights(point) || corners.iter().all(|corner| corner.dot(normal) <= 0.0) {
        return 0.0;
    }
    let n_dot_v = normal.dot(view).max(1.0e-4);
    let entry = ltc::lookup(roughness, n_dot_v);
    let tangent = {
        let projected = view - normal * normal.dot(view);
        if projected.length_squared() > 1.0e-12 {
            projected.normalize()
        } else {
            normal.any_orthonormal_vector()
        }
    };
    let bitangent = normal.cross(tangent);
    let [x_from_x, x_from_z, z_from_x, z_from_z] = entry.inverse;
    let transformed = corners.map(|corner| {
        let (x, y, z) = (corner.dot(tangent), corner.dot(bitangent), corner.dot(normal));
        DVec3::new(x_from_x * x + x_from_z * z, y, z_from_x * x + z_from_z * z)
    });
    let lobe = cosine_integral(&transformed, DVec3::Z) / PI;
    (f0 * entry.f0_scale + f90 * entry.f90_scale) * lobe
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_square_above(height: f64) -> RectLight {
        RectLight {
            center: DVec3::new(0.0, height, 0.0),
            half_width: DVec3::new(0.5, 0.0, 0.0),
            half_height: DVec3::new(0.0, 0.0, 0.5),
            normal: DVec3::NEG_Y,
            two_sided: false,
        }
    }

    #[test]
    fn a_polygon_covering_the_upper_hemisphere_has_cosine_integral_pi() {
        let big = 1.0e6;
        let corners = [
            DVec3::new(-big, 1.0, -big),
            DVec3::new(big, 1.0, -big),
            DVec3::new(big, 1.0, big),
            DVec3::new(-big, 1.0, big),
        ];

        let integral = cosine_integral(&corners, DVec3::Y);

        assert!((integral - PI).abs() < 1.0e-4, "{integral}");
    }

    #[test]
    fn a_square_straight_above_matches_the_closed_form_of_a_small_light() {
        // Far away the cosine-weighted solid angle is area cos / d^2.
        let light = unit_square_above(50.0);

        let integral = diffuse_cosine_integral(&light, DVec3::ZERO, DVec3::Y);

        assert!((integral * 2500.0 - 1.0).abs() < 1.0e-3, "{integral}");
    }

    #[test]
    fn analytic_diffuse_matches_quadrature_in_several_configurations() {
        let light = unit_square_above(1.0);
        for (point, normal) in [
            (DVec3::ZERO, DVec3::Y),
            (DVec3::new(0.7, 0.0, -0.3), DVec3::new(0.3, 1.0, 0.2).normalize()),
            (DVec3::new(2.0, 0.2, 0.0), DVec3::Y),
            (DVec3::new(0.2, 0.9, 0.1), DVec3::new(1.0, 0.0, 0.0)),
            (DVec3::new(-1.5, 0.0, 1.0), DVec3::new(1.0, 1.0, -1.0).normalize()),
        ] {
            let analytic = diffuse_cosine_integral(&light, point, normal);
            let numeric = diffuse_cosine_integral_numeric(&light, point, normal, 600);

            assert!(
                (analytic - numeric).abs() <= 1.0e-4 * numeric.abs().max(1.0e-3),
                "{point} {normal}: {analytic} vs {numeric}"
            );
        }
    }

    #[test]
    fn a_point_behind_a_one_sided_light_gets_nothing() {
        let light = unit_square_above(1.0);

        assert_eq!(
            diffuse_cosine_integral(&light, DVec3::new(0.0, 2.0, 0.0), DVec3::NEG_Y),
            0.0
        );
        assert!(
            diffuse_cosine_integral(
                &RectLight {
                    two_sided: true,
                    ..light
                },
                DVec3::new(0.0, 2.0, 0.0),
                DVec3::NEG_Y
            ) > 0.0
        );
    }
}
