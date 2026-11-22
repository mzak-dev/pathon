use pathon_api::{Pack, PackRegistrar};

use crate::bloom::BloomFeature;
use crate::exposure::ExposureFeature;
use crate::settings::PostSettings;
use crate::tonemap::TonemapFeature;

/// Exposure, bloom and tonemapping as one pack. The settings go to `Shared` and can be changed between frames.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PostPack {
    settings: PostSettings,
}

impl PostPack {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_settings(settings: PostSettings) -> Self {
        Self { settings }
    }
}

impl Pack for PostPack {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar
            .add_feature(ExposureFeature::new())
            .add_feature(BloomFeature::new())
            .add_feature(TonemapFeature::new())
            .insert_shared(self.settings);
    }
}
