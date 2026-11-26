//! The Linearly Transformed Cosines table of the GGX lobe (ADR-0048) and the lookup the shader mirrors.

use crate::ltc_table::{LTC_GGX, LTC_SIZE};

/// The table as the shader reads it: `LTC_SIZE * LTC_SIZE` nodes (view angle major, then roughness) of two `vec4`.
pub fn table_bytes() -> &'static [u8] {
    bytemuck::cast_slice(&LTC_GGX)
}

/// What the table gives for one roughness and view angle.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LtcEntry {
    /// `a, b, c, d` of the inverse transform: `x' = a x + b z`, `z' = c x + d z`, `y' = y` (in the frame of the normal
    /// with `x` along the view's tangential part).
    pub inverse: [f64; 4],
    /// Integral of the lobe times `1 - (1 - v.h)^5`: what `f0` multiplies.
    pub f0_scale: f64,
    /// Integral of the lobe times `(1 - v.h)^5`: what `f90` multiplies.
    pub f90_scale: f64,
}

/// Bilinear lookup at perceptual `roughness` and `n_dot_v`; the table is even in `roughness` and `sqrt(1 - n_dot_v)`.
pub fn lookup(roughness: f64, n_dot_v: f64) -> LtcEntry {
    let last = (LTC_SIZE - 1) as f64;
    let u = roughness.clamp(0.0, 1.0) * last;
    let v = (1.0 - n_dot_v.clamp(0.0, 1.0)).sqrt() * last;
    let (u0, v0) = (u.floor(), v.floor());
    let (fu, fv) = (u - u0, v - v0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the table, a small non-negative integer"
    )]
    let node = |du: f64, dv: f64| {
        let x = (u0 + du).min(last) as usize;
        let y = (v0 + dv).min(last) as usize;
        LTC_GGX[y * LTC_SIZE + x].map(f64::from)
    };
    let mut blended = [0.0; 6];
    for (weight, entry) in [
        ((1.0 - fu) * (1.0 - fv), node(0.0, 0.0)),
        (fu * (1.0 - fv), node(1.0, 0.0)),
        ((1.0 - fu) * fv, node(0.0, 1.0)),
        (fu * fv, node(1.0, 1.0)),
    ] {
        for (sum, value) in blended.iter_mut().zip(entry) {
            *sum += weight * value;
        }
    }
    LtcEntry {
        inverse: [blended[0], blended[1], blended[2], blended[3]],
        f0_scale: blended[4],
        f90_scale: blended[5],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_a_finite_entry_for_every_node_and_a_positive_scale() {
        assert_eq!(LTC_GGX.len(), LTC_SIZE * LTC_SIZE);
        for (index, node) in LTC_GGX.iter().enumerate() {
            assert!(node.iter().all(|value| value.is_finite()), "node {index}: {node:?}");
            assert!(node[5] >= 0.0 && node[4] > 0.0, "node {index}: {node:?}");
        }
        assert_eq!(table_bytes().len(), LTC_GGX.len() * 32);
    }

    #[test]
    fn a_rough_head_on_lobe_is_the_cosine_itself() {
        // Roughness 1 head on: the lobe is close to the clamped cosine, so the inverse is close to the identity.
        let entry = lookup(1.0, 1.0);

        assert!((entry.inverse[0] - 1.0).abs() < 0.2, "{entry:?}");
        assert!((entry.inverse[3] - 1.0).abs() < 0.2, "{entry:?}");
        assert!(
            entry.inverse[1].abs() < 0.1 && entry.inverse[2].abs() < 0.1,
            "{entry:?}"
        );
    }

    #[test]
    fn lookups_between_nodes_blend_the_neighbours() {
        let step = 1.0 / (LTC_SIZE - 1) as f64;
        let node = 16.0 * step;
        let low = lookup(node, 1.0);
        let high = lookup(node + step, 1.0);

        let middle = lookup(node + 0.5 * step, 1.0);

        assert!((middle.f0_scale - f64::midpoint(low.f0_scale, high.f0_scale)).abs() < 1e-6);
    }
}
