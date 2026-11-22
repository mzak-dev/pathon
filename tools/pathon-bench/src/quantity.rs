/// What a metric measures. The unit decides how a regression is judged: timings are noisy, counts are exact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Nanoseconds,
    Bytes,
    Count,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Nanoseconds => "ns",
            Self::Bytes => "bytes",
            Self::Count => "count",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Nanoseconds, Self::Bytes, Self::Count]
            .into_iter()
            .find(|unit| unit.name() == name)
    }

    pub fn is_timing(self) -> bool {
        self == Self::Nanoseconds
    }
}

/// A value with its unit, always stored in the base unit (ns, bytes, count).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Quantity {
    pub value: f64,
    pub unit: Unit,
}

const SUFFIXES: [(&str, f64, Unit); 10] = [
    ("ns", 1.0, Unit::Nanoseconds),
    ("us", 1e3, Unit::Nanoseconds),
    ("µs", 1e3, Unit::Nanoseconds),
    ("ms", 1e6, Unit::Nanoseconds),
    ("s", 1e9, Unit::Nanoseconds),
    ("KiB", 1024.0, Unit::Bytes),
    ("MiB", 1_048_576.0, Unit::Bytes),
    ("GiB", 1_073_741_824.0, Unit::Bytes),
    ("B", 1.0, Unit::Bytes),
    ("", 1.0, Unit::Count),
];

impl Quantity {
    /// Parses `2 ms`, `800 KiB`, `42` (a plain number is a count).
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let number_end = text
            .find(|character: char| !(character.is_ascii_digit() || matches!(character, '.' | '_' | '-')))
            .unwrap_or(text.len());
        let (number, suffix) = text.split_at(number_end);
        let value: f64 = number
            .replace('_', "")
            .parse()
            .map_err(|_| format!("`{text}` is not a quantity (a number and an optional unit: ms, KiB, ...)"))?;
        let suffix = suffix.trim();
        let (_, scale, unit) = SUFFIXES
            .iter()
            .find(|(name, _, _)| *name == suffix)
            .ok_or_else(|| format!("unknown unit `{suffix}` in `{text}`"))?;
        Ok(Self {
            value: value * scale,
            unit: *unit,
        })
    }

    /// Human-readable, in the largest unit that keeps the number at least 1 (`1.20 ms`, `664.0 KiB`).
    pub fn display(&self) -> String {
        format_value(self.value, self.unit)
    }
}

pub fn format_value(value: f64, unit: Unit) -> String {
    match unit {
        Unit::Nanoseconds if value >= 1e9 => format!("{:.2} s", value / 1e9),
        Unit::Nanoseconds if value >= 1e6 => format!("{:.2} ms", value / 1e6),
        Unit::Nanoseconds if value >= 1e3 => format!("{:.2} µs", value / 1e3),
        Unit::Nanoseconds => format!("{value:.0} ns"),
        Unit::Bytes if value >= 1_048_576.0 => format!("{:.2} MiB", value / 1_048_576.0),
        Unit::Bytes if value >= 1024.0 => format!("{:.1} KiB", value / 1024.0),
        Unit::Bytes => format!("{value:.0} B"),
        Unit::Count => format!("{value:.0}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantity(value: f64, unit: Unit) -> Quantity {
        Quantity { value, unit }
    }

    #[test]
    fn parses_times_sizes_and_counts() {
        assert_eq!(Quantity::parse("2 ms"), Ok(quantity(2e6, Unit::Nanoseconds)));
        assert_eq!(Quantity::parse("2.5ms"), Ok(quantity(2.5e6, Unit::Nanoseconds)));
        assert_eq!(Quantity::parse("800 KiB"), Ok(quantity(819_200.0, Unit::Bytes)));
        assert_eq!(Quantity::parse("1_200"), Ok(quantity(1200.0, Unit::Count)));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Quantity::parse("fast").is_err());
        assert!(Quantity::parse("3 parsecs").is_err());
    }

    #[test]
    fn displays_in_a_readable_unit() {
        assert_eq!(format_value(1_200_000.0, Unit::Nanoseconds), "1.20 ms");
        assert_eq!(format_value(680_000.0, Unit::Bytes), "664.1 KiB");
        assert_eq!(format_value(31.0, Unit::Count), "31");
    }
}
