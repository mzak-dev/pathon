use std::io::Write;

pub(crate) fn say(line: &str) {
    // A closed pipe (`| head`) is not an error worth a panic.
    writeln!(std::io::stdout().lock(), "{line}").ok();
}

pub(crate) fn warn(line: &str) {
    writeln!(std::io::stderr().lock(), "{line}").ok();
}
