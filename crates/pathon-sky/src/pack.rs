use pathon_api::{Pack, PackRegistrar};

use crate::environment::SkyEnvironmentFeature;
use crate::feature::SkyFeature;
use crate::{SkyQuality, SkySettings};

/// The sky as one pack: the atmosphere drawn into `hdr`, the sun published as `SunLight` and the sky offered to
/// `IblPack` as an `IblSource`. The settings go to `Shared` and can be changed between frames.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SkyPack {
    settings: SkySettings,
    quality: SkyQuality,
}

impl SkyPack {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_settings(mut self, settings: SkySettings) -> Self {
        self.settings = settings;
        self
    }

    /// Sizes of the tables and steps of the marches; fixed once the renderer is built.
    #[must_use]
    pub fn with_quality(mut self, quality: SkyQuality) -> Self {
        self.quality = quality;
        self
    }
}

impl Pack for SkyPack {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar
            .add_feature(SkyEnvironmentFeature::new())
            .add_feature(SkyFeature::new(self.quality))
            .insert_shared(self.settings);
    }
}
