use pathon_api::Diagnostic;
use pathon_api::rhi::RhiError;

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TextureError {
    #[error("[X0001] texture '{label}' has no variant this device can sample (offered: {offered})")]
    NoSupportedVariant { label: &'static str, offered: String },
    #[error("[X0002] texture '{label}' is invalid: {reason}")]
    InvalidSource { label: &'static str, reason: String },
    #[error("[X0003] texture '{label}' is {width}x{height}, the device allows {max}x{max}")]
    TooLarge {
        label: &'static str,
        width: u32,
        height: u32,
        max: u32,
    },
    #[error("[X0004] KTX2 container: {reason}")]
    Container { reason: String },
    #[error("[X0005] texture '{label}': cannot generate mips: {reason}")]
    MipGeneration { label: &'static str, reason: String },
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

impl TextureError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoSupportedVariant { .. } => "X0001",
            Self::InvalidSource { .. } => "X0002",
            Self::TooLarge { .. } => "X0003",
            Self::Container { .. } => "X0004",
            Self::MipGeneration { .. } => "X0005",
            Self::Rhi(error) => error.code(),
        }
    }

    fn hint(&self) -> Option<&'static str> {
        match self {
            Self::NoSupportedVariant { .. } => Some(
                "ship an uncompressed RGBA8 variant next to the compressed ones, or add the encoding this GPU samples",
            ),
            Self::InvalidSource { .. } => {
                Some("check the size, the layer count and the byte length of every mip level")
            }
            Self::TooLarge { .. } => Some("downscale the texture offline or ship a smaller mip chain"),
            Self::Container { .. } => Some("re-export the file as KTX2 without supercompression"),
            Self::MipGeneration { .. } => {
                Some("use RGBA8 or RGBA16F, supply the mip chain yourself, or call without_mip_generation()")
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

impl From<&TextureError> for Diagnostic {
    fn from(error: &TextureError) -> Self {
        error.to_diagnostic()
    }
}
