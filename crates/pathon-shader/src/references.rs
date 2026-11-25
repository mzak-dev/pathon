/// The `package::a::b::name` paths a source mentions, one list of segments (`["a", "b", "name"]`) each: imports and
/// inline uses alike. Which prefix names a module is for the caller to decide, because only it knows the modules.
#[must_use]
pub fn package_paths(source: &str) -> Vec<Vec<&str>> {
    const PREFIX: &str = "package::";
    let mut paths = Vec::new();
    for (position, _) in source.match_indices(PREFIX) {
        let preceded_by_identifier = source[..position]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        if preceded_by_identifier {
            continue;
        }
        let rest = &source[position + PREFIX.len()..];
        let length = rest
            .find(|character: char| !(character.is_alphanumeric() || character == '_' || character == ':'))
            .unwrap_or(rest.len());
        let segments: Vec<&str> = rest[..length]
            .split("::")
            .filter(|segment| !segment.is_empty())
            .collect();
        if !segments.is_empty() {
            paths.push(segments);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_imports_and_inline_paths_but_not_longer_identifiers() {
        let source = "import package::a::b::{x, y};\nlet v = package::c::f(); // mypackage::d::g\n";
        assert_eq!(package_paths(source), [vec!["a", "b"], vec!["c", "f"]]);
    }
}
