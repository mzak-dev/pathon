use std::collections::BTreeSet;

use crate::metadata::{Area, Member, Model};

// Unique crates in pathon-core's shipped dependency closure. Raising it by more than 10 needs a reason in the PR.
pub(crate) const CORE_CRATE_BASELINE: usize = 126;
const CORE_CRATE_TOLERANCE: usize = 10;

// ADR-0001: licences compatible with distributing pathon as MIT OR Apache-2.0.
const ALLOWED_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "Unlicense",
    "CC0-1.0",
];

// Third-party crates each allowed in exactly the listed pathon crates.
const PINNED_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("wgpu", &["pathon-rhi-wgpu"]),
    ("naga_oil", &["pathon-shader"]),
    ("winit", &["pathon-platform-winit"]),
    ("bevy", &["pathon-bevy"]),
    ("godot", &["pathon-rhi-godot", "pathon-godot"]),
];

// Crates that may create threads; core never does (ADR-0004).
const THREAD_OWNERS: &[&str] = &["pathon"];
const THREAD_PATTERNS: &[&str] = &["thread::spawn", "thread::scope", "thread::Builder", "spawn_scoped"];

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Violation {
    pub(crate) rule: &'static str,
    pub(crate) message: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Report {
    pub(crate) violations: Vec<Violation>,
    pub(crate) core_crates: usize,
}

fn violation(rule: &'static str, message: String) -> Violation {
    Violation { rule, message }
}

fn is_platform_crate(name: &str) -> bool {
    name.starts_with("pathon-platform")
}

fn is_backend_crate(name: &str) -> bool {
    name.starts_with("pathon-rhi-") && name != "pathon-rhi-types"
}

fn shipped(member: &Member) -> bool {
    member.area != Area::Tooling
}

fn pinned_dependencies(member: &Member, violations: &mut Vec<Violation>) {
    for (dependency, allowed) in PINNED_DEPENDENCIES {
        if member.deps.iter().any(|dep| dep == dependency) && !allowed.contains(&member.name.as_str()) {
            violations.push(violation(
                "pinned-dependency",
                format!("{} depends on {dependency}; only {allowed:?} may", member.name),
            ));
        }
    }
}

fn renderer_independent_of_platform(member: &Member, violations: &mut Vec<Violation>) {
    if is_platform_crate(&member.name) || member.name == "pathon" {
        return;
    }
    for dep in member.deps.iter().filter(|dep| is_platform_crate(dep)) {
        violations.push(violation(
            "platform-layer",
            format!(
                "{} depends on {dep}; the renderer must not depend on the platform layer",
                member.name
            ),
        ));
    }
}

// Regular packs see pathon-api only; vendor packs and host adapters may also use a backend's interop module.
fn pack_boundary(member: &Member, violations: &mut Vec<Violation>) {
    let kind = member.kind.as_deref();
    let is_pack = member.area == Area::Packs || matches!(kind, Some("pack" | "vendor"));
    let is_host = member.area == Area::Hosts || kind == Some("host");
    if !is_pack && !is_host {
        return;
    }
    let may_use_backend = matches!(kind, Some("vendor" | "host"));
    for dep in member.deps.iter().filter(|dep| dep.starts_with("pathon")) {
        let allowed = dep == "pathon-api" || (may_use_backend && (is_backend_crate(dep) || is_host));
        if !allowed {
            let hint = if is_backend_crate(dep) {
                "declare [package.metadata.pathon] kind = \"vendor\" to use interop"
            } else {
                "packs depend on pathon-api only"
            };
            violations.push(violation(
                "pack-boundary",
                format!("{} depends on {dep}: {hint}", member.name),
            ));
        }
    }
}

fn no_threads_in_core(member: &Member, violations: &mut Vec<Violation>) {
    if member.area != Area::Crates || THREAD_OWNERS.contains(&member.name.as_str()) {
        return;
    }
    for (path, text) in &member.sources {
        for pattern in THREAD_PATTERNS {
            if text.contains(pattern) {
                violations.push(violation(
                    "no-threads",
                    format!(
                        "{} uses `{pattern}` in {}; core never creates threads",
                        member.name,
                        path.display()
                    ),
                ));
            }
        }
    }
}

fn license_allowed(expression: &str) -> bool {
    let normalized = expression.replace('/', " OR ").replace(['(', ')'], " ");
    normalized.split(" AND ").all(|conjunct| {
        conjunct.split(" OR ").any(|alternative| {
            let base = alternative.split(" WITH ").next().unwrap_or_default().trim();
            ALLOWED_LICENSES.contains(&base)
        })
    })
}

fn licenses(model: &Model, violations: &mut Vec<Violation>) {
    for package in &model.packages {
        match &package.license {
            Some(license) if license_allowed(license) => {}
            Some(license) => violations.push(violation(
                "license",
                format!("{} is licensed {license}, outside the ADR-0001 allowlist", package.name),
            )),
            None => violations.push(violation(
                "license",
                format!("{} declares no SPDX licence", package.name),
            )),
        }
    }
}

pub(crate) fn core_closure(model: &Model) -> usize {
    let mut seen = BTreeSet::new();
    let mut stack = vec!["pathon-core".to_owned()];
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(targets) = model.edges.get(&name) {
            stack.extend(targets.iter().cloned());
        }
    }
    seen.len().saturating_sub(1)
}

pub(crate) fn check(model: &Model) -> Report {
    let mut violations = Vec::new();
    for member in model.members.iter().filter(|member| shipped(member)) {
        pinned_dependencies(member, &mut violations);
        renderer_independent_of_platform(member, &mut violations);
        pack_boundary(member, &mut violations);
        no_threads_in_core(member, &mut violations);
    }
    licenses(model, &mut violations);
    let core_crates = core_closure(model);
    if core_crates > CORE_CRATE_BASELINE + CORE_CRATE_TOLERANCE {
        violations.push(violation(
            "crate-budget",
            format!(
                "pathon-core pulls {core_crates} crates, more than the baseline {CORE_CRATE_BASELINE} + \
                 {CORE_CRATE_TOLERANCE}; justify the growth in the PR and raise CORE_CRATE_BASELINE"
            ),
        ));
    }
    Report {
        violations,
        core_crates,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::metadata::Package;

    fn member(name: &str, area: Area, deps: &[&str]) -> Member {
        Member {
            name: name.into(),
            area,
            kind: None,
            deps: deps.iter().map(|dep| (*dep).to_owned()).collect(),
            sources: Vec::new(),
        }
    }

    fn rules_hit(model: &Model) -> Vec<&'static str> {
        check(model).violations.iter().map(|violation| violation.rule).collect()
    }

    fn model_with(members: Vec<Member>) -> Model {
        Model {
            members,
            ..Model::default()
        }
    }

    #[test]
    fn clean_workspace_passes() {
        let model = model_with(vec![
            member("pathon-rhi-wgpu", Area::Crates, &["wgpu", "pathon-rhi-types"]),
            member("pathon-core", Area::Crates, &["pathon-api"]),
            member("pathon-ssao", Area::Packs, &["pathon-api"]),
            member("pathon-examples", Area::Tooling, &["wgpu", "winit"]),
        ]);
        assert_eq!(check(&model).violations, []);
    }

    #[test]
    fn wgpu_outside_the_wgpu_backend_fails() {
        let model = model_with(vec![member("pathon-core", Area::Crates, &["wgpu"])]);
        assert_eq!(rules_hit(&model), ["pinned-dependency"]);
    }

    #[test]
    fn winit_outside_the_winit_platform_fails() {
        let model = model_with(vec![member("pathon-api", Area::Crates, &["winit"])]);
        assert_eq!(rules_hit(&model), ["pinned-dependency"]);
    }

    #[test]
    fn host_libraries_stay_in_their_adapters() {
        let model = model_with(vec![member("pathon-core", Area::Crates, &["bevy", "godot"])]);
        assert_eq!(rules_hit(&model), ["pinned-dependency", "pinned-dependency"]);
    }

    #[test]
    fn renderer_depending_on_the_platform_fails() {
        let model = model_with(vec![member("pathon-graph", Area::Crates, &["pathon-platform"])]);
        assert_eq!(rules_hit(&model), ["platform-layer"]);
    }

    #[test]
    fn regular_pack_reaching_past_the_api_fails() {
        let model = model_with(vec![
            member("pathon-fxaa", Area::Packs, &["pathon-core"]),
            member("pathon-fsr4", Area::Packs, &["pathon-api", "pathon-rhi-wgpu"]),
        ]);
        assert_eq!(rules_hit(&model), ["pack-boundary", "pack-boundary"]);
    }

    #[test]
    fn vendor_pack_may_use_backend_interop() {
        let mut vendor = member("pathon-fsr4", Area::Packs, &["pathon-api", "pathon-rhi-wgpu"]);
        vendor.kind = Some("vendor".into());
        assert_eq!(check(&model_with(vec![vendor])).violations, []);
    }

    #[test]
    fn core_crate_creating_a_thread_fails() {
        let mut core = member("pathon-core", Area::Crates, &[]);
        core.sources
            .push((PathBuf::from("src/renderer.rs"), "std::thread::spawn(|| {});".into()));
        let mut facade = member("pathon", Area::Crates, &[]);
        facade
            .sources
            .push((PathBuf::from("src/runtime.rs"), "thread::scope(|scope| {});".into()));
        assert_eq!(rules_hit(&model_with(vec![core, facade])), ["no-threads"]);
    }

    #[test]
    fn licence_outside_the_allowlist_fails() {
        let model = Model {
            packages: vec![
                Package {
                    name: "fine".into(),
                    license: Some("(MIT OR Apache-2.0) AND Unicode-3.0".into()),
                },
                Package {
                    name: "llvm".into(),
                    license: Some("Apache-2.0 WITH LLVM-exception".into()),
                },
                Package {
                    name: "copyleft".into(),
                    license: Some("GPL-3.0-only".into()),
                },
                Package {
                    name: "unknown".into(),
                    license: None,
                },
            ],
            ..Model::default()
        };
        assert_eq!(rules_hit(&model), ["license", "license"]);
    }

    #[test]
    fn crate_budget_growth_fails() {
        let mut model = Model::default();
        let mut previous = "pathon-core".to_owned();
        for index in 0..=(CORE_CRATE_BASELINE + CORE_CRATE_TOLERANCE) {
            let next = format!("dep-{index}");
            model.edges.entry(previous).or_default().insert(next.clone());
            previous = next;
        }
        assert_eq!(rules_hit(&model), ["crate-budget"]);
    }
}
