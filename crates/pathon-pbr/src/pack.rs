use pathon_api::{Pack, PackRegistrar};

use crate::context::PbrContext;
use crate::depth_pyramid::DepthPyramidFeature;
use crate::lighting::LightingFeature;
use crate::prepass::PrepassFeature;
use crate::settings::PbrSettings;

/// Prepass, optional depth pyramid and clustered forward lighting as one pack. The settings go to `Shared` and can be
/// changed between frames.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PbrPack {
    settings: PbrSettings,
}

impl PbrPack {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_settings(settings: PbrSettings) -> Self {
        Self { settings }
    }
}

impl Pack for PbrPack {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar
            .add_feature(PrepassFeature::new())
            .add_feature(DepthPyramidFeature::new())
            .add_feature(LightingFeature::new())
            .insert_shared(self.settings)
            .insert_shared(PbrContext::default());
    }
}
