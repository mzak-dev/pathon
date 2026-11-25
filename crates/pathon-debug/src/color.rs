/// Straight (non-premultiplied) RGBA as it should look on screen, i.e. sRGB-encoded for an sRGB display.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DebugColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl DebugColor {
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const GRAY: Self = Self::rgb(0.5, 0.5, 0.5);
    pub const RED: Self = Self::rgb(1.0, 0.0, 0.0);
    pub const GREEN: Self = Self::rgb(0.0, 1.0, 0.0);
    pub const BLUE: Self = Self::rgb(0.0, 0.0, 1.0);
    pub const YELLOW: Self = Self::rgb(1.0, 1.0, 0.0);
    pub const CYAN: Self = Self::rgb(0.0, 1.0, 1.0);
    pub const MAGENTA: Self = Self::rgb(1.0, 0.0, 1.0);
    pub const ORANGE: Self = Self::rgb(1.0, 0.5, 0.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::new(r, g, b, 1.0)
    }

    #[must_use]
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub(crate) fn sanitized(self) -> Self {
        let clean = |value: f32| if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        Self::new(clean(self.r), clean(self.g), clean(self.b), clean(self.a))
    }
}

impl Default for DebugColor {
    fn default() -> Self {
        Self::WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing_clamps_and_zeroes_non_finite_channels() {
        let color = DebugColor::new(2.0, -1.0, f32::NAN, f32::INFINITY).sanitized();
        assert_eq!(color.to_array(), [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn alpha_can_be_replaced() {
        assert_eq!(DebugColor::RED.with_alpha(0.25).a, 0.25);
    }
}
