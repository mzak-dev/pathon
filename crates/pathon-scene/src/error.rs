use pathon_api::Diagnostic;

#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SceneError {
    #[error("[N0001] {kind} handle is stale or unknown")]
    StaleHandle { kind: &'static str },
    #[error("[N0002] invalid camera: {reason}")]
    InvalidCamera { reason: &'static str },
    #[error("[N0003] invalid light: {reason}")]
    InvalidLight { reason: &'static str },
    #[error("[N0004] {what} is not finite")]
    NotFinite { what: &'static str },
}

impl SceneError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::StaleHandle { .. } => "N0001",
            Self::InvalidCamera { .. } => "N0002",
            Self::InvalidLight { .. } => "N0003",
            Self::NotFinite { .. } => "N0004",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            Self::StaleHandle { .. } => "do not use a handle after despawn; spawn again and keep the new handle",
            Self::InvalidCamera { .. } => {
                "use 0 < near < far and a vertical fov between 0 and pi (orthographic: height > 0)"
            }
            Self::InvalidLight { .. } => {
                "use non-negative finite intensity and colour, and 0 < inner <= outer <= pi/2 for spots"
            }
            Self::NotFinite { .. } => "check the host data for NaN or infinity before extracting",
        }
    }

    pub fn to_diagnostic(self) -> Diagnostic {
        Diagnostic::error(self.code(), self.to_string()).with_hint(self.hint())
    }
}

impl From<&SceneError> for Diagnostic {
    fn from(error: &SceneError) -> Self {
        (*error).to_diagnostic()
    }
}
