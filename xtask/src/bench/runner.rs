use std::path::Path;
use std::process::{Command, Stdio};

use pathon_bench::{Metric, Unit, median, minimum};

use super::registry::Suite;

/// What one metric came to over all invocations of a probe.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Summary {
    pub(crate) name: String,
    pub(crate) unit: Unit,
    /// Median of the per-invocation medians: one slow process (cold caches, a busy core) does not decide.
    pub(crate) median: f64,
    pub(crate) min: f64,
    pub(crate) samples: usize,
    /// How far apart the per-invocation medians are, in percent of their median: a measure of how quiet the machine
    /// was.
    pub(crate) spread_pct: f64,
}

pub(crate) fn run_suite(
    root: &Path,
    suite: &Suite,
    repeats: usize,
    min_samples: usize,
) -> Result<Vec<Summary>, String> {
    let invocations = (0..repeats.max(1))
        .map(|_| invoke(root, suite))
        .collect::<Result<Vec<_>, _>>()?;
    summarize(&suite.id, &invocations, min_samples)
}

fn invoke(root: &Path, suite: &Suite) -> Result<Vec<Metric>, String> {
    let program = match suite.command[0].as_str() {
        "cargo" => std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()),
        other => other.to_owned(),
    };
    let output = Command::new(&program)
        .args(&suite.command[1..])
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| {
            format!(
                "suite `{}`: cannot run `{}`: {error}",
                suite.id,
                suite.command.join(" ")
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "suite `{}`: `{}` failed ({})",
            suite.id,
            suite.command.join(" "),
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let metrics = stdout
        .lines()
        .filter_map(Metric::parse_line)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("suite `{}`: {error}", suite.id))?;
    if metrics.is_empty() {
        return Err(format!(
            "suite `{}`: `{}` printed no `{}` lines",
            suite.id,
            suite.command.join(" "),
            pathon_bench::LINE_PREFIX
        ));
    }
    Ok(metrics)
}

pub(crate) fn summarize(suite: &str, invocations: &[Vec<Metric>], min_samples: usize) -> Result<Vec<Summary>, String> {
    let first = &invocations[0];
    let mut summaries = Vec::new();
    for metric in first {
        let mut medians = Vec::new();
        let mut all_samples = Vec::new();
        for invocation in invocations {
            let same = invocation
                .iter()
                .find(|other| other.name == metric.name)
                .ok_or_else(|| format!("suite `{suite}`: `{}` is missing from one of the runs", metric.name))?;
            if same.unit != metric.unit {
                return Err(format!(
                    "suite `{suite}`: `{}` changes its unit between runs",
                    metric.name
                ));
            }
            if metric.unit.is_timing() && same.samples.len() < min_samples {
                return Err(format!(
                    "suite `{suite}`: `{}` has {} samples per run, at least {min_samples} are needed for a median",
                    metric.name,
                    same.samples.len()
                ));
            }
            medians.push(median(&same.samples));
            all_samples.extend_from_slice(&same.samples);
        }
        summaries.push(Summary {
            name: metric.name.clone(),
            unit: metric.unit,
            median: median(&medians),
            min: minimum(&all_samples),
            samples: all_samples.len(),
            spread_pct: spread_pct(&medians),
        });
    }
    Ok(summaries)
}

fn spread_pct(medians: &[f64]) -> f64 {
    let centre = median(medians);
    if centre.abs() < f64::EPSILON {
        return 0.0;
    }
    (medians.iter().copied().fold(f64::MIN, f64::max) - minimum(medians)) / centre * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(name: &str, samples: &[f64]) -> Metric {
        Metric {
            name: name.into(),
            unit: Unit::Nanoseconds,
            samples: samples.to_vec(),
        }
    }

    #[test]
    fn the_result_is_the_median_of_the_medians_and_the_overall_minimum() {
        let runs = vec![
            vec![timing("a", &[10.0, 11.0, 12.0])],
            vec![timing("a", &[20.0, 21.0, 22.0])],
            vec![timing("a", &[10.0, 10.5, 11.0])],
        ];
        let summary = &summarize("s", &runs, 3).expect("valid")[0];
        assert!((summary.median - 11.0).abs() < f64::EPSILON);
        assert!((summary.min - 10.0).abs() < f64::EPSILON);
        assert_eq!(summary.samples, 9);
        assert!(
            (summary.spread_pct - 100.0 * 10.5 / 11.0).abs() < 1e-9,
            "medians 11, 21, 10.5 span 10.5 around 11"
        );
    }

    #[test]
    fn too_few_samples_missing_metrics_and_unit_changes_are_errors() {
        assert!(summarize("s", &[vec![timing("a", &[1.0, 2.0])]], 3).is_err());
        let with_a = vec![timing("a", &[1.0, 2.0, 3.0])];
        assert!(summarize("s", &[with_a.clone(), vec![timing("b", &[1.0, 2.0, 3.0])]], 3).is_err());
        let counted = vec![Metric {
            name: "a".into(),
            unit: Unit::Count,
            samples: vec![3.0],
        }];
        assert!(summarize("s", &[with_a, counted], 3).is_err());
    }

    #[test]
    fn a_count_needs_one_sample() {
        let counted = vec![Metric {
            name: "commands".into(),
            unit: Unit::Count,
            samples: vec![34.0],
        }];
        let summary = &summarize("s", &[counted.clone(), counted], 21).expect("valid")[0];
        assert!((summary.median - 34.0).abs() < f64::EPSILON);
    }
}
