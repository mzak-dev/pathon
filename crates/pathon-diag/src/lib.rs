#![doc = include_str!("../../../docs/crates/pathon-diag.md")]

mod codes;

use std::collections::HashSet;
use std::fmt;
use std::sync::{LazyLock, Mutex, MutexGuard};

pub use codes::{CODES, CodeInfo, is_registered};
use pathon_rhi_types::RhiError;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Error, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Warning, message)
    }

    pub fn note(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Note, message)
    }

    fn new(code: &'static str, severity: Severity, message: impl Into<String>) -> Self {
        debug_assert!(
            is_registered(code),
            "diagnostic code {code} has no entry in pathon_diag::CODES"
        );
        Self {
            code,
            severity,
            message: message.into(),
            hint: None,
        }
    }

    #[must_use]
    pub fn with_hint(self, hint: impl Into<String>) -> Self {
        Self {
            hint: Some(hint.into()),
            ..self
        }
    }
}

impl From<&RhiError> for Diagnostic {
    fn from(error: &RhiError) -> Self {
        Self::error(error.code(), error.to_string())
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        write!(f, "{severity}[{}]: {}", self.code, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  = hint: {hint}")?;
        }
        write!(f, "\n  = docs: docs/diag/{}.md", self.code)
    }
}

impl std::error::Error for Diagnostic {}

fn lock_reported() -> MutexGuard<'static, HashSet<Diagnostic>> {
    static REPORTED: LazyLock<Mutex<HashSet<Diagnostic>>> = LazyLock::new(Mutex::default);
    REPORTED
        .lock()
        .expect("the reported set is only touched by HashSet::insert, which does not panic")
}

// Every distinct diagnostic is logged once per process, so a per-frame condition does not flood the log (ADR-0007).
pub fn emit(diagnostic: &Diagnostic) {
    if !lock_reported().insert(diagnostic.clone()) {
        return;
    }
    match diagnostic.severity {
        Severity::Error => log::error!("{diagnostic}"),
        Severity::Warning => log::warn!("{diagnostic}"),
        Severity::Note => log::info!("{diagnostic}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn every_registered_code_has_a_documentation_page() {
        let pages = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/diag");
        let missing: Vec<&str> = CODES
            .iter()
            .map(|info| info.code)
            .filter(|code| !pages.join(format!("{code}.md")).exists())
            .collect();
        assert!(missing.is_empty(), "codes without a page in docs/diag: {missing:?}");
    }

    #[test]
    fn every_rhi_error_code_is_registered() {
        let errors = [
            RhiError::NoAdapter(String::new()),
            RhiError::MissingFeatures(pathon_rhi_types::Features::empty()),
            RhiError::InvalidHandle("buffer"),
            RhiError::SurfaceLost,
            RhiError::SurfaceUnavailable,
            RhiError::DeviceLost,
            RhiError::OutOfMemory,
            RhiError::Unsupported(""),
            RhiError::Backend(String::new()),
        ];
        for error in errors {
            assert!(is_registered(error.code()), "{} is not registered", error.code());
        }
    }

    #[test]
    fn codes_are_unique() {
        let unique: HashSet<&str> = CODES.iter().map(|info| info.code).collect();
        assert_eq!(unique.len(), CODES.len());
    }

    #[test]
    fn display_reads_like_rustc() {
        let text = Diagnostic::warning("F0102", "two features claim role Upscale")
            .with_hint("unregister one")
            .to_string();
        assert_eq!(
            text,
            "warning[F0102]: two features claim role Upscale\n  = hint: unregister one\n  = docs: docs/diag/F0102.md"
        );
    }
}
