use std::error::Error;
use std::fmt;

use crate::caps::Features;

#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum RhiError {
    NoAdapter(String),
    DeviceCreation(String),
    MissingFeatures(Features),
    InvalidHandle(&'static str),
    Validation { code: &'static str, message: String },
    SurfaceLost,
    SurfaceUnavailable,
    DeviceLost,
    OutOfMemory,
    Unsupported(&'static str),
    Backend(String),
}

impl RhiError {
    pub fn validation(code: &'static str, message: impl Into<String>) -> Self {
        Self::Validation {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAdapter(_) | Self::DeviceCreation(_) => "R0001",
            Self::MissingFeatures(_) => "R0002",
            Self::InvalidHandle(_) => "R0003",
            Self::SurfaceLost => "R0004",
            Self::SurfaceUnavailable => "R0005",
            Self::DeviceLost => "R0006",
            Self::OutOfMemory => "R0007",
            Self::Unsupported(_) => "R0008",
            Self::Backend(_) => "R0009",
            Self::Validation { code, .. } => code,
        }
    }
}

impl fmt::Display for RhiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            Self::NoAdapter(reason) => write!(f, "[{code}] no GPU adapter: {reason}"),
            Self::DeviceCreation(reason) => write!(f, "[{code}] device creation failed: {reason}"),
            Self::MissingFeatures(features) => write!(f, "[{code}] adapter lacks required features: {features:?}"),
            Self::InvalidHandle(kind) => write!(f, "[{code}] invalid or destroyed {kind} handle"),
            Self::Validation { message, .. } => write!(f, "[{code}] {message}"),
            Self::SurfaceLost => write!(f, "[{code}] surface lost"),
            Self::SurfaceUnavailable => write!(f, "[{code}] surface image not available this frame"),
            Self::DeviceLost => write!(f, "[{code}] device lost"),
            Self::OutOfMemory => write!(f, "[{code}] out of GPU memory"),
            Self::Unsupported(what) => write!(f, "[{code}] unsupported: {what}"),
            Self::Backend(message) => write!(f, "[{code}] backend error: {message}"),
        }
    }
}

impl Error for RhiError {}
