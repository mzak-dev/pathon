//! Offline fit of Linearly Transformed Cosines to the GGX BRDF of the standard material (ADR-0048).
//!
//! Written from the paper (Heitz, Dupuy, Hill and Neubelt 2016, "Real-Time Polygonal-Light Shading with Linearly
//! Transformed Cosines"), not from its code: a distribution `D(l) = D_o(M^-1 l / |M^-1 l|) |det M^-1| / |M^-1 l|^3`
//! with `D_o` the clamped cosine over pi, a matrix `M` per (roughness, view angle), found by minimising the L1
//! distance `|D_brdf - D_ltc|` (lower total variation than an L3 error in our trials) with Nelder-Mead on stratified
//! samples of both distributions (multiple importance sampling). Also the two scale factors that turn the normalised lobe into radiance: the integral of the lobe
//! times the Schlick Fresnel weights, for `f0` and `f90`.

use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

use glam::{DMat3, DVec3};

/// Table nodes per axis.
pub const SIZE: usize = 32;
/// Smallest `alpha` the table covers (roughness 0 would be a mirror).
pub const MIN_ALPHA: f64 = 1.5e-3;
/// Smallest cosine of the view angle (a grazing view is a division by zero in the BRDF).
pub const MIN_COS_VIEW: f64 = 1.0e-3;
/// Stratified samples per axis, per distribution.
const SAMPLES: usize = 40;

/// Perceptual roughness of node `index` along the first axis.
pub fn roughness_of(index: usize) -> f64 {
    index as f64 / (SIZE - 1) as f64
}

/// `alpha = roughness^2`, not below `MIN_ALPHA`.
pub fn alpha_of(roughness: f64) -> f64 {
    (roughness * roughness).max(MIN_ALPHA)
}

/// Cosine of the view angle of node `index` along the second axis: nodes are even in `sqrt(1 - cos)`, which puts more
/// of them near grazing angles, where the lobe changes fastest.
pub fn cos_view_of(index: usize) -> f64 {
    let t = index as f64 / (SIZE - 1) as f64;
    (1.0 - t * t).max(MIN_COS_VIEW)
}

fn view_vector(cos_view: f64) -> DVec3 {
    DVec3::new((1.0 - cos_view * cos_view).max(0.0).sqrt(), 0.0, cos_view)
}

/// `D(h) V(l, v) n.l` of the standard material (GGX, height-correlated Smith), Fresnel left out.
pub fn ggx_lobe(view: DVec3, light: DVec3, alpha: f64) -> f64 {
    if light.z <= 0.0 || view.z <= 0.0 {
        return 0.0;
    }
    let half = (view + light).normalize();
    let a2 = alpha * alpha;
    let f = half.z * half.z * (a2 - 1.0) + 1.0;
    let distribution = a2 / (PI * f * f);
    let ggx_v = light.z * (view.z * view.z * (1.0 - a2) + a2).sqrt();
    let ggx_l = view.z * (light.z * light.z * (1.0 - a2) + a2).sqrt();
    let visibility = 0.5 / (ggx_v + ggx_l);
    distribution * visibility * light.z
}

/// Density of sampling `l` by drawing the GGX half vector (what `ggx_sample` does).
fn ggx_pdf(view: DVec3, light: DVec3, alpha: f64) -> f64 {
    let half = (view + light).normalize();
    let v_dot_h = view.dot(half);
    if light.z <= 0.0 || v_dot_h <= 0.0 {
        return 0.0;
    }
    let a2 = alpha * alpha;
    let f = half.z * half.z * (a2 - 1.0) + 1.0;
    a2 / (PI * f * f) * half.z / (4.0 * v_dot_h)
}

fn ggx_sample(view: DVec3, alpha: f64, u1: f64, u2: f64) -> DVec3 {
    let phi = TAU * u1;
    let a2 = alpha * alpha;
    let cos_theta = ((1.0 - u2) / (1.0 + (a2 - 1.0) * u2)).sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let half = DVec3::new(sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta);
    2.0 * view.dot(half) * half - view
}

/// The integrals of the lobe: without Fresnel and weighted by `(1 - v.h)^5`, the Schlick term.
pub fn lobe_integrals(view: DVec3, alpha: f64) -> (f64, f64) {
    const N: usize = 256;
    let (mut plain, mut grazing) = (0.0, 0.0);
    for i in 0..N {
        for j in 0..N {
            let l = ggx_sample(view, alpha, (i as f64 + 0.5) / N as f64, (j as f64 + 0.5) / N as f64);
            let pdf = ggx_pdf(view, l, alpha);
            if pdf <= 0.0 {
                continue;
            }
            let value = ggx_lobe(view, l, alpha) / pdf;
            let half = (view + l).normalize();
            plain += value;
            grazing += value * (1.0 - view.dot(half)).clamp(0.0, 1.0).powi(5);
        }
    }
    let scale = 1.0 / (N * N) as f64;
    (plain * scale, grazing * scale)
}

/// A linearly transformed cosine, `M = [[m11, 0, m13], [0, m22, 0], [m31, 0, 1]]`.
#[derive(Clone, Copy, Debug)]
pub struct Ltc {
    pub m11: f64,
    pub m22: f64,
    pub m13: f64,
    pub m31: f64,
}

impl Ltc {
    fn matrix(&self) -> DMat3 {
        DMat3::from_cols(
            DVec3::new(self.m11, 0.0, self.m31),
            DVec3::new(0.0, self.m22, 0.0),
            DVec3::new(self.m13, 0.0, 1.0),
        )
    }

    /// Density of the direction `l`.
    pub fn pdf(&self, light: DVec3) -> f64 {
        let inverse = self.matrix().inverse();
        let original = inverse * light;
        let length = original.length();
        let cosine = (original.z / length).max(0.0) / PI;
        cosine * inverse.determinant().abs() / (length * length * length)
    }

    fn sample(&self, u1: f64, u2: f64) -> DVec3 {
        let phi = TAU * u1;
        let radius = u2.sqrt();
        let original = DVec3::new(radius * phi.cos(), radius * phi.sin(), (1.0 - u2).sqrt());
        (self.matrix() * original).normalize()
    }

    /// `(a, b, c, d)` of the inverse with its middle element scaled to 1: `x' = a x + b z`, `z' = c x + d z`, `y' = y`.
    pub fn inverse_parameters(&self) -> [f64; 4] {
        let (a, b, c, d) = (self.m11 / self.m22, self.m13 / self.m22, self.m31 / self.m22, 1.0 / self.m22);
        let determinant = a * d - b * c;
        [d / determinant, -b / determinant, -c / determinant, a / determinant]
    }

    fn from_parameters(p: &[f64]) -> Self {
        Self {
            m11: p[0].exp(),
            m22: p[1].exp(),
            m13: p[2],
            m31: p[3],
        }
    }

    fn to_parameters(self) -> [f64; 4] {
        [self.m11.ln(), self.m22.ln(), self.m13, self.m31]
    }
}

struct Target {
    view: DVec3,
    alpha: f64,
    lobe_integral: f64,
    brdf_samples: Vec<DVec3>,
    cosine_samples: Vec<(f64, f64)>,
}

impl Target {
    fn new(cos_view: f64, alpha: f64) -> Self {
        let view = view_vector(cos_view);
        let (lobe_integral, _) = lobe_integrals(view, alpha);
        let mut brdf_samples = Vec::new();
        let mut cosine_samples = Vec::new();
        for i in 0..SAMPLES {
            for j in 0..SAMPLES {
                let (u1, u2) = ((i as f64 + 0.5) / SAMPLES as f64, (j as f64 + 0.5) / SAMPLES as f64);
                let l = ggx_sample(view, alpha, u1, u2);
                if l.z > 0.0 {
                    brdf_samples.push(l);
                }
                cosine_samples.push((u1, u2));
            }
        }
        Self {
            view,
            alpha,
            lobe_integral,
            brdf_samples,
            cosine_samples,
        }
    }

    // Both estimators with the balance heuristic: sum of f / (n1 p1 + n2 p2) over the samples of both.
    fn integrate(&self, ltc: &Ltc, f: impl Fn(f64, f64) -> f64) -> f64 {
        let brdf_count = (SAMPLES * SAMPLES) as f64;
        let ltc_count = (SAMPLES * SAMPLES) as f64;
        let mut sum = 0.0;
        let mut accumulate = |l: DVec3| {
            let brdf_density = self.brdf_density(l);
            let ltc_density = ltc.pdf(l);
            let sampling = brdf_count * ggx_pdf(self.view, l, self.alpha) + ltc_count * ltc_density;
            if sampling > 0.0 {
                sum += f(brdf_density, ltc_density) / sampling;
            }
        };
        for l in &self.brdf_samples {
            accumulate(*l);
        }
        for (u1, u2) in &self.cosine_samples {
            accumulate(ltc.sample(*u1, *u2));
        }
        sum
    }

    fn brdf_density(&self, l: DVec3) -> f64 {
        ggx_lobe(self.view, l, self.alpha) / self.lobe_integral
    }

    fn error(&self, ltc: &Ltc) -> f64 {
        self.integrate(ltc, |brdf, lobe| (brdf - lobe).abs())
    }

    /// Half the L1 distance of the two densities: 0 for equal lobes, 1 for disjoint ones.
    fn total_variation(&self, ltc: &Ltc) -> f64 {
        0.5 * self.integrate(ltc, |brdf, lobe| (brdf - lobe).abs())
    }
}

fn nelder_mead(start: &[f64], steps: &[f64], mut f: impl FnMut(&[f64]) -> f64) -> (Vec<f64>, f64) {
    let n = start.len();
    let mut simplex: Vec<(Vec<f64>, f64)> = Vec::with_capacity(n + 1);
    simplex.push((start.to_vec(), f(start)));
    for i in 0..n {
        let mut point = start.to_vec();
        point[i] += steps[i];
        let value = f(&point);
        simplex.push((point, value));
    }
    for _ in 0..1500 {
        simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (best, worst) = (simplex[0].1, simplex[n].1);
        if (worst - best).abs() <= 1.0e-9 * best.abs().max(1.0e-30) {
            break;
        }
        let centroid: Vec<f64> = (0..n)
            .map(|k| simplex[..n].iter().map(|(p, _)| p[k]).sum::<f64>() / n as f64)
            .collect();
        let toward = |factor: f64| -> Vec<f64> {
            (0..n)
                .map(|k| centroid[k] + factor * (simplex[n].0[k] - centroid[k]))
                .collect()
        };
        let reflected = toward(-1.0);
        let reflected_value = f(&reflected);
        if reflected_value < simplex[0].1 {
            let expanded = toward(-2.0);
            let expanded_value = f(&expanded);
            simplex[n] = if expanded_value < reflected_value {
                (expanded, expanded_value)
            } else {
                (reflected, reflected_value)
            };
        } else if reflected_value < simplex[n - 1].1 {
            simplex[n] = (reflected, reflected_value);
        } else {
            let contracted = if reflected_value < simplex[n].1 {
                toward(-0.5)
            } else {
                toward(0.5)
            };
            let contracted_value = f(&contracted);
            if contracted_value < simplex[n].1.min(reflected_value) {
                simplex[n] = (contracted, contracted_value);
            } else {
                let best_point = simplex[0].0.clone();
                for entry in simplex.iter_mut().skip(1) {
                    for k in 0..n {
                        entry.0[k] = best_point[k] + 0.5 * (entry.0[k] - best_point[k]);
                    }
                    entry.1 = f(&entry.0);
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    simplex.swap_remove(0)
}

/// What the shader reads for one `(roughness, view angle)`: the inverse transform and the two Fresnel weights.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub inverse: [f64; 4],
    /// Integral of the lobe times `1 - (1 - v.h)^5`: what `f0` multiplies.
    pub f0_scale: f64,
    /// Integral of the lobe times `(1 - v.h)^5`: what `f90` multiplies.
    pub f90_scale: f64,
    pub ltc: Ltc,
    pub total_variation: f64,
}

/// Fits one node, starting the search at `start` (a neighbour's solution).
pub fn fit_entry(cos_view: f64, alpha: f64, start: Ltc) -> Entry {
    let target = Target::new(cos_view, alpha);
    let mut best = start.to_parameters().to_vec();
    let mut best_error = f64::INFINITY;
    let mut step: f64 = 0.2;
    for _ in 0..10 {
        let steps = [step, step, 0.1 * step.max(0.05), 0.1 * step.max(0.05)];
        let (point, error) = nelder_mead(&best, &steps, |p| target.error(&Ltc::from_parameters(p)));
        let improved = error < best_error * 0.999;
        if error < best_error {
            best = point;
            best_error = error;
        }
        if !improved {
            break;
        }
        step = (step * 0.7).max(0.03);
    }
    let ltc = Ltc::from_parameters(&best);
    let (plain, grazing) = lobe_integrals(target.view, alpha);
    Entry {
        inverse: ltc.inverse_parameters(),
        f0_scale: plain - grazing,
        f90_scale: grazing,
        ltc,
        total_variation: target.total_variation(&ltc),
    }
}

fn fit_row(view_index: usize, mut start: Ltc) -> Vec<Entry> {
    let mut row = vec![None; SIZE];
    for roughness_index in (0..SIZE).rev() {
        let entry = fit_entry(
            cos_view_of(view_index),
            alpha_of(roughness_of(roughness_index)),
            start,
        );
        start = entry.ltc;
        row[roughness_index] = Some(entry);
    }
    row.into_iter().map(|entry| entry.expect("every node fitted")).collect()
}

/// The whole table, `SIZE * SIZE` entries, view angle major: index `view * SIZE + roughness`.
pub fn fit_table() -> Vec<Entry> {
    // The roughest node of each view angle is seeded from the one before it; the rows then run in parallel.
    let mut seeds = Vec::with_capacity(SIZE);
    let mut seed = Ltc {
        m11: 1.0,
        m22: 1.0,
        m13: 0.0,
        m31: 0.0,
    };
    for view_index in 0..SIZE {
        let entry = fit_entry(cos_view_of(view_index), alpha_of(1.0), seed);
        seed = entry.ltc;
        seeds.push(entry.ltc);
    }
    let mut rows: Vec<Vec<Entry>> = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = seeds
            .iter()
            .enumerate()
            .map(|(view_index, seed)| scope.spawn(move || fit_row(view_index, *seed)))
            .collect();
        for handle in handles {
            rows.push(handle.join().expect("a fit thread"));
        }
    });
    rows.into_iter().flatten().collect()
}

/// The Rust source of `crates/pathon-pbr/src/ltc_table.rs`.
pub fn table_source(entries: &[Entry]) -> String {
    let mut source = String::new();
    source.push_str(
        "// Generated by `cargo run -p pathon-ltc-fit --release` (ADR-0048): do not edit by hand.\n\
         // Per node (view angle major, then roughness): the inverse transform `a, b, c, d` of the lobe (`x' = a x + b z`,\n\
         // `z' = c x + d z`, `y' = y`), the integral of the lobe times `1 - F5` and times `F5` (`F5 = (1 - v.h)^5`), padding.\n\n\
         pub(crate) const LTC_SIZE: usize = 32;\n\n\
         #[rustfmt::skip]\n\
         pub(crate) const LTC_GGX: [[f32; 8]; LTC_SIZE * LTC_SIZE] = [\n",
    );
    for entry in entries {
        let [a, b, c, d] = entry.inverse;
        // Writing to a `String` cannot fail.
        let _ = writeln!(
            source,
            "    [{:?}, {:?}, {:?}, {:?}, {:?}, {:?}, 0.0, 0.0],",
            a as f32, b as f32, c as f32, d as f32, entry.f0_scale as f32, entry.f90_scale as f32
        );
    }
    source.push_str("];\n");
    source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lobe_integrates_to_the_closed_form_albedo_of_alpha_one() {
        // alpha = 1 makes D = 1 / pi and the visibility 0.5 / (n.l + n.v); seen head on the integral is 1 - ln 2.
        let (plain, grazing) = lobe_integrals(view_vector(1.0), 1.0);

        assert!((plain - (1.0 - std::f64::consts::LN_2)).abs() < 2.0e-3, "{plain}");
        assert!(grazing < 0.1 * plain, "the Schlick term of a head-on view is small: {grazing}");
    }

    #[test]
    fn the_fitted_lobe_is_close_to_the_brdf_at_a_few_nodes() {
        for (roughness_index, view_index, limit) in [(31, 0, 0.02), (16, 8, 0.06), (8, 20, 0.12), (24, 28, 0.15)] {
            let start = Ltc {
                m11: 0.5,
                m22: 0.5,
                m13: 0.0,
                m31: 0.0,
            };

            let entry = fit_entry(cos_view_of(view_index), alpha_of(roughness_of(roughness_index)), start);

            assert!(
                entry.total_variation < limit,
                "roughness node {roughness_index}, view node {view_index}: {}",
                entry.total_variation
            );
        }
    }

    #[test]
    fn the_inverse_parameters_invert_the_matrix() {
        let ltc = Ltc {
            m11: 0.4,
            m22: 0.3,
            m13: 0.2,
            m31: -0.1,
        };
        let [a, b, c, d] = ltc.inverse_parameters();

        // M^-1 maps `M (x, y, z)` to `(x, y, z)` up to the scale that makes the middle element 1.
        let original = DVec3::new(0.3, 0.2, 0.9);
        let mapped = ltc.matrix() * original;
        let restored = DVec3::new(a * mapped.x + b * mapped.z, mapped.y, c * mapped.x + d * mapped.z);
        let scale = restored.y / original.y;

        assert!(((restored / scale) - original).length() < 1e-9, "{restored}");
    }
}
