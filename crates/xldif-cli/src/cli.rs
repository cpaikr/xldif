//! The command model. Help text here is the CLI's only command inventory.

use clap::Parser;

const ROOT_AFTER: &str = "\
Status:
  Scaffold only. `inspect` and `compare` arrive with the reader (see PLAN.md).

Output:
  Commands print one JSON object to stdout: {ok, command, result, warnings,
  error, metadata}. Exit status 0 = ok, 1 = failure, 2 = invalid invocation.
  Failures are always JSON; --pretty indents JSON. Help and --version are
  plain text.";

#[derive(Debug, Parser)]
#[command(
    name = "xldif",
    bin_name = "xldif",
    version,
    about = "Compare Excel workbooks: values, formulas, formatting, and workbook objects.",
    after_help = ROOT_AFTER
)]
pub struct Cli {
    /// Indent JSON output.
    #[arg(long, global = true)]
    pub pretty: bool,
}
