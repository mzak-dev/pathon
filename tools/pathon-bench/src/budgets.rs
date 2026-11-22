use std::collections::BTreeMap;
use std::path::Path;

use toml::{Table, Value};

use crate::quantity::{Quantity, Unit};

/// How `xtask bench` judges a change against the baseline of the same machine (ADR-0035).
#[derive(Clone, PartialEq, Debug)]
pub struct Regression {
    /// Allowed growth of a timing (median and minimum both) before it counts as a regression.
    pub time_pct: f64,
    /// Allowed growth of a count or a size; 0 because those are deterministic.
    pub count_pct: f64,
    /// Fewest samples of a timing that one invocation of a probe may report.
    pub min_samples: usize,
    /// How many times `xtask bench` starts each probe; the result is the median of the per-invocation medians.
    pub repeats: usize,
    overrides: BTreeMap<String, f64>,
}

impl Default for Regression {
    fn default() -> Self {
        Self {
            time_pct: 10.0,
            count_pct: 0.0,
            min_samples: 21,
            repeats: 3,
            overrides: BTreeMap::new(),
        }
    }
}

impl Regression {
    pub fn threshold_pct(&self, metric: &str, unit: Unit) -> f64 {
        self.overrides.get(metric).copied().unwrap_or(if unit.is_timing() {
            self.time_pct
        } else {
            self.count_pct
        })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Budget {
    max: Quantity,
    ci_max: Option<Quantity>,
}

/// `docs/bench/budgets.toml`: the one place where budgets and regression thresholds live. The local gate
/// (`xtask bench`, tests that measure time) uses `max`; the CI gate (`xtask budgets`) uses the looser `ci_max` of a
/// timing, because a shared runner is too noisy for the real budget, and `max` of everything else.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Budgets {
    regression: Regression,
    budgets: BTreeMap<String, Budget>,
}

impl Budgets {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let table: Table = text.parse().map_err(|error| format!("not valid TOML: {error}"))?;
        for key in table.keys() {
            if !matches!(key.as_str(), "regression" | "budgets") {
                return Err(format!("unknown section `{key}` (expected `regression` or `budgets`)"));
            }
        }
        Ok(Self {
            regression: table
                .get("regression")
                .map(parse_regression)
                .transpose()?
                .unwrap_or_default(),
            budgets: table.get("budgets").map(parse_budgets).transpose()?.unwrap_or_default(),
        })
    }

    pub fn regression(&self) -> &Regression {
        &self.regression
    }

    pub fn metric_names(&self) -> impl Iterator<Item = &str> {
        self.budgets.keys().map(String::as_str)
    }

    /// The budget of a metric in its base unit; panics on a missing entry so a test cannot silently lose its budget.
    pub fn max(&self, metric: &str) -> Quantity {
        self.budgets
            .get(metric)
            .unwrap_or_else(|| panic!("no budget `{metric}` in budgets.toml"))
            .max
    }

    /// The limit the CI gate applies: `ci_max` when the entry has one, `max` otherwise, nothing without an entry.
    pub fn ci_limit(&self, metric: &str) -> Option<Quantity> {
        let budget = self.budgets.get(metric)?;
        Some(budget.ci_max.unwrap_or(budget.max))
    }

    pub fn local_limit(&self, metric: &str) -> Option<Quantity> {
        self.budgets.get(metric).map(|budget| budget.max)
    }
}

fn as_table<'a>(value: &'a Value, what: &str) -> Result<&'a Table, String> {
    value.as_table().ok_or_else(|| format!("`{what}` must be a table"))
}

fn number(value: &Value, what: &str) -> Result<f64, String> {
    match value {
        Value::Float(number) => Ok(*number),
        // Config values are far below 2^53.
        #[expect(clippy::cast_precision_loss, reason = "config integers are small")]
        Value::Integer(number) => Ok(*number as f64),
        _ => Err(format!("`{what}` must be a number")),
    }
}

fn parse_regression(value: &Value) -> Result<Regression, String> {
    let table = as_table(value, "regression")?;
    let mut regression = Regression::default();
    for (key, value) in table {
        match key.as_str() {
            "time_pct" => regression.time_pct = number(value, key)?,
            "count_pct" => regression.count_pct = number(value, key)?,
            "min_samples" => regression.min_samples = whole_number(value, key)?,
            "repeats" => regression.repeats = whole_number(value, key)?,
            "overrides" => {
                for (metric, value) in as_table(value, "regression.overrides")? {
                    regression.overrides.insert(metric.clone(), number(value, metric)?);
                }
            }
            other => return Err(format!("unknown key `regression.{other}`")),
        }
    }
    Ok(regression)
}

fn whole_number(value: &Value, what: &str) -> Result<usize, String> {
    value
        .as_integer()
        .and_then(|number| usize::try_from(number).ok())
        .ok_or_else(|| format!("`{what}` must be a non-negative integer"))
}

fn quantity(value: &Value, what: &str) -> Result<Quantity, String> {
    match value {
        Value::String(text) => Quantity::parse(text).map_err(|error| format!("{what}: {error}")),
        Value::Integer(_) | Value::Float(_) => Ok(Quantity {
            value: number(value, what)?,
            unit: Unit::Count,
        }),
        _ => Err(format!(
            "`{what}` must be a quantity such as \"2 ms\", \"800 KiB\" or 42"
        )),
    }
}

fn parse_budgets(value: &Value) -> Result<BTreeMap<String, Budget>, String> {
    let mut budgets = BTreeMap::new();
    for (name, entry) in as_table(value, "budgets")? {
        let entry = as_table(entry, name)?;
        let mut max = None;
        let mut ci_max = None;
        for (key, value) in entry {
            match key.as_str() {
                "max" => max = Some(quantity(value, &format!("budgets.{name}.max"))?),
                "ci_max" => ci_max = Some(quantity(value, &format!("budgets.{name}.ci_max"))?),
                other => return Err(format!("unknown key `budgets.{name}.{other}`")),
            }
        }
        let max = max.ok_or_else(|| format!("budget `{name}` has no `max`"))?;
        if let Some(ci_max) = ci_max
            && ci_max.unit != max.unit
        {
            return Err(format!("budget `{name}`: `max` and `ci_max` have different units"));
        }
        budgets.insert(name.clone(), Budget { max, ci_max });
    }
    Ok(budgets)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[regression]
time_pct = 12.5
min_samples = 11

[regression.overrides]
"noisy.metric" = 30

[budgets."scene.spawn"]
max = "2 ms"
ci_max = "20 ms"

[budgets."core.commands"]
max = 40

[budgets."wasm.raw_bytes"]
max = "800 KiB"
"#;

    #[test]
    fn reads_thresholds_and_budgets() {
        let budgets = Budgets::parse(SAMPLE).expect("valid");
        let regression = budgets.regression();
        assert!((regression.time_pct - 12.5).abs() < f64::EPSILON);
        assert_eq!(regression.min_samples, 11);
        assert_eq!(regression.repeats, 3, "unset keys keep their defaults");
        assert!((regression.threshold_pct("noisy.metric", Unit::Nanoseconds) - 30.0).abs() < f64::EPSILON);
        assert!((regression.threshold_pct("core.commands", Unit::Count)).abs() < f64::EPSILON);
        assert!((budgets.max("scene.spawn").value - 2e6).abs() < f64::EPSILON);
    }

    #[test]
    fn the_ci_gate_uses_the_looser_limit_when_there_is_one() {
        let budgets = Budgets::parse(SAMPLE).expect("valid");
        assert!((budgets.ci_limit("scene.spawn").expect("entry").value - 20e6).abs() < f64::EPSILON);
        assert!((budgets.local_limit("scene.spawn").expect("entry").value - 2e6).abs() < f64::EPSILON);
        assert!((budgets.ci_limit("core.commands").expect("entry").value - 40.0).abs() < f64::EPSILON);
        assert!(budgets.ci_limit("unknown").is_none());
    }

    #[test]
    fn mistakes_in_the_file_are_errors_not_silence() {
        assert!(Budgets::parse("[budgetz]").is_err());
        assert!(Budgets::parse("[budgets.a]\nmaximum = 1").is_err());
        assert!(Budgets::parse("[budgets.a]\nci_max = 1").is_err());
        assert!(Budgets::parse("[budgets.a]\nmax = \"2 ms\"\nci_max = \"3 KiB\"").is_err());
        assert!(Budgets::parse("[regression]\nrepeats = -1").is_err());
    }
}
