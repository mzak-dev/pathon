use std::any::Any;

use pathon_rhi_types::MaybeSend;

use crate::feature::RenderFeature;
use crate::shared::Shared;

// The unit a user installs: one or more render features plus the data they share. A PBR pack registers its prepass,
// lighting and IBL features at once; a single feature is a pack by itself.
pub trait Pack {
    fn register(self, registrar: &mut PackRegistrar);
}

impl<Feature: RenderFeature + 'static> Pack for Feature {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar.add_feature(self);
    }
}

#[derive(Default)]
pub struct PackRegistrar {
    features: Vec<Box<dyn RenderFeature>>,
    shared: Shared,
}

impl std::fmt::Debug for PackRegistrar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.features.iter().map(|feature| feature.name()).collect();
        f.debug_struct("PackRegistrar")
            .field("features", &names)
            .field("shared", &self.shared)
            .finish()
    }
}

impl PackRegistrar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_feature(&mut self, feature: impl RenderFeature + 'static) -> &mut Self {
        self.features.push(Box::new(feature));
        self
    }

    pub fn add_pack(&mut self, pack: impl Pack) -> &mut Self {
        pack.register(self);
        self
    }

    pub fn insert_shared<T: Any + MaybeSend>(&mut self, value: T) -> &mut Self {
        self.shared.insert(value);
        self
    }

    pub fn into_parts(self) -> (Vec<Box<dyn RenderFeature>>, Shared) {
        (self.features, self.shared)
    }

    pub fn merge_into(self, features: &mut Vec<Box<dyn RenderFeature>>, shared: &mut Shared) {
        features.extend(self.features);
        shared.absorb(self.shared);
    }
}
