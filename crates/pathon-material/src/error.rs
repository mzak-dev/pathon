use pathon_api::Diagnostic;

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MaterialError {
    #[error("[Y0001] material parameter `{parameter}` is invalid: {reason}")]
    InvalidParameter { parameter: &'static str, reason: String },
    #[error("[Y0002] unknown or removed material {index}")]
    UnknownMaterial { index: u32 },
    #[error("[Y0003] the material shader modules do not compile with the given hooks: {message}")]
    InvalidHook { message: String },
}

impl MaterialError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidParameter { .. } => "Y0001",
            Self::UnknownMaterial { .. } => "Y0002",
            Self::InvalidHook { .. } => "Y0003",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            Self::InvalidParameter { .. } => "keep factors in 0..=1, colours finite and ior at 1 or more",
            Self::UnknownMaterial { .. } => "do not use a MaterialId after remove",
            Self::InvalidHook { .. } => {
                "a hook module must keep the signature of the default one (docs/crates/pathon-material.md)"
            }
        }
    }

    pub fn to_diagnostic(&self) -> Diagnostic {
        Diagnostic::error(self.code(), self.to_string()).with_hint(self.hint())
    }
}

impl From<&MaterialError> for Diagnostic {
    fn from(error: &MaterialError) -> Self {
        error.to_diagnostic()
    }
}
