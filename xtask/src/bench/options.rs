#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct BenchOptions {
    pub(crate) update: bool,
    pub(crate) skip_gpu: bool,
    pub(crate) allow_noisy: bool,
    pub(crate) suites: Vec<String>,
    pub(crate) repeats: Option<usize>,
    pub(crate) machine: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct BudgetsOptions {
    pub(crate) repeats: usize,
    pub(crate) run_suites: bool,
    pub(crate) metrics: Vec<(String, f64)>,
}

fn value_of<'args>(flag: &str, args: &mut impl Iterator<Item = &'args String>) -> Result<&'args str, String> {
    args.next()
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn repeats_of(flag: &str, text: &str) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|repeats| *repeats > 0)
        .ok_or_else(|| format!("{flag} needs a positive number, got `{text}`"))
}

impl BenchOptions {
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self {
            machine: std::env::var("PATHON_BENCH_MACHINE")
                .ok()
                .filter(|name| !name.is_empty()),
            ..Self::default()
        };
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--update" => options.update = true,
                "--skip-gpu" => options.skip_gpu = true,
                "--allow-noisy" => options.allow_noisy = true,
                "--suite" => options.suites.push(value_of(flag, &mut args)?.to_owned()),
                "--repeats" => options.repeats = Some(repeats_of(flag, value_of(flag, &mut args)?)?),
                "--machine" => options.machine = Some(value_of(flag, &mut args)?.to_owned()),
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        Ok(options)
    }
}

impl BudgetsOptions {
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self {
            repeats: 1,
            run_suites: true,
            metrics: Vec::new(),
        };
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--repeats" => options.repeats = repeats_of(flag, value_of(flag, &mut args)?)?,
                "--no-suites" => options.run_suites = false,
                "--metric" => {
                    let pair = value_of(flag, &mut args)?;
                    let (name, value) = pair
                        .split_once('=')
                        .ok_or_else(|| format!("--metric needs name=value, got `{pair}`"))?;
                    let value = value
                        .parse::<f64>()
                        .map_err(|_| format!("--metric {name}: `{value}` is not a number"))?;
                    options.metrics.push((name.to_owned(), value));
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn bench_flags() {
        let options = BenchOptions::parse(&args(&[
            "--update",
            "--suite",
            "scene",
            "--suite",
            "mesh",
            "--repeats",
            "5",
        ]))
        .expect("valid");
        assert!(options.update && !options.skip_gpu);
        assert_eq!(options.suites, ["scene", "mesh"]);
        assert_eq!(options.repeats, Some(5));
    }

    #[test]
    fn bench_rejects_unknown_flags_and_missing_values() {
        assert!(BenchOptions::parse(&args(&["--fast"])).is_err());
        assert!(BenchOptions::parse(&args(&["--suite"])).is_err());
        assert!(BenchOptions::parse(&args(&["--repeats", "0"])).is_err());
    }

    #[test]
    fn budgets_take_externally_measured_metrics() {
        let options = BudgetsOptions::parse(&args(&["--metric", "wasm.raw_bytes=681234"])).expect("valid");
        assert_eq!(options.metrics, [("wasm.raw_bytes".to_owned(), 681_234.0)]);
        assert_eq!(options.repeats, 1);
        assert!(options.run_suites);
        assert!(
            !BudgetsOptions::parse(&args(&["--no-suites"]))
                .expect("valid")
                .run_suites
        );
        assert!(BudgetsOptions::parse(&args(&["--metric", "wasm.raw_bytes"])).is_err());
        assert!(BudgetsOptions::parse(&args(&["--metric", "a=big"])).is_err());
    }
}
