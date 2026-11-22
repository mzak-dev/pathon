use pathon_bench::{Quantity, Regression};

use super::baseline::MetricEntry;
use super::runner::Summary;

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Verdict {
    NoBaseline,
    Unchanged,
    /// Percent change of the median, positive = slower or bigger.
    Regression(f64),
    Improvement(f64),
    UnitChanged,
}

impl Verdict {
    pub(crate) fn is_alarm(&self) -> bool {
        matches!(self, Self::Regression(_) | Self::UnitChanged)
    }
}

fn change_pct(now: f64, before: f64) -> f64 {
    if before.abs() < f64::EPSILON {
        return if now.abs() < f64::EPSILON { 0.0 } else { f64::INFINITY };
    }
    (now / before - 1.0) * 100.0
}

/// A timing regresses only when the median and the minimum both grew past the threshold: noise from other processes
/// inflates the median far more often than the minimum, a real slowdown moves both. A count or a size is exact, so
/// any growth past its (zero by default) threshold is a regression.
pub(crate) fn compare(summary: &Summary, baseline: Option<&MetricEntry>, regression: &Regression) -> Verdict {
    let Some(baseline) = baseline else {
        return Verdict::NoBaseline;
    };
    if baseline.unit != summary.unit.name() {
        return Verdict::UnitChanged;
    }
    let threshold = regression.threshold_pct(&summary.name, summary.unit);
    let median_change = change_pct(summary.median, baseline.median);
    if !summary.unit.is_timing() {
        return match median_change {
            change if change > threshold => Verdict::Regression(change),
            change if change < -threshold => Verdict::Improvement(change),
            _ => Verdict::Unchanged,
        };
    }
    let min_change = change_pct(summary.min, baseline.min);
    if median_change > threshold && min_change > threshold {
        Verdict::Regression(median_change)
    } else if median_change < -threshold && min_change < -threshold {
        Verdict::Improvement(median_change)
    } else {
        Verdict::Unchanged
    }
}

/// The budget is compared with the median: the same statistic the baseline uses.
pub(crate) fn over_budget(summary: &Summary, limit: Quantity) -> Result<bool, String> {
    if limit.unit != summary.unit {
        return Err(format!(
            "`{}` is measured in {} but its budget is in {}",
            summary.name,
            summary.unit.name(),
            limit.unit.name()
        ));
    }
    Ok(summary.median > limit.value)
}

#[cfg(test)]
mod tests {
    use pathon_bench::Unit;

    use super::*;

    fn summary(unit: Unit, median: f64, min: f64) -> Summary {
        Summary {
            name: "m".into(),
            unit,
            median,
            min,
            samples: 63,
            spread_pct: 0.0,
        }
    }

    fn entry(unit: Unit, median: f64, min: f64) -> MetricEntry {
        MetricEntry {
            unit: unit.name().into(),
            median,
            min,
            samples: 63,
        }
    }

    #[test]
    fn a_timing_regresses_when_median_and_minimum_both_pass_the_threshold() {
        let baseline = entry(Unit::Nanoseconds, 100.0, 90.0);
        let regression = Regression::default();
        let verdict = compare(&summary(Unit::Nanoseconds, 125.0, 110.0), Some(&baseline), &regression);
        assert!(matches!(verdict, Verdict::Regression(change) if (change - 25.0).abs() < 1e-9));
    }

    #[test]
    fn a_noisy_median_alone_is_not_a_regression() {
        let baseline = entry(Unit::Nanoseconds, 100.0, 90.0);
        let verdict = compare(
            &summary(Unit::Nanoseconds, 125.0, 92.0),
            Some(&baseline),
            &Regression::default(),
        );
        assert_eq!(verdict, Verdict::Unchanged);
    }

    #[test]
    fn small_changes_stay_inside_the_threshold() {
        let baseline = entry(Unit::Nanoseconds, 100.0, 90.0);
        let verdict = compare(
            &summary(Unit::Nanoseconds, 109.0, 98.0),
            Some(&baseline),
            &Regression::default(),
        );
        assert_eq!(verdict, Verdict::Unchanged);
    }

    #[test]
    fn a_clear_speedup_is_reported_as_an_improvement() {
        let baseline = entry(Unit::Nanoseconds, 100.0, 90.0);
        let verdict = compare(
            &summary(Unit::Nanoseconds, 70.0, 60.0),
            Some(&baseline),
            &Regression::default(),
        );
        assert!(matches!(verdict, Verdict::Improvement(_)));
    }

    #[test]
    fn any_growth_of_a_count_is_a_regression() {
        let baseline = entry(Unit::Count, 85.0, 85.0);
        let regression = Regression::default();
        assert!(compare(&summary(Unit::Count, 86.0, 86.0), Some(&baseline), &regression).is_alarm());
        assert_eq!(
            compare(&summary(Unit::Count, 85.0, 85.0), Some(&baseline), &regression),
            Verdict::Unchanged
        );
        assert!(matches!(
            compare(&summary(Unit::Count, 80.0, 80.0), Some(&baseline), &regression),
            Verdict::Improvement(_)
        ));
    }

    #[test]
    fn a_count_that_grows_from_zero_is_a_regression() {
        let baseline = entry(Unit::Count, 0.0, 0.0);
        assert!(compare(&summary(Unit::Count, 1.0, 1.0), Some(&baseline), &Regression::default()).is_alarm());
    }

    #[test]
    fn without_a_baseline_or_with_another_unit_there_is_nothing_to_compare() {
        let regression = Regression::default();
        assert_eq!(
            compare(&summary(Unit::Count, 1.0, 1.0), None, &regression),
            Verdict::NoBaseline
        );
        let baseline = entry(Unit::Bytes, 1.0, 1.0);
        assert_eq!(
            compare(&summary(Unit::Count, 1.0, 1.0), Some(&baseline), &regression),
            Verdict::UnitChanged
        );
    }

    #[test]
    fn the_budget_is_compared_with_the_median() {
        let limit = Quantity {
            value: 2e6,
            unit: Unit::Nanoseconds,
        };
        assert_eq!(over_budget(&summary(Unit::Nanoseconds, 1.9e6, 1.0), limit), Ok(false));
        assert_eq!(over_budget(&summary(Unit::Nanoseconds, 2.1e6, 2.1e6), limit), Ok(true));
        assert!(over_budget(&summary(Unit::Bytes, 1.0, 1.0), limit).is_err());
    }
}
