// Bit set of layers: an instance is seen by a camera or lit by a light when their masks intersect.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct LayerMask(pub u32);

impl LayerMask {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(u32::MAX);
    pub const DEFAULT: Self = Self(1);

    pub const fn layer(index: u32) -> Self {
        Self(1 << index)
    }

    #[must_use]
    pub const fn with(self, index: u32) -> Self {
        Self(self.0 | (1 << index))
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl Default for LayerMask {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_intersect_on_a_shared_layer_only() {
        let ui = LayerMask::layer(3);

        assert!(LayerMask::ALL.intersects(ui));
        assert!(ui.with(5).intersects(LayerMask::layer(5)));
        assert!(!ui.intersects(LayerMask::DEFAULT));
        assert!(!LayerMask::NONE.intersects(LayerMask::ALL));
    }
}
