//! `xldif`: the process boundary. Parses arguments, runs one command, and
//! writes exactly one stdout document: a JSON envelope, or plain text for
//! help, `--version`, and `--format text` views. stderr stays empty, even
//! when a command panics.

mod cli;
mod commands;
mod envelope;

use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::Mutex;

use clap::Parser as _;
use clap::error::ErrorKind;

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

/// Runs a command with a panic turned into an `internal_error` failure, so
/// a bug still ends in one JSON document, exit status 1, and an empty
/// stderr.
fn guarded(run: impl FnOnce() -> Result<commands::Output, Failure>) -> Result<commands::Output, Failure> {
    static PANIC: Mutex<Option<String>> = Mutex::new(None);
    // The hook replaces the default one, which writes to stderr, while the
    // command runs; it keeps the message and source location for the
    // envelope instead.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        if let Ok(mut slot) = PANIC.lock() {
            *slot = Some(info.to_string());
        }
    }));
    // Nothing the command touched is used again after a panic.
    let outcome = catch_unwind(AssertUnwindSafe(run));
    std::panic::set_hook(default_hook);
    outcome.unwrap_or_else(|_| {
        let detail = PANIC.lock().ok().and_then(|mut slot| slot.take()).unwrap_or_default();
        Err(Failure::internal(&detail.split_whitespace().collect::<Vec<_>>().join(" ")))
    })
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
    // Bare `xldif` is exactly `xldif --help`: the same parser renders both.
    let parse_args = if argv.len() == 1 { vec![argv[0].clone(), "--help".to_owned()] } else { argv.clone() };
    let cli = match Cli::try_parse_from(&parse_args) {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            write_stdout(&e.to_string());
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let rendered = e.to_string();
            let message =
                rendered.lines().next().unwrap_or("invalid arguments").trim_start_matches("error: ").to_owned();
            let command = command_name(&argv);
            let hint = if command.is_empty() || e.kind() == ErrorKind::InvalidSubcommand {
                "run `xldif --help` for commands".to_owned()
            } else {
                format!("run `xldif {command} --help` for options and examples")
            };
            let failure = Failure::usage(message, hint);
            write_stdout(&Envelope::failure(&command, &failure).render(pretty));
            return ExitCode::from(failure.exit_code());
        }
    };
    let name = cli.command.name();
    match guarded(|| commands::run(cli.command)) {
        Ok(commands::Output::Text(text)) => {
            write_stdout(&text);
            ExitCode::SUCCESS
        }
        Ok(commands::Output::Json { result, warnings }) => {
            write_stdout(&Envelope::success(name, result, &warnings).render(cli.pretty));
            ExitCode::SUCCESS
        }
        Err(failure) => {
            write_stdout(&Envelope::failure(name, &failure).render(cli.pretty));
            ExitCode::from(failure.exit_code())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_panic_becomes_an_internal_error() {
        let failure = super::guarded(|| panic!("boom")).err().expect("a failure");
        assert_eq!((failure.code, failure.exit_code()), ("internal_error", 1));
        assert!(failure.message.contains("boom") && failure.message.contains("main.rs"), "{}", failure.message);
    }
}
