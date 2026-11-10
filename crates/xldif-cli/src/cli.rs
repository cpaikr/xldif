//! The command model. Help text here is the CLI's only command inventory.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

const ROOT_AFTER: &str = "\
Typical flow:
  1. xldif inspect book.xlsx                  sheets, ranges, counts (no contents)
  2. xldif cells book.xlsx --sheet BS --range A1:F40 --format text

Output:
  Commands print one JSON object to stdout: {ok, command, result, warnings,
  error, metadata}. Exit status 0 = ok, 1 = failure, 2 = invalid invocation.
  Views accept --format text for a compact listing; failures stay JSON.
  --pretty indents JSON. Help and --version are plain text.

Values are the results Excel last saved; xldif never recalculates. A warning
says when they may be stale (manual calculation, recalculate-on-open, or
formulas without a stored result).";

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

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Text,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Workbook structure without cell contents: sheets, kinds, visibility,
    /// used ranges, cell and formula counts, defined names, calculation
    /// settings, and the date system.
    Inspect {
        /// The workbook (.xlsx or .xlsm).
        book: PathBuf,
        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,
    },
    /// Cells with a value or formula, row by row: address, type, stored
    /// value, and formula text.
    #[command(after_help = "\
Examples:
  xldif cells book.xlsx --sheet BS
  xldif cells book.xlsx --sheet BS --range B10:F30 --format text
  xldif cells book.xlsx --sheet BS --limit 0      every cell

Shared formulas show their text on the anchor cell only for now; the cells
that share it show an empty formula with formulaKind \"shared\".")]
    Cells {
        /// The workbook (.xlsx or .xlsm).
        book: PathBuf,
        /// The sheet's exact name.
        #[arg(long)]
        sheet: String,
        /// Only cells inside this range, such as A1:F40 or B5.
        #[arg(long)]
        range: Option<String>,
        /// The most cells to list; 0 lists every cell.
        #[arg(long, default_value_t = 1000)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,
    },
}

impl Command {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Inspect { .. } => "inspect",
            Self::Cells { .. } => "cells",
        }
    }
}
