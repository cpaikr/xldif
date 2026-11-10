//! Reads a workbook package into the [`Workbook`] model: sheet order,
//! visibility and kind, defined names, calculation settings, the date
//! system, and every written cell's value, formula, and style index.
//!
//! Formatting resolution, comments, and other objects arrive in M3.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::Arc;

use crate::cellref::{MAX_COL, MAX_ROW, parse_cell};
use crate::model::{
    BookFormat, CalcMode, Calculation, Cell, DefinedName, Formula, FormulaKind, LineProps, Row, Sheet, SheetKind,
    Value, Visibility, Warning, Workbook,
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
    formula_bytes_left: u64,
}

impl Budget {
    fn take_n(&mut self, n: usize) -> Result<(), Error> {
        for _ in 0..n.min(self.left.saturating_add(1)) {
            self.take()?;
        }
        Ok(())
    }

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

    let mut budget =
        Budget { left: limits.max_records, limit: limits.max_records, formula_bytes_left: limits.max_formula_bytes };
    let text = pkg.read_text(&workbook_part)?.ok_or_else(|| missing(&workbook_part))?;
    let wb = parse_workbook(&text, &workbook_part, &mut budget)?;
    let rels = pkg.relationships(&workbook_part)?;
    let by_id: HashMap<&str, &Relationship> = rels.iter().map(|r| (r.id.as_str(), r)).collect();

    let shared = match rels.iter().find(|r| r.kind == "sharedStrings" && !r.external) {
        Some(rel) => match pkg.read_text(&rel.target)? {
            Some(text) => parse_shared_strings(&text, &rel.target, &mut budget)?,
            None => Vec::new(),
        },
        None => Vec::new(),
    };

    let styles = match rels.iter().find(|r| r.kind == "styles" && !r.external) {
        Some(rel) => match pkg.read_text(&rel.target)? {
            Some(text) => crate::styles::parse_styles(&text, &rel.target, &mut budget.left)?,
            None => crate::styles::Styles::default(),
        },
        None => crate::styles::Styles::default(),
    };

    let theme = match rels.iter().find(|r| r.kind == "theme" && !r.external) {
        Some(rel) => match pkg.read_text(&rel.target)? {
            Some(text) => Some(theme_colors(&text, &rel.target)?),
            None => None,
        },
        None => None,
    };
    let people = match rels.iter().find(|r| r.kind == "person" && !r.external) {
        Some(rel) => match pkg.read_text(&rel.target)? {
            Some(text) => crate::sheet_objects::persons(&text, &rel.target)?,
            None => HashMap::new(),
        },
        None => HashMap::new(),
    };

    let mut warnings = Vec::new();
    let mut sheets = Vec::with_capacity(wb.sheets.len());
    for entry in &wb.sheets {
        let rel = by_id.get(entry.rel_id.as_str()).copied().filter(|r| !r.external);
        let kind = match rel.map(|r| r.kind.as_str()) {
            Some("chartsheet") => SheetKind::Chartsheet,
            Some("dialogsheet") => SheetKind::Dialogsheet,
            Some("xlMacrosheet" | "xlIntlMacrosheet" | "macrosheet") => SheetKind::Macrosheet,
            _ => SheetKind::Worksheet,
        };
        let mut sheet = Sheet {
            name: entry.name.clone(),
            kind,
            visibility: entry.visibility,
            dimension: None,
            rows: Vec::new(),
            layout: crate::model::Layout::default(),
            merges: Vec::new(),
            comments: Vec::new(),
            validations: Vec::new(),
            conditional_formats: Vec::new(),
            hyperlinks: Vec::new(),
        };
        let text = match rel {
            Some(rel) => pkg.read_text(&rel.target)?,
            None => None,
        };
        match (text, rel) {
            (Some(text), Some(rel)) if kind != SheetKind::Chartsheet => {
                let sheet_rels = pkg.relationships(&rel.target)?;
                let context = SheetContext { shared: &shared, rels: &sheet_rels, styles: &styles };
                parse_sheet(&text, &rel.target, &context, &mut sheet, &mut budget, &mut warnings)?;
                read_comments(pkg, &sheet_rels, &people, &mut sheet)?;
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
    Ok(Workbook {
        format,
        date1904: wb.date1904,
        calculation: wb.calculation,
        sheets,
        defined_names,
        styles,
        theme,
        warnings,
    })
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

/// A sheet's notes and threaded comments. Excel also saves a legacy note
/// for each threaded comment (for older versions); a cell with a thread
/// keeps only the thread, so one comment is not reported twice.
fn read_comments<R: Read + Seek>(
    pkg: &mut Package<R>,
    rels: &[Relationship],
    people: &HashMap<String, String>,
    sheet: &mut Sheet,
) -> Result<(), Error> {
    let mut threads: Vec<crate::model::Comment> = Vec::new();
    for rel in rels.iter().filter(|r| r.kind == "threadedComment" && !r.external) {
        if let Some(text) = pkg.read_text(&rel.target)? {
            threads.extend(crate::sheet_objects::threaded_comments(&text, &rel.target, people)?);
        }
    }
    for rel in rels.iter().filter(|r| r.kind == "comments" && !r.external) {
        if let Some(text) = pkg.read_text(&rel.target)? {
            let notes = crate::sheet_objects::legacy_comments(&text, &rel.target)?;
            let threaded: HashSet<(u32, u32)> = threads.iter().map(|t| (t.row, t.col)).collect();
            sheet.comments.extend(notes.into_iter().filter(|n| !threaded.contains(&(n.row, n.col))));
        }
    }
    sheet.comments.extend(threads);
    sheet.comments.sort_by_key(|c| (c.row, c.col));
    Ok(())
}

/// What a sheet part refers to outside itself.
struct SheetContext<'a> {
    shared: &'a [Arc<str>],
    rels: &'a [Relationship],
    styles: &'a crate::styles::Styles,
}

/// The theme's color scheme as one fingerprint: each named slot and its
/// color. Other parts of the theme (fonts, effects, generator-specific
/// extensions) differ between writers without changing what cells look like.
fn theme_colors(text: &str, part: &str) -> Result<u64, Error> {
    let mut p = Parser::new(text, part);
    loop {
        match p.next()? {
            Event::Start(e) if e.name == "clrScheme" => {
                let described = p.describe_rest()?;
                return Ok(crate::align::fnv(&[described.as_bytes()]));
            }
            Event::Eof => return Ok(0),
            _ => {}
        }
    }
}

fn parse_sheet(
    text: &str,
    part: &str,
    context: &SheetContext<'_>,
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
                    if let Some(props) = crate::sheet_objects::row_props(&e, index) {
                        sheet.layout.rows.push(props);
                    }
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
                    let value = match type_value(&kind, raw.value, raw.inline, context.shared) {
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
                "worksheet" | "macrosheet" | "dialogsheet" | "sheetData" | "cols" | "mergeCells" | "hyperlinks"
                | "dataValidations" => {}
                "col" => {
                    budget.take()?;
                    if let Some(props) = crate::sheet_objects::col_props(&e) {
                        sheet.layout.cols.push(props);
                    }
                }
                "mergeCell" => {
                    budget.take()?;
                    if let Some(range) = e.attr("ref").and_then(crate::cellref::Range::parse) {
                        sheet.merges.push(range);
                    }
                }
                "hyperlink" => {
                    budget.take()?;
                    if let Some(range) = e.attr("ref").and_then(crate::cellref::Range::parse) {
                        let target = crate::sheet_objects::hyperlink_target(&e, context.rels);
                        sheet.hyperlinks.push(crate::model::Hyperlink { range, target });
                    }
                }
                "dataValidation" => {
                    let rule = crate::sheet_objects::data_validation(&e, &mut p)?;
                    budget.take_n(1 + rule.ranges.len())?;
                    sheet.validations.push(rule);
                }
                "conditionalFormatting" => {
                    let rules = crate::sheet_objects::conditional_formatting(&e, &mut p, context.styles)?;
                    budget.take_n(1 + rules.len() + rules.first().map_or(0, |r| r.ranges.len()))?;
                    sheet.conditional_formats.extend(rules);
                }
                _ => p.skip_element()?,
            },
            Event::End(name) if name == "sheetData" => finish(row.take(), &mut sheet.rows),
            Event::Eof => break,
            _ => {}
        }
    }
    finish(row.take(), &mut sheet.rows);
    normalize(&mut sheet.rows);
    sheet.layout.rows = disjoint_lines(std::mem::take(&mut sheet.layout.rows));
    sheet.layout.cols = disjoint_lines(std::mem::take(&mut sheet.layout.cols));
    expand_shared_formulas(&mut sheet.rows, budget)?;
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

/// A shared formula's anchor: where it is written and its tokens.
struct Anchor {
    row: u32,
    col: u32,
    tokens: Vec<crate::formula::Token>,
    /// The anchor's rendered length and its reference count, which bound
    /// every follower's length.
    len: u64,
    refs: u64,
}

impl Anchor {
    /// The most a reference's text grows when shifted: each point at most
    /// from `A1` to `$XFD$1048576` (2 to 12 bytes), or the whole reference
    /// to `#REF!`, which is shorter than two points.
    const REF_GROWTH: u64 = 2 * 10 + 1;

    fn new(row: u32, col: u32, text: &str) -> Self {
        let tokens = crate::formula::tokenize(text);
        let len = crate::formula::render(&tokens).len() as u64;
        let refs = tokens.iter().filter(|t| matches!(t, crate::formula::Token::Ref(_))).count() as u64;
        Anchor { row, col, tokens, len, refs }
    }

    /// An upper bound on a follower's text length, and so on what its
    /// shifted tokens hold.
    fn max_len(&self) -> u64 {
        self.len + self.refs * Self::REF_GROWTH
    }
}

/// Row or column records made sorted and disjoint, so a lookup is one
/// binary search and listing their lines costs at most the grid's extent.
/// Excel writes no overlapping records; where a file does, the record
/// written first keeps the lines it covers and later ones take what is
/// left (a choice, unverified against Excel).
fn disjoint_lines(records: Vec<LineProps>) -> Vec<LineProps> {
    // Covered spans, merged regardless of their records: start to end.
    let mut covered: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    let mut out = Vec::with_capacity(records.len());
    for rec in records {
        let (a, b) = (rec.index, rec.last);
        let mut overlap: Vec<(u32, u32)> =
            covered.range(..a).next_back().filter(|(_, e)| **e >= a).map(|(s, e)| (*s, *e)).into_iter().collect();
        overlap.extend(covered.range(a..=b).map(|(s, e)| (*s, *e)));
        let mut cursor = a;
        for &(s, e) in &overlap {
            if s > cursor {
                out.push(LineProps { index: cursor, last: s - 1, ..rec.clone() });
            }
            cursor = cursor.max(e.saturating_add(1));
        }
        if cursor <= b {
            out.push(LineProps { index: cursor, last: b, ..rec.clone() });
        }
        // Every span visited merges into one, so the work is amortized.
        for (s, _) in &overlap {
            covered.remove(s);
        }
        let start = overlap.first().map_or(a, |o| o.0.min(a));
        let end = overlap.last().map_or(b, |o| o.1.max(b));
        covered.insert(start, end);
    }
    out.sort_by_key(|l| l.index);
    out
}

/// Gives every cell of a shared formula its own text. Excel stores the text
/// once, on the anchor cell (the one with `ref`), and the other cells name
/// it by `si`; each reads as the anchor's formula shifted by its offset.
/// A follower whose anchor is missing keeps its empty text.
fn expand_shared_formulas(rows: &mut [Row], budget: &mut Budget) -> Result<(), Error> {
    let mut anchors: HashMap<u32, Anchor> = HashMap::new();
    for row in rows.iter() {
        for cell in &row.cells {
            if let Some(f) = &cell.formula
                && f.kind == FormulaKind::Shared
                && !f.text.is_empty()
                && let Some(si) = f.shared_index
            {
                anchors.entry(si).or_insert_with(|| Anchor::new(row.index, cell.col, &f.text));
            }
        }
    }
    if anchors.is_empty() {
        return Ok(());
    }
    for row in rows.iter_mut() {
        for cell in &mut row.cells {
            let Some(f) = &mut cell.formula else { continue };
            if f.kind != FormulaKind::Shared || !f.text.is_empty() {
                continue;
            }
            if let Some(a) = f.shared_index.and_then(|si| anchors.get(&si)) {
                // Charge the most this follower can take before allocating
                // its tokens and text, then give back what it did not use.
                let bound = a.max_len();
                budget.formula_bytes_left = budget.formula_bytes_left.checked_sub(bound).ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidArchive,
                        "the workbook's shared formulas expand to more formula text than xldif reads",
                    )
                })?;
                let shifted = crate::formula::shift(
                    &a.tokens,
                    i64::from(row.index) - i64::from(a.row),
                    i64::from(cell.col) - i64::from(a.col),
                );
                f.text = crate::formula::render(&shifted);
                debug_assert!(f.text.len() as u64 <= bound, "a shifted formula outgrew its bound");
                budget.formula_bytes_left += bound.saturating_sub(f.text.len() as u64);
            }
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(index: u32, last: u32, style: u32) -> LineProps {
        LineProps { index, last, size: None, hidden: false, style: Some(style) }
    }

    fn spans(lines: &[LineProps]) -> Vec<(u32, u32, u32)> {
        lines.iter().map(|l| (l.index, l.last, l.style.unwrap())).collect()
    }

    #[test]
    fn the_first_overlapping_record_keeps_its_lines() {
        let got = disjoint_lines(vec![line(1, 10, 1), line(2, 2, 2), line(3, 3, 3)]);
        assert_eq!(spans(&got), [(1, 10, 1)]);
        let got = disjoint_lines(vec![line(5, 5, 1), line(8, 9, 2), line(1, 10, 3)]);
        assert_eq!(spans(&got), [(1, 4, 3), (5, 5, 1), (6, 7, 3), (8, 9, 2), (10, 10, 3)]);
    }

    #[test]
    fn touching_and_unordered_records_stay_apart() {
        let got = disjoint_lines(vec![line(4, 6, 1), line(1, 3, 2), line(7, 7, 3), line(2, 5, 4)]);
        assert_eq!(spans(&got), [(1, 3, 2), (4, 6, 1), (7, 7, 3)]);
    }

    #[test]
    fn repeated_full_width_records_collapse() {
        let records = (0..10_000).map(|i| line(1, MAX_COL, i)).collect();
        assert_eq!(spans(&disjoint_lines(records)), [(1, MAX_COL, 0)]);
    }

    #[test]
    fn a_follower_never_outgrows_its_anchors_bound() {
        for text in ["A1", "B1:C2+$D$3", "Sheet2!1:1+A:A+SUM(A1,'My sheet'!B2:C3)", "A1048576+XFD1"] {
            let a = Anchor::new(1, 1, text);
            for (rows, cols) in [(0, 0), (1, 1), (1_048_575, 16_383), (2_000_000, 0), (0, 20_000)] {
                let len = crate::formula::render(&crate::formula::shift(&a.tokens, rows, cols)).len() as u64;
                assert!(len <= a.max_len(), "{text} shifted by ({rows}, {cols}): {len} > {}", a.max_len());
            }
        }
    }
}
