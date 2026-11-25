use pathon_api::Diagnostic;
use pathon_api::rhi::RhiError;

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IblError {
    #[error("[I0001] invalid IBL settings: {reason}")]
    InvalidSettings { reason: String },
    #[error("[I0002] invalid environment image: {reason}")]
    InvalidImage { reason: String },
    #[error("[I0003] this device cannot build image-based lighting: {reason}")]
    Unsupported { reason: String },
    #[error("[I0004] environment image is {width}x{height}, the device allows {max}x{max}")]
    ImageTooLarge { width: u32, height: u32, max: u32 },
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

impl IblError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidSettings { .. } => "I0001",
            Self::InvalidImage { .. } => "I0002",
            Self::Unsupported { .. } => "I0003",
            Self::ImageTooLarge { .. } => "I0004",
            Self::Rhi(error) => error.code(),
        }
    }

    fn hint(&self) -> Option<&'static str> {
        match self {
            Self::InvalidSettings { .. } => Some("sizes are powers of two; start from IblSettings::DESKTOP or COMPACT"),
            Self::InvalidImage { .. } => {
                Some("pass width * height texels of finite, non-negative radiance (RGB or RGBA, f32 or f16)")
            }
            Self::Unsupported { .. } => {
                Some("the maps are written by compute into Rgba16Float: it needs STORAGE and FILTERABLE")
            }
            Self::ImageTooLarge { .. } => {
                Some("downscale the equirectangular image; 2048x1024 is enough for a 256 cube")
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

impl From<&IblError> for Diagnostic {
    fn from(error: &IblError) -> Self {
        error.to_diagnostic()
    }
}
