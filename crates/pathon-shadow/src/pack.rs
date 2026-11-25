use pathon_api::{Pack, PackRegistrar};

use crate::feature::ShadowFeature;
use crate::input::ShadowScene;
use crate::settings::ShadowSettings;

/// Cascaded shadow maps for one directional light. The settings and an empty [`ShadowScene`] go to `Shared`; the host
/// (or the lighting pack) fills the scene every frame.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ShadowPack {
    settings: ShadowSettings,
}

impl ShadowPack {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_settings(settings: ShadowSettings) -> Self {
        Self { settings }
    }
}

impl Pack for ShadowPack {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar
            .add_feature(ShadowFeature::new())
            .insert_shared(self.settings)
            .insert_shared(ShadowScene::new());
    }
}
