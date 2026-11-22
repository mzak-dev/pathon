use pathon_api::Diagnostic;
use pathon_api::rhi::{Features, RhiError};

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BindlessError {
    #[error("[B0002] bindless textures were requested but the device cannot provide them: {reason}")]
    BindlessUnavailable { reason: String },
    #[error("[B0003] the texture table is full ({capacity} slots, {removed_waiting} of them waiting for reuse)")]
    TableFull { capacity: u32, removed_waiting: u32 },
    #[error("[B0004] texture handle (slot {index}) is stale or does not belong to this table")]
    StaleHandle { index: u32 },
    #[error("[B0005] material '{material}' uses {requested} textures, a per-material bind group holds {max}")]
    TooManyMaterialTextures {
        material: u32,
        requested: usize,
        max: usize,
    },
    #[error("[B0006] invalid texture table configuration: {reason}")]
    InvalidConfiguration { reason: String },
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

impl BindlessError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::BindlessUnavailable { .. } => "B0002",
            Self::TableFull { .. } => "B0003",
            Self::StaleHandle { .. } => "B0004",
            Self::TooManyMaterialTextures { .. } => "B0005",
            Self::InvalidConfiguration { .. } => "B0006",
            Self::Rhi(error) => error.code(),
        }
    }

    fn hint(&self) -> Option<&'static str> {
        match self {
            Self::BindlessUnavailable { .. } => Some(
                "use PathPreference::Auto to fall back to a bind group per material, or request TEXTURE_BINDING_ARRAY \
                 and NON_UNIFORM_INDEXING in the DeviceRequest",
            ),
            Self::TableFull { .. } => Some(
                "unregister textures that no material uses, or create the binder with a larger capacity (and \
                 recompile the texture source, its array length is part of the shader)",
            ),
            Self::StaleHandle { .. } => Some(
                "do not keep a TextureHandle after unregister; the slot is reused only after frames_in_flight frames, \
                 but the generation changes at once",
            ),
            Self::TooManyMaterialTextures { .. } => {
                Some("merge texture maps (e.g. pack occlusion with roughness and metalness)")
            }
            Self::InvalidConfiguration { .. } => {
                Some("use a capacity between 1 and MAX_CAPACITY and at least one frame in flight")
            }
            Self::Rhi(_) => None,
        }
    }

    pub fn to_diagnostic(&self) -> Diagnostic {
        let diagnostic = Diagnostic::error(self.code(), self.to_string());
        match self.hint() {
            Some(hint) => diagnostic.with_hint(hint),
            None => diagnostic,
        }
    }
}

impl From<&BindlessError> for Diagnostic {
    fn from(error: &BindlessError) -> Self {
        error.to_diagnostic()
    }
}

/// Why a device cannot index a texture array in a shader.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BindlessShortfall {
    MissingFeatures(Features),
    CapacityAboveLimit { requested: u32, limit: u32 },
}

impl std::fmt::Display for BindlessShortfall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFeatures(missing) => write!(f, "the device lacks {missing:?}"),
            Self::CapacityAboveLimit { requested, limit } => write!(
                f,
                "{requested} array elements requested, the device allows {limit} per shader stage"
            ),
        }
    }
}

/// `B0001`: bindless was not requested explicitly and the device cannot do it, so textures go through one bind group
/// per material.
pub(crate) fn fallback_diagnostic(shortfall: &BindlessShortfall) -> Diagnostic {
    Diagnostic::warning(
        "B0001",
        format!("binding arrays are unavailable ({shortfall}); textures use one bind group per material"),
    )
    .with_hint("expected on the Web profile; on desktop request TEXTURE_BINDING_ARRAY and NON_UNIFORM_INDEXING")
}
