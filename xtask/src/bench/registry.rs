use std::path::Path;

use toml::{Table, Value};

/// One probe: a command that prints measurements in the `pathon-bench` protocol. Adding a measurement to the repo is
/// adding a `[[suite]]` to `docs/bench/registry.toml`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Suite {
    pub(crate) id: String,
    pub(crate) command: Vec<String>,
    /// Run by `cargo xtask budgets` in CI (deterministic on a shared runner, or checked with a wide margin).
    pub(crate) ci: bool,
    /// Needs a GPU: skipped by `--skip-gpu`.
    pub(crate) gpu: bool,
}

pub(crate) fn load(path: &Path) -> Result<Vec<Suite>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn parse(text: &str) -> Result<Vec<Suite>, String> {
    let table: Table = text.parse().map_err(|error| format!("not valid TOML: {error}"))?;
    if let Some(key) = table.keys().find(|key| key.as_str() != "suite") {
        return Err(format!("unknown section `{key}` (expected `[[suite]]`)"));
    }
    let entries = match table.get("suite") {
        None => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        Some(_) => return Err("`suite` must be an array of tables (`[[suite]]`)".into()),
    };
    let mut suites: Vec<Suite> = Vec::new();
    for entry in entries {
        let suite = parse_suite(entry)?;
        if suites.iter().any(|known| known.id == suite.id) {
            return Err(format!("suite `{}` is listed twice", suite.id));
        }
        suites.push(suite);
    }
    Ok(suites)
}

fn parse_suite(entry: &Value) -> Result<Suite, String> {
    let table = entry.as_table().ok_or("`[[suite]]` must be a table")?;
    let mut id = None;
    let mut command = None;
    let (mut ci, mut gpu) = (false, false);
    for (key, value) in table {
        match key.as_str() {
            "id" => id = value.as_str().map(str::to_owned),
            "command" => {
                command = value
                    .as_str()
                    .map(|command| command.split_whitespace().map(str::to_owned).collect::<Vec<_>>());
            }
            "ci" => ci = value.as_bool().ok_or("`ci` must be true or false")?,
            "gpu" => gpu = value.as_bool().ok_or("`gpu` must be true or false")?,
            other => return Err(format!("unknown key `{other}` in a suite")),
        }
    }
    let id = id.filter(|id| !id.is_empty()).ok_or("a suite needs a string `id`")?;
    let command = command
        .filter(|command| !command.is_empty())
        .ok_or_else(|| format!("suite `{id}` needs a string `command`"))?;
    Ok(Suite { id, command, ci, gpu })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_suites_with_their_flags() {
        let suites = parse(
            r#"
[[suite]]
id = "scene"
command = "cargo bench -p pathon-scene --bench probe"
ci = true

[[suite]]
id = "pbr"
command = "cargo bench -p pathon-pbr --bench scene"
gpu = true
"#,
        )
        .expect("valid");
        assert_eq!(suites.len(), 2);
        assert_eq!(
            suites[0].command,
            ["cargo", "bench", "-p", "pathon-scene", "--bench", "probe"]
        );
        assert!(suites[0].ci && !suites[0].gpu);
        assert!(!suites[1].ci && suites[1].gpu);
    }

    #[test]
    fn mistakes_are_errors() {
        assert!(parse("[[suite]]\nid = \"a\"").is_err(), "no command");
        assert!(parse("[[suite]]\ncommand = \"x\"").is_err(), "no id");
        assert!(parse("[[suite]]\nid = \"a\"\ncommand = \"x\"\nci = 1").is_err());
        assert!(parse("[[suite]]\nid = \"a\"\ncommand = \"x\"\n[[suite]]\nid = \"a\"\ncommand = \"y\"").is_err());
        assert!(parse("[[suit]]").is_err());
    }
}
