//! Reads a workbook package into the [`Workbook`] model: sheet order,
//! visibility and kind, defined names, calculation settings, the date
//! system, and every written cell's value, formula, and style index.
//!
//! Formatting resolution, comments, and other objects arrive in M3.

use std::io::{Read, Seek};
use std::path::Path;

use crate::cellref::{MAX_COL, MAX_ROW, parse_cell};
use crate::model::{
    BookFormat, CalcMode, Calculation, Cell, DefinedName, Formula, FormulaKind, Row, Sheet, SheetKind, Value,
    Visibility, Warning, Workbook,
};
use crate::package::{Package, Relationship};
use crate::xml::{Event, Parser, decode_xstring};
use crate::{Error, ErrorCode};

/// Written cells across one workbook, a bound on memory (~100 bytes each).
pub const MAX_CELLS: usize = 10_000_000;

pub fn read_path(path: &Path) -> Result<Workbook, Error> {
    read_package(&mut Package::open(path)?)
}

pub fn read_bytes(bytes: Vec<u8>) -> Result<Workbook, Error> {
    read_package(&mut Package::from_bytes(bytes)?)
}

fn missing(part: &str) -> Error {
    Error::new(ErrorCode::InvalidXml, format!("the workbook has no {part} part"))
}

/// A `<sheet>` entry of `workbook.xml` before its part is read.
struct SheetEntry {
    name: String,
    visibility: Visibility,
    rel_id: String,
}

/// Defined names keep their `localSheetId` until sheet names are known.
struct NameEntry {
    name: String,
    local_sheet: Option<usize>,
    refers_to: String,
    hidden: bool,
}

struct WorkbookPart {
    sheets: Vec<SheetEntry>,
    names: Vec<NameEntry>,
    date1904: bool,
    calculation: Calculation,
}

fn read_package<R: Read + Seek>(pkg: &mut Package<R>) -> Result<Workbook, Error> {
    let root = pkg.relationships("")?;
    let workbook_part = root
        .iter()
        .find(|r| r.kind == "officeDocument")
        .map(|r| r.target.clone())
        .ok_or_else(|| missing("officeDocument relationship"))?;
    let content_type = pkg.content_type(&workbook_part)?.unwrap_or_default();
    if workbook_part.to_ascii_lowercase().ends_with(".bin") || content_type.contains("sheet.binary") {
        return Err(Error::new(ErrorCode::UnsupportedFormat, "binary .xlsb workbooks are not supported")
            .with_hint("save the workbook from Excel as .xlsx, then compare that copy"));
    }
    let format = if content_type.contains("macroEnabled") { BookFormat::Xlsm } else { BookFormat::Xlsx };

    let text = pkg.read_text(&workbook_part)?.ok_or_else(|| missing(&workbook_part))?;
    let wb = parse_workbook(&text, &workbook_part)?;
    let rels = pkg.relationships(&workbook_part)?;
    let target = |id: &str| rels.iter().find(|r| r.id == id);

    let shared = match rels.iter().find(|r| r.kind == "sharedStrings") {
        Some(rel) => match pkg.read_text(&rel.target)? {
            Some(text) => parse_shared_strings(&text, &rel.target)?,
            None => Vec::new(),
        },
        None => Vec::new(),
    };

    let mut warnings = Vec::new();
    let mut budget = MAX_CELLS;
    let mut sheets = Vec::with_capacity(wb.sheets.len());
    for entry in &wb.sheets {
        let rel: Option<&Relationship> = target(&entry.rel_id);
        let kind = match rel.map(|r| r.kind.as_str()) {
            Some("chartsheet") => SheetKind::Chartsheet,
            Some("dialogsheet") => SheetKind::Dialogsheet,
            Some("xlMacrosheet" | "xlIntlMacrosheet" | "macrosheet") => SheetKind::Macrosheet,
            _ => SheetKind::Worksheet,
        };
        let mut sheet =
            Sheet { name: entry.name.clone(), kind, visibility: entry.visibility, dimension: None, rows: Vec::new() };
        let text = match rel {
            Some(rel) => pkg.read_text(&rel.target)?,
            None => None,
        };
        match (text, rel) {
            (Some(text), Some(rel)) if kind != SheetKind::Chartsheet => {
                parse_sheet(&text, &rel.target, &shared, &mut sheet, &mut budget, &mut warnings)?;
            }
            (None, _) => warnings.push(Warning {
                code: "sheet_part_missing",
                message: format!("sheet {:?} has no readable part; it is treated as empty", entry.name),
            }),
            _ => {}
        }
        sheets.push(sheet);
    }

    let defined_names = wb
        .names
        .into_iter()
        .map(|n| DefinedName {
            scope: n.local_sheet.and_then(|i| sheets.get(i)).map(|s| s.name.clone()),
            name: n.name,
            refers_to: n.refers_to,
            hidden: n.hidden,
        })
        .collect();
    Ok(Workbook { format, date1904: wb.date1904, calculation: wb.calculation, sheets, defined_names, warnings })
}

fn truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1" | "true"))
}

fn parse_workbook(text: &str, part: &str) -> Result<WorkbookPart, Error> {
    let mut p = Parser::new(text, part);
    let mut wb = WorkbookPart {
        sheets: Vec::new(),
        names: Vec::new(),
        date1904: false,
        calculation: Calculation { mode: CalcMode::Auto, full_calc_on_load: false },
    };
    loop {
        match p.next()? {
            Event::Start(e) => match e.name.as_str() {
                "workbookPr" => wb.date1904 = truthy(e.attr("date1904")),
                "calcPr" => {
                    wb.calculation.mode = match e.attr("calcMode") {
                        Some("manual") => CalcMode::Manual,
                        Some("autoNoTable") => CalcMode::AutoNoTable,
                        _ => CalcMode::Auto,
                    };
                    wb.calculation.full_calc_on_load = truthy(e.attr("fullCalcOnLoad"));
                }
                "sheet" => {
                    let (Some(name), Some(rel_id)) = (e.attr("name"), e.attr("id")) else {
                        return Err(Error::new(ErrorCode::InvalidXml, format!("{part}: a sheet lacks name or r:id")));
                    };
                    let visibility = match e.attr("state") {
                        Some("hidden") => Visibility::Hidden,
                        Some("veryHidden") => Visibility::VeryHidden,
                        _ => Visibility::Visible,
                    };
                    wb.sheets.push(SheetEntry { name: name.to_owned(), visibility, rel_id: rel_id.to_owned() });
                }
                "definedName" => {
                    let name = e.attr("name").unwrap_or_default().to_owned();
                    let local_sheet = e.attr("localSheetId").and_then(|v| v.parse().ok());
                    let hidden = truthy(e.attr("hidden"));
                    let refers_to = p.text_until_end(&[])?;
                    wb.names.push(NameEntry { name, local_sheet, refers_to, hidden });
                }
                // Nothing else in workbook.xml matters yet; skip whole subtrees
                // so their children (extLst, fileVersion…) cost nothing.
                "workbook" | "sheets" | "definedNames" => {}
                _ => p.skip_element()?,
            },
            Event::Eof => return Ok(wb),
            _ => {}
        }
    }
}

/// The shared string table: each `<si>` flattened to plain text. Phonetic
/// guides (`<rPh>`, furigana-style readings) are not part of the text.
fn parse_shared_strings(text: &str, part: &str) -> Result<Vec<String>, Error> {
    let mut p = Parser::new(text, part);
    let mut out = Vec::new();
    loop {
        match p.next()? {
            Event::Start(e) if e.name == "si" => out.push(decode_xstring(&p.text_until_end(&["rPh", "phoneticPr"])?)),
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// The parts of one `<c>` element as read, before typing its value.
#[derive(Default)]
struct RawCell {
    value: Option<String>,
    inline: Option<String>,
    formula: Option<Formula>,
}

fn parse_sheet(
    text: &str,
    part: &str,
    shared: &[String],
    sheet: &mut Sheet,
    budget: &mut usize,
    warnings: &mut Vec<Warning>,
) -> Result<(), Error> {
    let mut p = Parser::new(text, part);
    let mut row: Option<Row> = None;
    let mut next_row = 1u32;
    let mut next_col = 1u32;
    let mut bad_values = 0usize;
    loop {
        match p.next()? {
            Event::Start(e) => match e.name.as_str() {
                "dimension" => sheet.dimension = e.attr("ref").map(str::to_owned),
                "row" => {
                    if let Some(done) = row.take() {
                        sheet.rows.push(done);
                    }
                    let index = match e.attr("r") {
                        Some(r) => r.parse().ok().filter(|r| (1..=MAX_ROW).contains(r)).ok_or_else(|| {
                            Error::new(ErrorCode::InvalidXml, format!("{part}: row number {r:?} is out of range"))
                        })?,
                        None => next_row,
                    };
                    next_row = index + 1;
                    next_col = 1;
                    row = Some(Row { index, cells: Vec::new() });
                }
                "c" => {
                    let (row_index, col) = match e.attr("r").map(|r| (r, parse_cell(r))) {
                        Some((_, Some(rc))) => rc,
                        Some((r, None)) => {
                            return Err(Error::new(ErrorCode::InvalidXml, format!("{part}: bad cell reference {r:?}")));
                        }
                        None => (row.as_ref().map_or(next_row, |r| r.index), next_col),
                    };
                    if col > MAX_COL {
                        return Err(Error::new(ErrorCode::InvalidXml, format!("{part}: column beyond XFD")));
                    }
                    next_col = col + 1;
                    let current = row.get_or_insert_with(|| Row { index: row_index, cells: Vec::new() });
                    if current.index != row_index {
                        // A cell reference naming another row than its <row>:
                        // trust the reference, as Excel does.
                        let done = std::mem::replace(current, Row { index: row_index, cells: Vec::new() });
                        sheet.rows.push(done);
                    }
                    *budget = budget.checked_sub(1).ok_or_else(|| {
                        Error::new(ErrorCode::InvalidArchive, format!("the workbook has more than {MAX_CELLS} cells"))
                    })?;
                    let style = e.attr("s").and_then(|s| s.parse().ok()).unwrap_or(0);
                    let kind = e.attr("t").unwrap_or("n").to_owned();
                    let raw = parse_cell_children(&mut p)?;
                    let value = match type_value(&kind, raw.value, raw.inline, shared) {
                        Ok(v) => v,
                        Err(raw) => {
                            bad_values += 1;
                            Value::Text(raw)
                        }
                    };
                    row.as_mut().expect("row set above").cells.push(Cell { col, value, formula: raw.formula, style });
                }
                "worksheet" | "sheetData" => {}
                _ => p.skip_element()?,
            },
            Event::End(name) if name == "sheetData" => {
                if let Some(done) = row.take() {
                    sheet.rows.push(done);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if let Some(done) = row.take() {
        sheet.rows.push(done);
    }
    normalize(&mut sheet.rows);
    if bad_values > 0 {
        warnings.push(Warning {
            code: "unreadable_value",
            message: format!("sheet {:?}: {bad_values} cell value(s) could not be read as their type", sheet.name),
        });
    }
    Ok(())
}

fn parse_cell_children(p: &mut Parser<'_>) -> Result<RawCell, Error> {
    let mut raw = RawCell::default();
    loop {
        match p.next()? {
            Event::Start(e) => match e.name.as_str() {
                "v" => raw.value = Some(p.text_until_end(&[])?),
                "is" => raw.inline = Some(decode_xstring(&p.text_until_end(&["rPh", "phoneticPr"])?)),
                "f" => {
                    let kind = match e.attr("t") {
                        Some("shared") => FormulaKind::Shared,
                        Some("array") => FormulaKind::Array,
                        Some("dataTable") => FormulaKind::DataTable,
                        _ => FormulaKind::Normal,
                    };
                    let shared_index = e.attr("si").and_then(|v| v.parse().ok());
                    let range = e.attr("ref").map(str::to_owned);
                    let text = p.text_until_end(&[])?;
                    raw.formula = Some(Formula { kind, text, shared_index, range });
                }
                _ => p.skip_element()?,
            },
            Event::End(_) => return Ok(raw),
            Event::Text(_) => {}
            Event::Eof => return Err(Error::new(ErrorCode::InvalidXml, "a cell is not closed")),
        }
    }
}

/// Types a cell's stored value by its `t` attribute. `Err` carries raw text
/// that does not fit the declared type.
fn type_value(kind: &str, value: Option<String>, inline: Option<String>, shared: &[String]) -> Result<Value, String> {
    if kind == "inlineStr" {
        return Ok(inline.map_or(Value::Empty, Value::Text));
    }
    let Some(v) = value else {
        return Ok(Value::Empty);
    };
    match kind {
        "s" => v.trim().parse::<usize>().ok().and_then(|i| shared.get(i)).map(|s| Value::Text(s.clone())).ok_or(v),
        "str" => Ok(Value::Text(decode_xstring(&v))),
        "b" => match v.trim() {
            "1" | "true" => Ok(Value::Bool(true)),
            "0" | "false" => Ok(Value::Bool(false)),
            _ => Err(v),
        },
        "e" => Ok(Value::Error(v)),
        // ISO 8601 dates are rare (Excel itself never writes them); keep the text.
        "d" => Ok(Value::Text(v)),
        _ => match v.trim() {
            "" => Ok(Value::Empty),
            t => t.parse::<f64>().ok().filter(|n| n.is_finite()).map(Value::Number).ok_or(v),
        },
    }
}

/// Sorts rows and cells, merging rows split by out-of-order references; a
/// later duplicate of the same cell wins.
fn normalize(rows: &mut Vec<Row>) {
    rows.sort_by_key(|r| r.index);
    let mut merged: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        match merged.last_mut() {
            Some(last) if last.index == row.index => last.cells.extend(row.cells),
            _ => merged.push(row),
        }
    }
    for row in &mut merged {
        row.cells.sort_by_key(|c| c.col);
        let mut deduped: Vec<Cell> = Vec::with_capacity(row.cells.len());
        for cell in row.cells.drain(..) {
            match deduped.last_mut() {
                Some(last) if last.col == cell.col => *last = cell,
                _ => deduped.push(cell),
            }
        }
        row.cells = deduped;
    }
    merged.retain(|r| !r.cells.is_empty());
    *rows = merged;
}
