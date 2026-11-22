//! CPU references of the GPU passes: slow, in `f64`, and written as plain numerical integration of the definitions,
//! so the compute shaders are checked against the maths and not against themselves.

use std::f64::consts::{PI, TAU};

use pathon_api::glam::DVec3;

fn vector(array: [f64; 3]) -> DVec3 {
    DVec3::from_array(array)
}

/// Direction of the equirectangular position `(u, v)`, both in `0..=1`: `v = 0` is the +Y pole and `u = 0.5` looks
/// down -Z.
pub fn equirect_direction(u: f64, v: f64) -> [f64; 3] {
    let longitude = (u - 0.5) * TAU;
    let latitude = v * PI;
    [
        latitude.sin() * longitude.sin(),
        latitude.cos(),
        -latitude.sin() * longitude.cos(),
    ]
}

/// The inverse of [`equirect_direction`].
pub fn equirect_uv(direction: [f64; 3]) -> [f64; 2] {
    [
        direction[0].atan2(-direction[2]) / TAU + 0.5,
        direction[1].clamp(-1.0, 1.0).acos() / PI,
    ]
}

/// Direction through the point `(s, t)` in `-1..=1` of a cube face (faces +X, -X, +Y, -Y, +Z, -Z; `t` grows
/// downwards in the texture), the convention of every cube texture in WebGPU.
pub fn face_direction(face: u32, s: f64, t: f64) -> [f64; 3] {
    let direction = match face {
        0 => DVec3::new(1.0, -t, -s),
        1 => DVec3::new(-1.0, -t, s),
        2 => DVec3::new(s, 1.0, t),
        3 => DVec3::new(s, -1.0, -t),
        4 => DVec3::new(s, -t, 1.0),
        _ => DVec3::new(-s, -t, -1.0),
    };
    direction.normalize().to_array()
}

/// Direction through the centre of texel `(x, y)` of a `size` × `size` face.
pub fn texel_direction(face: u32, x: u32, y: u32, size: u32) -> [f64; 3] {
    let coordinate = |texel: u32| 2.0 * (f64::from(texel) + 0.5) / f64::from(size) - 1.0;
    face_direction(face, coordinate(x), coordinate(y))
}

fn tangent_frame(normal: DVec3) -> (DVec3, DVec3) {
    let up = if normal.z.abs() < 0.999 { DVec3::Z } else { DVec3::X };
    let tangent = up.cross(normal).normalize();
    (tangent, normal.cross(tangent))
}

// The half vector of the GGX distribution D(h) n.h for stratum (`xi_y`, `xi_x`), z being the normal.
fn ggx_half_vector(alpha: f64, xi_x: f64, xi_y: f64) -> DVec3 {
    let phi = TAU * xi_x;
    let cos_theta = ((1.0 - xi_y) / (1.0 + (alpha * alpha - 1.0) * xi_y)).sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    DVec3::new(sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta)
}

fn visibility_smith_ggx(n_dot_l: f64, n_dot_v: f64, alpha: f64) -> f64 {
    let a2 = alpha * alpha;
    let ggx_v = n_dot_l * (n_dot_v * n_dot_v * (1.0 - a2) + a2).sqrt();
    let ggx_l = n_dot_v * (n_dot_l * n_dot_l * (1.0 - a2) + a2).sqrt();
    let sum = ggx_v + ggx_l;
    if sum > 0.0 { 0.5 / sum } else { 0.0 }
}

/// The split-sum BRDF at `(n_dot_v, roughness)` as `[scale, bias]`: the directional albedo of the specular lobe is
/// `f0 * scale + f90 * bias`. An integral over the half vector in a 512 × 256 grid of equal-probability strata.
pub fn brdf_lut_texel(n_dot_v: f64, roughness: f64) -> [f64; 2] {
    const STRATA: (u32, u32) = (512, 256);
    let alpha = roughness * roughness;
    let v = DVec3::new((1.0 - n_dot_v * n_dot_v).max(0.0).sqrt(), 0.0, n_dot_v);
    let (mut scale, mut bias) = (0.0, 0.0);
    for i in 0..STRATA.0 {
        for j in 0..STRATA.1 {
            let xi_y = (f64::from(i) + 0.5) / f64::from(STRATA.0);
            let xi_x = (f64::from(j) + 0.5) / f64::from(STRATA.1);
            let h = ggx_half_vector(alpha, xi_x, xi_y);
            let l = 2.0 * v.dot(h) * h - v;
            let v_dot_h = v.dot(h).max(0.0);
            if l.z > 0.0 && h.z > 0.0 {
                let g_vis = 4.0 * visibility_smith_ggx(l.z, n_dot_v, alpha) * l.z * v_dot_h / h.z;
                let fresnel = (1.0 - v_dot_h).powi(5);
                scale += (1.0 - fresnel) * g_vis;
                bias += fresnel * g_vis;
            }
        }
    }
    let count = f64::from(STRATA.0 * STRATA.1);
    [scale / count, bias / count]
}

/// What the prefiltered cube holds at `normal` for a perceptual `roughness`: the environment averaged over GGX
/// lobes of the mirror direction (normal = view), weighted by `n.l`. Stratified quadrature of the same estimator the
/// shader samples, so only the sampling is replaced.
pub fn prefiltered_radiance(environment: impl Fn([f64; 3]) -> [f64; 3], normal: [f64; 3], roughness: f64) -> [f64; 3] {
    const STRATA: (u32, u32) = (256, 256);
    let n = vector(normal).normalize();
    if roughness <= 0.0 {
        return environment(n.to_array());
    }
    let alpha = roughness * roughness;
    let (tangent, bitangent) = tangent_frame(n);
    let mut sum = DVec3::ZERO;
    let mut weight = 0.0;
    for i in 0..STRATA.0 {
        for j in 0..STRATA.1 {
            let xi_y = (f64::from(i) + 0.5) / f64::from(STRATA.0);
            let xi_x = (f64::from(j) + 0.5) / f64::from(STRATA.1);
            let local = ggx_half_vector(alpha, xi_x, xi_y);
            let h = tangent * local.x + bitangent * local.y + n * local.z;
            let l = 2.0 * n.dot(h) * h - n;
            let n_dot_l = n.dot(l);
            if n_dot_l > 0.0 {
                sum += vector(environment(l.to_array())) * n_dot_l;
                weight += n_dot_l;
            }
        }
    }
    (sum / weight).to_array()
}

/// Irradiance divided by pi at `normal`: the radiance a white Lambertian surface sends back (`ibl_diffuse`).
/// Midpoint quadrature over the whole sphere, 256 × 512 cells.
pub fn irradiance_over_pi(environment: impl Fn([f64; 3]) -> [f64; 3], normal: [f64; 3]) -> [f64; 3] {
    const CELLS: (u32, u32) = (256, 512);
    let n = vector(normal).normalize();
    let (d_theta, d_phi) = (PI / f64::from(CELLS.0), TAU / f64::from(CELLS.1));
    let mut sum = DVec3::ZERO;
    for i in 0..CELLS.0 {
        let theta = (f64::from(i) + 0.5) * d_theta;
        for j in 0..CELLS.1 {
            let phi = (f64::from(j) + 0.5) * d_phi;
            let l = DVec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            let cosine = n.dot(l);
            if cosine > 0.0 {
                sum += vector(environment(l.to_array())) * (cosine * theta.sin() * d_theta * d_phi);
            }
        }
    }
    (sum / PI).to_array()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equirect_mapping_round_trips() {
        for (u, v) in [(0.5, 0.5), (0.1, 0.3), (0.9, 0.8), (0.25, 0.05)] {
            let [u2, v2] = equirect_uv(equirect_direction(u, v));
            assert!((u - u2).abs() < 1e-9 && (v - v2).abs() < 1e-9, "{u} {v} -> {u2} {v2}");
        }
        // The picture centre looks down -Z and the top row at +Y.
        let centre = equirect_direction(0.5, 0.5);
        assert!((centre[2] + 1.0).abs() < 1e-12);
        assert!((equirect_direction(0.3, 0.0)[1] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn face_centres_point_along_their_axes() {
        let axes = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        for (face, axis) in axes.into_iter().enumerate() {
            let direction = face_direction(u32::try_from(face).expect("six faces"), 0.0, 0.0);
            for (actual, expected) in direction.iter().zip(axis) {
                assert!((actual - expected).abs() < 1e-12, "face {face}: {direction:?}");
            }
        }
    }

    #[test]
    fn a_smooth_surface_reflects_everything_and_a_rough_one_loses_some() {
        let [scale, bias] = brdf_lut_texel(0.8, 0.02);
        assert!((scale + bias - 1.0).abs() < 0.02, "{scale} + {bias}");
        let [rough_scale, rough_bias] = brdf_lut_texel(0.5, 1.0);
        assert!(rough_scale + rough_bias < 1.0 && rough_scale + rough_bias > 0.4);
    }

    #[test]
    fn a_constant_environment_is_its_own_prefilter_and_irradiance() {
        let furnace = |_: [f64; 3]| [1.0, 2.0, 0.5];
        for roughness in [0.0, 0.3, 1.0] {
            let value = prefiltered_radiance(furnace, [0.3, 0.8, -0.5], roughness);
            assert!((value[1] - 2.0).abs() < 1e-9, "{roughness}: {value:?}");
        }
        let irradiance = irradiance_over_pi(furnace, [0.0, 1.0, 0.0]);
        assert!(
            (irradiance[0] - 1.0).abs() < 1e-3 && (irradiance[1] - 2.0).abs() < 2e-3,
            "{irradiance:?}"
        );
    }
}
