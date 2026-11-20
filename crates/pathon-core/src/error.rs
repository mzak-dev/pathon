use pathon_diag::Diagnostic;
use pathon_rhi::RhiError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    #[error("device lost: call Renderer::recover() for an owned device or Renderer::reset() with a new one")]
    DeviceLost,
    #[error("render target does not exist")]
    UnknownTarget,
    #[error("record_frame renders into offscreen or imported targets only; window surfaces need render_frame")]
    SurfaceInRecordFrame,
    #[error("only a renderer that created its device can recover it; reset() with the host's new device instead")]
    BorrowedDevice,
    #[error(transparent)]
    Rhi(RhiError),
    #[error("{0}")]
    Diagnostic(Diagnostic),
}

impl From<RhiError> for RenderError {
    fn from(error: RhiError) -> Self {
        match error {
            RhiError::DeviceLost => Self::DeviceLost,
            other => Self::Rhi(other),
        }
    }
}
