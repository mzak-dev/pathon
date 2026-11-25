//! CPU twins of the shader functions, for tests that check pixels against the maths instead of against a picture.

use pathon_api::glam::{Vec2, Vec3};

/// Coverage of the pixel whose centre is `pixel_center` (framebuffer pixels, y down) by the line from `start` to `end`
/// that is `thickness` pixels wide, with round caps and a one pixel antialiasing ramp, as `lines.wgsl` shades it.
pub fn segment_coverage(start: Vec2, end: Vec2, thickness: f32, pixel_center: Vec2) -> f32 {
    let segment = end - start;
    let length_squared = segment.length_squared();
    let along = if length_squared > 1e-8 {
        ((pixel_center - start).dot(segment) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let distance = (pixel_center - (start + segment * along)).length();
    (thickness * 0.5 + 0.5 - distance).clamp(0.0, 1.0)
}

/// Coverage of a pixel at `distance_to_edge` pixels from the nearest edge inside a triangle, as the barycentric
/// wireframe shades it.
pub fn barycentric_coverage(thickness: f32, distance_to_edge: f32) -> f32 {
    (thickness * 0.5 + 0.5 - distance_to_edge).clamp(0.0, 1.0)
}

pub fn srgb_oetf(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb_eotf(display: f32) -> f32 {
    if display <= 0.040_45 {
        display / 12.92
    } else {
        ((display + 0.055) / 1.055).powf(2.4)
    }
}

/// The colour a material id gets in the preview.
pub fn material_color(id: u32) -> Vec3 {
    let state = id.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    let hashed = (word >> 22) ^ word;
    let channel = |shift: u32| ((hashed >> shift) & 255) as f32 / 255.0;
    Vec3::new(channel(0), channel(8), channel(16)) * 0.75 + Vec3::splat(0.25)
}

/// The ramp of the depth preview: black, purple, red, orange, pale yellow.
pub fn heat(value: f32) -> Vec3 {
    const STOPS: [Vec3; 5] = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.35, 0.05, 0.55),
        Vec3::new(0.85, 0.15, 0.25),
        Vec3::new(0.98, 0.65, 0.05),
        Vec3::new(1.0, 1.0, 0.8),
    ];
    let position = value.clamp(0.0, 1.0) * 4.0;
    let index = (position.floor() as usize).min(3);
    STOPS[index].lerp(STOPS[index + 1], position - index as f32)
}

/// The colour of a motion vector in pixels: hue is the direction, brightness the length up to `full_length`.
pub fn motion_color(motion: Vec2, full_length: f32) -> Vec3 {
    let length = motion.length();
    if length < 1e-4 {
        return Vec3::ZERO;
    }
    let hue = (motion.y.atan2(motion.x) / std::f32::consts::TAU).rem_euclid(1.0) * 6.0;
    let rgb = Vec3::new((hue - 3.0).abs() - 1.0, 2.0 - (hue - 2.0).abs(), 2.0 - (hue - 4.0).abs());
    rgb.clamp(Vec3::ZERO, Vec3::ONE) * (length / full_length).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_on_the_centre_line_is_fully_covered_and_the_fringe_ramps_down() {
        let (start, end) = (Vec2::new(2.0, 10.0), Vec2::new(30.0, 10.0));
        let at = |y: f32| segment_coverage(start, end, 3.0, Vec2::new(16.5, y));

        assert_eq!(at(10.5), 1.0);
        assert_eq!(at(9.5), 1.0);
        assert_eq!(at(11.5), 0.5, "the pixel row whose centre is 1.5 px from the line");
        assert_eq!(at(12.5), 0.0);
    }

    #[test]
    fn round_caps_end_the_line_a_little_past_its_endpoints() {
        let (start, end) = (Vec2::new(2.0, 10.0), Vec2::new(30.0, 10.0));
        assert_eq!(segment_coverage(start, end, 1.0, Vec2::new(30.0, 10.0)), 1.0);
        assert_eq!(segment_coverage(start, end, 1.0, Vec2::new(30.5, 10.0)), 0.5);
        assert_eq!(segment_coverage(start, end, 1.0, Vec2::new(31.5, 10.0)), 0.0);
    }

    #[test]
    fn material_colors_are_stable_distinct_and_never_black() {
        assert_eq!(material_color(7), material_color(7));
        assert_ne!(material_color(7), material_color(8));
        for id in 0..256 {
            assert!(material_color(id).min_element() >= 0.25);
        }
    }

    #[test]
    fn the_heat_ramp_hits_its_stops() {
        assert_eq!(heat(0.0), Vec3::ZERO);
        assert_eq!(heat(1.0), Vec3::new(1.0, 1.0, 0.8));
        assert!((heat(0.25) - Vec3::new(0.35, 0.05, 0.55)).length() < 1e-6);
    }

    #[test]
    fn motion_hue_follows_the_direction_and_brightness_the_length() {
        assert_eq!(motion_color(Vec2::ZERO, 8.0), Vec3::ZERO);
        let right = motion_color(Vec2::new(4.0, 0.0), 8.0);
        assert!((right - Vec3::new(1.0, 0.0, 0.0) * 0.5).length() < 1e-5, "{right}");
        let fast_right = motion_color(Vec2::new(80.0, 0.0), 8.0);
        assert!((fast_right - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn srgb_encoding_matches_known_points() {
        assert_eq!(srgb_oetf(0.0), 0.0);
        assert!((srgb_oetf(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_oetf(0.18) - 0.461_356).abs() < 1e-4);
    }
}
