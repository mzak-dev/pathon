/// Median of the samples (mean of the two middle ones for an even count); `NaN` for no samples.
pub fn median(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return f64::NAN;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        f64::midpoint(sorted[middle - 1], sorted[middle])
    }
}

/// Smallest sample: the least disturbed by other processes, so the best estimate of the cost of CPU-bound code.
pub fn minimum(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(f64::NAN, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_and_even_counts() {
        assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < f64::EPSILON);
        assert!((median(&[4.0, 1.0, 2.0, 3.0]) - 2.5).abs() < f64::EPSILON);
        assert!(median(&[]).is_nan());
    }

    #[test]
    fn minimum_ignores_order() {
        assert!((minimum(&[3.0, 1.0, 2.0]) - 1.0).abs() < f64::EPSILON);
    }
}
