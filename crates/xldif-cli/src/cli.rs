//! The command model. Help text here is the CLI's only command inventory.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

const ROOT_AFTER: &str = "\
Typical flow:
  1. xldif inspect book.xlsx                  sheets, ranges, counts (no contents)
  2. xldif compare old.xlsx new.xlsx          what changed, rows and columns aligned
  3. xldif compare old.xlsx new.xlsx --sheet BS --kind value --format text
  4. xldif cells new.xlsx --sheet BS --range A1:F40 --format text

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
pub enum Kind {
    Value,
    Formula,
    Format,
    Row,
    Column,
    Merge,
    Comment,
    Validation,
    ConditionalFormat,
    Hyperlink,
    Sheet,
    Name,
    Theme,
    ExternalLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Align {
    /// Match rows and columns by labels and values.
    Content,
    /// Compare A1 with A1.
    Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompareFormat {
    Json,
    Text,
    /// A self-contained side-by-side report; needs -o.
    Html,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Workbook structure without cell contents: sheets, kinds, visibility,
    /// used ranges, cell and formula counts, defined names, calculation
    /// settings, the date system, and the files external links name
    /// (`externalLinks`, in the order formulas number them: `[1]` is the
    /// first).
    Inspect {
        /// The workbook (.xlsx or .xlsm).
        book: PathBuf,
        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,
    },
    /// What changed between two workbooks. Sheets pair by name (or by
    /// content when renamed); rows and columns pair by labels and values, so
    /// an inserted row is one `row` change. Values are compared on paired
    /// rows and columns; formulas are compared after Excel's adjustment for
    /// inserted and deleted rows; formats, row and column layout, merges,
    /// comments, validation, conditional formats, hyperlinks, sheet
    /// visibility, defined names, the theme, and the files external links
    /// name are compared too.
    #[command(after_help = "Examples:
  xldif compare v1.xlsx v2.xlsx
  xldif compare v1.xlsx v2.xlsx --sheet BS --kind value --format text
  xldif compare v1.xlsx v2.xlsx --limit 0 --tolerance 0.005
  xldif compare v1.xlsx v2.xlsx --format html -o report.html

Reading the result:
  summary counts every change; `listed` and `omitted` say what the filters and
  --limit left out, by sheet and kind. Each change has a stable id, a kind
  (value, formula, format, row, column, merge, comment, validation,
  conditional-format, hyperlink, sheet, name, theme, external-link), a status (changed, onlyBase, onlyOther, moved), base
  and other locations, a numeric delta, and inferred row/column labels. A
  formula change shows both formulas and `expected`: the base formula as
  Excel would have adjusted it for inserted and deleted rows and columns.
  The same change filled down a column folds into one change with a range;
  formulas that differ only by that adjustment are counted in
  summary.formulasAdjusted and not listed. A reference to another workbook
  ([1]Prices!A1) is compared by the file its number names, not by the number;
  an externalLink change (id externalLink:<base number>:<other number>) names
  a file linked on one side only, or the two files one number names.
  alignment.rows/cols are runs [base start, other start, length].")]
    Compare {
        /// The earlier workbook (left side).
        base: PathBuf,
        /// The later workbook (right side).
        other: PathBuf,
        /// Only the sheet with this name in either workbook.
        #[arg(long)]
        sheet: Option<String>,
        /// Only these kinds of change.
        #[arg(long, value_enum, value_delimiter = ',')]
        kind: Vec<Kind>,
        /// The most changes to list; 0 lists every change.
        #[arg(long, default_value_t = 500)]
        limit: usize,
        /// Also list sheets with no changes.
        #[arg(long)]
        all: bool,
        /// Numbers closer than this are equal.
        #[arg(long, default_value_t = 0.0)]
        tolerance: f64,
        #[arg(long, value_enum, default_value_t = Align::Content)]
        align: Align,
        #[arg(long, value_enum, default_value_t = CompareFormat::Json)]
        format: CompareFormat,
        /// Write the report here (required for html); stdout then carries a
        /// summary envelope.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        /// Replace an existing output file (never a compared workbook).
        #[arg(long)]
        force: bool,
    },
    /// Cells with a value or formula, row by row: address, type, stored
    /// value, and formula text.
    #[command(after_help = "\
Examples:
  xldif cells book.xlsx --sheet BS
  xldif cells book.xlsx --sheet BS --range B10:F30 --format text
  xldif cells book.xlsx --sheet BS --limit 0      every cell

A shared formula shows, on every cell that shares it, the formula as that
cell reads it (formulaKind \"shared\"); formulaRange appears on the anchor.")]
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
            Self::Compare { .. } => "compare",
            Self::Cells { .. } => "cells",
        }
    }
}
