// Workspace commands. `cargo xtask deps-check` (CI): rules and rationale in docs/deps-check.md.
// `cargo xtask bench` (by hand) and `cargo xtask budgets` (CI): docs/bench/README.md, ADR-0035.

mod bench;
mod console;
mod metadata;
mod rules;

use std::process::ExitCode;

use console::{say, warn};

const USAGE: &str = "usage: cargo xtask deps-check
       cargo xtask bench [--update] [--suite ID]... [--repeats N] [--machine NAME] [--skip-gpu] [--allow-noisy]
       cargo xtask budgets [--repeats N] [--no-suites] [--metric NAME=VALUE]...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let outcome = match args.first().map(String::as_str) {
        Some("deps-check") => Ok(deps_check()),
        Some("bench") => bench::bench(rest),
        Some("budgets") => bench::budgets(rest),
        _ => {
            warn(USAGE);
            return ExitCode::FAILURE;
        }
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            warn(&format!("error: {error}"));
            ExitCode::FAILURE
        }
    }
}

fn deps_check() -> bool {
    let model = match metadata::load() {
        Ok(model) => model,
        Err(error) => {
            warn(&format!("deps-check: {error}"));
            return false;
        }
    };
    let report = rules::check(&model);
    say(&format!(
        "core dependency closure: {} crates (baseline {})",
        report.core_crates,
        rules::CORE_CRATE_BASELINE
    ));
    if report.violations.is_empty() {
        say("deps-check: ok");
        return true;
    }
    for violation in &report.violations {
        warn(&format!("error[{}]: {}", violation.rule, violation.message));
    }
    warn(&format!("deps-check: {} violation(s)", report.violations.len()));
    false
}
