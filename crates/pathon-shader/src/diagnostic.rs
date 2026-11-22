use std::fmt;

use pathon_api::{Diagnostic, Severity};

pub(crate) const SYNTAX_ERROR: &str = "S0101";
pub(crate) const UNKNOWN_MODULE: &str = "S0102";
pub(crate) const UNKNOWN_IMPORT: &str = "S0103";
pub(crate) const CYCLIC_DECLARATIONS: &str = "S0104";
pub(crate) const INVALID_CONDITIONAL: &str = "S0105";
pub(crate) const SUSPICIOUS_FLAG: &str = "S0106";
pub(crate) const UNKNOWN_NAME: &str = "S0107";
pub(crate) const INVALID_SHADER: &str = "S0108";
pub(crate) const INVALID_BINDING: &str = "S0109";
pub(crate) const UNKNOWN_OVERRIDE: &str = "S0110";
pub(crate) const COMPOSITION_ERROR: &str = "S0111";
pub(crate) const CACHE_REJECTED: &str = "S0201";
pub(crate) const CACHE_NOT_SAVED: &str = "S0202";

/// A place in the source a developer wrote: the module's own file and line, not the composed text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SourceLocation {
    /// `package::lighting` for a module, the shader's file label for the root.
    pub module: String,
    /// The file label given when the source was registered; the module path when none was.
    pub file: String,
    /// 1-based.
    pub line: u32,
    /// 1-based, in characters.
    pub column: u32,
    /// In characters; 0 when only the line is known.
    pub length: u32,
    pub line_text: String,
    /// `false` when only the declaration holding the problem could be found: composition rewrites the text, so the
    /// line is where that declaration starts (ADR-0032).
    pub exact: bool,
}

/// What went wrong in shader source, where, and what to try; renders like a rustc diagnostic.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct ShaderDiagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub location: Option<SourceLocation>,
    pub hint: Option<String>,
    /// Context that is not advice, e.g. which shader was being composed.
    pub notes: Vec<String>,
}

impl ShaderDiagnostic {
    pub(crate) fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Error, message)
    }

    pub(crate) fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Warning, message)
    }

    fn new(code: &'static str, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
            location: None,
            hint: None,
            notes: Vec::new(),
        }
    }

    #[must_use]
    pub(crate) fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    #[must_use]
    pub(crate) fn at(self, location: Option<SourceLocation>) -> Self {
        Self { location, ..self }
    }

    #[must_use]
    pub(crate) fn with_hint(self, hint: impl Into<String>) -> Self {
        Self {
            hint: Some(hint.into()),
            ..self
        }
    }

    #[must_use]
    pub(crate) fn with_optional_hint(self, hint: Option<String>) -> Self {
        Self { hint, ..self }
    }

    /// The same problem for `pathon-diag`: location and source line go into the message, so the logged text and
    /// `Diagnostic::to_string` show them without the diagnostic type knowing about shaders.
    #[must_use]
    pub fn to_diagnostic(&self) -> Diagnostic {
        let mut message = match &self.location {
            Some(location) => format!(
                "{}:{}:{}: {}\n{}",
                location.file,
                location.line,
                location.column,
                self.message,
                snippet(location)
            ),
            None => self.message.clone(),
        };
        for note in &self.notes {
            message.push_str("\nnote: ");
            message.push_str(note);
        }
        let diagnostic = match self.severity {
            Severity::Error => Diagnostic::error(self.code, message),
            Severity::Warning => Diagnostic::warning(self.code, message),
            Severity::Note => Diagnostic::note(self.code, message),
        };
        match &self.hint {
            Some(hint) => diagnostic.with_hint(hint.clone()),
            None => diagnostic,
        }
    }
}

fn snippet(location: &SourceLocation) -> String {
    let number = location.line.to_string();
    let gutter = " ".repeat(number.len());
    let source_line = format!("{gutter} |\n{number} | {}", location.line_text);
    if !location.exact {
        return source_line;
    }
    let padding: String = location
        .line_text
        .chars()
        .take(location.column.saturating_sub(1) as usize)
        .map(|character| if character == '\t' { '\t' } else { ' ' })
        .collect();
    let carets = "^".repeat(location.length.max(1) as usize);
    format!("{source_line}\n{gutter} | {padding}{carets}")
}

impl fmt::Display for ShaderDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        write!(f, "{severity}[{}]: {}", self.code, self.message)?;
        if let Some(location) = &self.location {
            let gutter = " ".repeat(location.line.to_string().len());
            write!(
                f,
                "\n{gutter}--> {}:{}:{}",
                location.file, location.line, location.column
            )?;
            if location.module != location.file {
                write!(f, " (in `{}`)", location.module)?;
            }
            write!(f, "\n{}", snippet(location))?;
            if !location.exact {
                write!(
                    f,
                    "\n{gutter} = note: composition rewrote this declaration, so the line shown is where it starts"
                )?;
            }
        }
        for note in &self.notes {
            write!(f, "\n  = note: {note}")?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\n  = hint: {hint}")?;
        }
        write!(f, "\n  = docs: docs/diag/{}.md", self.code)
    }
}

impl std::error::Error for ShaderDiagnostic {}
