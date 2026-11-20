// Workspace checks run in CI: `cargo xtask deps-check`. Rules and rationale: docs/deps-check.md.

mod metadata;
mod rules;

use std::process::ExitCode;

#[expect(clippy::print_stdout, clippy::print_stderr, reason = "a CLI reports to the console")]
fn main() -> ExitCode {
    let command = std::env::args().nth(1);
    if command.as_deref() != Some("deps-check") {
        eprintln!("usage: cargo xtask deps-check");
        return ExitCode::FAILURE;
    }
    let model = match metadata::load() {
        Ok(model) => model,
        Err(error) => {
            eprintln!("deps-check: {error}");
            return ExitCode::FAILURE;
        }
    };
    let report = rules::check(&model);
    println!(
        "core dependency closure: {} crates (baseline {})",
        report.core_crates,
        rules::CORE_CRATE_BASELINE
    );
    if report.violations.is_empty() {
        println!("deps-check: ok");
        return ExitCode::SUCCESS;
    }
    for violation in &report.violations {
        eprintln!("error[{}]: {}", violation.rule, violation.message);
    }
    eprintln!("deps-check: {} violation(s)", report.violations.len());
    ExitCode::FAILURE
}
