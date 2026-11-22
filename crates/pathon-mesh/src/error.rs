use std::fmt;

use pathon_api::Diagnostic;
use pathon_api::rhi::RhiError;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ArenaKind {
    Vertices,
    Indices,
    DeformedVertices,
}

impl fmt::Display for ArenaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Vertices => "vertices",
            Self::Indices => "indices",
            Self::DeformedVertices => "deformed vertices",
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MeshError {
    #[error("[M0001] invalid vertex layout: {reason}")]
    InvalidLayout { reason: String },
    #[error("[M0002] mesh '{label}' does not match the pool layout: {reason}")]
    InvalidData { label: &'static str, reason: String },
    #[error(
        "[M0003] pool '{pool}' has no room for {requested} {kind}: {free} free, {pending} more wait for frames in flight"
    )]
    Exhausted {
        pool: &'static str,
        kind: ArenaKind,
        requested: u64,
        free: u64,
        pending: u64,
    },
    #[error(
        "[M0004] pool '{pool}' has {free} {kind} free but the largest contiguous range is {largest_free}, \
         {requested} needed"
    )]
    Fragmented {
        pool: &'static str,
        kind: ArenaKind,
        requested: u64,
        free: u64,
        largest_free: u64,
    },
    #[error("[M0005] mesh '{label}' needs {requested} {kind} in one buffer, at most {max} fit")]
    TooLarge {
        label: &'static str,
        kind: ArenaKind,
        requested: u64,
        max: u64,
    },
    #[error("[M0006] unknown or removed mesh")]
    UnknownMesh,
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

impl MeshError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidLayout { .. } => "M0001",
            Self::InvalidData { .. } => "M0002",
            Self::Exhausted { .. } => "M0003",
            Self::Fragmented { .. } => "M0004",
            Self::TooLarge { .. } => "M0005",
            Self::UnknownMesh => "M0006",
            Self::Rhi(error) => error.code(),
        }
    }

    fn hint(&self) -> Option<&'static str> {
        match self {
            Self::InvalidLayout { .. } => Some("start from VertexLayout::standard() or give Position as Float32x3"),
            Self::InvalidData { .. } => Some("build the streams with the layout the pool was created with"),
            Self::Exhausted { .. } => Some("remove unused meshes or raise the budget in PoolDesc"),
            Self::Fragmented { .. } => {
                Some("raise the budget in PoolDesc so a new chunk can open, or upload large meshes first")
            }
            Self::TooLarge { .. } => Some("split the mesh into smaller parts"),
            Self::UnknownMesh => Some("do not use a MeshId after remove or forget_gpu"),
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

impl From<&MeshError> for Diagnostic {
    fn from(error: &MeshError) -> Self {
        error.to_diagnostic()
    }
}
