// wesl and naga walk the syntax tree recursively, so a few thousand nested brackets, a few hundred chained operators or
// a long chain of imports overflow the stack and abort the process, which no `catch_unwind` stops. Files the user
// edits are not trusted to avoid that, so they are measured before the composer sees them.

use std::collections::{BTreeMap, HashMap, HashSet};

use pathon_shader::package_paths;

use crate::config::SourceLimits;

#[derive(PartialEq, Eq, Debug)]
pub(crate) struct Violation {
    pub(crate) line: usize,
    pub(crate) detail: String,
}

const OPERATORS: &[u8] = b"+-*/%&|^";

pub(crate) fn check_source(source: &str, limits: &SourceLimits) -> Result<(), Violation> {
    let bytes = source.as_bytes();
    let (mut depth, mut operators, mut line, mut index) = (0_usize, 0_usize, 1_usize, 0_usize);
    let mut block_comment_depth = 0_usize;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        if block_comment_depth > 0 {
            match (byte, next) {
                (b'/', Some(b'*')) => {
                    block_comment_depth += 1;
                    index += 1;
                }
                (b'*', Some(b'/')) => {
                    block_comment_depth -= 1;
                    index += 1;
                }
                (b'\n', _) => line += 1,
                _ => {}
            }
            index += 1;
            continue;
        }
        match (byte, next) {
            (b'/', Some(b'/')) => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                continue;
            }
            (b'/', Some(b'*')) => {
                block_comment_depth = 1;
                index += 2;
                continue;
            }
            _ => {}
        }
        match byte {
            b'\n' => line += 1,
            b'(' | b'{' | b'[' => {
                depth += 1;
                if byte == b'{' {
                    operators = 0;
                }
                if depth > limits.max_nesting_depth {
                    return Err(Violation {
                        line,
                        detail: format!("brackets are nested more than {} levels deep", limits.max_nesting_depth),
                    });
                }
            }
            b')' | b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if byte == b'}' {
                    operators = 0;
                }
            }
            b';' => operators = 0,
            _ if OPERATORS.contains(&byte) => {
                operators += 1;
                if operators > limits.max_operators_per_statement {
                    return Err(Violation {
                        line,
                        detail: format!(
                            "a statement chains more than {} operators",
                            limits.max_operators_per_statement
                        ),
                    });
                }
            }
            _ => {}
        }
        index += 1;
    }
    Ok(())
}

/// The longest chain of modules importing modules, counting the module a root imports as one level, and the chain
/// itself for the message. Imports are the `package::` paths found anywhere in a source, which covers `import` lines
/// and inline uses alike; paths that are no module of the set (a host's own modules) end the chain.
pub(crate) fn longest_import_chain<'a>(
    modules: &BTreeMap<&'a str, &'a str>,
    roots: impl IntoIterator<Item = &'a str>,
) -> (usize, Vec<&'a str>) {
    let edges: HashMap<&str, Vec<&str>> = modules
        .iter()
        .map(|(name, source)| (*name, imported_modules(source, modules)))
        .collect();
    let chains = chains_by_module(&edges);
    let from_roots = roots
        .into_iter()
        .map(|source| {
            let imported = imported_modules(source, modules);
            longest_of(&imported, &chains)
        })
        .max_by_key(Vec::len)
        .unwrap_or_default();
    let from_modules = chains
        .values()
        .max_by_key(|chain| chain.len())
        .cloned()
        .unwrap_or_default();
    let longest = if from_roots.len() > from_modules.len() {
        from_roots
    } else {
        from_modules
    };
    (longest.len(), longest)
}

fn longest_of<'a>(imported: &[&'a str], chains: &HashMap<&'a str, Vec<&'a str>>) -> Vec<&'a str> {
    imported
        .iter()
        .filter_map(|name| chains.get(name))
        .max_by_key(|chain| chain.len())
        .cloned()
        .unwrap_or_default()
}

fn imported_modules<'a>(source: &str, modules: &BTreeMap<&'a str, &'a str>) -> Vec<&'a str> {
    let mut found = Vec::new();
    for segments in package_paths(source) {
        let module = (1..=segments.len())
            .rev()
            .find_map(|length| modules.get_key_value(segments[..length].join("::").as_str()));
        if let Some((name, _)) = module
            && !found.contains(name)
        {
            found.push(*name);
        }
    }
    found
}

// The longest chain starting at each module, found without recursion so the check itself cannot overflow. A cycle
// ends a chain where it closes; wesl reports the cycle itself.
fn chains_by_module<'a>(edges: &HashMap<&'a str, Vec<&'a str>>) -> HashMap<&'a str, Vec<&'a str>> {
    let mut chains: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut in_progress: HashSet<&str> = HashSet::new();
    let mut names: Vec<&str> = edges.keys().copied().collect();
    names.sort_unstable();
    for start in names {
        if chains.contains_key(start) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        in_progress.insert(start);
        while let Some(&(node, next)) = stack.last() {
            let children = &edges[node];
            if let Some(&child) = children.get(next) {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                if edges.contains_key(child) && !chains.contains_key(child) && !in_progress.contains(child) {
                    in_progress.insert(child);
                    stack.push((child, 0));
                }
            } else {
                let tail = children
                    .iter()
                    .filter_map(|child| chains.get(child))
                    .max_by_key(|chain| chain.len());
                let mut chain = vec![node];
                if let Some(tail) = tail {
                    chain.extend_from_slice(tail);
                }
                chains.insert(node, chain);
                in_progress.remove(node);
                stack.pop();
            }
        }
    }
    chains
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> SourceLimits {
        SourceLimits::new()
    }

    #[test]
    fn ordinary_shader_code_passes() {
        let source = "fn f(a: f32) -> f32 { return (a * 2.0 + 1.0) / (a - 3.0); } // ((((( not code\n";
        assert_eq!(check_source(source, &limits()), Ok(()));
    }

    #[test]
    fn deep_brackets_are_reported_with_their_line() {
        let source = format!("fn f() {{\n let x = {}1{};\n}}", "(".repeat(40), ")".repeat(40));
        let violation = check_source(&source, &limits()).expect_err("too deep");
        assert_eq!(violation.line, 2);
    }

    #[test]
    fn a_long_operator_chain_is_reported_but_separate_statements_are_not() {
        let chain = format!("fn f() {{ let x = 1{}; }}", " + 1".repeat(200));
        assert!(check_source(&chain, &limits()).is_err());
        let statements = format!("fn f() {{ {} }}", "x = x + 1; ".repeat(500));
        assert_eq!(check_source(&statements, &limits()), Ok(()));
    }

    #[test]
    fn comments_do_not_count() {
        let source = format!("/* {} /* nested */ {} */ fn f() {{}}", "(".repeat(100), "(".repeat(100));
        assert_eq!(check_source(&source, &limits()), Ok(()));
    }

    #[test]
    fn import_chains_are_measured_through_modules_and_roots() {
        let sources: Vec<(String, String)> = (0..5)
            .map(|index| {
                let body = if index < 4 {
                    format!("import package::m{}::f{};\nfn f{index}() {{}}", index + 1, index + 1)
                } else {
                    format!("fn f{index}() {{}}")
                };
                (format!("m{index}"), body)
            })
            .collect();
        let modules: BTreeMap<&str, &str> = sources
            .iter()
            .map(|(name, source)| (name.as_str(), source.as_str()))
            .collect();
        let (depth, chain) = longest_import_chain(&modules, ["import package::m0::f0;"]);
        assert_eq!(depth, 5);
        assert_eq!(chain, ["m0", "m1", "m2", "m3", "m4"]);
    }

    #[test]
    fn import_cycles_end_the_chain_instead_of_looping() {
        let modules = BTreeMap::from([("a", "import package::b::g;"), ("b", "import package::a::f;")]);
        let (depth, _) = longest_import_chain(&modules, []);
        assert_eq!(depth, 2);
    }
}
