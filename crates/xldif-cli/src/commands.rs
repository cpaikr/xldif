//! Runs one parsed command against the library.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;
use xldif::cellref::Range;
use xldif::views::{self, Cells, Inspection, JsonValue};
use xldif::{Error, ErrorCode};

use crate::cli::{Command, Format};
use crate::envelope::{Failure, Warning};

pub enum Output {
    Text(String),
    Json { result: Value, warnings: Vec<Warning> },
}

fn json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("view types always serialize")
}

fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn run(command: Command) -> Result<Output, Failure> {
    match command {
        Command::Inspect { book, format } => {
            let wb = xldif::read_path(&book)?;
            let (inspection, warnings) = views::inspect(&file_label(&book), &wb);
            let warnings: Vec<Warning> = warnings.into_iter().map(Warning::from).collect();
            Ok(match format {
                Format::Json => Output::Json { result: json(&inspection), warnings },
                Format::Text => Output::Text(inspect_text(&inspection, &warnings)),
            })
        }
        Command::Cells { book, sheet, range, limit, format } => {
            let range = range
                .map(|r| {
                    Range::parse(&r).ok_or_else(|| {
                        Error::new(ErrorCode::InvalidArgument, format!("--range {r:?} is not a cell or range"))
                            .with_hint("use A1 notation, such as B5 or A1:F40")
                    })
                })
                .transpose()?;
            let wb = xldif::read_path(&book)?;
            let found = views::find_sheet(&wb, &sheet)?;
            let listed = views::cells(found, range, (limit > 0).then_some(limit));
            let warnings: Vec<Warning> = wb.warnings.iter().cloned().map(Warning::from).collect();
            Ok(match format {
                Format::Json => Output::Json { result: json(&listed), warnings },
                Format::Text => Output::Text(cells_text(&listed)),
            })
        }
    }
}

fn inspect_text(i: &Inspection, warnings: &[Warning]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} ({:?}, {} dates, calculation {:?}{})",
        i.file,
        i.format,
        if i.date1904 { "1904" } else { "1900" },
        i.calculation.mode,
        if i.calculation.full_calc_on_load { ", recalculates on open" } else { "" }
    );
    for s in &i.sheets {
        let _ = writeln!(
            out,
            "{}\t{}\t{:?}\t{:?}\t{}\tcells {}\tformulas {}",
            s.index + 1,
            s.name,
            s.kind,
            s.visibility,
            s.used_range.as_deref().unwrap_or("-"),
            s.cells,
            s.formulas
        );
    }
    for n in &i.defined_names {
        let scope = n.scope.as_deref().unwrap_or("workbook");
        let _ = writeln!(out, "name\t{}\t{scope}\t{}", n.name, n.refers_to);
    }
    for w in warnings {
        let _ = writeln!(out, "warning\t{}\t{}", w.code, w.message);
    }
    out
}

fn cells_text(c: &Cells) -> String {
    let mut out = String::new();
    for cell in &c.cells {
        let value = match &cell.value {
            None => String::new(),
            Some(JsonValue::Number(n)) => n.to_string(),
            Some(JsonValue::Text(t)) => t.replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r"),
            Some(JsonValue::Bool(b)) => b.to_string(),
        };
        let _ = write!(out, "{}\t{}\t{value}", cell.cell, cell.kind);
        if let Some(f) = &cell.formula {
            let _ = write!(out, "\t={f}");
        }
        out.push('\n');
    }
    if c.omitted > 0 {
        let _ = writeln!(out, "... {} more cells (raise --limit, or 0 for all)", c.omitted);
    }
    out
}
