use pathon_api::{Pack, PackRegistrar};

use crate::feature::GiFeature;
use crate::settings::GiSettings;

/// Global illumination from a field of irradiance probes. The settings go to `Shared` and can be changed between
/// frames; the scene is read from `Shared` too (`Snapshot`, `GeometryPool`, `MaterialTable`, the same ones `PbrPack`
/// draws), and the lighting pack reads the result as [`crate::GiFrame`].
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct GiPack {
    settings: GiSettings,
}

impl GiPack {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_settings(settings: GiSettings) -> Self {
        Self { settings }
    }
}

impl Pack for GiPack {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar.add_feature(GiFeature::new()).insert_shared(self.settings);
    }
}
