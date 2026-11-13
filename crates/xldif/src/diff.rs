//! Content-aligned comparison of two workbooks.
//!
//! Sheets pair by name, then unpaired sheets by content (a rename). Inside a
//! sheet pair, columns align first (by header text and value overlap), then
//! rows align by their label and their values in the aligned columns, so an
//! inserted column cannot break row matching and an inserted row is one
//! change. Values are compared on paired rows × paired columns; cells in
//! inserted or deleted rows and columns are covered by that row or column
//! change, not listed one by one.
//!
//! Formulas on paired cells are compared after carrying the base formula
//! through the alignment, the way Excel adjusts references when rows and
//! columns are inserted or deleted (references to other sheets follow those
//! sheets' alignments). A formula that differs only by that adjustment is
//! counted as adjusted, not listed. Runs of the same formula change filled
//! down or across a column fold into one change with a range.
//!
//! This module holds the result types, the comparison of two workbooks, and
//! the listing a command prints. [`grid`] turns a sheet into what alignment
//! reads, [`axis`] aligns one sheet pair, and [`cells`] lists that pair's
//! changes; formats and objects are in `diff_objects`.

mod axis;
mod cells;
mod grid;

pub use axis::Axis;
use axis::{Aligned, align_grids};
use cells::{Shared, date_offset, order_key, sheet_changes};
use grid::{Grid, Reading, fnv, value_bytes};
pub use grid::{label_key, numeric_text};

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::align::{self, Item};
use crate::cellref::{MAX_COL, MAX_ROW, Range, cell_name, col_name};
use crate::formula::{self, AxisMap, SheetMap};
use crate::links::LinkMap;
use crate::model::{Sheet, Warning, Workbook};
use crate::numfmt::Formats;
use crate::views::{JsonValue, stale_value_warnings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignMode {
    /// Match rows and columns by content (the default).
    Content,
    /// Compare A1 with A1.
    Position,
}

#[derive(Debug, Clone, Copy)]
pub struct CompareOptions {
    /// Numbers closer than this are equal.
    pub tolerance: f64,
    pub align: AlignMode,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self { tolerance: 0.0, align: AlignMode::Content }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Same,
    Changed,
    OnlyBase,
    OnlyOther,
    Moved,
}

impl Status {
    /// The JSON spelling, for text views.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Changed => "changed",
            Self::OnlyBase => "onlyBase",
            Self::OnlyOther => "onlyOther",
            Self::Moved => "moved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
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
    /// A link to another workbook: the file `[1]` in a formula names.
    ExternalLink,
}

impl Kind {
    pub const ALL: [Self; 14] = [
        Self::Value,
        Self::Formula,
        Self::Format,
        Self::Row,
        Self::Column,
        Self::Merge,
        Self::Comment,
        Self::Validation,
        Self::ConditionalFormat,
        Self::Hyperlink,
        Self::Sheet,
        Self::Name,
        Self::Theme,
        Self::ExternalLink,
    ];

    /// Kinds that are workbook objects rather than cells, rows, or columns.
    pub const fn is_object(self) -> bool {
        matches!(
            self,
            Self::Merge
                | Self::Comment
                | Self::Validation
                | Self::ConditionalFormat
                | Self::Hyperlink
                | Self::Sheet
                | Self::Name
                | Self::Theme
                | Self::ExternalLink
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Format => "format",
            Self::Merge => "merge",
            Self::Comment => "comment",
            Self::Validation => "validation",
            Self::ConditionalFormat => "conditionalFormat",
            Self::Hyperlink => "hyperlink",
            Self::Sheet => "sheet",
            Self::Name => "name",
            Self::Theme => "theme",
            Self::ExternalLink => "externalLink",
            Self::Value => "value",
            Self::Formula => "formula",
            Self::Row => "row",
            Self::Column => "column",
        }
    }
}

/// Where a change is on one side's sheet.
///
/// Changes are built with a place and name it as text only when written
/// out: `cell`, `range`, `row`, and `column` in JSON, and the references in
/// a change's id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Place {
    /// Nowhere on a sheet: a workbook object such as a defined name.
    #[default]
    Nowhere,
    Cell {
        row: u32,
        col: u32,
    },
    /// A folded run of more than one cell (a formula filled down, a block
    /// formatted alike): written as its first cell and its range.
    Block(Range),
    /// The ranges an object covers (a merge, a rule): written as `range`,
    /// several separated by spaces.
    Ranges(Vec<Range>),
    /// A row, or a folded run of rows.
    Rows {
        first: u32,
        last: u32,
    },
    /// A column, or a folded run of columns (1 = A).
    Columns {
        first: u32,
        last: u32,
    },
}

/// A place as JSON writes it.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaceText {
    #[serde(skip_serializing_if = "Option::is_none")]
    cell: Option<String>,
    /// For a folded change: the cells it covers, such as `D10:D40`.
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    row: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<String>,
}

impl Place {
    /// The cells of `range`: one cell, or a block.
    pub fn block(range: Range) -> Self {
        if range.first_row == range.last_row && range.first_col == range.last_col {
            Self::Cell { row: range.first_row, col: range.first_col }
        } else {
            Self::Block(range)
        }
    }

    pub const fn row(n: u32) -> Self {
        Self::Rows { first: n, last: n }
    }

    pub const fn column(n: u32) -> Self {
        Self::Columns { first: n, last: n }
    }

    fn text(&self) -> PlaceText {
        let run = |first: u32, last: u32, one: &dyn Fn(u32) -> String| {
            (first != last).then(|| format!("{}:{}", one(first), one(last)))
        };
        match self {
            Self::Nowhere => PlaceText::default(),
            Self::Cell { row, col } => PlaceText { cell: Some(cell_name(*row, *col)), ..PlaceText::default() },
            Self::Block(r) => PlaceText {
                cell: Some(cell_name(r.first_row, r.first_col)),
                range: Some(r.name()),
                ..PlaceText::default()
            },
            Self::Ranges(ranges) => PlaceText {
                range: Some(ranges.iter().map(Range::name).collect::<Vec<_>>().join(" ")),
                ..PlaceText::default()
            },
            Self::Rows { first, last } => {
                PlaceText { row: Some(*first), range: run(*first, *last, &|n| n.to_string()), ..PlaceText::default() }
            }
            Self::Columns { first, last } => PlaceText {
                column: Some(col_name(*first)),
                range: run(*first, *last, &col_name),
                ..PlaceText::default()
            },
        }
    }

    /// The place as ids and listings name it: a block's or an object's
    /// range, else the cell, row, or column. `None` for nowhere.
    pub fn reference(&self) -> Option<String> {
        let text = self.text();
        text.range.or(text.cell).or_else(|| text.row.map(|r| r.to_string())).or(text.column)
    }
}

impl Serialize for Place {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.text().serialize(serializer)
    }
}

/// One side of a change: where it is and what is there.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    /// Written as `cell`, `range`, `row`, and `column`, whichever apply.
    #[serde(flatten)]
    pub place: Place,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<JsonValue>,
    /// The value as the cell shows it, when its number format changes how
    /// it reads (`1,234.50`, `31-Mar-24`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// A date-formatted number as ISO 8601 (`2024-03-31`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// The formula as stored (without `=`); for a folded change, the one in
    /// its first cell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// An object's content: a comment's text, a rule, a link target, a
    /// defined name's reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Location {
    /// The cell, as `B5`: a cell's own, or a block's first.
    pub fn cell(&self) -> Option<String> {
        self.place.text().cell
    }

    /// The range, as `D10:D40`, `3:5`, or `B:D`: a block's, an object's, or
    /// a folded run of rows or columns.
    pub fn range(&self) -> Option<String> {
        self.place.text().range
    }

    /// The row, or the first of a run of rows.
    pub fn row(&self) -> Option<u32> {
        self.place.text().row
    }

    /// The column, as `B`, or the first of a run of columns.
    pub fn column(&self) -> Option<String> {
        self.place.text().column
    }

    /// The cells this location names: its cell, its block, or its ranges.
    /// Empty for rows, columns, and workbook objects.
    pub fn ranges(&self) -> Vec<Range> {
        match &self.place {
            Place::Cell { row, col } => {
                vec![Range { first_row: *row, first_col: *col, last_row: *row, last_col: *col }]
            }
            Place::Block(range) => vec![*range],
            Place::Ranges(ranges) => ranges.clone(),
            Place::Nowhere | Place::Rows { .. } | Place::Columns { .. } => Vec::new(),
        }
    }

    /// The first and last row or column this location names, for a `row` or
    /// `column` change.
    pub const fn lines(&self) -> Option<(u32, u32)> {
        match self.place {
            Place::Rows { first, last } | Place::Columns { first, last } => Some((first, last)),
            _ => None,
        }
    }
}

/// One property that differs, as `base` and `other` show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Property {
    pub name: String,
    pub base: String,
    pub other: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Labels {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// The base side's row label, when it differs from `row`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_row: Option<String>,
    /// Labels are found by looking left and up from the cell; they are a
    /// reading aid, not part of the comparison.
    pub inferred: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// Derived from the change's location, so it is stable across filters,
    /// limits, and reruns: `s<sheet>:<kind>:<base ref>:<other ref>`, with
    /// `:layout` after it for a row's or column's size, hidden state, or
    /// style. No two changes of one comparison share an id.
    pub id: String,
    pub kind: Kind,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other: Option<Location>,
    /// `other - base`, for two numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    /// For a formula change: the base formula as Excel would have adjusted
    /// it for the inserted and deleted rows and columns, when that differs
    /// from the base text. The other side's formula differs from this too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Labels>,
    /// For a format, layout, or object change: what differs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<Property>,
}

/// `[base start, other start, length]`.
pub type Run = [u32; 3];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentView {
    /// `content`, or `position` when asked for with `--align position`.
    pub method: &'static str,
    /// Part of the sheet was too large to compare every row with every
    /// other: it was searched only near its diagonal, or paired by
    /// position, and rows there may be paired with the wrong ones.
    pub fell_back: bool,
    /// Runs of paired rows by row number. Rows not covered are listed as
    /// `row` changes (or are blank).
    pub rows: Vec<Run>,
    pub row_moves: Vec<Run>,
    /// Runs of paired columns by column number (1 = A).
    pub cols: Vec<Run>,
    pub col_moves: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetComparison {
    /// `s<n>`: base sheets in order, then sheets only in the other book.
    pub id: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
    /// Paired by content under a different name.
    pub renamed: bool,
    /// Its position among the paired sheets changed.
    pub moved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alignment: Option<AlignmentView>,
    /// The pairing `alignment` summarizes, in full, for a renderer that
    /// draws the two sheets side by side: it must show the rows and columns
    /// the comparison paired, not work them out again from the workbooks.
    #[serde(skip)]
    pub axes: Option<SheetAxes>,
    pub changes: Vec<Change>,
}

/// One sheet pair's rows and columns as the comparison paired them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetAxes {
    pub rows: Axis,
    pub cols: Axis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub file: String,
    pub sheets: usize,
    pub date1904: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub sheets_changed: usize,
    pub sheets_only_base: usize,
    pub sheets_only_other: usize,
    pub sheets_renamed: usize,
    pub rows_only_base: usize,
    pub rows_only_other: usize,
    pub rows_moved: usize,
    pub columns_only_base: usize,
    pub columns_only_other: usize,
    pub columns_moved: usize,
    /// Rows whose height, hidden state, or row style changed.
    pub rows_changed: usize,
    /// Columns whose width, hidden state, or column style changed.
    pub columns_changed: usize,
    pub value: usize,
    /// Formula changes, counting a folded run once.
    pub formula: usize,
    /// Formulas whose text differs only because Excel adjusted them for
    /// inserted or deleted rows and columns; not listed.
    pub formulas_adjusted: usize,
    /// Format changes, counting a folded block once.
    pub format: usize,
    /// Merges, comments, validation, conditional formats, hyperlinks, sheet
    /// visibility, defined names, and the theme.
    pub objects: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub base: Side,
    pub other: Side,
    pub summary: Summary,
    pub sheets: Vec<SheetComparison>,
    /// Workbook-level changes: defined names, the theme, and the files
    /// external links name.
    pub objects: Vec<Change>,
    #[serde(skip)]
    pub warnings: Vec<Warning>,
}

pub fn compare(
    base_file: &str,
    base: &Workbook,
    other_file: &str,
    other: &Workbook,
    options: CompareOptions,
) -> Comparison {
    let mut warnings = Vec::new();
    for (side, book) in [("base", base), ("other", other)] {
        for w in book.warnings.iter().cloned().chain(stale_value_warnings(book)) {
            warnings.push(Warning { code: w.code, message: format!("{side}: {}", w.message) });
        }
    }
    if base.date1904 != other.date1904 {
        warnings.push(Warning {
            code: "date_system_mismatch",
            message: "the workbooks use different date systems (1900 and 1904), so equal dates have different serial \
                      numbers; cells formatted as dates on both sides are compared by the date they show"
                .to_owned(),
        });
    }

    let pairs = pair_sheets(base, other);
    let moved = moved_sheets(&pairs);
    let formats = (Formats::new(&base.styles, base.date1904), Formats::new(&other.styles, other.date1904));
    let date_offsets = (base.date1904 != other.date1904).then(|| (date_offset(base), date_offset(other)));
    let links = LinkMap::new(&base.external_links, &other.external_links);
    // Align every sheet pair first: formulas refer across sheets, so each
    // sheet's comparison needs every other sheet's row and column maps.
    let aligned: Vec<(usize, usize, bool, Grid, Grid, Aligned)> = pairs
        .iter()
        .map(|&(bi, oi, renamed)| {
            let reading = |offset: f64, links| Reading {
                tolerance: options.tolerance,
                date_offset: date_offsets.map(|_| offset),
                links,
            };
            let gb = Grid::new(&base.sheets[bi], &formats.0, reading(date_offset(base), Some(&links)));
            let go = Grid::new(&other.sheets[oi], &formats.1, reading(date_offset(other), None));
            let al = align_grids(&gb, &go, options);
            (bi, oi, renamed, gb, go, al)
        })
        .collect();
    let maps: HashMap<String, SheetMap> = aligned
        .iter()
        .map(|(bi, oi, renamed, _, _, al)| {
            let renamed_to = renamed.then(|| other.sheets[*oi].name.clone());
            (base.sheets[*bi].name.to_lowercase(), al.sheet_map(renamed_to))
        })
        .collect();
    // The same maps read from the other side, keyed by the other workbook's
    // sheet names: what an address there held on the base side.
    let inverse: HashMap<String, SheetMap> =
        aligned.iter().map(|(_, oi, _, _, _, al)| (other.sheets[*oi].name.to_lowercase(), al.inverse_map())).collect();

    let mut sheets = Vec::new();
    let mut adjusted = 0;
    let mut paired_other = vec![false; other.sheets.len()];
    let shared = Shared { maps: &maps, inverse: &inverse, options, formats, date_offsets, links: &links };
    for (bi, b) in base.sheets.iter().enumerate() {
        let id = format!("s{}", sheets.len());
        let Some((_, oi, renamed, gb, go, al)) = aligned.iter().find(|a| a.0 == bi) else {
            sheets.push(lone_sheet(id, Status::OnlyBase, &b.name));
            continue;
        };
        paired_other[*oi] = true;
        let o = &other.sheets[*oi];
        let this = &maps[&b.name.to_lowercase()];
        let back = &inverse[&o.name.to_lowercase()];
        let (mut changes, n_adjusted) = sheet_changes(&id, gb, go, al, (this, back), &shared);
        adjusted += n_adjusted;
        let (bs, os) = (
            crate::diff_objects::Side { sheet: b, styles: &base.styles },
            crate::diff_objects::Side { sheet: o, styles: &other.styles },
        );
        changes.extend(crate::diff_objects::sheet_object_changes(&id, bs, os, this, back, &links));
        changes.sort_by_key(order_key);
        if al.fell_back {
            warnings.push(Warning {
                code: "alignment_fallback",
                message: format!(
                    "sheet {:?}: part of the sheet was too large to align fully by content; rows or columns there \
                     may be paired with the wrong ones",
                    b.name
                ),
            });
        }
        let is_moved = moved.contains(&bi);
        let status = if changes.is_empty() && !renamed && !is_moved { Status::Same } else { Status::Changed };
        sheets.push(SheetComparison {
            id,
            status,
            base: Some(b.name.clone()),
            other: Some(o.name.clone()),
            renamed: *renamed,
            moved: is_moved,
            alignment: Some(al.view()),
            axes: Some(SheetAxes { rows: al.rows.clone(), cols: al.cols.clone() }),
            changes,
        });
    }
    for (oi, o) in other.sheets.iter().enumerate() {
        if !paired_other[oi] {
            let id = format!("s{}", sheets.len());
            sheets.push(lone_sheet(id, Status::OnlyOther, &o.name));
        }
    }
    let mut objects = workbook_objects(base, other, &maps, &links);
    objects.extend(crate::links::changes(&base.external_links, &other.external_links));
    let mut summary = summarize(&sheets);
    summary.formulas_adjusted = adjusted;
    summary.objects += objects.len();
    Comparison {
        base: Side { file: base_file.to_owned(), sheets: base.sheets.len(), date1904: base.date1904 },
        other: Side { file: other_file.to_owned(), sheets: other.sheets.len(), date1904: other.date1904 },
        summary,
        sheets,
        objects,
        warnings,
    }
}

/// Defined names (compared by name and scope, their references carried
/// through the alignments like formulas) and the theme.
fn workbook_objects(
    base: &Workbook,
    other: &Workbook,
    maps: &HashMap<String, SheetMap>,
    links: &LinkMap,
) -> Vec<Change> {
    let blank = || Change {
        id: String::new(),
        kind: Kind::Name,
        status: Status::Changed,
        base: None,
        other: None,
        delta: None,
        expected: None,
        labels: None,
        properties: Vec::new(),
    };
    let scope_key = |scope: &Option<String>, renames: bool| -> String {
        let s = scope.clone().unwrap_or_default().to_lowercase();
        if renames { maps.get(&s).and_then(|m| m.renamed_to.clone()).map_or(s, |r| r.to_lowercase()) } else { s }
    };
    let key = |n: &crate::model::DefinedName, renames: bool| (n.name.to_lowercase(), scope_key(&n.scope, renames));
    let label = |n: &crate::model::DefinedName| match &n.scope {
        Some(scope) => format!("{scope}!{}", n.name),
        None => n.name.clone(),
    };
    let other_names: HashMap<(String, String), &crate::model::DefinedName> =
        other.defined_names.iter().map(|n| (key(n, false), n)).collect();
    let workbook_scope =
        SheetMap { renamed_to: None, rows: AxisMap::identity(MAX_ROW), cols: AxisMap::identity(MAX_COL) };
    let mut out = Vec::new();
    let mut matched = HashSet::new();
    for n in &base.defined_names {
        let k = key(n, true);
        let loc = |d: &crate::model::DefinedName| Location { text: Some(d.refers_to.clone()), ..Location::default() };
        match other_names.get(&k) {
            Some(o) => {
                matched.insert(k.clone());
                let (stored, to) = (formula::tokenize(&n.refers_to), formula::tokenize(&o.refers_to));
                let relinked = links.relink(&stored);
                let tb = relinked.as_deref().unwrap_or(&stored);
                let moved = formula::remap(tb, &workbook_scope, |s| maps.get(&s.to_lowercase()));
                if !formula::same(&moved, &to) {
                    out.push(Change {
                        id: format!("name:{}", label(n)),
                        base: Some(loc(n)),
                        other: Some(loc(o)),
                        expected: (!formula::same(&moved, &stored)).then(|| formula::render(&moved)),
                        properties: vec![Property {
                            name: "refersTo".into(),
                            base: n.refers_to.clone(),
                            other: o.refers_to.clone(),
                        }],
                        ..blank()
                    });
                }
            }
            None => out.push(Change {
                id: format!("name:{}", label(n)),
                status: Status::OnlyBase,
                base: Some(loc(n)),
                ..blank()
            }),
        }
    }
    for o in &other.defined_names {
        if !matched.contains(&key(o, false)) {
            let loc = Location { text: Some(o.refers_to.clone()), ..Location::default() };
            out.push(Change {
                id: format!("name:{}", label(o)),
                status: Status::OnlyOther,
                other: Some(loc),
                ..blank()
            });
        }
    }
    // A theme added, removed, or with a different color scheme.
    if base.theme != other.theme {
        out.push(Change { id: "theme".into(), kind: Kind::Theme, ..blank() });
    }
    out
}

fn lone_sheet(id: String, status: Status, name: &str) -> SheetComparison {
    let (base, other) =
        if status == Status::OnlyBase { (Some(name.to_owned()), None) } else { (None, Some(name.to_owned())) };
    SheetComparison {
        id,
        status,
        base,
        other,
        renamed: false,
        moved: false,
        alignment: None,
        axes: None,
        changes: Vec::new(),
    }
}

fn summarize(sheets: &[SheetComparison]) -> Summary {
    let mut s = Summary::default();
    for sheet in sheets {
        match sheet.status {
            Status::OnlyBase => s.sheets_only_base += 1,
            Status::OnlyOther => s.sheets_only_other += 1,
            Status::Changed => s.sheets_changed += 1,
            Status::Same | Status::Moved => {}
        }
        s.sheets_renamed += usize::from(sheet.renamed);
        for c in &sheet.changes {
            match (c.kind, c.status) {
                (Kind::Value, _) => s.value += 1,
                (Kind::Formula, _) => s.formula += 1,
                (Kind::Format, _) => s.format += 1,
                (k, _) if k.is_object() => s.objects += 1,
                (Kind::Row, Status::OnlyBase) => s.rows_only_base += 1,
                (Kind::Row, Status::OnlyOther) => s.rows_only_other += 1,
                (Kind::Row, Status::Moved) => s.rows_moved += 1,
                (Kind::Column, Status::OnlyBase) => s.columns_only_base += 1,
                (Kind::Column, Status::OnlyOther) => s.columns_only_other += 1,
                (Kind::Column, Status::Moved) => s.columns_moved += 1,
                (Kind::Row, Status::Changed) => s.rows_changed += 1,
                (Kind::Column, Status::Changed) => s.columns_changed += 1,
                _ => {}
            }
        }
    }
    s
}

// ---------------------------------------------------------------------------
// sheets

/// `(base index, other index, renamed)` for each sheet pair.
fn pair_sheets(base: &Workbook, other: &Workbook) -> Vec<(usize, usize, bool)> {
    let mut pairs: Vec<(usize, usize, bool)> = Vec::new();
    for (bi, b) in base.sheets.iter().enumerate() {
        if let Some(oi) = other.sheets.iter().position(|o| o.name == b.name) {
            pairs.push((bi, oi, false));
        }
    }
    let tokens = |s: &Sheet| {
        let mut t: Vec<u64> = s.cells().filter_map(|(_, c)| value_bytes(&c.value, 0.0)).map(|b| fnv(&[&b])).collect();
        t.sort_unstable();
        Item { key: None, tokens: t, shape: 0 }
    };
    let lone_base: Vec<usize> = (0..base.sheets.len()).filter(|i| !pairs.iter().any(|p| p.0 == *i)).collect();
    let lone_other: Vec<usize> = (0..other.sheets.len()).filter(|i| !pairs.iter().any(|p| p.1 == *i)).collect();
    let base_items: Vec<Item> = lone_base.iter().map(|&i| tokens(&base.sheets[i])).collect();
    let other_items: Vec<Item> = lone_other.iter().map(|&i| tokens(&other.sheets[i])).collect();
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (a, bi) in base_items.iter().zip(&lone_base) {
        for (b, oi) in other_items.iter().zip(&lone_other) {
            let s = align::similarity(a, b);
            if s >= align::MIN_SIMILARITY {
                candidates.push((s, *bi, *oi));
            }
        }
    }
    candidates.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
    for (_, bi, oi) in candidates {
        if !pairs.iter().any(|p| p.0 == bi || p.1 == oi) {
            pairs.push((bi, oi, true));
        }
    }
    pairs.sort_unstable();
    pairs
}

/// Base indexes of paired sheets outside the longest run kept in order.
fn moved_sheets(pairs: &[(usize, usize, bool)]) -> HashSet<usize> {
    let order: Vec<usize> = pairs.iter().map(|p| p.1).collect();
    let keep: HashSet<usize> = align::longest_increasing(&order).into_iter().collect();
    pairs.iter().enumerate().filter(|(k, _)| !keep.contains(k)).map(|(_, p)| p.0).collect()
}

/// What to list from a comparison; the summary always counts everything.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only the sheet with this name on either side.
    pub sheet: Option<String>,
    /// Only these kinds; empty means all.
    pub kinds: Vec<Kind>,
    /// At most this many changes in total; `None` lists all.
    pub limit: Option<usize>,
    /// Also list sheets that are the same.
    pub all: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub base: Side,
    pub other: Side,
    pub summary: Summary,
    /// Changes listed below.
    pub listed: usize,
    /// Changes left out by the filter or limit, by sheet name and kind.
    pub omitted: std::collections::BTreeMap<String, std::collections::BTreeMap<&'static str, usize>>,
    pub sheets: Vec<SheetComparison>,
    /// Workbook-level changes (defined names, theme); left out when
    /// `--sheet` is given.
    pub objects: Vec<Change>,
}

impl Comparison {
    pub fn list(self, filter: &Filter) -> Listing {
        let mut listed = 0;
        let mut omitted: std::collections::BTreeMap<String, std::collections::BTreeMap<&'static str, usize>> =
            std::collections::BTreeMap::new();
        let mut sheets = Vec::new();
        for mut sheet in self.sheets {
            let name = sheet.other.clone().or_else(|| sheet.base.clone()).unwrap_or_default();
            let named =
                filter.sheet.as_ref().is_none_or(|n| Some(n) == sheet.base.as_ref() || Some(n) == sheet.other.as_ref());
            let mut kept = Vec::new();
            for change in std::mem::take(&mut sheet.changes) {
                let wanted = named && (filter.kinds.is_empty() || filter.kinds.contains(&change.kind));
                if wanted && filter.limit.is_none_or(|l| listed < l) {
                    listed += 1;
                    kept.push(change);
                } else {
                    *omitted.entry(name.clone()).or_default().entry(change.kind.as_str()).or_default() += 1;
                }
            }
            sheet.changes = kept;
            if named && (filter.all || sheet.status != Status::Same) {
                sheets.push(sheet);
            }
        }
        let mut objects = Vec::new();
        for change in self.objects {
            let wanted = filter.sheet.is_none() && (filter.kinds.is_empty() || filter.kinds.contains(&change.kind));
            if wanted && filter.limit.is_none_or(|l| listed < l) {
                listed += 1;
                objects.push(change);
            } else {
                *omitted.entry("(workbook)".to_owned()).or_default().entry(change.kind.as_str()).or_default() += 1;
            }
        }
        Listing { base: self.base, other: self.other, summary: self.summary, listed, omitted, sheets, objects }
    }
}

#[cfg(test)]
mod tests {
    use super::{Location, Place};
    use crate::cellref::Range;

    #[test]
    fn a_place_is_written_as_the_fields_a_reader_expects() {
        let range = |text: &str| Range::parse(text).unwrap();
        let written = |place: Place| serde_json::to_string(&Location { place, ..Location::default() }).unwrap();
        assert_eq!(written(Place::Nowhere), "{}");
        assert_eq!(written(Place::Cell { row: 5, col: 2 }), r#"{"cell":"B5"}"#);
        assert_eq!(written(Place::block(range("D10:D40"))), r#"{"cell":"D10","range":"D10:D40"}"#);
        assert_eq!(written(Place::block(range("D10"))), r#"{"cell":"D10"}"#);
        assert_eq!(written(Place::Ranges(vec![range("A1:B2"), range("D5")])), r#"{"range":"A1:B2 D5"}"#);
        assert_eq!(written(Place::row(3)), r#"{"row":3}"#);
        assert_eq!(written(Place::Rows { first: 3, last: 5 }), r#"{"range":"3:5","row":3}"#);
        assert_eq!(written(Place::column(2)), r#"{"column":"B"}"#);
        assert_eq!(written(Place::Columns { first: 2, last: 4 }), r#"{"range":"B:D","column":"B"}"#);
        // Ids and listings name a place by its range first.
        assert_eq!(Place::block(range("D10:D40")).reference().as_deref(), Some("D10:D40"));
        assert_eq!(Place::Columns { first: 2, last: 4 }.reference().as_deref(), Some("B:D"));
        assert_eq!(Place::Nowhere.reference(), None);
    }
}
