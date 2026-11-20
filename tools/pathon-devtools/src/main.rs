// pathon-devtools [--demo | --file SNAPSHOT.json | --connect ADDRESS]   (default: --connect 127.0.0.1:7425)

use std::fs;
use std::process::ExitCode;

use pathon_devtools::ui::{self, App};
use pathon_devtools::{DEFAULT_ADDRESS, Snapshot, connect, spawn_demo};

#[expect(clippy::print_stderr, reason = "a CLI reports bad arguments to the console")]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (sender, receiver) = crossbeam_channel::unbounded();
    let app = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--demo"] => {
            spawn_demo(sender);
            App::new("demo")
        }
        ["--file", path] => {
            let snapshot = fs::read_to_string(path)
                .map_err(|error| error.to_string())
                .and_then(|text| Snapshot::from_line(text.trim()).map_err(|error| error.to_string()));
            match snapshot {
                Ok(snapshot) => {
                    let mut app = App::new(format!("plik {path}"));
                    app.connected = true;
                    app.on_snapshot(snapshot);
                    app
                }
                Err(error) => {
                    eprintln!("cannot read {path}: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        [] => {
            connect(DEFAULT_ADDRESS.to_owned(), sender);
            App::new(DEFAULT_ADDRESS)
        }
        ["--connect", address] => {
            connect((*address).to_owned(), sender);
            App::new(*address)
        }
        _ => {
            eprintln!("usage: pathon-devtools [--demo | --file SNAPSHOT.json | --connect ADDRESS]");
            return ExitCode::FAILURE;
        }
    };
    match ui::run(&receiver, app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("terminal error: {error}");
            ExitCode::FAILURE
        }
    }
}
