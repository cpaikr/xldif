//! Runs one parsed command against the library.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;
use xldif::cellref::Range;
use xldif::diff::{self, AlignMode, Change, CompareOptions, Filter, Listing, Location};
use xldif::views::{self, Cells, Inspection, JsonValue};
use xldif::{Error, ErrorCode};

use crate::cli::{Align, Command, CompareFormat, Format, Kind};
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

pub fn run(command: Command, unsaved: bool) -> Result<Output, Failure> {
    match command {
        Command::Inspect { book, format } => {
            let wb = crate::excel::read(&book, unsaved)?;
            let (inspection, warnings) = views::inspect(&file_label(&book), &wb);
            let warnings: Vec<Warning> = warnings.into_iter().map(Warning::from).collect();
            Ok(match format {
                Format::Json => Output::Json { result: json(&inspection), warnings },
                Format::Text => Output::Text(inspect_text(&inspection, &warnings)),
            })
        }
        Command::Compare { base, other, sheet, kind, limit, all, tolerance, align, format, output, force } => {
            if format == CompareFormat::Html && output.is_none() {
                return Err(Error::new(ErrorCode::InvalidArgument, "--format html needs -o FILE")
                    .with_hint("for example: xldif compare a.xlsx b.xlsx --format html -o report.html")
                    .into());
            }
            if !(tolerance >= 0.0 && tolerance.is_finite()) {
                return Err(
                    Error::new(ErrorCode::InvalidArgument, "--tolerance must be a finite number of 0 or more").into()
                );
            }
            let (b, o) = (crate::excel::read(&base, unsaved)?, crate::excel::read(&other, unsaved)?);
            let options = CompareOptions {
                tolerance,
                align: match align {
                    Align::Content => AlignMode::Content,
                    Align::Position => AlignMode::Position,
                },
            };
            if let Some(name) = &sheet
                && b.sheet(name).is_none()
                && o.sheet(name).is_none()
            {
                let names: std::collections::BTreeSet<&str> =
                    b.sheets.iter().chain(&o.sheets).map(|s| s.name.as_str()).collect();
                return Err(Error::new(
                    ErrorCode::InvalidArgument,
                    format!("neither workbook has a sheet named {name:?}"),
                )
                .with_hint(format!("sheets: {}", views::quoted_names(names.into_iter())))
                .into());
            }
            let comparison = diff::compare(&file_label(&base), &b, &file_label(&other), &o, options);
            let report_warnings = comparison.warnings.clone();
            let warnings: Vec<Warning> = report_warnings.iter().cloned().map(Warning::from).collect();
            let kinds = kind
                .into_iter()
                .map(|k| match k {
                    Kind::Value => diff::Kind::Value,
                    Kind::Formula => diff::Kind::Formula,
                    Kind::Format => diff::Kind::Format,
                    Kind::Merge => diff::Kind::Merge,
                    Kind::Comment => diff::Kind::Comment,
                    Kind::Validation => diff::Kind::Validation,
                    Kind::ConditionalFormat => diff::Kind::ConditionalFormat,
                    Kind::Hyperlink => diff::Kind::Hyperlink,
                    Kind::Sheet => diff::Kind::Sheet,
                    Kind::Name => diff::Kind::Name,
                    Kind::Theme => diff::Kind::Theme,
                    Kind::ExternalLink => diff::Kind::ExternalLink,
                    Kind::Row => diff::Kind::Row,
                    Kind::Column => diff::Kind::Column,
                })
                .collect();
            if format == CompareFormat::Html {
                // The report shows every change and every sheet; a limit would
                // silently hide changes from a reader who cannot see it.
                let listing = comparison.list(&Filter { sheet, kinds, limit: None, all: true });
                let html = xldif::html::render_html(&listing, &b, &o, &report_warnings);
                let path = output.expect("checked above");
                write_output(&path, html.as_bytes(), &[&base, &other], force)?;
                let result = serde_json::json!({
                    "written": path.display().to_string(),
                    "bytes": html.len(),
                    "summary": listing.summary,
                    "listed": listing.listed,
                });
                return Ok(Output::Json { result, warnings });
            }
            let listing = comparison.list(&Filter { sheet, kinds, limit: (limit > 0).then_some(limit), all });
            let rendered = match format {
                CompareFormat::Text => Output::Text(compare_text(&listing, &warnings)),
                _ => Output::Json { result: json(&listing), warnings: warnings.clone() },
            };
            match (output, rendered) {
                (None, rendered) => Ok(rendered),
                (Some(path), Output::Text(text)) => {
                    write_output(&path, text.as_bytes(), &[&base, &other], force)?;
                    Ok(Output::Json {
                        result: serde_json::json!({"written": path.display().to_string(), "bytes": text.len()}),
                        warnings,
                    })
                }
                (Some(path), Output::Json { result, warnings }) => {
                    let text = serde_json::to_string_pretty(&result).expect("JSON values always serialize");
                    write_output(&path, text.as_bytes(), &[&base, &other], force)?;
                    Ok(Output::Json {
                        result: serde_json::json!({"written": path.display().to_string(), "bytes": text.len()}),
                        warnings,
                    })
                }
            }
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
            let wb = crate::excel::read(&book, unsaved)?;
            let found = views::find_sheet(&wb, &sheet)?;
            let listed = views::cells(&wb, found, range, (limit > 0).then_some(limit));
            let warnings: Vec<Warning> = wb.warnings.iter().cloned().map(Warning::from).collect();
            Ok(match format {
                Format::Json => Output::Json { result: json(&listed), warnings },
                Format::Text => Output::Text(cells_text(&listed, &warnings)),
            })
        }
    }
}

/// Writes a report through a temporary file and a rename, refusing to
/// replace a compared workbook at all and any other file without --force.
fn write_output(path: &Path, bytes: &[u8], inputs: &[&Path], force: bool) -> Result<(), Failure> {
    let same_file = |a: &Path, b: &Path| match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    };
    if inputs.iter().any(|input| same_file(path, input)) {
        return Err(Error::new(ErrorCode::OutputExists, "refusing to write the report over a compared workbook").into());
    }
    if path.exists() && !force {
        return Err(Error::new(ErrorCode::OutputExists, format!("{} already exists", path.display()))
            .with_hint("pass --force to replace it")
            .into());
    }
    if path.is_dir() {
        return Err(Error::new(ErrorCode::InvalidArgument, format!("{} is a directory", path.display()))
            .with_hint("give -o a file name, such as report.html")
            .into());
    }
    let io =
        |e: std::io::Error| Failure::from(Error::new(ErrorCode::IoError, format!("writing {}: {e}", path.display())));
    // A fresh temporary file beside the target (create_new never touches an
    // existing file, so it cannot clobber an input), renamed over the target
    // only once it is complete; rename replaces an existing file atomically.
    let mut attempt = 0u32;
    let (temp, mut file) = loop {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".xldif-{}-{attempt}.tmp", std::process::id()));
        let temp = std::path::PathBuf::from(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => attempt += 1,
            Err(e) => return Err(io(e)),
        }
    };
    let written = std::io::Write::write_all(&mut file, bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| std::fs::rename(&temp, path)) {
        let _ = std::fs::remove_file(&temp);
        return Err(io(e));
    }
    Ok(())
}

fn inspect_text(i: &Inspection, warnings: &[Warning]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} ({:?}, {} dates, calculation {:?}{})",
        escape(&i.file),
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
            escape(&s.name),
            s.kind,
            s.visibility,
            s.used_range.as_deref().unwrap_or("-"),
            s.cells,
            s.formulas
        );
    }
    for n in &i.defined_names {
        let scope = escape(n.scope.as_deref().unwrap_or("workbook"));
        let _ = writeln!(out, "name\t{}\t{scope}\t{}", escape(&n.name), escape(&n.refers_to));
    }
    for (n, target) in (1..).zip(&i.external_links) {
        let _ = writeln!(out, "link\t[{n}]\t{}", escape(target));
    }
    write_warnings(&mut out, warnings);
    out
}

fn write_warnings(out: &mut String, warnings: &[Warning]) {
    for w in warnings {
        let _ = writeln!(out, "warning\t{}\t{}", w.code, escape(&w.message));
    }
}

/// Text from a workbook, made safe for a one-record-per-line listing in a
/// terminal: backslashes, tabs, and line breaks are escaped so they cannot
/// split or blur a line, and every other control character (such as ESC,
/// which could start a terminal escape sequence) is shown as `\u{..}`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

fn cells_text(c: &Cells, warnings: &[Warning]) -> String {
    let mut out = String::new();
    for cell in &c.cells {
        let value = match &cell.value {
            None => String::new(),
            Some(JsonValue::Number(n)) => n.to_string(),
            Some(JsonValue::Text(t)) => escape(t),
            Some(JsonValue::Bool(b)) => b.to_string(),
        };
        let _ = write!(out, "{}\t{}\t{value}", cell.cell, cell.kind);
        if let Some(shown) = &cell.display {
            let _ = write!(out, " [{}]", escape(shown));
        }
        if let Some(f) = &cell.formula {
            let _ = write!(out, "\t={}", escape(f));
        }
        out.push('\n');
    }
    if c.cells.is_empty() && c.omitted == 0 {
        out.push_str("(no cells)\n");
    }
    if c.omitted > 0 {
        let _ = writeln!(out, "... {} more cells (raise --limit, or 0 for all)", c.omitted);
    }
    write_warnings(&mut out, warnings);
    out
}

fn value_text(v: Option<&JsonValue>) -> String {
    match v {
        None => "(empty)".to_owned(),
        Some(JsonValue::Number(n)) => n.to_string(),
        // Debug quoting also escapes control characters.
        Some(JsonValue::Text(t)) => format!("{t:?}"),
        Some(JsonValue::Bool(b)) => b.to_string(),
    }
}

/// A value with what the cell shows, when that differs: `45382 [31-Mar-24]`.
fn located_value(l: Option<&Location>) -> String {
    let value = value_text(l.and_then(|l| l.value.as_ref()));
    match l.and_then(|l| l.display.as_deref()) {
        Some(shown) => format!("{value} [{}]", escape(shown)),
        None => value,
    }
}

fn reference(l: Option<&Location>) -> String {
    l.and_then(|l| l.place.reference()).unwrap_or_else(|| "-".to_owned())
}

fn change_text(c: &Change) -> String {
    let mut line = format!(
        "{}\t{}\t{}→{}",
        c.kind.as_str(),
        c.status.as_str(),
        reference(c.base.as_ref()),
        reference(c.other.as_ref())
    );
    if c.kind == diff::Kind::Value {
        let _ = write!(line, "\t{} → {}", located_value(c.base.as_ref()), located_value(c.other.as_ref()));
        if let Some(d) = c.delta {
            let _ = write!(line, " ({d:+})");
        }
    }
    if c.kind == diff::Kind::Formula {
        let shown = |l: Option<&Location>| {
            l.and_then(|l| l.formula.as_deref()).map_or_else(|| "(none)".to_owned(), |f| format!("={}", escape(f)))
        };
        let _ = write!(line, "\t{} → {}", shown(c.base.as_ref()), shown(c.other.as_ref()));
        if let Some(e) = &c.expected {
            let _ = write!(line, " (expected ={})", escape(e));
        }
    }
    for p in &c.properties {
        let _ = write!(line, "\t{}: {} → {}", p.name, escape(&p.base), escape(&p.other));
    }
    if c.properties.is_empty()
        && let Some(text) = c.other.as_ref().or(c.base.as_ref()).and_then(|l| l.text.as_deref())
    {
        let _ = write!(line, "\t{}", escape(text));
    }
    if let Some(l) = &c.labels {
        let parts: Vec<&str> = [l.row.as_deref(), l.column.as_deref()].into_iter().flatten().collect();
        if !parts.is_empty() {
            let _ = write!(line, "\t{}", escape(&parts.join(" | ")));
        }
    }
    line
}

fn compare_text(l: &Listing, warnings: &[Warning]) -> String {
    let mut out = String::new();
    let s = &l.summary;
    let _ = writeln!(
        out,
        "{} → {}: {} sheet(s) changed, {} only in base, {} only in other, {} renamed; rows +{} -{} moved {} restyled {}; columns +{} -{} moved {} restyled {}; {} value, {} formula, {} format, {} object change(s)",
        escape(&l.base.file),
        escape(&l.other.file),
        s.sheets_changed,
        s.sheets_only_base,
        s.sheets_only_other,
        s.sheets_renamed,
        s.rows_only_other,
        s.rows_only_base,
        s.rows_moved,
        s.rows_changed,
        s.columns_only_other,
        s.columns_only_base,
        s.columns_moved,
        s.columns_changed,
        s.value,
        s.formula,
        s.format,
        s.objects
    );
    for sheet in &l.sheets {
        let name = match (&sheet.base, &sheet.other) {
            (Some(b), Some(o)) if b != o => format!("{} → {}", escape(b), escape(o)),
            (Some(n), _) | (None, Some(n)) => escape(n),
            (None, None) => String::new(),
        };
        let moved = if sheet.moved { ", moved" } else { "" };
        let _ = writeln!(out, "\n[{name}] {}{moved}", sheet.status.as_str());
        for c in &sheet.changes {
            let _ = writeln!(out, "{}", change_text(c));
        }
    }
    if !l.objects.is_empty() {
        out.push_str("\n[workbook]\n");
        for c in &l.objects {
            // Workbook objects have no cell; their id names them (name:Rate).
            let _ = writeln!(out, "{}\t{}", escape(&c.id), change_text(c));
        }
    }
    let omitted: usize = l.omitted.values().flat_map(|k| k.values()).sum();
    if omitted > 0 {
        let _ = writeln!(out, "\n... {omitted} more change(s) left out (narrow with --sheet/--kind, or --limit 0)");
    }
    write_warnings(&mut out, warnings);
    out
}
