use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::machine::{Machine, RunInfo};
use super::runner::Summary;

const SCHEMA: u32 = 1;

/// `docs/bench/baseline.json`: results per machine, because a time means nothing on another CPU or GPU.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Baseline {
    pub(crate) schema: u32,
    pub(crate) machines: BTreeMap<String, MachineEntry>,
}

impl Default for Baseline {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            machines: BTreeMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct MachineEntry {
    pub(crate) os_family: String,
    pub(crate) os: String,
    pub(crate) cpu: String,
    pub(crate) gpus: Vec<String>,
    pub(crate) suites: BTreeMap<String, SuiteEntry>,
}

/// A suite is recorded as a whole, with the conditions it was measured in: a partial `--update` cannot leave a
/// metric next to a commit and a compiler it was not measured with.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct SuiteEntry {
    pub(crate) recorded_at: String,
    pub(crate) commit: String,
    pub(crate) dirty: bool,
    pub(crate) rustc: String,
    pub(crate) repeats: usize,
    pub(crate) metrics: BTreeMap<String, MetricEntry>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct MetricEntry {
    pub(crate) unit: String,
    pub(crate) median: f64,
    pub(crate) min: f64,
    pub(crate) samples: usize,
}

impl MetricEntry {
    pub(crate) fn from_summary(summary: &Summary) -> Self {
        Self {
            unit: summary.unit.name().to_owned(),
            median: round_tenth(summary.median),
            min: round_tenth(summary.min),
            samples: summary.samples,
        }
    }
}

fn round_tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

impl Baseline {
    /// A missing file is an empty baseline: the first `--update` creates it.
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let baseline: Self =
                    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
                if baseline.schema == SCHEMA {
                    Ok(baseline)
                } else {
                    Err(format!(
                        "{}: schema {} is not supported (expected {SCHEMA})",
                        path.display(),
                        baseline.schema
                    ))
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        }
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        let mut text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        text.push('\n');
        std::fs::write(path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    /// The entry name for this machine: the one asked for, else the entry recorded on the same OS and CPU, else a name
    /// made from them (so a new machine can be recorded with a plain `--update`).
    pub(crate) fn machine_key(&self, requested: Option<&str>, machine: &Machine) -> String {
        if let Some(requested) = requested {
            return requested.to_owned();
        }
        self.machines
            .iter()
            .find(|(_, entry)| entry.os_family == machine.os_family && entry.cpu == machine.cpu)
            .map_or_else(|| machine.default_key(), |(key, _)| key.clone())
    }

    pub(crate) fn record(
        &mut self,
        key: &str,
        machine: &Machine,
        suite: &str,
        info: &RunInfo,
        repeats: usize,
        summaries: &[Summary],
    ) {
        let entry = self.machines.entry(key.to_owned()).or_insert_with(|| MachineEntry {
            os_family: machine.os_family.clone(),
            os: machine.os.clone(),
            cpu: machine.cpu.clone(),
            gpus: machine.gpus.clone(),
            suites: BTreeMap::new(),
        });
        entry.os.clone_from(&machine.os);
        entry.gpus.clone_from(&machine.gpus);
        entry.suites.insert(
            suite.to_owned(),
            SuiteEntry {
                recorded_at: info.recorded_at.clone(),
                commit: info.commit.clone(),
                dirty: info.dirty,
                rustc: info.rustc.clone(),
                repeats,
                metrics: summaries
                    .iter()
                    .map(|summary| (summary.name.clone(), MetricEntry::from_summary(summary)))
                    .collect(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use pathon_bench::Unit;

    use super::*;

    fn machine(cpu: &str) -> Machine {
        Machine {
            os_family: "windows".into(),
            os: "Windows 11".into(),
            cpu: cpu.into(),
            gpus: vec!["GPU".into()],
        }
    }

    fn info() -> RunInfo {
        RunInfo {
            recorded_at: "2026-11-22T10:00:00Z".into(),
            commit: "abc".into(),
            dirty: false,
            rustc: "1.100.0".into(),
        }
    }

    fn summary(name: &str) -> Summary {
        Summary {
            name: name.into(),
            unit: Unit::Nanoseconds,
            median: 1234.56,
            min: 1200.0,
            samples: 93,
            spread_pct: 1.0,
        }
    }

    #[test]
    fn a_recorded_suite_survives_a_json_round_trip() {
        let mut baseline = Baseline::default();
        baseline.record("pc", &machine("Ryzen"), "scene", &info(), 3, &[summary("scene.a")]);
        let text = serde_json::to_string_pretty(&baseline).expect("serialises");
        let parsed: Baseline = serde_json::from_str(&text).expect("parses");
        assert_eq!(parsed, baseline);
        let metric = &parsed.machines["pc"].suites["scene"].metrics["scene.a"];
        assert!((metric.median - 1234.6).abs() < 1e-9, "rounded to a tenth of a unit");
    }

    #[test]
    fn updating_one_suite_leaves_the_other_suites_and_machines_alone() {
        let mut baseline = Baseline::default();
        baseline.record("pc", &machine("Ryzen"), "scene", &info(), 3, &[summary("scene.a")]);
        baseline.record("pc", &machine("Ryzen"), "mesh", &info(), 3, &[summary("mesh.a")]);
        baseline.record("laptop", &machine("M5"), "scene", &info(), 3, &[summary("scene.a")]);
        baseline.record("pc", &machine("Ryzen"), "scene", &info(), 3, &[summary("scene.b")]);
        let pc = &baseline.machines["pc"];
        assert_eq!(pc.suites["scene"].metrics.keys().collect::<Vec<_>>(), ["scene.b"]);
        assert!(pc.suites.contains_key("mesh"));
        assert!(baseline.machines.contains_key("laptop"));
    }

    #[test]
    fn the_machine_is_found_by_its_cpu_unless_a_name_is_given() {
        let mut baseline = Baseline::default();
        baseline.record(
            "windows-rx9070xt",
            &machine("Ryzen"),
            "scene",
            &info(),
            3,
            &[summary("a")],
        );
        assert_eq!(baseline.machine_key(None, &machine("Ryzen")), "windows-rx9070xt");
        assert_eq!(baseline.machine_key(Some("other"), &machine("Ryzen")), "other");
        assert_eq!(baseline.machine_key(None, &machine("Core i9")), "windows-core-i9");
    }
}
