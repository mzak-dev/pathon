// `cargo xtask bench` (local, by hand) and `cargo xtask budgets` (CI). Rationale and thresholds: ADR-0035,
// usage and file formats: docs/bench/README.md.

mod baseline;
mod judge;
mod machine;
mod options;
mod registry;
mod runner;

use std::path::{Path, PathBuf};

use pathon_bench::{Budgets, Quantity, format_value};

use self::baseline::{Baseline, MetricEntry};
use self::judge::{Verdict, compare, over_budget};
use self::options::{BenchOptions, BudgetsOptions};
use self::registry::Suite;
use self::runner::{Summary, run_suite};
use crate::console::{say, warn};

const BUDGETS: &str = "docs/bench/budgets.toml";
const REGISTRY: &str = "docs/bench/registry.toml";
const BASELINE: &str = "docs/bench/baseline.json";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

fn select_suites(all: Vec<Suite>, wanted: &[String], skip_gpu: bool) -> Result<Vec<Suite>, String> {
    if let Some(unknown) = wanted.iter().find(|id| !all.iter().any(|suite| &suite.id == *id)) {
        let known: Vec<&str> = all.iter().map(|suite| suite.id.as_str()).collect();
        return Err(format!(
            "no suite `{unknown}` in {REGISTRY} (known: {})",
            known.join(", ")
        ));
    }
    Ok(all
        .into_iter()
        .filter(|suite| wanted.is_empty() || wanted.contains(&suite.id))
        .filter(|suite| !(skip_gpu && suite.gpu))
        .collect())
}

fn print_table(header: [&str; 7], rows: &[[String; 7]]) {
    let mut widths = header.map(str::len);
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: &[&str]| {
        let mut text = String::from("  ");
        for (index, cell) in cells.iter().enumerate() {
            let padding = widths[index].saturating_sub(cell.chars().count());
            if index == 0 {
                text.push_str(cell);
                text.push_str(&" ".repeat(padding));
            } else {
                text.push_str(&" ".repeat(padding));
                text.push_str(cell);
            }
            text.push_str("  ");
        }
        say(text.trim_end());
    };
    line(&header);
    for row in rows {
        line(&row.each_ref().map(String::as_str));
    }
}

fn spread_cell(summary: &Summary) -> String {
    if summary.unit.is_timing() {
        format!("{:.0}%", summary.spread_pct)
    } else {
        String::new()
    }
}

fn format_change(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Regression(change) | Verdict::Improvement(change) => format!("{change:+.1}%"),
        Verdict::Unchanged => "~".into(),
        Verdict::NoBaseline | Verdict::UnitChanged => String::new(),
    }
}

fn describe(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::NoBaseline => "no baseline",
        Verdict::Unchanged => "ok",
        Verdict::Regression(_) => "REGRESSION",
        Verdict::Improvement(_) => "faster (consider --update)",
        Verdict::UnitChanged => "UNIT CHANGED (run --update)",
    }
}

pub(crate) fn bench(args: &[String]) -> Result<bool, String> {
    let options = BenchOptions::parse(args)?;
    let root = workspace_root();
    let budgets = Budgets::load(&root.join(BUDGETS))?;
    let suites = select_suites(registry::load(&root.join(REGISTRY))?, &options.suites, options.skip_gpu)?;
    let machine = machine::detect();
    let info = machine::run_info(&root);
    let baseline_path = root.join(BASELINE);
    let mut baseline = Baseline::load(&baseline_path)?;
    let key = baseline.machine_key(options.machine.as_deref(), &machine);
    let repeats = options.repeats.unwrap_or(budgets.regression().repeats);

    say(&format!(
        "machine `{key}`: {} · {} · {}",
        machine.os,
        machine.cpu,
        machine.gpus.join(", ")
    ));
    say(&format!(
        "commit {}{} · rustc {} · {repeats} runs per suite, median of medians",
        info.commit,
        if info.dirty { " (uncommitted changes)" } else { "" },
        info.rustc
    ));
    if !baseline.machines.contains_key(&key) && !options.update {
        say("no baseline for this machine yet: run `cargo xtask bench --update` to record it");
    }

    let mut alarms = Vec::new();
    let mut results = Vec::new();
    for suite in &suites {
        say(&format!("\nsuite `{}`: {}", suite.id, suite.command.join(" ")));
        let summaries = run_suite(&root, suite, repeats, budgets.regression().min_samples)?;
        let recorded = baseline
            .machines
            .get(&key)
            .and_then(|entry| entry.suites.get(&suite.id));
        let mut rows = Vec::new();
        for summary in &summaries {
            let before = recorded.and_then(|entry| entry.metrics.get(&summary.name));
            let verdict = if options.update {
                Verdict::NoBaseline
            } else {
                compare(summary, before, budgets.regression())
            };
            let exceeded = match budgets.local_limit(&summary.name) {
                Some(limit) if over_budget(summary, limit)? => Some(limit),
                _ => None,
            };
            if verdict.is_alarm() {
                alarms.push(format!("{}: {}", summary.name, describe(&verdict)));
            }
            if exceeded.is_some() {
                alarms.push(format!("{}: over its budget", summary.name));
            }
            rows.push(table_row(
                summary,
                before,
                &verdict,
                status_text(&verdict, exceeded, options.update),
            ));
        }
        print_table(
            ["metric", "median", "min", "spread", "baseline", "change", "verdict"],
            &rows,
        );
        results.push((suite, summaries));
    }

    save_latest(&root, &key, &machine, &info, repeats, &results)?;
    if options.update {
        let noisy = noisy_metrics(&results, budgets.regression().time_pct);
        if !noisy.is_empty() && !options.allow_noisy {
            return Err(format!(
                "the runs differ by more than {}% for {}: the machine was not quiet, a baseline recorded now would \
                 cause false alarms (nothing written; close other programs and retry, or pass --allow-noisy)",
                budgets.regression().time_pct,
                noisy.join(", ")
            ));
        }
        for (suite, summaries) in &results {
            baseline.record(&key, &machine, &suite.id, &info, repeats, summaries);
        }
        baseline.save(&baseline_path)?;
        say(&format!("\nbaseline of `{key}` updated in {BASELINE}"));
    }
    report_alarms(&alarms, "regressions or exceeded budgets");
    Ok(alarms.is_empty())
}

fn noisy_metrics(results: &[(&Suite, Vec<Summary>)], threshold_pct: f64) -> Vec<String> {
    results
        .iter()
        .flat_map(|(_, summaries)| summaries)
        .filter(|summary| summary.unit.is_timing() && summary.spread_pct > threshold_pct)
        .map(|summary| format!("{} ({:.0}%)", summary.name, summary.spread_pct))
        .collect()
}

fn table_row(summary: &Summary, before: Option<&MetricEntry>, verdict: &Verdict, status: String) -> [String; 7] {
    [
        summary.name.clone(),
        format_value(summary.median, summary.unit),
        format_value(summary.min, summary.unit),
        spread_cell(summary),
        before.map_or_else(String::new, |entry| format_value(entry.median, summary.unit)),
        format_change(verdict),
        status,
    ]
}

fn status_text(verdict: &Verdict, exceeded: Option<Quantity>, updating: bool) -> String {
    match exceeded {
        Some(limit) => format!("OVER BUDGET {}", limit.display()),
        None if updating => "recorded".into(),
        None => describe(verdict).into(),
    }
}

fn save_latest(
    root: &Path,
    key: &str,
    machine: &machine::Machine,
    info: &machine::RunInfo,
    repeats: usize,
    results: &[(&Suite, Vec<Summary>)],
) -> Result<(), String> {
    let mut latest = Baseline::default();
    for (suite, summaries) in results {
        latest.record(key, machine, &suite.id, info, repeats, summaries);
    }
    let directory = root.join("target").join("bench");
    std::fs::create_dir_all(&directory).map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    latest.save(&directory.join("latest.json"))
}

fn report_alarms(alarms: &[String], what: &str) {
    if alarms.is_empty() {
        say("\nno regressions, all budgets kept");
        return;
    }
    warn(&format!("\n{} {what}:", alarms.len()));
    for alarm in alarms {
        warn(&format!("  {alarm}"));
    }
}

/// CI gate: the suites marked `ci`, plus sizes passed by the workflow, against the limits of `budgets.toml`.
pub(crate) fn budgets(args: &[String]) -> Result<bool, String> {
    let options = BudgetsOptions::parse(args)?;
    let root = workspace_root();
    let budgets = Budgets::load(&root.join(BUDGETS))?;
    let suites: Vec<Suite> = registry::load(&root.join(REGISTRY))?
        .into_iter()
        .filter(|suite| suite.ci && options.run_suites)
        .collect();

    let mut summaries = Vec::new();
    for suite in &suites {
        say(&format!("suite `{}`: {}", suite.id, suite.command.join(" ")));
        summaries.extend(run_suite(
            &root,
            suite,
            options.repeats,
            budgets.regression().min_samples,
        )?);
    }
    for (name, value) in &options.metrics {
        let limit = budgets
            .ci_limit(name)
            .ok_or_else(|| format!("`--metric {name}`: no budget `{name}` in {BUDGETS}"))?;
        summaries.push(Summary {
            name: name.clone(),
            unit: limit.unit,
            median: *value,
            min: *value,
            samples: 1,
            spread_pct: 0.0,
        });
    }

    let mut alarms = Vec::new();
    let mut rows = Vec::new();
    for summary in &summaries {
        let limit = budgets.ci_limit(&summary.name);
        let over = match limit {
            Some(limit) => over_budget(summary, limit)?,
            None => false,
        };
        if over {
            alarms.push(format!("{}: over its budget", summary.name));
        }
        rows.push([
            summary.name.clone(),
            format_value(summary.median, summary.unit),
            String::new(),
            String::new(),
            limit.map_or_else(|| "none".into(), |limit| limit.display()),
            String::new(),
            if over { "OVER BUDGET" } else { "ok" }.into(),
        ]);
    }
    say("");
    print_table(["metric", "value", "", "", "limit", "", "verdict"], &rows);
    report_alarms(
        &alarms,
        "exceeded budgets (raise them in docs/bench/budgets.toml only on purpose)",
    );
    Ok(alarms.is_empty())
}
