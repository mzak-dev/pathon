use std::collections::BTreeMap;
use std::ops::Range;

use wesl::Mangler;
use wesl::mangler::EscapeMangler;
use wesl::syntax::{GlobalDeclaration, GlobalDeclarationNode, ModulePath, TranslationUnit};

use crate::diagnostic::SourceLocation;
use crate::text::{self, Token, TokenKind};

#[derive(Clone, Debug)]
pub(crate) struct SourceFile<'a> {
    pub(crate) module: String,
    pub(crate) file: String,
    pub(crate) source: &'a str,
}

/// Every source a composition read, so a position in the composed text can be turned back into a file and a line.
#[derive(Debug)]
pub(crate) struct Sources<'a> {
    pub(crate) root: SourceFile<'a>,
    /// Keyed by the path as registered (`lighting::pbr`).
    pub(crate) modules: BTreeMap<String, SourceFile<'a>>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeclarationSpan {
    pub(crate) name: String,
    pub(crate) range: Range<usize>,
}

impl<'a> Sources<'a> {
    pub(crate) fn file_for_path(&self, path: &ModulePath) -> Option<&SourceFile<'a>> {
        if path.components.is_empty() {
            Some(&self.root)
        } else {
            self.modules.get(&path.components.join("::"))
        }
    }

    pub(crate) fn file_with_source(&self, source: &str) -> Option<&SourceFile<'a>> {
        std::iter::once(&self.root)
            .chain(self.modules.values())
            .find(|file| file.source == source)
    }

    pub(crate) fn module_paths(&self) -> impl Iterator<Item = String> {
        self.modules.keys().map(|path| format!("package::{path}"))
    }

    pub(crate) fn location(file: &SourceFile<'_>, range: &Range<usize>, exact: bool) -> SourceLocation {
        let (line, column, line_text) = text::position(file.source, range.start);
        let on_line = range
            .end
            .min(range.start + (line_text.len() + 1).saturating_sub(column as usize));
        let covered = file.source.get(range.start..on_line.max(range.start)).unwrap_or("");
        SourceLocation {
            module: file.module.clone(),
            file: file.file.clone(),
            line,
            column,
            length: if exact { text::char_count(covered.trim_end()) } else { 0 },
            line_text: line_text.to_owned(),
            exact,
        }
    }

    /// Where the composed text at `error` came from: the exact tokens when the declaration holding it can be lined up
    /// with its original, otherwise the name of that declaration (ADR-0032).
    pub(crate) fn locate_composed(&self, composed: &str, error: &Range<usize>) -> Option<SourceLocation> {
        let Some(composed_declaration) = declarations(composed)
            .into_iter()
            .find(|declaration| declaration.range.start <= error.start && error.start < declaration.range.end)
        else {
            return self.locate_directive(composed.get(error.clone())?);
        };
        let (file, item) = self.origin_of(&composed_declaration.name);
        let candidates: Vec<DeclarationSpan> = declarations(file.source)
            .into_iter()
            .filter(|declaration| declaration.name == item)
            .collect();
        let composed_tokens = declaration_tokens(composed, &composed_declaration.range);
        let original = candidates
            .iter()
            .find(|candidate| declaration_tokens(file.source, &candidate.range).len() == composed_tokens.len())
            .or_else(|| candidates.first())?;
        if let Some(range) = original_range(
            composed,
            error,
            &composed_declaration.range,
            file.source,
            &original.range,
        ) {
            return Some(Self::location(file, &range, true));
        }
        let name =
            declaration_name_range(file.source, original, &item).unwrap_or(original.range.start..original.range.start);
        Some(Self::location(file, &name, false))
    }

    // Directives (`enable`, `requires`, `diagnostic`) are outside every declaration and composition keeps their text.
    fn locate_directive(&self, text: &str) -> Option<SourceLocation> {
        let name = text.trim();
        if name.is_empty() {
            return None;
        }
        std::iter::once(&self.root)
            .chain(self.modules.values())
            .find_map(|file| Self::find_word(file, name, 0).map(|range| Self::location(file, &range, true)))
    }

    /// The file and original name of a declaration in the composed text: modules prefix their names (ADR-0021).
    pub(crate) fn origin_of(&self, composed_name: &str) -> (&SourceFile<'a>, String) {
        match unmangle(composed_name) {
            Some((module, item)) => match self.modules.get(&module) {
                Some(file) => (file, item),
                None => (&self.root, composed_name.to_owned()),
            },
            None => (&self.root, composed_name.to_owned()),
        }
    }

    /// Where `needle` first appears as a whole word, in the blamed file when it is there and else in the one file that
    /// has it: wesl blames the importing file for an undefined name inside a module.
    pub(crate) fn find_word_near(&self, blamed: Option<&SourceFile<'a>>, needle: &str) -> Option<SourceLocation> {
        let in_blamed = blamed.and_then(|file| Self::find_word(file, needle, 0).map(|range| (file, range)));
        let (file, range) = in_blamed.or_else(|| {
            let mut holders = std::iter::once(&self.root)
                .chain(self.modules.values())
                .filter_map(|file| Self::find_word(file, needle, 0).map(|range| (file, range)));
            let first = holders.next()?;
            holders.next().is_none().then_some(first)
        })?;
        Some(Self::location(file, &range, true))
    }

    /// The first occurrence of `needle` as a whole word in `file`, for errors wesl reports without a position.
    pub(crate) fn find_word(file: &SourceFile<'_>, needle: &str, from: usize) -> Option<Range<usize>> {
        let source = file.source;
        let mut search_from = from.min(source.len());
        while let Some(found) = source.get(search_from..)?.find(needle) {
            let start = search_from + found;
            let end = start + needle.len();
            let boundary = |character: Option<char>| character.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if boundary(source[..start].chars().next_back()) && boundary(source[end..].chars().next()) {
                return Some(start..end);
            }
            search_from = end;
        }
        None
    }
}

/// `(module path as registered, item)` for a name composition mangled.
pub(crate) fn unmangle(name: &str) -> Option<(String, String)> {
    EscapeMangler
        .unmangle(name)
        .map(|(path, item)| (path.components.join("::"), item))
}

/// The unmangled form of every mangled name in `message`, e.g. `package_colors_pack` to `colors::pack`.
pub(crate) fn unmangle_text(message: &str) -> String {
    let mut result = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(start) = rest.find("package_") {
        let (before, candidate) = rest.split_at(start);
        let preceded_by_word = before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let length = candidate
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(candidate.len());
        let (word, after) = candidate.split_at(length);
        result.push_str(before);
        match unmangle(word) {
            Some((module, item)) if !preceded_by_word => {
                result.push_str(&module);
                result.push_str("::");
                result.push_str(&item);
            }
            _ => result.push_str(word),
        }
        rest = after;
    }
    result.push_str(rest);
    result
}

pub(crate) fn declarations(source: &str) -> Vec<DeclarationSpan> {
    let mut spans = Vec::new();
    if let Ok(unit) = source.parse::<TranslationUnit>() {
        collect_declarations(&unit.global_declarations, &mut spans);
    }
    spans
}

fn collect_declarations(nodes: &[GlobalDeclarationNode], spans: &mut Vec<DeclarationSpan>) {
    for node in nodes {
        let name = match &**node {
            GlobalDeclaration::Declaration(declaration) => Some(declaration.ident.name().to_string()),
            GlobalDeclaration::TypeAlias(alias) => Some(alias.ident.name().to_string()),
            GlobalDeclaration::Struct(structure) => Some(structure.ident.name().to_string()),
            GlobalDeclaration::Function(function) => Some(function.ident.name().to_string()),
            GlobalDeclaration::Compound(compound) => {
                collect_declarations(&compound.body, spans);
                None
            }
            GlobalDeclaration::Void | GlobalDeclaration::ConstAssert(_) => None,
        };
        if let Some(name) = name {
            let span = node.span();
            spans.push(DeclarationSpan {
                name,
                range: span.start..span.end,
            });
        }
    }
}

fn declaration_tokens<'a>(source: &'a str, range: &Range<usize>) -> Vec<Token<'a>> {
    let slice = source.get(range.clone()).unwrap_or("");
    text::tokens(slice)
        .into_iter()
        .map(|token| Token {
            range: (token.range.0 + range.start, token.range.1 + range.start),
            ..token
        })
        .collect()
}

fn declaration_name_range(source: &str, declaration: &DeclarationSpan, name: &str) -> Option<Range<usize>> {
    declaration_tokens(source, &declaration.range)
        .into_iter()
        .find(|token| token.kind == TokenKind::Identifier && token.text == name)
        .map(|token| token.range())
}

fn same_token(composed: &Token<'_>, original: &Token<'_>) -> bool {
    if composed.kind != original.kind {
        return false;
    }
    match composed.kind {
        TokenKind::Punctuation => composed.text == original.text,
        TokenKind::Number => true,
        TokenKind::Identifier => {
            composed.text == original.text || unmangle(composed.text).is_some_and(|(_, item)| item == original.text)
        }
    }
}

/// Composition prints the syntax tree again, so a declaration keeps its tokens (with renamed identifiers) while
/// whitespace, comments and line breaks change. Equal token counts line the two texts up one to one; otherwise the
/// tokens under the error are searched for in the original, which holds when they are unique within the declaration.
fn original_range(
    composed: &str,
    error: &Range<usize>,
    composed_declaration: &Range<usize>,
    original: &str,
    original_declaration: &Range<usize>,
) -> Option<Range<usize>> {
    let composed_tokens = declaration_tokens(composed, composed_declaration);
    let original_tokens = declaration_tokens(original, original_declaration);
    let first = composed_tokens.iter().position(|token| token.range.1 > error.start)?;
    let last = composed_tokens
        .iter()
        .rposition(|token| token.range.0 < error.end.max(error.start + 1))?
        .max(first);
    let aligned = composed_tokens.len() == original_tokens.len()
        && composed_tokens
            .iter()
            .zip(&original_tokens)
            .all(|(c, o)| same_token(c, o));
    if aligned {
        return Some(original_tokens[first].range.0..original_tokens[last].range.1);
    }
    let needle = &composed_tokens[first..=last];
    let occurrences = |tokens: &[Token<'_>], same: &dyn Fn(&Token<'_>, &Token<'_>) -> bool| -> Vec<usize> {
        (0..tokens.len().saturating_sub(needle.len() - 1))
            .filter(|start| needle.iter().zip(&tokens[*start..]).all(|(n, t)| same(n, t)))
            .collect()
    };
    let in_composed = occurrences(&composed_tokens, &|n, t| n.text == t.text);
    let in_original = occurrences(&original_tokens, &|n, t| same_token(n, t));
    // Code removed by `@if` can hold the same tokens, so a count that differs means the match would be a guess.
    if in_composed.len() != in_original.len() {
        return None;
    }
    let start = *in_original.get(in_composed.iter().position(|start| *start == first)?)?;
    Some(original_tokens[start].range.0..original_tokens[start + needle.len() - 1].range.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmangle_text_restores_module_paths_and_leaves_other_words() {
        assert_eq!(
            unmangle_text("call to package_colors_pack failed"),
            "call to colors::pack failed"
        );
        assert_eq!(unmangle_text("package_cam__1unused_tex"), "cam::unused_tex");
        assert_eq!(unmangle_text("my_package_x"), "my_package_x");
    }
}
