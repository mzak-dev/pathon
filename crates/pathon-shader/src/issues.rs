use std::ops::Range;

use naga::front::wgsl;
use naga::valid::{Capabilities, EntryPointError, ModuleInfo, ValidationError, ValidationFlags, Validator};
use naga::{Module, Span};
use wesl::Error as WeslError;
use wesl::error::{CondCompError, Detail, ImportError, ResolveError, UsageError, ValidateError};
use wesl::syntax::ModulePath;

use crate::diagnostic::{
    COMPOSITION_ERROR, CYCLIC_DECLARATIONS, INVALID_BINDING, INVALID_CONDITIONAL, INVALID_SHADER, SYNTAX_ERROR,
    ShaderDiagnostic, SourceLocation, UNKNOWN_IMPORT, UNKNOWN_MODULE, UNKNOWN_NAME,
};
use crate::locate::{SourceFile, Sources, declarations, unmangle_text};
use crate::text::{closest, list_of};

const LISTED: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IssueKind {
    Syntax,
    UnknownName,
    Binding,
    Other,
}

/// A problem naga found in WGSL text, before it is tied to any file.
#[derive(Clone, Debug)]
pub(crate) struct NagaIssue {
    pub(crate) kind: IssueKind,
    pub(crate) message: String,
    pub(crate) range: Option<Range<usize>>,
    pub(crate) hint: Option<String>,
}

pub(crate) fn parse_and_validate(source: &str) -> Result<(Module, ModuleInfo), NagaIssue> {
    let module = wgsl::parse_str(source).map_err(|error| {
        let message = plain_unknown_identifier(error.message());
        let kind = if message.starts_with("cannot find") || message.starts_with("unknown ") {
            IssueKind::UnknownName
        } else if message.starts_with("expected") || message.starts_with("unexpected") {
            IssueKind::Syntax
        } else {
            IssueKind::Other
        };
        NagaIssue {
            kind,
            range: error.labels().find_map(|(span, _)| span.to_range()),
            hint: error.notes().next().map(str::to_owned),
            message,
        }
    })?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map_err(|error| {
            let collision = matches!(
                error.as_inner(),
                ValidationError::EntryPoint {
                    source: EntryPointError::BindingCollision(_),
                    ..
                }
            );
            let binding = collision || matches!(error.as_inner(), ValidationError::GlobalVariable { .. });
            let message = error_chain(error.as_inner());
            let (message, hint) = if collision {
                (
                    "this resource has the same `@group` and `@binding` as another one the entry point uses".to_owned(),
                    Some("give every resource of a bind group its own `@binding` number".to_owned()),
                )
            } else if message.contains("Binding decoration is missing") {
                (
                    message,
                    Some("uniform, storage and texture resources need `@group(g) @binding(b)`".to_owned()),
                )
            } else {
                (message, None)
            };
            NagaIssue {
                kind: if binding { IssueKind::Binding } else { IssueKind::Other },
                // naga lists the enclosing function first and the offending expression last.
                range: error
                    .spans()
                    .filter_map(|(span, _): &(Span, String)| span.to_range())
                    .min_by_key(Range::len),
                hint,
                message,
            }
        })?;
    Ok((module, info))
}

// "no definition in scope for identifier: `x`" reads as naga's internals; say what the developer sees.
fn plain_unknown_identifier(message: &str) -> String {
    message
        .strip_prefix("no definition in scope for identifier: ")
        .map_or_else(
            || message.to_owned(),
            |name| format!("cannot find {name} in this scope"),
        )
}

// naga nests the cause under the place it was found ("Function 'f' is invalid: ..."), and prints arena handles.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![strip_handles(&error.to_string())];
    let mut source = error.source();
    while let Some(next) = source {
        parts.push(strip_handles(&next.to_string()));
        source = next.source();
    }
    plain_types(&parts.join(": "))
}

// naga prints types as its own Debug output: "(of type Scalar(Scalar { kind: Float, width: 4 }))".
fn plain_types(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("(of type ") {
        result.push_str(&rest[..start]);
        let inner = &rest[start + "(of type ".len()..];
        let mut depth = 1_usize;
        let end = inner
            .char_indices()
            .find(|(_, character)| {
                match character {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map_or(inner.len(), |(index, _)| index);
        result.push_str(&inner[..end]);
        rest = inner.get(end + 1..).unwrap_or("");
    }
    result.push_str(rest);
    [
        ("Scalar(Scalar { kind: Float, width: 4 })", "f32"),
        ("Scalar(Scalar { kind: Float, width: 2 })", "f16"),
        ("Scalar(Scalar { kind: Float, width: 8 })", "f64"),
        ("Scalar(Scalar { kind: Uint, width: 4 })", "u32"),
        ("Scalar(Scalar { kind: Sint, width: 4 })", "i32"),
        ("Scalar(Scalar { kind: Bool, width: 1 })", "bool"),
    ]
    .iter()
    .fold(result, |text, (debug, name)| text.replace(debug, name))
}

fn strip_handles(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(" [") {
        let after = &rest[start + 2..];
        match after.find(']') {
            Some(end) if after[..end].chars().all(|c| c.is_ascii_digit()) && end > 0 => {
                result.push_str(&rest[..start]);
                rest = &after[end + 1..];
            }
            _ => {
                result.push_str(&rest[..start + 2]);
                rest = after;
            }
        }
    }
    result.push_str(rest);
    result
}

/// A naga problem in `wgsl` as a diagnostic: located in the original files when `sources` is given, in `wgsl` itself
/// when it is the text the developer wrote.
pub(crate) fn naga_diagnostic(issue: &NagaIssue, wgsl: &str, sources: Option<&Sources<'_>>) -> ShaderDiagnostic {
    let message = unmangle_text(&issue.message);
    let range = issue.range.clone().filter(|range| range.start <= wgsl.len());
    let code = match issue.kind {
        IssueKind::Syntax => SYNTAX_ERROR,
        IssueKind::UnknownName => UNKNOWN_NAME,
        IssueKind::Binding => INVALID_BINDING,
        IssueKind::Other => INVALID_SHADER,
    };
    let mut diagnostic = ShaderDiagnostic::error(code, message).with_optional_hint(issue.hint.clone());
    let Some(range) = range else {
        return diagnostic;
    };
    let location = if let Some(sources) = sources {
        sources.locate_composed(wgsl, &range)
    } else {
        let file = SourceFile {
            module: "wgsl".to_owned(),
            file: "wgsl".to_owned(),
            source: wgsl,
        };
        Some(Sources::location(&file, &range, true))
    };
    if issue.kind == IssueKind::UnknownName {
        diagnostic = diagnostic.with_optional_hint(unknown_name_hint(wgsl, &range).or_else(|| issue.hint.clone()));
    }
    if location.is_none() {
        let (line, column, _) = crate::text::position(wgsl, range.start);
        diagnostic = diagnostic.with_note(format!("the problem is at {line}:{column} of the composed text"));
    }
    diagnostic.at(location)
}

fn unknown_name_hint(wgsl: &str, range: &Range<usize>) -> Option<String> {
    let name = wgsl.get(range.clone())?;
    let identifiers: Vec<String> = crate::text::tokens(wgsl)
        .into_iter()
        .filter(|token| {
            token.kind == crate::text::TokenKind::Identifier && !crate::RESERVED_SEGMENTS.contains(&token.text)
        })
        .map(|token| unmangle_text(token.text))
        .collect();
    closest(name, identifiers.iter().map(String::as_str)).map(|found| format!("did you mean `{found}`?"))
}

/// A wesl error as a diagnostic, located in the file the developer wrote.
pub(crate) fn wesl_diagnostic(error: &WeslError, sources: &Sources<'_>) -> ShaderDiagnostic {
    match error {
        WeslError::Error(diagnostic) => classify(&diagnostic.error, Some(&diagnostic.detail), sources),
        other => classify(other, None, sources),
    }
}

fn classify(error: &WeslError, detail: Option<&Detail>, sources: &Sources<'_>) -> ShaderDiagnostic {
    match error {
        WeslError::Error(diagnostic) => classify(&diagnostic.error, Some(&diagnostic.detail), sources),
        WeslError::ParseError(parse) => syntax_error(&parse.error.to_string(), &parse.span.range(), detail, sources),
        WeslError::ResolveError(resolve) | WeslError::ImportError(ImportError::ResolveError(resolve)) => {
            resolve_error(resolve, detail, sources)
        }
        WeslError::ImportError(import) => {
            let ImportError::DuplicateSymbol(name) = import else {
                return generic(&import.to_string(), detail, sources);
            };
            let location = detail_file(detail, sources)
                .and_then(|file| Sources::find_word(file, name, 0).map(|range| Sources::location(file, &range, true)));
            ShaderDiagnostic::error(
                COMPOSITION_ERROR,
                format!("`{name}` is declared or imported more than once"),
            )
            .with_hint("rename one of the declarations or import it under another name (`import a::b as c;`)")
            .at(location)
        }
        WeslError::UsageError(usage) => usage_error(usage, detail, sources),
        WeslError::ValidateError(validate) => validate_error(validate, detail, sources),
        WeslError::CondCompError(condition) => conditional_error(condition, detail, sources),
        other => generic(&other.to_string(), detail, sources),
    }
}

fn detail_file<'s, 'a>(detail: Option<&Detail>, sources: &'s Sources<'a>) -> Option<&'s SourceFile<'a>> {
    let detail = detail?;
    detail
        .module_path
        .as_ref()
        .and_then(|path| sources.file_for_path(path))
        .filter(|file| detail.source.as_deref().is_none_or(|source| file.source == source))
        .or_else(|| {
            detail
                .source
                .as_deref()
                .and_then(|source| sources.file_with_source(source))
        })
}

fn generic(message: &str, detail: Option<&Detail>, sources: &Sources<'_>) -> ShaderDiagnostic {
    let location = detail
        .and_then(|detail| detail.span.map(|span| span.range()))
        .and_then(|range| detail_file(detail, sources).map(|file| Sources::location(file, &range, true)));
    ShaderDiagnostic::error(COMPOSITION_ERROR, unmangle_text(message)).at(location)
}

fn syntax_error(
    message: &str,
    range: &Range<usize>,
    detail: Option<&Detail>,
    sources: &Sources<'_>,
) -> ShaderDiagnostic {
    let (summary, hint) = describe_syntax_error(message);
    let location = detail_file(detail, sources).map(|file| Sources::location(file, range, true));
    ShaderDiagnostic::error(SYNTAX_ERROR, summary)
        .with_optional_hint(hint)
        .at(location)
}

// "unexpected token `{`, expected `\"@\"`, `IdentPatternToken`, ..." lists parser terminals; show them as words.
fn describe_syntax_error(message: &str) -> (String, Option<String>) {
    let Some((found, expected)) = message.split_once(", expected `") else {
        return (message.to_owned(), None);
    };
    let found = found
        .strip_prefix("unexpected token ")
        .map_or_else(|| found.to_owned(), |token| format!("unexpected {token}"));
    let mut words: Vec<String> = Vec::new();
    for terminal in expected.trim_end_matches('`').split(", ") {
        let word = match terminal.trim_matches('"') {
            "IdentPatternToken" => "an identifier".to_owned(),
            "ReservedWord" => continue,
            literal if literal.starts_with("Tok") => "a literal".to_owned(),
            other => format!("`{other}`"),
        };
        if !words.contains(&word) {
            words.push(word);
        }
    }
    let hint = if words.is_empty() {
        None
    } else if words.len() > 6 {
        Some("the parser expected an expression, a statement or a declaration here; check the line above for a missing `;`, `)` or `}`".to_owned())
    } else {
        Some(format!("expected one of: {}", words.join(", ")))
    };
    (found, hint)
}

fn resolve_error(error: &ResolveError, detail: Option<&Detail>, sources: &Sources<'_>) -> ShaderDiagnostic {
    match error {
        ResolveError::ModuleNotFound(path, _) => unknown_module(path, detail, sources),
        ResolveError::Error(diagnostic) => classify(&diagnostic.error, Some(&diagnostic.detail), sources),
        other => generic(&other.to_string(), detail, sources),
    }
}

fn unknown_module(path: &ModulePath, detail: Option<&Detail>, sources: &Sources<'_>) -> ShaderDiagnostic {
    let wanted = path.to_string();
    let location = detail_file(detail, sources).and_then(|file| {
        let range = file
            .source
            .find(&wanted)
            .map(|start| start..start + wanted.len())
            .or_else(|| {
                path.components
                    .last()
                    .and_then(|last| Sources::find_word(file, last, 0))
            })?;
        Some(Sources::location(file, &range, true))
    });
    let registered: Vec<String> = sources.module_paths().collect();
    let hint = match closest(&wanted, registered.iter().map(String::as_str)) {
        Some(found) => format!("did you mean `{found}`?"),
        None if registered.is_empty() => {
            "no modules are registered; register the file with `add_module` before composing".to_owned()
        }
        None => format!("registered modules: {}", list_of(&registered, LISTED)),
    };
    ShaderDiagnostic::error(UNKNOWN_MODULE, format!("module `{wanted}` is not registered"))
        .with_hint(hint)
        .at(location)
}

fn usage_error(error: &UsageError, detail: Option<&Detail>, sources: &Sources<'_>) -> ShaderDiagnostic {
    let UsageError::NotFound(path, name) = error else {
        return ShaderDiagnostic::error(UNKNOWN_IMPORT, unmangle_text(&error.to_string()));
    };
    let wanted = path.to_string();
    let importer = detail_file(detail, sources);
    let location = importer.and_then(|file| {
        let after_module = file.source.find(&wanted).map_or(0, |start| start + wanted.len());
        let range = Sources::find_word(file, name, after_module).or_else(|| Sources::find_word(file, name, 0))?;
        Some(Sources::location(file, &range, true))
    });
    let declared: Vec<String> = sources.file_for_path(path).map_or_default(|file| {
        declarations(file.source)
            .into_iter()
            .map(|declaration| declaration.name)
            .collect()
    });
    let hint = match closest(name, declared.iter().map(String::as_str)) {
        Some(found) => format!("did you mean `{found}`?"),
        None if declared.is_empty() => format!("`{wanted}` declares nothing that can be imported"),
        None => format!("`{wanted}` declares: {}", list_of(&declared, LISTED)),
    };
    ShaderDiagnostic::error(UNKNOWN_IMPORT, format!("module `{wanted}` has no declaration `{name}`"))
        .with_hint(hint)
        .at(location)
}

fn validate_error(error: &ValidateError, detail: Option<&Detail>, sources: &Sources<'_>) -> ShaderDiagnostic {
    match error {
        ValidateError::Cycle(declaration, through) => {
            let location = declaration_location(detail, sources, 0);
            ShaderDiagnostic::error(
                CYCLIC_DECLARATIONS,
                format!(
                    "`{}` depends on itself through `{}`",
                    unmangle_text(declaration),
                    unmangle_text(through)
                ),
            )
            .with_hint("WGSL has no recursion: move what both declarations need into a third module")
            .at(location)
        }
        ValidateError::Duplicate(name) => {
            ShaderDiagnostic::error(COMPOSITION_ERROR, format!("`{name}` is declared twice"))
                .with_hint("rename one of the declarations")
                .at(declaration_location(detail, sources, 1))
        }
        ValidateError::UndefinedSymbol(name) => {
            let location = sources.find_word_near(detail_file(detail, sources), name);
            ShaderDiagnostic::error(UNKNOWN_NAME, format!("cannot find `{name}` in this scope")).at(location)
        }
        other => generic(&other.to_string(), detail, sources),
    }
}

// The name token of the `nth` declaration called like the one wesl blames.
fn declaration_location(detail: Option<&Detail>, sources: &Sources<'_>, nth: usize) -> Option<SourceLocation> {
    let file = detail_file(detail, sources)?;
    let name = detail?.declaration.as_deref()?;
    let declaration = declarations(file.source)
        .into_iter()
        .filter(|declaration| declaration.name == name)
        .nth(nth)
        .or_else(|| {
            declarations(file.source)
                .into_iter()
                .find(|declaration| declaration.name == name)
        })?;
    let range = Sources::find_word(file, name, declaration.range.start)?;
    Some(Sources::location(file, &range, true))
}

fn conditional_error(error: &CondCompError, detail: Option<&Detail>, sources: &Sources<'_>) -> ShaderDiagnostic {
    let hint = match error {
        CondCompError::DuplicateIf => {
            "combine the conditions in one attribute: `@if(a && b)`, or chain with `@elif` and `@else`"
        }
        CondCompError::NoPrecedingIf => {
            "`@elif` and `@else` must follow a declaration or statement with `@if` or `@elif`"
        }
        CondCompError::InvalidExpression(_) => "`@if` takes flag names combined with `!`, `&&`, `||` and parentheses",
        CondCompError::InvalidFeatureFlag(_) | CondCompError::UnexpectedFeatureFlag(_) => {
            "flags are WGSL identifiers; enable them in `ShaderFeatures`"
        }
    };
    let location = detail
        .and_then(|detail| detail.span.map(|span| span.range()))
        .and_then(|range| detail_file(detail, sources).map(|file| Sources::location(file, &range, true)));
    ShaderDiagnostic::error(INVALID_CONDITIONAL, error.to_string())
        .with_hint(hint)
        .at(location)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_stripped_from_naga_messages() {
        assert_eq!(strip_handles("Function [0] 'f' is invalid"), "Function 'f' is invalid");
        assert_eq!(strip_handles("array [x] stays"), "array [x] stays");
    }

    #[test]
    fn naga_type_debug_output_becomes_wgsl_type_names() {
        let message = "Operation Add can't work with (of type Scalar(Scalar { kind: Float, width: 4 })) and (of type Scalar(Scalar { kind: Uint, width: 4 }))";
        assert_eq!(plain_types(message), "Operation Add can't work with f32 and u32");
    }

    #[test]
    fn terminals_become_words() {
        let (message, hint) =
            describe_syntax_error("unexpected token `{`, expected `\"@\", \")\", IdentPatternToken, ReservedWord`");
        assert_eq!(message, "unexpected `{`");
        assert_eq!(hint.as_deref(), Some("expected one of: `@`, `)`, an identifier"));
    }
}
