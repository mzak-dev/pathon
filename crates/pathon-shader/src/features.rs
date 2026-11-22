use std::collections::BTreeSet;

use crate::diagnostic::{SUSPICIOUS_FLAG, ShaderDiagnostic};
use crate::locate::Sources;
use crate::{RESERVED_SEGMENTS, ShaderError, is_identifier, text};

/// The set of enabled WESL flags (ASCII identifiers that are not WGSL/WESL reserved words); a disabled flag is simply
/// absent, so equal permutations compare equal.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ShaderFeatures {
    enabled: BTreeSet<String>,
}

impl ShaderFeatures {
    pub fn with(mut self, name: &str) -> Result<Self, ShaderError> {
        self.set(name, true)?;
        Ok(self)
    }

    pub fn set(&mut self, name: &str, enabled: bool) -> Result<(), ShaderError> {
        if !is_identifier(name) || RESERVED_SEGMENTS.contains(&name) {
            return Err(ShaderError::InvalidFeatureName { name: name.to_owned() });
        }
        if enabled {
            self.enabled.insert(name.to_owned());
        } else {
            self.enabled.remove(name);
        }
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.enabled.iter().map(String::as_str)
    }
}

/// Names used in `@if(...)` and `@elif(...)` of `source`.
pub(crate) fn used_flags(source: &str) -> BTreeSet<String> {
    let tokens = text::tokens(source);
    let mut flags = BTreeSet::new();
    for (index, window) in tokens.windows(3).enumerate() {
        let is_condition = window[0].text == "@" && matches!(window[1].text, "if" | "elif") && window[2].text == "(";
        if !is_condition {
            continue;
        }
        let mut depth = 1_usize;
        for token in &tokens[index + 3..] {
            match token.text {
                "(" => depth += 1,
                ")" => depth -= 1,
                name if token.kind == text::TokenKind::Identifier && !matches!(name, "true" | "false") => {
                    flags.insert(name.to_owned());
                }
                _ => {}
            }
            if depth == 0 {
                break;
            }
        }
    }
    flags
}

/// A flag nobody's `@if` reads does nothing, which is how a misspelled flag fails: the permutation silently takes
/// the `@else` branch (ADR-0023). Warns for each enabled flag no source reads.
pub(crate) fn flag_warnings(sources: &Sources<'_>, features: &ShaderFeatures) -> Vec<ShaderDiagnostic> {
    let used: BTreeSet<String> = std::iter::once(&sources.root)
        .chain(sources.modules.values())
        .flat_map(|file| used_flags(file.source))
        .collect();
    features
        .iter()
        .filter(|flag| !used.contains(*flag))
        .map(|flag| {
            let hint = match text::closest(flag, used.iter().map(String::as_str)) {
                Some(found) => format!("did you mean `{found}`?"),
                None if used.is_empty() => "no `@if` in the shader or its modules reads a flag".to_owned(),
                None => format!("flags read by `@if`: {}", text::list_of(&used, 8)),
            };
            ShaderDiagnostic::warning(
                SUSPICIOUS_FLAG,
                format!("flag `{flag}` is enabled but no `@if` reads it"),
            )
            .with_note(format!("composing `{}`", sources.root.file))
            .with_hint(hint)
        })
        .collect()
}
