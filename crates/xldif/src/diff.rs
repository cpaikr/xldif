//! Content-aligned comparison of two workbooks (values only for now).
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
//! Formatting and workbook objects (M3) are not compared yet.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::align::{self, Alignment, Item, Options};
use crate::cellref::{MAX_COL, MAX_ROW, Range, cell_name, col_name};
use crate::formula::{self, AxisMap, SheetMap};
use crate::model::{Cell, Formula, FormulaKind, Row, Sheet, Value, Warning, Workbook};
use crate::numfmt::Formats;
use crate::views::{JsonValue, json_value, stale_value_warnings};

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
}

impl Kind {
    pub const ALL: [Self; 13] = [
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
            Self::Value => "value",
            Self::Formula => "formula",
            Self::Row => "row",
            Self::Column => "column",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<String>,
    /// For a folded change: the cells it covers, such as `D10:D40`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
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
    /// limits, and reruns: `s<sheet>:<kind>:<base ref>:<other ref>`.
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
    /// Part of the sheet was too large to align by content and was paired
    /// by position instead.
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
    pub changes: Vec<Change>,
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
    /// Workbook-level changes: defined names and the theme.
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
                      numbers"
                .to_owned(),
        });
    }

    let pairs = pair_sheets(base, other);
    let moved = moved_sheets(&pairs);
    let formats = (Formats::new(&base.styles, base.date1904), Formats::new(&other.styles, other.date1904));
    // Align every sheet pair first: formulas refer across sheets, so each
    // sheet's comparison needs every other sheet's row and column maps.
    let aligned: Vec<(usize, usize, bool, Grid, Grid, Aligned)> = pairs
        .iter()
        .map(|&(bi, oi, renamed)| {
            let (gb, go) = (Grid::new(&base.sheets[bi], &formats.0), Grid::new(&other.sheets[oi], &formats.1));
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
    let shared = Shared { maps: &maps, inverse: &inverse, options, formats };
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
        changes.extend(crate::diff_objects::sheet_object_changes(&id, bs, os, this, back));
        changes.sort_by_key(order_key);
        if al.fell_back {
            warnings.push(Warning {
                code: "alignment_fallback",
                message: format!(
                    "sheet {:?}: part of the sheet was too large to align by content and was compared by position",
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
            changes,
        });
    }
    for (oi, o) in other.sheets.iter().enumerate() {
        if !paired_other[oi] {
            let id = format!("s{}", sheets.len());
            sheets.push(lone_sheet(id, Status::OnlyOther, &o.name));
        }
    }
    let objects = workbook_objects(base, other, &maps);
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
fn workbook_objects(base: &Workbook, other: &Workbook, maps: &HashMap<String, SheetMap>) -> Vec<Change> {
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
                let (tb, to) = (formula::tokenize(&n.refers_to), formula::tokenize(&o.refers_to));
                let moved = formula::remap(&tb, &workbook_scope, |s| maps.get(&s.to_lowercase()));
                if !formula::same(&moved, &to) {
                    out.push(Change {
                        id: format!("name:{}", label(n)),
                        base: Some(loc(n)),
                        other: Some(loc(o)),
                        expected: (!formula::same(&moved, &tb)).then(|| formula::render(&moved)),
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
    SheetComparison { id, status, base, other, renamed: false, moved: false, alignment: None, changes: Vec::new() }
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

/// A deterministic 64-bit FNV-1a hash, so tokens never depend on a random seed.
fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in *part {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn value_bytes(v: &Value) -> Option<Vec<u8>> {
    Some(match v {
        Value::Empty => return None,
        Value::Number(n) => [b"n".as_slice(), &n.to_bits().to_le_bytes()].concat(),
        Value::Text(t) if t.is_empty() => return None,
        Value::Text(t) => [b"t".as_slice(), t.as_bytes()].concat(),
        Value::Bool(b) => vec![b'b', u8::from(*b)],
        Value::Error(e) => [b"e".as_slice(), e.as_bytes()].concat(),
    })
}

/// A label made comparable: whitespace removed (Korean statements space out
/// characters, as in "자 산 총 계"), case folded, and a leading outline
/// marker dropped ("Ⅰ.", "(1)", "1.", "가."), since renumbering a section
/// does not make its rows different rows.
pub fn label_key(text: &str) -> Option<String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    let key = strip_outline_marker(&compact);
    let key = if key.is_empty() { compact.as_str() } else { key };
    (!key.is_empty()).then(|| key.to_owned())
}

fn is_roman(c: char) -> bool {
    ('\u{2160}'..='\u{217F}').contains(&c)
}

fn is_hangul_counter(c: char) -> bool {
    "가나다라마바사아자차카타파하".contains(c)
}

/// Drops one leading outline marker: `(1)`, `(가)`, `1.`, `1)`, `가.`, `Ⅰ.`, or
/// a bare Roman numeral directly before the label.
fn strip_outline_marker(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix('(') {
        if let Some(end) = rest.find(')') {
            let inner = &rest[..end];
            let counter =
                inner.chars().all(|c| c.is_ascii_digit()) || inner.chars().all(|c| is_roman(c) || is_hangul_counter(c));
            if !inner.is_empty() && inner.chars().count() <= 3 && counter {
                return &rest[end + 1..];
            }
        }
        return s;
    }
    let digits: usize = s.chars().take_while(char::is_ascii_digit).map(char::len_utf8).sum();
    let romans: usize = s.chars().take_while(|c| is_roman(*c)).map(char::len_utf8).sum();
    let hangul = s.chars().next().filter(|c| is_hangul_counter(*c)).map_or(0, char::len_utf8);
    // Outline numbers stay short; "2025.12.31" is a date, not section 2025.
    for (lead, max) in [(digits, 2), (romans, 9), (hangul, 3)] {
        if lead == 0 || lead > max {
            continue;
        }
        let rest = &s[lead..];
        if let Some(stripped) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            return stripped;
        }
        if lead == romans && !rest.is_empty() {
            return rest;
        }
    }
    s
}

/// A number written as text, as exported statements do: "1,234", "(1,234)",
/// "-12.5", "12%". `None` for anything else.
pub fn numeric_text(text: &str) -> Option<f64> {
    let t = text.trim();
    let (negative, t) = match t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(inner) => (true, inner.trim()),
        None => (false, t),
    };
    let (scale, t) = match t.strip_suffix('%') {
        Some(x) => (0.01, x),
        None => (1.0, t),
    };
    let digits: String = t.chars().filter(|c| *c != ',').collect();
    if !digits.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let well_formed =
        digits.chars().enumerate().all(|(i, c)| c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+')));
    if !well_formed {
        return None;
    }
    let n: f64 = digits.parse().ok()?;
    Some(if negative { -n } else { n } * scale)
}

/// What a cell at `row` contributes to alignment: its value and, when it
/// has one, its formula in position-free form (so `=B5*2` in row 5 and
/// `=B6*2` in row 6 are the same token, as they are after an insertion). A
/// cell with neither (formatting only, or an empty string) contributes
/// nothing, so its row or column is not an item at all.
fn cell_tokens(cell: &Cell, row: u32) -> impl Iterator<Item = Vec<u8>> + '_ {
    let formula = cell.formula.as_ref().filter(|f| !f.text.is_empty()).map(|f| {
        let form = formula::relative_form(&formula::tokenize(&f.text), row, cell.col);
        [b"f".as_slice(), form.as_bytes()].concat()
    });
    value_bytes(&cell.value).into_iter().chain(formula)
}

fn display_label(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 60 { collapsed.chars().take(60).collect::<String>() + "…" } else { collapsed }
}

/// A cell's text as a column header: text that is not a number, or a year
/// stored as a number (period headers often are). Date headers are read in
/// [`Grid::new`], which needs the cell below them.
fn header_text(cell: &Cell) -> Option<String> {
    let year = |n: f64| n.fract() == 0.0 && (1900.0..=2100.0).contains(&n);
    match &cell.value {
        Value::Number(n) if year(*n) => Some(format!("{n}")),
        // "2025" typed as text is a year too; "2,025" is an amount.
        Value::Text(t) if !t.contains(',') && numeric_text(t).is_some_and(year) => Some(t.trim().to_owned()),
        _ => text_of(cell).filter(|t| numeric_text(t).is_none()).map(display_label),
    }
}

fn text_of(cell: &Cell) -> Option<&str> {
    match &cell.value {
        Value::Text(t) if !t.trim().is_empty() => Some(t),
        _ => None,
    }
}

/// How many content rows at the top of a sheet may hold column headers.
const HEADER_ROWS: usize = 10;

/// Rows and columns of one sheet with content, for alignment.
struct Grid<'a> {
    sheet: &'a Sheet,
    /// Content rows, in order.
    rows: Vec<&'a Row>,
    /// Columns with content, in order, each with its content cells by row.
    cols: Vec<(u32, Vec<(u32, &'a Cell)>)>,
    /// Each column's header texts in the top content rows, in row order;
    /// the first is the column's label.
    headers: HashMap<u32, Vec<String>>,
}

impl<'a> Grid<'a> {
    fn new(sheet: &'a Sheet, formats: &Formats) -> Self {
        let has_tokens =
            |c: &Cell| value_bytes(&c.value).is_some() || c.formula.as_ref().is_some_and(|f| !f.text.is_empty());
        let rows: Vec<&Row> = sheet.rows.iter().filter(|r| r.cells.iter().any(has_tokens)).collect();
        let mut by_col: std::collections::BTreeMap<u32, Vec<(u32, &Cell)>> = std::collections::BTreeMap::new();
        for row in &rows {
            for cell in row.cells.iter().filter(|c| has_tokens(c)) {
                by_col.entry(cell.col).or_default().push((row.index, cell));
            }
        }
        let cols: Vec<(u32, Vec<(u32, &Cell)>)> = by_col.into_iter().collect();
        let top: HashSet<u32> = rows.iter().take(HEADER_ROWS).map(|r| r.index).collect();
        let date = |c: &Cell| formats.iso_date(c.style, &c.value);
        let headers = cols
            .iter()
            .map(|(col, cells)| {
                // A date heads a period column: the column's only date, over
                // figures. Where dates are the records (due dates, or dates
                // among other entries) they must not key the column.
                let one_date = cells.iter().filter(|(_, c)| date(c).is_some()).take(2).count() == 1;
                let figure = |c: &Cell| matches!(c.value, Value::Number(_)) && date(c).is_none();
                let texts = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, (r, _))| top.contains(r))
                    .filter_map(|(i, (_, c))| match date(c) {
                        Some(d) => (one_date && cells.get(i + 1).is_some_and(|(_, next)| figure(next))).then_some(d),
                        None => header_text(c),
                    })
                    .collect();
                (*col, texts)
            })
            .collect();
        Self { sheet, rows, cols, headers }
    }

    /// The written cells of one row, in column order.
    fn row_cells(&self, row: u32) -> &'a [Cell] {
        self.sheet.rows.binary_search_by_key(&row, |r| r.index).map_or(&[][..], |i| &self.sheet.rows[i].cells[..])
    }

    fn cell(&self, row: u32, col: u32) -> Option<&'a Cell> {
        let r = self.sheet.rows.binary_search_by_key(&row, |r| r.index).ok()?;
        let cells = &self.sheet.rows[r].cells;
        cells.binary_search_by_key(&col, |c| c.col).ok().map(|i| &cells[i])
    }

    /// The leftmost text cell of a row: its label.
    fn row_label(row: &Row) -> Option<&str> {
        row.cells.iter().find_map(text_of)
    }

    /// The column's header from the top rows; failing that, the nearest
    /// text above `row` in `col` that is not a number written as text. The
    /// header comes first because in a text column the nearest text above
    /// is usually the previous record, not a heading.
    fn column_label(&self, row: u32, col: u32) -> Option<String> {
        if let Some(header) = self.headers.get(&col).and_then(|h| h.first()) {
            return Some(header.clone());
        }
        let below = self.sheet.rows.partition_point(|r| r.index < row);
        self.sheet.rows[..below]
            .iter()
            .rev()
            .take(30)
            .find_map(|r| {
                let cell = &r.cells[r.cells.binary_search_by_key(&col, |c| c.col).ok()?];
                text_of(cell).filter(|t| numeric_text(t).is_none())
            })
            .map(display_label)
    }

    fn column_items(&self) -> Vec<Item> {
        self.cols
            .iter()
            .map(|(col, cells)| {
                let header = self.headers.get(col).map(Vec::as_slice).unwrap_or_default();
                let mut tokens: Vec<u64> = cells
                    .iter()
                    .flat_map(|(r, c)| {
                        // A column's formulas change form whenever a column they
                        // refer to moves, so they only stand in for a missing value.
                        let value = value_bytes(&c.value);
                        let formula = if value.is_none() { cell_tokens(c, *r).collect() } else { Vec::new() };
                        value.into_iter().chain(formula)
                    })
                    .map(|b| fnv(&[&b]))
                    .collect();
                tokens.sort_unstable();
                Item { key: label_key(&header.join("/")), tokens, shape: 0 }
            })
            .collect()
    }

    /// Row items whose tokens tie each value to its aligned column, so a
    /// value only counts as shared when it sits in the matching column.
    fn row_items(&self, col_ids: &HashMap<u32, u32>) -> Vec<Item> {
        self.rows
            .iter()
            .map(|row| {
                let mut tokens = Vec::new();
                let mut shape = Vec::new();
                for c in &row.cells {
                    let Some(id) = col_ids.get(&c.col) else { continue };
                    let before = tokens.len();
                    tokens.extend(cell_tokens(c, row.index).map(|bytes| fnv(&[&id.to_le_bytes(), &bytes])));
                    if tokens.len() > before {
                        shape.extend_from_slice(&id.to_le_bytes());
                    }
                }
                tokens.sort_unstable();
                // Cells arrive in column order, so the shape needs no sorting.
                let shape = if shape.is_empty() { 0 } else { fnv(&[&shape]) };
                Item { key: Self::row_label(row).and_then(label_key), tokens, shape }
            })
            .collect()
    }
}

/// `(base index, other index, renamed)` for each sheet pair.
fn pair_sheets(base: &Workbook, other: &Workbook) -> Vec<(usize, usize, bool)> {
    let mut pairs: Vec<(usize, usize, bool)> = Vec::new();
    for (bi, b) in base.sheets.iter().enumerate() {
        if let Some(oi) = other.sheets.iter().position(|o| o.name == b.name) {
            pairs.push((bi, oi, false));
        }
    }
    let tokens = |s: &Sheet| {
        let mut t: Vec<u64> = s.cells().filter_map(|(_, c)| value_bytes(&c.value)).map(|b| fnv(&[&b])).collect();
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

fn same_value(a: &Value, b: &Value, tolerance: f64) -> bool {
    match (a, b) {
        // Values are finite, so a zero tolerance means exact equality.
        (Value::Number(x), Value::Number(y)) => (x - y).abs() <= tolerance,
        (Value::Text(x), Value::Empty) | (Value::Empty, Value::Text(x)) => x.is_empty(),
        _ => a == b,
    }
}

/// Runs of `(base, other)` pairs, by sheet row or column number, that keep a
/// constant offset and have no one-sided item between them.
fn runs(pairs: &[(u32, u32)], lone_base: &[u32], lone_other: &[u32]) -> Vec<Run> {
    let between =
        |lone: &[u32], lo: u32, hi: u32| lone.partition_point(|&x| x <= lo) < lone.partition_point(|&x| x < hi);
    let mut out: Vec<Run> = Vec::new();
    for &(b, o) in pairs {
        if let Some(last) = out.last_mut() {
            let (lb, lo) = (last[0] + last[2] - 1, last[1] + last[2] - 1);
            if b > lb && b - lb == o.wrapping_sub(lo) && !between(lone_base, lb, b) && !between(lone_other, lo, o) {
                last[2] += b - lb;
                continue;
            }
        }
        out.push([b, o, 1]);
    }
    out
}

struct Axis {
    /// `(base number, other number)` pairs in order.
    pairs: Vec<(u32, u32)>,
    moves: Vec<(u32, u32)>,
    lone_base: Vec<u32>,
    lone_other: Vec<u32>,
}

impl Axis {
    fn new(al: &Alignment, base: &[u32], other: &[u32]) -> Self {
        let pairs: Vec<(u32, u32)> = al.pairs.iter().map(|&(i, j)| (base[i], other[j])).collect();
        let moves: Vec<(u32, u32)> = al.moves.iter().map(|&(i, j)| (base[i], other[j])).collect();
        let used_b: HashSet<u32> = pairs.iter().chain(&moves).map(|p| p.0).collect();
        let used_o: HashSet<u32> = pairs.iter().chain(&moves).map(|p| p.1).collect();
        let lone_base = base.iter().copied().filter(|x| !used_b.contains(x)).collect();
        let lone_other = other.iter().copied().filter(|x| !used_o.contains(x)).collect();
        Self { pairs, moves, lone_base, lone_other }
    }

    /// A1 against A1: every row or column number pairs with itself.
    fn identity(base: &[u32], other: &[u32]) -> Self {
        let all: std::collections::BTreeSet<u32> = base.iter().chain(other).copied().collect();
        Self {
            pairs: all.into_iter().map(|n| (n, n)).collect(),
            moves: Vec::new(),
            lone_base: Vec::new(),
            lone_other: Vec::new(),
        }
    }

    fn view_runs(&self) -> (Vec<Run>, Vec<Run>) {
        let mut moves: Vec<Run> = self.moves.iter().map(|&(b, o)| [b, o, 1]).collect();
        moves.sort_unstable();
        // A run must not span a moved item either, or it would map that
        // number to itself as well as to where it moved.
        let barrier = |lone: &[u32], moved: &mut dyn Iterator<Item = u32>| {
            let mut all: Vec<u32> = lone.iter().copied().chain(moved).collect();
            all.sort_unstable();
            all
        };
        let base = barrier(&self.lone_base, &mut self.moves.iter().map(|m| m.0));
        let other = barrier(&self.lone_other, &mut self.moves.iter().map(|m| m.1));
        (runs(&self.pairs, &base, &other), moves)
    }
}

/// One sheet pair's row and column alignment.
struct Aligned {
    cols: Axis,
    rows: Axis,
    method: &'static str,
    fell_back: bool,
}

impl Aligned {
    fn view(&self) -> AlignmentView {
        let (rows, row_moves) = self.rows.view_runs();
        let (cols, col_moves) = self.cols.view_runs();
        AlignmentView { method: self.method, fell_back: self.fell_back, rows, row_moves, cols, col_moves }
    }

    fn sheet_map(&self, renamed_to: Option<String>) -> SheetMap {
        let axis = |a: &Axis, max: u32| {
            AxisMap::new(a.pairs.clone(), a.moves.clone(), a.lone_base.clone(), a.lone_other.clone(), max)
        };
        SheetMap { renamed_to, rows: axis(&self.rows, MAX_ROW), cols: axis(&self.cols, MAX_COL) }
    }

    /// The map from the other side back to the base side.
    fn inverse_map(&self) -> SheetMap {
        let flip = |v: &[(u32, u32)]| v.iter().map(|&(b, o)| (o, b)).collect::<Vec<_>>();
        let axis = |a: &Axis, max: u32| {
            AxisMap::new(flip(&a.pairs), flip(&a.moves), a.lone_other.clone(), a.lone_base.clone(), max)
        };
        SheetMap { renamed_to: None, rows: axis(&self.rows, MAX_ROW), cols: axis(&self.cols, MAX_COL) }
    }
}

fn align_grids(gb: &Grid, go: &Grid, options: CompareOptions) -> Aligned {
    let col_numbers = |g: &Grid| g.cols.iter().map(|(c, _)| *c).collect::<Vec<u32>>();
    let row_numbers = |g: &Grid| g.rows.iter().map(|r| r.index).collect::<Vec<u32>>();
    let (bc, oc, br, or) = (col_numbers(gb), col_numbers(go), row_numbers(gb), row_numbers(go));
    match options.align {
        AlignMode::Position => Aligned {
            cols: Axis::identity(&bc, &oc),
            rows: Axis::identity(&br, &or),
            method: "position",
            fell_back: false,
        },
        AlignMode::Content => {
            let col_al = align::align(&gb.column_items(), &go.column_items(), Options { positional_equal_gaps: true });
            let cols = Axis::new(&col_al, &bc, &oc);
            // Paired columns share an id in row tokens; moved columns too.
            let mut base_ids = HashMap::new();
            let mut other_ids = HashMap::new();
            for (k, &(cb, co)) in cols.pairs.iter().chain(&cols.moves).enumerate() {
                base_ids.insert(cb, k as u32);
                other_ids.insert(co, k as u32);
            }
            let row_al = align::align(
                &gb.row_items(&base_ids),
                &go.row_items(&other_ids),
                Options { positional_equal_gaps: false },
            );
            let fell_back = col_al.fell_back || row_al.fell_back;
            Aligned { cols, rows: Axis::new(&row_al, &br, &or), method: "content", fell_back }
        }
    }
}

/// What one paired cell's formulas amount to.
enum FormulaDiff {
    Same,
    /// Different text, but only by Excel's adjustment for the alignment.
    Adjusted,
    Changed {
        /// The base formula adjusted, for display when it differs from
        /// the base text.
        expected: Option<String>,
        /// The adjusted base formula as it reads in the other cell,
        /// position-free; part of what folding compares.
        moved: Option<Vec<formula::Token>>,
        base: Option<String>,
        other: Option<String>,
    },
}

fn diff_formulas(
    fb: Option<&Formula>,
    fo: Option<&Formula>,
    (this, back): (&SheetMap, &SheetMap),
    shared: &Shared,
) -> FormulaDiff {
    fn usable(f: Option<&Formula>) -> Option<&Formula> {
        f.filter(|f| f.kind != FormulaKind::DataTable && !f.text.is_empty())
    }
    let (maps, inverse) = (shared.maps, shared.inverse);
    let text = |f: Option<&Formula>| f.map(|f| f.text.clone());
    match (usable(fb), usable(fo)) {
        (None, None) => FormulaDiff::Same,
        (b, o) if b.is_none() || o.is_none() => {
            FormulaDiff::Changed { expected: None, moved: None, base: text(b), other: text(o) }
        }
        (Some(fb), Some(fo)) => {
            let (tb, to) = (formula::tokenize(&fb.text), formula::tokenize(&fo.text));
            // The adjusted base formula is what the other side should read.
            let moved = formula::remap(&tb, this, |name| maps.get(&name.to_lowercase()));
            let unchanged = formula::same(&tb, &to);
            // Unchanged text whose address now holds a cell that came from
            // another row or column: a deleted row's place taken by the row
            // below it. This also catches a removed blank row, which the
            // maps carry to the same number and so call no adjustment.
            let displaced = unchanged && formula::retargets(&to, back, |name| inverse.get(&name.to_lowercase()));
            if formula::same(&moved, &to) && !displaced {
                if unchanged { FormulaDiff::Same } else { FormulaDiff::Adjusted }
            } else if unchanged
                && !displaced
                && !formula::widens(&tb, &moved)
                && !formula::retargets(&tb, this, |name| maps.get(&name.to_lowercase()))
            {
                // Unchanged text where the adjustment only reflects a deleted
                // row at a range's edge: a row edited in place can align as
                // deleted plus inserted, so this is not evidence of a change.
                // Unchanged text is reported when Excel would have widened a
                // range over inserted rows (a total no longer covering a new
                // line) or when a referenced cell that still exists moved.
                FormulaDiff::Same
            } else {
                let expected = (!formula::same(&moved, &tb)).then(|| formula::render(&moved));
                FormulaDiff::Changed { expected, moved: Some(moved), base: text(Some(fb)), other: text(Some(fo)) }
            }
        }
        _ => unreachable!("every combination is covered above"),
    }
}

/// A formula change before folding: where it is and its position-free forms.
struct FormulaEdit {
    rb: u32,
    cb: u32,
    ro: u32,
    co: u32,
    /// Index of the row pair, so "neighbouring" follows the alignment.
    k: usize,
    /// On a moved row; such edits never fold with others.
    moved: bool,
    status: Status,
    base: Option<String>,
    other: Option<String>,
    expected: Option<String>,
    /// `(base form, other form, expected form)` relative to each cell.
    shape: (String, String, String),
}

/// What every sheet pair's comparison reads besides its own grids.
struct Shared<'a> {
    /// Every sheet's row and column maps, for references across sheets.
    maps: &'a HashMap<String, SheetMap>,
    /// Those maps from the other side back to the base side, by the other
    /// workbook's sheet names.
    inverse: &'a HashMap<String, SheetMap>,
    options: CompareOptions,
    /// The base and other workbooks' number formats.
    formats: (Formats<'a>, Formats<'a>),
}

fn sheet_changes(
    id: &str,
    gb: &Grid,
    go: &Grid,
    al: &Aligned,
    (this, back): (&SheetMap, &SheetMap),
    shared: &Shared,
) -> (Vec<Change>, usize) {
    let Shared { options, formats, .. } = shared;
    let (col_al, row_al) = (&al.cols, &al.rows);
    let mut changes = Vec::new();
    for &c in &col_al.lone_base {
        changes.push(axis_change(id, Kind::Column, Status::OnlyBase, Some(col_name(c)), None, None));
    }
    for &c in &col_al.lone_other {
        changes.push(axis_change(id, Kind::Column, Status::OnlyOther, None, Some(col_name(c)), None));
    }
    for &(cb, co) in &col_al.moves {
        changes.push(axis_change(id, Kind::Column, Status::Moved, Some(col_name(cb)), Some(col_name(co)), None));
    }

    let label = |g: &Grid, r: u32| {
        g.sheet
            .rows
            .binary_search_by_key(&r, |x| x.index)
            .ok()
            .and_then(|i| Grid::row_label(&g.sheet.rows[i]))
            .map(display_label)
    };
    for &r in &row_al.lone_base {
        changes.push(row_change(id, Status::OnlyBase, Some(r), None, label(gb, r)));
    }
    for &r in &row_al.lone_other {
        changes.push(row_change(id, Status::OnlyOther, None, Some(r), label(go, r)));
    }
    for &(rb, ro) in &row_al.moves {
        changes.push(row_change(id, Status::Moved, Some(rb), Some(ro), label(go, ro)));
    }

    let labels_for = |rb: u32, cb: u32, ro: u32, co: u32| {
        let row_label = label(go, ro).or_else(|| label(gb, rb));
        let base_row = label(gb, rb).filter(|l| Some(l) != row_label.as_ref());
        let column = go.column_label(ro, co).or_else(|| gb.column_label(rb, cb));
        Labels { row: row_label, column, base_row, inferred: true }
    };

    // Visit populated cells only: a sparse sheet's paired rows times paired
    // columns can be billions of empty combinations.
    let to_other: HashMap<u32, u32> = col_al.pairs.iter().chain(&col_al.moves).copied().collect();
    let to_base: HashMap<u32, u32> = col_al.pairs.iter().chain(&col_al.moves).map(|&(b, o)| (o, b)).collect();
    let mut edits: Vec<FormulaEdit> = Vec::new();
    let mut adjusted = 0;
    // Moved rows get indexes far past the ordered pairs, so folding never
    // treats the last paired row and the first moved row as neighbours.
    let ordered = row_al.pairs.iter().enumerate();
    let moved = row_al.moves.iter().enumerate().map(|(i, m)| (row_al.pairs.len() + 1 + 2 * i, m));
    for (k, &(rb, ro)) in ordered.chain(moved) {
        let from_base = gb.row_cells(rb).iter().filter_map(|c| to_other.get(&c.col).map(|&co| (c.col, co)));
        let from_other = go
            .row_cells(ro)
            .iter()
            .filter_map(|c| to_base.get(&c.col).map(|&cb| (cb, c.col)))
            .filter(|&(cb, _)| gb.cell(rb, cb).is_none());
        let cols: Vec<(u32, u32)> = from_base.chain(from_other).collect();
        for (cb, co) in cols {
            let (cell_b, cell_o) = (gb.cell(rb, cb), go.cell(ro, co));
            let (fb, fo) = (cell_b.and_then(|c| c.formula.as_ref()), cell_o.and_then(|c| c.formula.as_ref()));
            match diff_formulas(fb, fo, (this, back), shared) {
                FormulaDiff::Same => {}
                FormulaDiff::Adjusted => adjusted += 1,
                FormulaDiff::Changed { expected, moved, base, other } => {
                    let form = |f: &Option<String>, r: u32, c: u32| {
                        f.as_deref().map(|f| formula::relative_form(&formula::tokenize(f), r, c)).unwrap_or_default()
                    };
                    let moved_form = moved.as_deref().map(|m| formula::relative_form(m, ro, co)).unwrap_or_default();
                    edits.push(FormulaEdit {
                        rb,
                        cb,
                        ro,
                        co,
                        k,
                        moved: k > row_al.pairs.len(),
                        status: match (&base, &other) {
                            (None, _) => Status::OnlyOther,
                            (_, None) => Status::OnlyBase,
                            _ => Status::Changed,
                        },
                        shape: (form(&base, rb, cb), form(&other, ro, co), moved_form),
                        base,
                        other,
                        expected,
                    });
                }
            }

            let vb = cell_b.map_or(&Value::Empty, |c| &c.value);
            let vo = cell_o.map_or(&Value::Empty, |c| &c.value);
            if same_value(vb, vo, options.tolerance) {
                continue;
            }
            let status = match (value_bytes(vb), value_bytes(vo)) {
                (None, _) => Status::OnlyOther,
                (_, None) => Status::OnlyBase,
                _ => Status::Changed,
            };
            let delta = match (vb, vo) {
                (Value::Number(x), Value::Number(y)) => Some(y - x),
                (Value::Text(x), Value::Text(y)) => numeric_text(x).zip(numeric_text(y)).map(|(x, y)| y - x),
                _ => None,
            };
            let (bn, on) = (cell_name(rb, cb), cell_name(ro, co));
            changes.push(Change {
                id: format!("{id}:value:{bn}:{on}"),
                kind: Kind::Value,
                status,
                base: Some(cell_location(bn, vb, cell_b.map_or(0, |c| c.style), &formats.0)),
                other: Some(cell_location(on, vo, cell_o.map_or(0, |c| c.style), &formats.1)),
                delta,
                expected: None,
                labels: Some(labels_for(rb, cb, ro, co)),
                properties: Vec::new(),
            });
        }
    }
    changes.extend(fold_formula_edits(id, edits, &labels_for));
    changes.sort_by_key(order_key);
    (changes, adjusted)
}

/// Folds formula edits that are the same change filled down a column:
/// neighbouring cells in alignment order whose base, other, and adjusted
/// formulas have the same position-free form.
fn fold_formula_edits(
    id: &str,
    mut edits: Vec<FormulaEdit>,
    labels_for: &dyn Fn(u32, u32, u32, u32) -> Labels,
) -> Vec<Change> {
    let mut out = Vec::new();
    // Down columns first: (column pair, row order).
    edits.sort_by_key(|e| (e.cb, e.co, e.k));
    let mut runs: Vec<Vec<FormulaEdit>> = Vec::new();
    for e in edits {
        if let Some(run) = runs.last_mut()
            && let Some(last) = run.last()
            && (last.cb, last.co) == (e.cb, e.co)
            && !e.moved
            && !last.moved
            && e.k == last.k + 1
            // Folded ranges must cover exactly the changed cells on both sides.
            && e.rb == last.rb + 1
            && e.ro == last.ro + 1
            && last.status == e.status
            && last.shape == e.shape
        {
            run.push(e);
            continue;
        }
        runs.push(vec![e]);
    }
    for run in runs {
        let (first, last) = (&run[0], &run[run.len() - 1]);
        let span = |r1: u32, c1: u32, r2: u32, c2: u32| {
            let range =
                Range { first_row: r1.min(r2), first_col: c1.min(c2), last_row: r1.max(r2), last_col: c1.max(c2) };
            (run.len() > 1).then(|| range.name())
        };
        let (bn, on) = (cell_name(first.rb, first.cb), cell_name(first.ro, first.co));
        let base_range = span(first.rb, first.cb, last.rb, last.cb);
        let other_range = span(first.ro, first.co, last.ro, last.co);
        let reference = |range: &Option<String>, cell: &str| range.clone().unwrap_or_else(|| cell.to_owned());
        out.push(Change {
            id: format!("{id}:formula:{}:{}", reference(&base_range, &bn), reference(&other_range, &on)),
            kind: Kind::Formula,
            status: first.status,
            base: Some(Location {
                cell: Some(bn),
                range: base_range,
                formula: first.base.clone(),
                ..Location::default()
            }),
            other: Some(Location {
                cell: Some(on),
                range: other_range,
                formula: first.other.clone(),
                ..Location::default()
            }),
            delta: None,
            expected: first.expected.clone(),
            labels: Some(labels_for(first.rb, first.cb, first.ro, first.co)),
            properties: Vec::new(),
        });
    }
    out
}

fn cell_location(cell: String, v: &Value, style: u32, formats: &Formats) -> Location {
    let kind = (!matches!(v, Value::Empty)).then(|| v.type_name());
    Location {
        cell: Some(cell),
        kind,
        value: json_value(v),
        display: formats.display(style, v),
        date: formats.iso_date(style, v),
        ..Location::default()
    }
}

fn axis_change(
    id: &str,
    kind: Kind,
    status: Status,
    base: Option<String>,
    other: Option<String>,
    label: Option<String>,
) -> Change {
    let reference = |x: &Option<String>| x.clone().unwrap_or_else(|| "-".to_owned());
    let loc = |c: Option<String>| c.map(|c| Location { column: Some(c), ..Location::default() });
    Change {
        id: format!("{id}:{}:{}:{}", kind.as_str(), reference(&base), reference(&other)),
        kind,
        status,
        base: loc(base),
        other: loc(other),
        delta: None,
        expected: None,
        labels: label.map(|l| Labels { row: None, column: Some(l), base_row: None, inferred: true }),
        properties: Vec::new(),
    }
}

fn row_change(id: &str, status: Status, base: Option<u32>, other: Option<u32>, label: Option<String>) -> Change {
    let reference = |x: Option<u32>| x.map_or_else(|| "-".to_owned(), |r| r.to_string());
    let loc = |r: Option<u32>| r.map(|r| Location { row: Some(r), ..Location::default() });
    Change {
        id: format!("{id}:row:{}:{}", reference(base), reference(other)),
        kind: Kind::Row,
        status,
        base: loc(base),
        other: loc(other),
        delta: None,
        expected: None,
        labels: label.map(|l| Labels { row: Some(l), column: None, base_row: None, inferred: true }),
        properties: Vec::new(),
    }
}

/// Columns first, then rows and values by position on the other side (the
/// base side for removals).
fn order_key(c: &Change) -> (u8, u32, u32, u8) {
    let parse = |l: &Option<Location>| -> (u32, u32) {
        let Some(l) = l else { return (u32::MAX, u32::MAX) };
        if let Some(cell) = &l.cell {
            crate::cellref::parse_cell(cell).unwrap_or((u32::MAX, u32::MAX))
        } else if let Some(r) = l.row {
            (r, 0)
        } else {
            (0, l.column.as_deref().and_then(crate::cellref::parse_col).unwrap_or(0))
        }
    };
    let (r, col) = if c.other.is_some() { parse(&c.other) } else { parse(&c.base) };
    (
        u8::from(c.kind != Kind::Column),
        r,
        col,
        u8::from(matches!(c.kind, Kind::Value | Kind::Formula)) + u8::from(c.kind == Kind::Value),
    )
}

// ---------------------------------------------------------------------------
// listing

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
    use super::{label_key, numeric_text};

    #[test]
    fn labels_drop_spacing_case_and_outline_markers() {
        for (text, key) in [
            ("자 산 총 계", "자산총계"),
            ("    Ⅰ. 유동자산", "유동자산"),
            ("Ⅱ비유동자산", "비유동자산"),
            ("(1)  당좌자산", "당좌자산"),
            ("1. Cash", "cash"),
            ("가. 현금", "현금"),
            ("2025", "2025"),
            ("2025.12.31", "2025.12.31"),
            ("가지급금", "가지급금"),
            ("(주)우성", "(주)우성"),
            ("(1)", "(1)"),
        ] {
            assert_eq!(label_key(text).as_deref(), Some(key), "{text}");
        }
        assert_eq!(label_key("   "), None);
    }

    #[test]
    fn numbers_written_as_text_parse() {
        assert_eq!(numeric_text("19,555,301,961"), Some(19_555_301_961.0));
        assert_eq!(numeric_text("(1,000)"), Some(-1000.0));
        assert_eq!(numeric_text("-12.5"), Some(-12.5));
        assert_eq!(numeric_text("50%"), Some(0.5));
        for not in ["", "-", "1-2", "abc", "12원", "1.2.3x"] {
            assert_eq!(numeric_text(not), None, "{not}");
        }
    }
}
