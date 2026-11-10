//! Read views over a [`Workbook`]: serializable results the CLI prints.

use serde::Serialize;

use crate::cellref::{Range, cell_name};
use crate::model::{
    BookFormat, Calculation, DefinedName, FormulaKind, Sheet, SheetKind, Value, Visibility, Warning, Workbook,
};
use crate::{Error, ErrorCode};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetSummary {
    pub index: usize,
    pub name: String,
    pub kind: SheetKind,
    pub visibility: Visibility,
    /// The `<dimension>` the file declares, which may be stale.
    pub declared_dimension: Option<String>,
    /// The range covering every cell with a value or formula.
    pub used_range: Option<String>,
    /// Cells with a value or formula.
    pub cells: usize,
    pub formulas: usize,
    /// Formula cells with no cached result.
    pub formulas_without_value: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub file: String,
    pub format: BookFormat,
    pub date1904: bool,
    pub calculation: Calculation,
    pub sheets: Vec<SheetSummary>,
    pub defined_names: Vec<DefinedName>,
}

fn summarize(index: usize, sheet: &Sheet) -> SheetSummary {
    let mut cells = 0;
    let mut formulas = 0;
    let mut without_value = 0;
    for (_, cell) in sheet.cells().filter(|(_, c)| c.has_content()) {
        cells += 1;
        if cell.formula.is_some() {
            formulas += 1;
            if cell.value == Value::Empty {
                without_value += 1;
            }
        }
    }
    SheetSummary {
        index,
        name: sheet.name.clone(),
        kind: sheet.kind,
        visibility: sheet.visibility,
        declared_dimension: sheet.dimension.clone(),
        used_range: sheet.used_range().map(|r| r.name()),
        cells,
        formulas,
        formulas_without_value: without_value,
    }
}

/// Workbook structure and counts, without cell contents, plus the warnings
/// a comparison of this workbook would need: stale cached values and the
/// reader's own notes.
pub fn inspect(file: &str, book: &Workbook) -> (Inspection, Vec<Warning>) {
    let sheets: Vec<SheetSummary> = book.sheets.iter().enumerate().map(|(i, s)| summarize(i, s)).collect();
    let mut warnings = book.warnings.clone();
    warnings.extend(stale_value_warnings(book));
    let inspection = Inspection {
        file: file.to_owned(),
        format: book.format,
        date1904: book.date1904,
        calculation: book.calculation,
        sheets,
        defined_names: book.defined_names.clone(),
    };
    (inspection, warnings)
}

/// Warnings that a workbook's stored values may not be current.
pub fn stale_value_warnings(book: &Workbook) -> Vec<Warning> {
    let mut out = Vec::new();
    if book.calculation.mode == crate::model::CalcMode::Manual {
        out.push(Warning {
            code: "manual_calculation",
            message: "the workbook is set to manual calculation, so cached values may be stale".to_owned(),
        });
    }
    if book.calculation.full_calc_on_load {
        out.push(Warning {
            code: "full_calc_on_load",
            message: "the workbook asks Excel to recalculate on open, so cached values may be placeholders".to_owned(),
        });
    }
    let missing = book
        .sheets
        .iter()
        .flat_map(Sheet::cells)
        .filter(|(_, c)| c.formula.is_some() && c.value == Value::Empty)
        .count();
    if missing > 0 {
        out.push(Warning {
            code: "formula_without_value",
            message: format!("{missing} formula cell(s) have no cached value"),
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JsonValue {
    Number(f64),
    Text(String),
    Bool(bool),
}

/// A stored value as JSON; error literals appear as text (the `type` field
/// says which), and an empty value is absent.
pub fn json_value(v: &Value) -> Option<JsonValue> {
    match v {
        Value::Empty => None,
        Value::Number(n) => Some(JsonValue::Number(*n)),
        Value::Text(t) => Some(JsonValue::Text(t.to_string())),
        Value::Error(t) => Some(JsonValue::Text(t.clone())),
        Value::Bool(b) => Some(JsonValue::Bool(*b)),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellView {
    pub cell: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Absent for an empty value; error literals appear as text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_kind: Option<FormulaKind>,
    /// The range a shared or array formula's anchor covers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_range: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cells {
    pub sheet: String,
    pub range: Option<String>,
    pub cells: Vec<CellView>,
    /// Cells left out by the limit.
    pub omitted: usize,
}

/// Sheet names quoted, since a name may itself contain a comma.
pub fn quoted_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names.map(|n| format!("{n:?}")).collect::<Vec<_>>().join(", ")
}

/// Looks a sheet up by exact name, listing the names when it is absent.
pub fn find_sheet<'a>(book: &'a Workbook, name: &str) -> Result<&'a Sheet, Error> {
    book.sheet(name).ok_or_else(|| {
        Error::new(ErrorCode::InvalidArgument, format!("no sheet is named {name:?}"))
            .with_hint(format!("sheets: {}", quoted_names(book.sheets.iter().map(|s| s.name.as_str()))))
    })
}

/// Cells with content in `sheet`, row by row, optionally inside `range`,
/// at most `limit` of them (`None` lists all).
pub fn cells(sheet: &Sheet, range: Option<Range>, limit: Option<usize>) -> Cells {
    let mut out = Vec::new();
    let mut omitted = 0;
    for (row, cell) in sheet.cells().filter(|(_, c)| c.has_content()) {
        if range.is_some_and(|r| !r.contains(row, cell.col)) {
            continue;
        }
        if limit.is_some_and(|l| out.len() >= l) {
            omitted += 1;
            continue;
        }
        let value = json_value(&cell.value);
        let formula = cell.formula.as_ref();
        out.push(CellView {
            cell: cell_name(row, cell.col),
            kind: cell.value.type_name(),
            value,
            formula: formula.map(|f| f.text.clone()),
            formula_kind: formula.map(|f| f.kind),
            formula_range: formula.and_then(|f| f.range.clone()),
        });
    }
    Cells { sheet: sheet.name.clone(), range: range.map(|r| r.name()), cells: out, omitted }
}
