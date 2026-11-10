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
    /// One formula shared across a range. Every cell's `text` is the formula
    /// as that cell reads it; the file stores it on the anchor only. `text`
    /// stays empty only when the anchor is missing.
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
    pub layout: Layout,
    pub merges: Vec<crate::cellref::Range>,
    pub comments: Vec<Comment>,
    pub validations: Vec<RangeRule>,
    pub conditional_formats: Vec<RangeRule>,
    pub hyperlinks: Vec<Hyperlink>,
}

/// Row and column records that change how a sheet looks: hidden, a height
/// or width set by hand, or a style for the whole row or column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub rows: Vec<LineProps>,
    /// Column records; `index` is each record's first column and `last` its
    /// last (a record may cover several columns).
    pub cols: Vec<LineProps>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineProps {
    pub index: u32,
    pub last: u32,
    /// As written, only when set by hand (`customHeight`/`customWidth`).
    pub size: Option<String>,
    pub hidden: bool,
    /// The style unwritten cells in this row or column take, when set.
    pub style: Option<u32>,
}

/// A note or threaded comment on one cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub row: u32,
    pub col: u32,
    pub author: String,
    /// The text; for a thread, each entry as `author: text` on its own line.
    pub text: String,
}

/// A rule over ranges: data validation or conditional formatting, with the
/// rule described as one comparable string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeRule {
    /// Shared by every rule of one `<conditionalFormatting>`.
    pub ranges: std::sync::Arc<[crate::cellref::Range]>,
    /// Everything but the formulas, described canonically.
    pub rule: String,
    /// The rule's formulas, as stored. They are compared after carrying
    /// them through the alignment, as cell formulas are.
    pub formulas: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    pub range: crate::cellref::Range,
    /// The external address, or the location inside the workbook.
    pub target: String,
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
    /// Resolved cell formats, by the `style` index cells carry.
    pub styles: crate::styles::Styles,
    /// A fingerprint of the theme part, whose colors themed cells use.
    pub theme: Option<u64>,
    pub warnings: Vec<Warning>,
}

impl Workbook {
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }
}
