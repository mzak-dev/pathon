use std::io::Write;
use std::time::Duration;

use crate::quantity::Unit;

/// Every line of the measurement protocol starts with this word, so cargo's own output cannot be mistaken for it.
pub const LINE_PREFIX: &str = "PATHON_BENCH";

/// One named measurement: several samples of one quantity. A probe prints it with [`emit`]; `xtask bench` reads it.
#[derive(Clone, PartialEq, Debug)]
pub struct Metric {
    pub name: String,
    pub unit: Unit,
    pub samples: Vec<f64>,
}

impl Metric {
    pub fn timing(name: &str, samples: &[Duration]) -> Self {
        Self {
            name: name.to_owned(),
            unit: Unit::Nanoseconds,
            samples: samples.iter().map(|sample| sample.as_secs_f64() * 1e9).collect(),
        }
    }

    pub fn count(name: &str, value: usize) -> Self {
        Self {
            name: name.to_owned(),
            unit: Unit::Count,
            samples: vec![value as f64],
        }
    }

    pub fn bytes(name: &str, value: usize) -> Self {
        Self {
            name: name.to_owned(),
            unit: Unit::Bytes,
            samples: vec![value as f64],
        }
    }

    /// `PATHON_BENCH<TAB>name<TAB>unit<TAB>sample sample ...`
    pub fn to_line(&self) -> String {
        let samples: Vec<String> = self.samples.iter().map(f64::to_string).collect();
        format!(
            "{LINE_PREFIX}\t{}\t{}\t{}",
            self.name,
            self.unit.name(),
            samples.join(" ")
        )
    }

    /// `None` when the line is not part of the protocol (cargo and test output), `Some(Err)` when it is malformed.
    pub fn parse_line(line: &str) -> Option<Result<Self, String>> {
        let rest = line.trim_end().strip_prefix(LINE_PREFIX)?.strip_prefix('\t')?;
        Some(parse_fields(rest).map_err(|reason| format!("bad measurement line `{line}`: {reason}")))
    }
}

fn parse_fields(fields: &str) -> Result<Metric, String> {
    let mut parts = fields.split('\t');
    let (Some(name), Some(unit), Some(samples), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err("expected name, unit and samples".into());
    };
    let unit = Unit::parse(unit).ok_or_else(|| format!("unknown unit `{unit}`"))?;
    let samples = samples
        .split_whitespace()
        .map(|sample| sample.parse::<f64>().map_err(|_| format!("`{sample}` is not a number")))
        .collect::<Result<Vec<_>, _>>()?;
    if name.is_empty() || samples.is_empty() {
        return Err("empty name or no samples".into());
    }
    Ok(Metric {
        name: name.to_owned(),
        unit,
        samples,
    })
}

/// Prints the metric on stdout for `xtask bench`.
pub fn emit(metric: &Metric) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", metric.to_line()).expect("stdout is writable");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_round_trips() {
        let samples = [Duration::from_micros(1200), Duration::from_micros(1300)];
        let metric = Metric::timing("scene.extract.spawn_10k", &samples);
        let parsed = Metric::parse_line(&metric.to_line())
            .expect("protocol line")
            .expect("valid");
        assert_eq!(parsed, metric);
    }

    #[test]
    fn foreign_lines_are_ignored_and_broken_ones_reported() {
        assert!(Metric::parse_line("   Compiling pathon v0.0.1").is_none());
        assert!(Metric::parse_line("PATHON_BENCH\tname\tns").is_some_and(|parsed| parsed.is_err()));
        assert!(Metric::parse_line("PATHON_BENCH\tname\tparsecs\t1").is_some_and(|parsed| parsed.is_err()));
        assert!(Metric::parse_line("PATHON_BENCH\tname\tcount\t").is_some_and(|parsed| parsed.is_err()));
    }
}
