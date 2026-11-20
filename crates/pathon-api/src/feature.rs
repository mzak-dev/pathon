use std::any::Any;
use std::sync::Arc;

use pathon_graph::Stage;
use pathon_rhi_types::{Backend, Caps, Features, MaybeSend, RhiError};

use crate::context::{PrepareCtx, RecordCtx, SetupCtx};
use crate::view::ViewCtx;

// A role has one active holder per renderer; a second claimant is disabled with F0102.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    Geometry,
    Shadows,
    TemporalResolve,
    Tonemap,
}

// Phase 1: known before the device exists, so the device can be created with what packs need.
#[derive(Clone, Default)]
pub struct Requirements {
    pub backend: Option<Backend>,
    pub required_features: Features,
    pub optional_features: Features,
    pub extensions: Vec<Arc<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for Requirements {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Requirements")
            .field("backend", &self.backend)
            .field("required_features", &self.required_features)
            .field("optional_features", &self.optional_features)
            .field("extensions", &self.extensions.len())
            .finish()
    }
}

// Phase 2: decided on the real `Caps`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Negotiation {
    Full,
    Fallback { reason: String },
    Disabled { reason: String },
}

pub trait RenderFeature: MaybeSend {
    fn name(&self) -> &'static str;

    fn stage(&self) -> Stage;

    fn priority(&self) -> i32 {
        0
    }

    fn before(&self) -> &[&'static str] {
        &[]
    }

    fn after(&self) -> &[&'static str] {
        &[]
    }

    fn role(&self) -> Option<Role> {
        None
    }

    fn requirements(&self) -> Requirements {
        Requirements::default()
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let _ = caps;
        Negotiation::Full
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let _ = ctx;
        Ok(())
    }

    fn prepare_view(&mut self, ctx: &mut PrepareCtx<'_>) {
        let _ = ctx;
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError>;

    fn on_resize(&mut self, view: &ViewCtx) {
        let _ = view;
    }

    // Every GPU handle the pack held is dead; recreate them.
    fn on_device_reset(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.setup(ctx)
    }
}

const _: Option<&dyn RenderFeature> = None;
