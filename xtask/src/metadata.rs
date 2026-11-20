use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Area {
    Crates,
    Packs,
    Hosts,
    Tooling,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Member {
    pub(crate) name: String,
    pub(crate) area: Area,
    pub(crate) kind: Option<String>,
    // Direct normal and build dependencies; dev-dependencies do not ship.
    pub(crate) deps: Vec<String>,
    pub(crate) sources: Vec<(PathBuf, String)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Package {
    pub(crate) name: String,
    pub(crate) license: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Model {
    pub(crate) members: Vec<Member>,
    pub(crate) packages: Vec<Package>,
    // Normal and build edges of the resolved graph, by package name.
    pub(crate) edges: HashMap<String, BTreeSet<String>>,
}

fn area_of(manifest: &Path, root: &Path) -> Area {
    let relative = manifest.strip_prefix(root).unwrap_or(manifest);
    match relative
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
    {
        Some("crates") => Area::Crates,
        Some("packs") => Area::Packs,
        Some("hosts") => Area::Hosts,
        _ => Area::Tooling,
    }
}

fn rust_sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && let Ok(text) = fs::read_to_string(&path)
        {
            out.push((path, text));
        }
    }
}

fn shipped_dependency(dependency: &Value) -> Option<String> {
    let kind = dependency.get("kind").and_then(Value::as_str);
    if kind == Some("dev") {
        return None;
    }
    dependency.get("name").and_then(Value::as_str).map(str::to_owned)
}

pub(crate) fn load() -> Result<Model, String> {
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|error| format!("bad metadata: {error}"))?;
    let root = PathBuf::from(
        json["workspace_root"]
            .as_str()
            .ok_or("metadata has no workspace_root")?,
    );
    let member_ids: BTreeSet<&str> = json["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();

    let mut model = Model::default();
    let mut names_by_id = HashMap::new();
    for package in json["packages"].as_array().into_iter().flatten() {
        let id = package["id"].as_str().unwrap_or_default();
        let name = package["name"].as_str().unwrap_or_default().to_owned();
        names_by_id.insert(id.to_owned(), name.clone());
        if member_ids.contains(id) {
            let manifest = PathBuf::from(package["manifest_path"].as_str().unwrap_or_default());
            let mut sources = Vec::new();
            if let Some(dir) = manifest.parent() {
                rust_sources(&dir.join("src"), &mut sources);
            }
            model.members.push(Member {
                name,
                area: area_of(&manifest, &root),
                kind: package["metadata"]["pathon"]["kind"].as_str().map(str::to_owned),
                deps: package["dependencies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(shipped_dependency)
                    .collect(),
                sources,
            });
        } else {
            model.packages.push(Package {
                name,
                license: package["license"].as_str().map(str::to_owned),
            });
        }
    }
    for node in json["resolve"]["nodes"].as_array().into_iter().flatten() {
        let Some(name) = node["id"].as_str().and_then(|id| names_by_id.get(id)) else {
            continue;
        };
        let targets = model.edges.entry(name.clone()).or_default();
        for dep in node["deps"].as_array().into_iter().flatten() {
            let shipped = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|kind| kind["kind"].as_str() != Some("dev"));
            if shipped && let Some(target) = dep["pkg"].as_str().and_then(|id| names_by_id.get(id)) {
                targets.insert(target.clone());
            }
        }
    }
    Ok(model)
}
