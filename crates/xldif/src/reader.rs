//! Reads a workbook package into the [`Workbook`] model: sheet order,
//! visibility and kind, defined names, calculation settings, the date
//! system, and every written cell's value, formula, and style index.
//!
//! Formatting resolution, comments, and other objects arrive in M3.

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::Arc;

use crate::cellref::{MAX_COL, MAX_ROW, parse_cell};
use crate::model::{
    BookFormat, CalcMode, Calculation, Cell, DefinedName, Formula, FormulaKind, Row, Sheet, SheetKind, Value,
    Visibility, Warning, Workbook,
};
use crate::package::{Limits, Package, Relationship};
use crate::xml::{Event, Parser, decode_xstring};
use crate::{Error, ErrorCode};

/// Child elements of a string item that are not part of its text: phonetic
/// readings shown above East Asian text.
const PHONETIC: &[&str] = &["rPh", "phoneticPr"];

pub fn read_path(path: &Path) -> Result<Workbook, Error> {
    read_path_with(path, &Limits::default())
}

pub fn read_path_with(path: &Path, limits: &Limits) -> Result<Workbook, Error> {
    read_package(&mut Package::open(path, limits)?, limits)
}

pub fn read_bytes(bytes: Vec<u8>) -> Result<Workbook, Error> {
    read_bytes_with(bytes, &Limits::default())
}

pub fn read_bytes_with(bytes: Vec<u8>, limits: &Limits) -> Result<Workbook, Error> {
    read_package(&mut Package::from_bytes(bytes, limits)?, limits)
}

fn missing(part: &str) -> Error {
    Error::new(ErrorCode::InvalidXml, format!("the workbook has no {part} part"))
}

/// Counts records (cells, rows, shared strings, sheets, names) against
/// [`Limits::max_records`], so memory stays proportional to the limit
/// whatever the file's compression ratio.
struct Budget {
    left: usize,
    limit: usize,
}

impl Budget {
    fn take(&mut self) -> Result<(), Error> {
        self.left = self.left.checked_sub(1).ok_or_else(|| {
            Error::new(ErrorCode::InvalidArchive, format!("the workbook has more than {} records", self.limit))
                .with_hint("cells, rows, shared strings, sheets, and names all count toward this bound")
        })?;
        Ok(())
    }
}

/// A `<sheet>` entry of `workbook.xml` before its part is read.
struct SheetEntry {
    name: String,
    visibility: Visibility,
    rel_id: String,
}

/// Defined names keep their raw `localSheetId` until sheet names are known.
struct NameEntry {
    name: String,
    local_sheet: Option<String>,
    refers_to: String,
    hidden: bool,
}

struct WorkbookPart {
    sheets: Vec<SheetEntry>,
    names: Vec<NameEntry>,
    date1904: bool,
    calculation: Calculation,
}

fn read_package<R: Read + Seek>(pkg: &mut Package<R>, limits: &Limits) -> Result<Workbook, Error> {
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

    let mut budget = Budget { left: limits.max_records, limit: limits.max_records };
    let text = pkg.read_text(&workbook_part)?.ok_or_else(|| missing(&workbook_part))?;
    let wb = parse_workbook(&text, &workbook_part, &mut budget)?;
    let rels = pkg.relationships(&workbook_part)?;
    let by_id: HashMap<&str, &Relationship> = rels.iter().map(|r| (r.id.as_str(), r)).collect();

    let shared = match rels.iter().find(|r| r.kind == "sharedStrings") {
        Some(rel) => match pkg.read_text(&rel.target)? {
            Some(text) => parse_shared_strings(&text, &rel.target, &mut budget)?,
            None => Vec::new(),
        },
        None => Vec::new(),
    };

    let mut warnings = Vec::new();
    let mut sheets = Vec::with_capacity(wb.sheets.len());
    for entry in &wb.sheets {
        let rel = by_id.get(entry.rel_id.as_str()).copied();
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

    let mut defined_names = Vec::with_capacity(wb.names.len());
    for n in wb.names {
        let scope = if let Some(id) = &n.local_sheet {
            let Some(sheet) = id.parse::<usize>().ok().and_then(|i| sheets.get(i)) else {
                // Promoting it to workbook scope could collide with a real
                // workbook-level name of the same spelling.
                warnings.push(Warning {
                    code: "defined_name_scope_invalid",
                    message: format!(
                        "defined name {:?} names sheet index {id:?}, which does not exist; it is left out",
                        n.name
                    ),
                });
                continue;
            };
            Some(sheet.name.clone())
        } else {
            None
        };
        defined_names.push(DefinedName { name: n.name, scope, refers_to: n.refers_to, hidden: n.hidden });
    }
    Ok(Workbook { format, date1904: wb.date1904, calculation: wb.calculation, sheets, defined_names, warnings })
}

fn truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1" | "true"))
}

fn parse_workbook(text: &str, part: &str, budget: &mut Budget) -> Result<WorkbookPart, Error> {
    let mut p = Parser::new(text, part);
    let mut wb = WorkbookPart {
        sheets: Vec::new(),
        names: Vec::new(),
        date1904: false,
        calculation: Calculation { mode: CalcMode::Auto, full_calc_on_load: false },
    };
    // The officeDocument of a Word or PowerPoint file is not a workbook.
    let root = loop {
        match p.next()? {
            Event::Text(t) if t.trim().is_empty() => {}
            other => break other,
        }
    };
    match root {
        Event::Start(e) if e.name == "workbook" => {}
        _ => {
            return Err(Error::new(ErrorCode::UnsupportedFormat, "the file is an Office document but not a workbook")
                .with_hint("xldif compares Excel workbooks (.xlsx, .xlsm)"));
        }
    }
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
                    budget.take()?;
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
                    budget.take()?;
                    let name = e.attr("name").unwrap_or_default().to_owned();
                    let local_sheet = e.attr("localSheetId").map(str::to_owned);
                    let hidden = truthy(e.attr("hidden"));
                    let refers_to = p.text_until_end(&[])?;
                    wb.names.push(NameEntry { name, local_sheet, refers_to, hidden });
                }
                // Nothing else in workbook.xml matters yet; skip whole subtrees
                // so their children (extLst, fileVersion…) cost nothing.
                "sheets" | "definedNames" => {}
                _ => p.skip_element()?,
            },
            Event::Eof => return Ok(wb),
            _ => {}
        }
    }
}

/// The shared string table: each `<si>` flattened to the text of its `<t>`
/// elements, without phonetic readings. Whitespace between elements (an
/// indented part) is not text.
fn parse_shared_strings(text: &str, part: &str, budget: &mut Budget) -> Result<Vec<Arc<str>>, Error> {
    let mut p = Parser::new(text, part);
    let mut out = Vec::new();
    loop {
        match p.next()? {
            Event::Start(e) if e.name == "si" => {
                budget.take()?;
                out.push(Arc::from(decode_xstring(&p.text_of("t", PHONETIC)?)));
            }
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
    shared: &[Arc<str>],
    sheet: &mut Sheet,
    budget: &mut Budget,
    warnings: &mut Vec<Warning>,
) -> Result<(), Error> {
    let mut p = Parser::new(text, part);
    let mut row: Option<Row> = None;
    let mut next_row = 1u32;
    let mut next_col = 1u32;
    let mut bad_values = 0usize;
    // Rows without cells carry only formatting, which M3 reads; they are
    // not kept, so a part full of empty `<row/>` costs no memory.
    let finish = |row: Option<Row>, rows: &mut Vec<Row>| {
        if let Some(done) = row.filter(|r| !r.cells.is_empty()) {
            rows.push(done);
        }
    };
    let out_of_grid = |what: &str| Error::new(ErrorCode::InvalidXml, format!("{part}: {what} is outside the grid"));
    loop {
        match p.next()? {
            Event::Start(e) => match e.name.as_str() {
                "dimension" => sheet.dimension = e.attr("ref").map(str::to_owned),
                "row" => {
                    budget.take()?;
                    finish(row.take(), &mut sheet.rows);
                    let index = match e.attr("r") {
                        Some(r) => r.parse().map_err(|_| out_of_grid(&format!("row number {r:?}")))?,
                        None => next_row,
                    };
                    if !(1..=MAX_ROW).contains(&index) {
                        return Err(out_of_grid(&format!("row {index}")));
                    }
                    next_row = index + 1;
                    next_col = 1;
                    row = Some(Row { index, cells: Vec::new() });
                }
                "c" => {
                    budget.take()?;
                    let (row_index, col) = match e.attr("r").map(|r| (r, parse_cell(r))) {
                        Some((_, Some(rc))) => rc,
                        Some((r, None)) => {
                            return Err(Error::new(ErrorCode::InvalidXml, format!("{part}: bad cell reference {r:?}")));
                        }
                        None => (row.as_ref().map_or(next_row, |r| r.index), next_col),
                    };
                    if !(1..=MAX_ROW).contains(&row_index) || col > MAX_COL {
                        return Err(out_of_grid(&format!("a cell in row {row_index}, column {col}")));
                    }
                    next_col = col + 1;
                    if row.as_ref().is_some_and(|r| r.index != row_index) {
                        // A cell reference naming another row than its <row>:
                        // the reference wins (unverified against Excel).
                        finish(row.take(), &mut sheet.rows);
                    }
                    let current = row.get_or_insert_with(|| Row { index: row_index, cells: Vec::new() });
                    let style = e.attr("s").and_then(|s| s.parse().ok()).unwrap_or(0);
                    let kind = e.attr("t").unwrap_or("n").to_owned();
                    let raw = parse_cell_children(&mut p)?;
                    let value = match type_value(&kind, raw.value, raw.inline, shared) {
                        Ok(v) => v,
                        Err(raw) => {
                            bad_values += 1;
                            Value::Text(raw.into())
                        }
                    };
                    current.cells.push(Cell { col, value, formula: raw.formula, style });
                }
                // Macro and dialog sheets share the worksheet's cell layout
                // (macrosheet roots are unverified against an Excel-saved file).
                "worksheet" | "macrosheet" | "dialogsheet" | "sheetData" => {}
                _ => p.skip_element()?,
            },
            Event::End(name) if name == "sheetData" => finish(row.take(), &mut sheet.rows),
            Event::Eof => break,
            _ => {}
        }
    }
    finish(row.take(), &mut sheet.rows);
    normalize(&mut sheet.rows);
    expand_shared_formulas(&mut sheet.rows);
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
                "is" => raw.inline = Some(decode_xstring(&p.text_of("t", PHONETIC)?)),
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
fn type_value(kind: &str, value: Option<String>, inline: Option<String>, shared: &[Arc<str>]) -> Result<Value, String> {
    if kind == "inlineStr" {
        return Ok(inline.map_or(Value::Empty, |t| Value::Text(t.into())));
    }
    let Some(v) = value else {
        return Ok(Value::Empty);
    };
    match kind {
        // Cells share the table's string rather than copying it, so many
        // cells naming one long string cost one copy.
        "s" => v.trim().parse::<usize>().ok().and_then(|i| shared.get(i)).map(|s| Value::Text(Arc::clone(s))).ok_or(v),
        "str" => Ok(Value::Text(decode_xstring(&v).into())),
        "b" => match v.trim() {
            "1" | "true" => Ok(Value::Bool(true)),
            "0" | "false" => Ok(Value::Bool(false)),
            _ => Err(v),
        },
        "e" => Ok(Value::Error(v)),
        // ISO 8601 dates. Excel's transitional saves use serial numbers; a
        // strict save may use these (unverified), so keep the text.
        "d" => Ok(Value::Text(v.into())),
        _ => match v.trim() {
            "" => Ok(Value::Empty),
            t => t.parse::<f64>().ok().filter(|n| n.is_finite()).map(Value::Number).ok_or(v),
        },
    }
}

/// Gives every cell of a shared formula its own text. Excel stores the text
/// once, on the anchor cell (the one with `ref`), and the other cells name
/// it by `si`; each reads as the anchor's formula shifted by its offset.
/// A follower whose anchor is missing keeps its empty text.
fn expand_shared_formulas(rows: &mut [Row]) {
    let mut anchors: HashMap<u32, (u32, u32, Vec<crate::formula::Token>)> = HashMap::new();
    for row in rows.iter() {
        for cell in &row.cells {
            if let Some(f) = &cell.formula
                && f.kind == FormulaKind::Shared
                && !f.text.is_empty()
                && let Some(si) = f.shared_index
            {
                anchors.entry(si).or_insert_with(|| (row.index, cell.col, crate::formula::tokenize(&f.text)));
            }
        }
    }
    if anchors.is_empty() {
        return;
    }
    for row in rows.iter_mut() {
        for cell in &mut row.cells {
            let Some(f) = &mut cell.formula else { continue };
            if f.kind != FormulaKind::Shared || !f.text.is_empty() {
                continue;
            }
            if let Some((r, c, tokens)) = f.shared_index.and_then(|si| anchors.get(&si)) {
                let shifted = crate::formula::shift(
                    tokens,
                    i64::from(row.index) - i64::from(*r),
                    i64::from(cell.col) - i64::from(*c),
                );
                f.text = crate::formula::render(&shifted);
            }
        }
    }
}

/// Sorts rows and cells in place, merging rows split by out-of-order
/// references; a later duplicate of the same cell wins (the sorts are stable).
fn normalize(rows: &mut Vec<Row>) {
    rows.sort_by_key(|r| r.index);
    rows.dedup_by(|later, earlier| {
        if later.index == earlier.index {
            earlier.cells.append(&mut later.cells);
            true
        } else {
            false
        }
    });
    for row in rows.iter_mut() {
        row.cells.sort_by_key(|c| c.col);
        // `dedup_by` keeps the first of a run; reverse so the last written wins.
        row.cells.reverse();
        row.cells.dedup_by_key(|c| c.col);
        row.cells.reverse();
    }
}
