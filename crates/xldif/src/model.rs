//! The immutable workbook model the reader produces and comparisons consume.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BookFormat {
    Xlsx,
    /// Macro-enabled. Macros are carried but never compared.
    Xlsm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CalcMode {
    Auto,
    Manual,
    AutoNoTable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Calculation {
    pub mode: CalcMode,
    /// Excel recalculates everything on open; cached values may be stale or
    /// placeholders (library-written files often set this).
    pub full_calc_on_load: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SheetKind {
    Worksheet,
    Chartsheet,
    Dialogsheet,
    Macrosheet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Visibility {
    Visible,
    Hidden,
    VeryHidden,
}

/// A cell's stored value. Formula cells hold their cached result.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// No value: a formatting-only cell, or a formula without a cached result.
    Empty,
    Number(f64),
    /// Shared with the workbook's string table: cells repeating one string
    /// hold one copy.
    Text(std::sync::Arc<str>),
    Bool(bool),
    /// An error literal such as `#DIV/0!`.
    Error(String),
}

impl Value {
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Number(_) => "number",
            Self::Text(_) => "text",
            Self::Bool(_) => "bool",
            Self::Error(_) => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FormulaKind {
    Normal,
    /// One formula shared across a range; `text` is empty on cells after the
    /// anchor until M2 expands them.
    Shared,
    /// An array formula; only the anchor cell carries it.
    Array,
    /// A what-if data table.
    DataTable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formula {
    pub kind: FormulaKind,
    /// As stored, without a leading `=`.
    pub text: String,
    /// The `si` index linking a shared formula's cells.
    pub shared_index: Option<u32>,
    /// The range an anchor covers (shared or array).
    pub range: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// 1-based column.
    pub col: u32,
    pub value: Value,
    pub formula: Option<Formula>,
    /// Index into the workbook's cell formats (`cellXfs`); 0 when absent.
    pub style: u32,
}

impl Cell {
    /// A cell with neither a value nor a formula carries only formatting.
    pub fn has_content(&self) -> bool {
        self.value != Value::Empty || self.formula.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// 1-based row number.
    pub index: u32,
    /// Sorted by column, one entry per column.
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub kind: SheetKind,
    pub visibility: Visibility,
    /// The `<dimension>` the file declares, which may be stale.
    pub dimension: Option<String>,
    /// Sorted by row index; rows without written cells are absent.
    pub rows: Vec<Row>,
}

impl Sheet {
    pub fn cells(&self) -> impl Iterator<Item = (u32, &Cell)> {
        self.rows.iter().flat_map(|r| r.cells.iter().map(move |c| (r.index, c)))
    }

    /// The smallest range covering every cell with content, if any.
    pub fn used_range(&self) -> Option<crate::cellref::Range> {
        let mut range: Option<crate::cellref::Range> = None;
        for (row, cell) in self.cells().filter(|(_, c)| c.has_content()) {
            let r = range.get_or_insert(crate::cellref::Range {
                first_row: row,
                first_col: cell.col,
                last_row: row,
                last_col: cell.col,
            });
            r.first_row = r.first_row.min(row);
            r.last_row = r.last_row.max(row);
            r.first_col = r.first_col.min(cell.col);
            r.last_col = r.last_col.max(cell.col);
        }
        range
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinedName {
    pub name: String,
    /// The sheet a local name belongs to; `None` for workbook scope.
    pub scope: Option<String>,
    pub refers_to: String,
    pub hidden: bool,
}

/// Something the reader noticed that the caller should know about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workbook {
    pub format: BookFormat,
    /// Dates count from 1904-01-01 instead of 1900-01-01.
    pub date1904: bool,
    pub calculation: Calculation,
    pub sheets: Vec<Sheet>,
    pub defined_names: Vec<DefinedName>,
    pub warnings: Vec<Warning>,
}

impl Workbook {
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }
}
