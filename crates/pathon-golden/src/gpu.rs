use std::env;
use std::fmt::Display;

use pathon_rhi::Caps;

const REQUIRE_GPU_ENV: &str = "PATHON_REQUIRE_GPU";

/// Unwraps the GPU setup of a test, or reports a skip when the machine has no adapter.
///
/// With `PATHON_REQUIRE_GPU=1` (set by the CI jobs that must have a GPU) a missing adapter fails the test instead,
/// so a runner without Metal or WARP can never look green by accident. `is_missing_adapter` tells a missing adapter
/// from a real setup error, which always panics.
#[track_caller]
pub fn gpu_or_skip<T, E: Display>(
    test: &str,
    setup: Result<T, E>,
    is_missing_adapter: impl FnOnce(&E) -> bool,
) -> Option<T> {
    let error = match setup {
        Ok(value) => return Some(value),
        Err(error) => error,
    };
    assert!(
        is_missing_adapter(&error) && !env_flag(REQUIRE_GPU_ENV),
        "{test}: {error}"
    );
    announce_skip(test, &error.to_string());
    None
}

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| !matches!(value.as_str(), "" | "0" | "false"))
}

#[expect(
    clippy::print_stderr,
    reason = "a skipped test has to say so, libtest has no skip result"
)]
fn announce_skip(test: &str, reason: &str) {
    eprintln!("SKIPPED {test}: {reason}\nset {REQUIRE_GPU_ENV}=1 to fail instead of skipping");
    if env::var_os("GITHUB_ACTIONS").is_some() {
        eprintln!("::warning title=Golden test skipped::{test} did not run, no GPU adapter on this runner ({reason})");
    }
}

/// Says which adapter rendered a golden image; visible with `--nocapture` and the first thing to read when a backend
/// drifts.
#[expect(
    clippy::print_stderr,
    reason = "the log of a CI run has to name the adapter behind a result"
)]
pub fn announce_adapter(test: &str, caps: &Caps) {
    let adapter = &caps.adapter;
    eprintln!(
        "{test}: rendered on {} ({:?}, {:?}, driver {})",
        adapter.name, caps.backend, adapter.device_type, adapter.driver
    );
}
