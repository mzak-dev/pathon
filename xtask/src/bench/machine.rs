use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// The hardware and system a measurement was made on. Best effort: a field that cannot be read is `unknown`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Machine {
    pub(crate) os_family: String,
    pub(crate) os: String,
    pub(crate) cpu: String,
    pub(crate) gpus: Vec<String>,
}

/// The state of the tree a measurement was made on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct RunInfo {
    pub(crate) recorded_at: String,
    pub(crate) commit: String,
    /// Uncommitted changes outside `docs/bench`: the numbers do not belong to `commit` alone.
    pub(crate) dirty: bool,
    pub(crate) rustc: String,
}

const UNKNOWN: &str = "unknown";

fn output_of(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn detect() -> Machine {
    match std::env::consts::OS {
        "windows" => detect_windows(),
        "macos" => detect_macos(),
        _ => detect_linux(),
    }
}

fn detect_windows() -> Machine {
    let script = "$o = Get-CimInstance Win32_OperatingSystem; \"$($o.Caption) $($o.Version)\"; \
                  (Get-CimInstance Win32_Processor | Select-Object -First 1).Name; \
                  (Get-CimInstance Win32_VideoController).Name -join '|'";
    let text = output_of("powershell", &["-NoProfile", "-NonInteractive", "-Command", script]).unwrap_or_default();
    let mut lines = text.lines().map(str::trim);
    Machine {
        os_family: "windows".into(),
        os: lines.next().filter(|line| !line.is_empty()).unwrap_or(UNKNOWN).into(),
        cpu: lines.next().filter(|line| !line.is_empty()).unwrap_or(UNKNOWN).into(),
        gpus: lines.next().map_or_default(split_gpus),
    }
}

fn split_gpus(line: &str) -> Vec<String> {
    line.split('|')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn detect_macos() -> Machine {
    let version = output_of("sw_vers", &["-productVersion"]).unwrap_or_else(|| UNKNOWN.into());
    let displays = output_of("system_profiler", &["SPDisplaysDataType"]).unwrap_or_default();
    Machine {
        os_family: "macos".into(),
        os: format!("macOS {version}"),
        cpu: output_of("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_else(|| UNKNOWN.into()),
        gpus: displays
            .lines()
            .filter_map(|line| line.trim().strip_prefix("Chipset Model:"))
            .map(|name| name.trim().to_owned())
            .collect(),
    }
}

fn detect_linux() -> Machine {
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let os = release
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map_or_else(|| "Linux".to_owned(), |name| name.trim_matches('"').to_owned());
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpuinfo
        .lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|line| line.split_once(':'))
        .map_or_else(|| UNKNOWN.into(), |(_, name)| name.trim().to_owned());
    let pci = output_of("lspci", &[]).unwrap_or_default();
    Machine {
        os_family: "linux".into(),
        os,
        cpu,
        gpus: pci
            .lines()
            .filter(|line| line.contains("VGA compatible controller") || line.contains("3D controller"))
            .filter_map(|line| line.split_once(": ").map(|(_, name)| name.trim().to_owned()))
            .collect(),
    }
}

impl Machine {
    /// `windows-amd-ryzen-5-5600g`: the OS and the CPU model, without the marketing words.
    pub(crate) fn default_key(&self) -> String {
        let mut cpu = self.cpu.to_lowercase();
        for noise in ["(r)", "(tm)", " with radeon graphics", " processor", " cpu"] {
            cpu = cpu.replace(noise, "");
        }
        let slug: String = format!("{}-{cpu}", self.os_family)
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        slug.split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-")
    }
}

pub(crate) fn run_info(root: &Path) -> RunInfo {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let rustc = output_of("rustc", &["-vV"])
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("release: "))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| UNKNOWN.into());
    RunInfo {
        recorded_at: iso_utc(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
        ),
        commit: git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| UNKNOWN.into()),
        dirty: git(&["status", "--porcelain", "--", ".", ":(exclude)docs/bench"])
            .is_some_and(|status| !status.is_empty()),
        rustc,
    }
}

// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm), so no date crate is needed for one timestamp.
fn iso_utc(unix_seconds: u64) -> String {
    let days = i64::try_from(unix_seconds / 86_400).unwrap_or(0) + 719_468;
    let seconds_of_day = unix_seconds % 86_400;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_phase = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_phase + 2) / 5 + 1;
    let month = if month_phase < 10 {
        month_phase + 3
    } else {
        month_phase - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3600,
        seconds_of_day % 3600 / 60,
        seconds_of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_iso_8601() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_795_356_645), "2026-11-22T14:10:45Z");
    }

    #[test]
    fn the_default_key_is_the_os_and_the_cpu_model() {
        let machine = Machine {
            os_family: "windows".into(),
            os: "Windows 11".into(),
            cpu: "AMD Ryzen 5 5600G with Radeon Graphics".into(),
            gpus: Vec::new(),
        };
        assert_eq!(machine.default_key(), "windows-amd-ryzen-5-5600g");
        let apple = Machine {
            os_family: "macos".into(),
            cpu: "Apple M5".into(),
            ..machine
        };
        assert_eq!(apple.default_key(), "macos-apple-m5");
    }

    #[test]
    fn gpus_are_split_on_the_separator() {
        assert_eq!(
            split_gpus("AMD Radeon RX 9070 XT| AMD Radeon(TM) Graphics"),
            ["AMD Radeon RX 9070 XT", "AMD Radeon(TM) Graphics"]
        );
        assert_eq!(split_gpus(""), Vec::<String>::new());
    }
}
