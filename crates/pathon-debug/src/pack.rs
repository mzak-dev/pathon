use pathon_api::{Pack, PackRegistrar};

use crate::draw::DebugDraw;
use crate::lines::LinesFeature;
use crate::preview::SlotPreviewFeature;
use crate::settings::DebugSettings;
use crate::wireframe::WireframeFeature;

/// Debug lines, wireframes and slot previews as one pack. `DebugDraw` and `DebugSettings` go to `Shared`, where the
/// application fills them between frames.
#[derive(Debug, Default)]
pub struct DebugPack {
    settings: DebugSettings,
    draw: DebugDraw,
}

impl DebugPack {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_settings(mut self, settings: DebugSettings) -> Self {
        self.settings = settings;
        self
    }

    #[must_use]
    pub fn with_draw(mut self, draw: DebugDraw) -> Self {
        self.draw = draw;
        self
    }
}

impl Pack for DebugPack {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar
            .add_feature(SlotPreviewFeature::new())
            .add_feature(WireframeFeature::new())
            .add_feature(LinesFeature::new())
            .insert_shared(self.settings)
            .insert_shared(self.draw);
    }
}
