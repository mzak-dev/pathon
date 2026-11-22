use std::sync::Arc;

use pathon_api::graph::Stage;
use pathon_api::rhi::{Caps, Format, FormatCaps, RhiError};
use pathon_api::{Negotiation, RecordCtx, RenderFeature, SetupCtx};

use crate::{EquirectImage, IblBuilder, IblEnvironment, IblSettings};

/// The environment an application wants lit, put in `Shared`; a new `Arc` or other settings mean a new environment, so
/// keep one `Arc` per picture.
#[derive(Clone, Debug)]
pub struct IblSource {
    pub image: Arc<EquirectImage>,
    pub settings: IblSettings,
}

impl IblSource {
    pub fn new(image: Arc<EquirectImage>, settings: IblSettings) -> Self {
        Self { image, settings }
    }

    fn same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.image, &other.image) && self.settings == other.settings
    }
}

/// Builds the environment on the first frame after the [`IblSource`] in `Shared` changes and publishes it there as an
/// [`IblEnvironment`]; the work is not a graph node (docs/crates/pathon-ibl.md).
#[derive(Debug, Default)]
pub struct IblPack {
    builder: IblBuilder,
    built_from: Option<IblSource>,
    environment: Option<IblEnvironment>,
}

impl IblPack {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RenderFeature for IblPack {
    fn name(&self) -> &'static str {
        "ibl"
    }

    fn stage(&self) -> Stage {
        Stage::Setup
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let needed = FormatCaps::SAMPLED | FormatCaps::FILTERABLE | FormatCaps::STORAGE;
        if caps.format(Format::Rgba16Float).contains(needed) {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "Rgba16Float cannot be written by compute and filtered".into(),
            }
        }
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(source) = ctx.shared().get::<IblSource>().cloned() else {
            return Ok(());
        };
        if self.built_from.as_ref().is_some_and(|built| built.same_as(&source)) {
            return Ok(());
        }
        // A failed build is not retried every frame: the picture or the settings have to change first.
        self.built_from = Some(source.clone());
        match self
            .builder
            .build_environment(ctx.device(), &source.image, &source.settings)
        {
            Ok(environment) => {
                if let Some(previous) = self.environment.replace(environment) {
                    previous
                        .owned_resources()
                        .into_iter()
                        .for_each(|resource| ctx.release_later(resource));
                }
                ctx.shared_mut().insert(environment);
            }
            Err(error) => ctx.report(error.to_diagnostic()),
        }
        Ok(())
    }

    fn on_device_reset(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.builder = IblBuilder::new();
        self.built_from = None;
        self.environment = None;
        ctx.shared().remove::<IblEnvironment>();
        Ok(())
    }
}
