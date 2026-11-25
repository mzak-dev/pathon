use pathon_api::Diagnostic;
use pathon_api::rhi::RhiError;

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SkinError {
    #[error("[W0001] invalid skinned instance: {reason}")]
    InvalidInstance { reason: String },
    #[error("[W0002] invalid morph targets '{label}': {reason}")]
    InvalidMorph { label: &'static str, reason: String },
    #[error("[W0003] deformed output '{label}' does not fit: {reason}")]
    InvalidTargets { label: &'static str, reason: String },
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

impl SkinError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInstance { .. } => "W0001",
            Self::InvalidMorph { .. } => "W0002",
            Self::InvalidTargets { .. } => "W0003",
            Self::Rhi(error) => error.code(),
        }
    }

    fn hint(&self) -> Option<&'static str> {
        match self {
            Self::InvalidInstance { .. } => Some(
                "the deformed range holds exactly the mesh's vertices; bone and weight slices stay inside SkinFrame",
            ),
            Self::InvalidMorph { .. } => {
                Some("give every delta stream either no entries or one per vertex, all finite")
            }
            Self::InvalidTargets { .. } => {
                Some("create DeformedPositions and DeformedSurface with the same capacity, within the storage limit")
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
