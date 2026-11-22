use std::ops::Range;

/// 1-based line and column (in characters) of a byte offset, plus the text of that line.
pub(crate) fn position(source: &str, offset: usize) -> (u32, u32, &str) {
    let offset = offset.min(source.len());
    let offset = (0..=offset)
        .rev()
        .find(|index| source.is_char_boundary(*index))
        .unwrap_or(0);
    let line_start = source[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = source[offset..]
        .find('\n')
        .map_or(source.len(), |newline| offset + newline);
    let line = source[..line_start].matches('\n').count() + 1;
    let column = source[line_start..offset].chars().count() + 1;
    let text = source[line_start..line_end].trim_end_matches('\r');
    (saturating_u32(line), saturating_u32(column), text)
}

pub(crate) fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(crate) fn char_count(text: &str) -> u32 {
    saturating_u32(text.chars().count())
}

/// The candidate most like `name` when it is close enough to be a typo of it.
pub(crate) fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let allowed = name.chars().count().div_ceil(3).clamp(1, 3);
    candidates
        .into_iter()
        .filter(|candidate| *candidate != name)
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= allowed)
        .min_by(|(left, left_name), (right, right_name)| left.cmp(right).then(left_name.cmp(right_name)))
        .map(|(_, candidate)| candidate)
}

// Edit distance where swapping two neighbours is one edit, the commonest typing slip (`valeu` for `value`).
fn distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut before_previous: Vec<usize> = Vec::new();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.iter().enumerate() {
        let mut current = vec![row + 1];
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            let mut best = substitution.min(previous[column + 1] + 1).min(current[column] + 1);
            let swapped = row > 0 && column > 0 && left[row - 1] == *right_char && *left_char == right[column - 1];
            if swapped {
                best = best.min(before_previous[column - 1] + 1);
            }
            current.push(best);
        }
        before_previous = previous;
        previous = current;
    }
    previous[right.len()]
}

pub(crate) fn list_of(items: impl IntoIterator<Item = impl AsRef<str>>, limit: usize) -> String {
    let items: Vec<String> = items.into_iter().map(|item| format!("`{}`", item.as_ref())).collect();
    if items.len() > limit {
        format!("{}, and {} more", items[..limit].join(", "), items.len() - limit)
    } else {
        items.join(", ")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TokenKind {
    Identifier,
    Number,
    Punctuation,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Token<'a> {
    pub(crate) text: &'a str,
    pub(crate) kind: TokenKind,
    pub(crate) range: (usize, usize),
}

impl Token<'_> {
    pub(crate) fn range(&self) -> Range<usize> {
        self.range.0..self.range.1
    }
}

/// WGSL tokens without whitespace and comments; operators are split into single characters, which is enough to line
/// two spellings of the same code up.
pub(crate) fn tokens(source: &str) -> Vec<Token<'_>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
        } else if source[index..].starts_with("//") {
            index = source[index..].find('\n').map_or(bytes.len(), |end| index + end);
        } else if source[index..].starts_with("/*") {
            index = block_comment_end(source, index);
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let end = scan(bytes, index, |byte| byte.is_ascii_alphanumeric() || byte == b'_');
            tokens.push(Token {
                text: &source[index..end],
                kind: TokenKind::Identifier,
                range: (index, end),
            });
            index = end;
        } else if byte.is_ascii_digit() {
            let end = number_end(bytes, index);
            tokens.push(Token {
                text: &source[index..end],
                kind: TokenKind::Number,
                range: (index, end),
            });
            index = end;
        } else {
            let width = source[index..].chars().next().map_or(1, char::len_utf8);
            tokens.push(Token {
                text: &source[index..index + width],
                kind: TokenKind::Punctuation,
                range: (index, index + width),
            });
            index += width;
        }
    }
    tokens
}

fn scan(bytes: &[u8], from: usize, accepts: impl Fn(u8) -> bool) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| !accepts(*byte))
        .map_or(bytes.len(), |length| from + length)
}

// `1e-3` and `0x1p+2` keep their sign inside the token; `1 - 2` does not.
fn number_end(bytes: &[u8], from: usize) -> usize {
    let hexadecimal = bytes[from..].starts_with(b"0x") || bytes[from..].starts_with(b"0X");
    let exponent = if hexadecimal { b"pP" } else { b"eE" };
    let mut index = from;
    while index < bytes.len() {
        let byte = bytes[index];
        let signed_exponent = (byte == b'+' || byte == b'-') && index > from && exponent.contains(&bytes[index - 1]);
        if !(byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' || signed_exponent) {
            break;
        }
        index += 1;
    }
    index
}

fn block_comment_end(source: &str, from: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = from;
    while index < source.len() {
        if source[index..].starts_with("/*") {
            depth += 1;
            index += 2;
        } else if source[index..].starts_with("*/") {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return index;
            }
        } else {
            index += source[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    source.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_counts_characters_not_bytes() {
        let source = "let zażółć = 1;\nlet x = ;";
        let (line, column, text) = position(source, source.rfind(';').expect("the source has a semicolon"));
        assert_eq!((line, column, text), (2, 9, "let x = ;"));
    }

    #[test]
    fn closest_finds_typos_and_ignores_unrelated_names() {
        assert_eq!(closest("shadw", ["shadows", "fog", "shadow"]), Some("shadow"));
        assert_eq!(closest("colour", ["color", "scale"]), Some("color"));
        assert_eq!(closest("valeu", ["value", "other"]), Some("value"));
        assert_eq!(closest("x", ["unrelated", "other"]), None);
    }

    #[test]
    fn tokens_skip_comments_and_keep_signed_exponents() {
        let texts: Vec<&str> = tokens("a /* b /* c */ */ + 1e-3 // d\n- 2")
            .iter()
            .map(|token| token.text)
            .collect();
        assert_eq!(texts, ["a", "+", "1e-3", "-", "2"]);
    }
}
