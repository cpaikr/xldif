//! `xldif`: the process boundary. Parses arguments, runs one command, and
//! writes exactly one stdout document: a JSON envelope, or plain text for
//! help and `--version`. stderr stays empty.

mod cli;
mod envelope;

use std::io::Write as _;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory as _, Parser as _};

use crate::cli::Cli;
use crate::envelope::{Envelope, Failure};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn write_stdout(text: &str) {
    let mut out = std::io::stdout().lock();
    // A closed pipe leaves nothing useful to report.
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

fn command_name(argv: &[String]) -> String {
    argv.iter().skip(1).find(|a| !a.starts_with('-')).cloned().unwrap_or_default()
}

fn root_help() -> String {
    Cli::command().render_help().to_string()
}

fn main() -> ExitCode {
    let argv_os: Vec<std::ffi::OsString> = std::env::args_os().collect();
    // Arguments that are not valid Unicode cannot name anything this tool
    // matches, so they are an invalid invocation rather than a panic.
    let Some(argv) = argv_os.iter().map(|a| a.to_str().map(str::to_owned)).collect::<Option<Vec<String>>>() else {
        let failure =
            Failure::usage("arguments must be valid Unicode".to_owned(), "run `xldif --help` for commands".to_owned());
        write_stdout(&Envelope::failure("", &failure).render(false));
        return ExitCode::from(failure.exit_code());
    };
    let pretty = argv.iter().any(|a| a == "--pretty");
    match Cli::try_parse_from(&argv) {
        // No commands exist yet, so any successful parse (bare `xldif`
        // included) is a request for help. Bare `xldif` stays exactly
        // `xldif --help` once commands arrive.
        Ok(_) => {
            write_stdout(&root_help());
            ExitCode::SUCCESS
        }
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            write_stdout(&e.to_string());
            ExitCode::SUCCESS
        }
        Err(e) => {
            let rendered = e.to_string();
            let message =
                rendered.lines().next().unwrap_or("invalid arguments").trim_start_matches("error: ").to_owned();
            let failure = Failure::usage(message, "run `xldif --help` for commands".to_owned());
            write_stdout(&Envelope::failure(&command_name(&argv), &failure).render(pretty));
            ExitCode::from(failure.exit_code())
        }
    }
}
