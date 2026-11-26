//! The changes of one sheet pair's cells, rows, and columns: values and
//! formulas on paired cells, and one-sided and moved rows and columns.

use std::collections::HashMap;

use super::axis::Aligned;
use super::grid::{Grid, display_label, numeric_text, value_bytes};
use super::{Change, CompareOptions, Kind, Labels, Location, Place, Status};
use crate::cellref::{Range, cell_name, col_name};
use crate::formula::{self, SheetMap};
use crate::links::LinkMap;
use crate::model::{Formula, FormulaKind, Value, Workbook};
use crate::numfmt::Formats;
use crate::views::json_value;

pub(super) fn same_value(a: &Value, b: &Value, tolerance: f64) -> bool {
    match (a, b) {
        // Values are finite, so a zero tolerance means exact equality.
        (Value::Number(x), Value::Number(y)) => (x - y).abs() <= tolerance,
        (Value::Text(x), Value::Empty) | (Value::Empty, Value::Text(x)) => x.is_empty(),
        _ => a == b,
    }
}

/// What one paired cell's formulas amount to.
pub(super) enum FormulaDiff {
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

pub(super) fn diff_formulas(
    fb: Option<&Formula>,
    fo: Option<&Formula>,
    (this, back): (&SheetMap, &SheetMap),
    shared: &Shared,
) -> FormulaDiff {
    pub(super) fn usable(f: Option<&Formula>) -> Option<&Formula> {
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
            let (stored, to) = (formula::tokenize(&fb.text), formula::tokenize(&fo.text));
            // External workbooks are numbered per workbook, so the base
            // formula is read in the other workbook's numbering first.
            let relinked = shared.links.relink(&stored);
            let tb = relinked.as_deref().unwrap_or(&stored);
            // The adjusted base formula is what the other side should read.
            let moved = formula::remap(tb, this, |name| maps.get(&name.to_lowercase()));
            let unchanged = formula::same(tb, &to);
            // Unchanged text whose address now holds a cell that came from
            // another row or column: a deleted row's place taken by the row
            // below it.
            let displaced = unchanged && formula::retargets(&to, back, |name| inverse.get(&name.to_lowercase()));
            if formula::same(&moved, &to) && !displaced {
                if unchanged { FormulaDiff::Same } else { FormulaDiff::Adjusted }
            } else if unchanged
                && !displaced
                && !formula::widens(tb, &moved)
                && !formula::retargets(tb, this, |name| maps.get(&name.to_lowercase()))
            {
                // Unchanged text where the adjustment only reflects a deleted
                // row at a range's edge: a row edited in place can align as
                // deleted plus inserted, so this is not evidence of a change.
                // Unchanged text is reported when Excel would have widened a
                // range over inserted rows (a total no longer covering a new
                // line) or when a referenced cell that still exists moved.
                FormulaDiff::Same
            } else {
                let expected = (!formula::same(&moved, &stored)).then(|| formula::render(&moved));
                FormulaDiff::Changed { expected, moved: Some(moved), base: text(Some(fb)), other: text(Some(fo)) }
            }
        }
        _ => unreachable!("every combination is covered above"),
    }
}

/// A formula change before folding: where it is and its position-free forms.
pub(super) struct FormulaEdit {
    pub(super) rb: u32,
    pub(super) cb: u32,
    pub(super) ro: u32,
    pub(super) co: u32,
    /// Index of the row pair, so "neighbouring" follows the alignment.
    pub(super) k: usize,
    /// On a moved row; such edits never fold with others.
    pub(super) moved: bool,
    pub(super) status: Status,
    pub(super) base: Option<String>,
    pub(super) other: Option<String>,
    pub(super) expected: Option<String>,
    /// `(base form, other form, expected form)` relative to each cell.
    pub(super) shape: (String, String, String),
}

/// What every sheet pair's comparison reads besides its own grids.
pub(super) struct Shared<'a> {
    /// Every sheet's row and column maps, for references across sheets.
    pub(super) maps: &'a HashMap<String, SheetMap>,
    /// Those maps from the other side back to the base side, by the other
    /// workbook's sheet names.
    pub(super) inverse: &'a HashMap<String, SheetMap>,
    pub(super) options: CompareOptions,
    /// The base and other workbooks' number formats.
    pub(super) formats: (Formats<'a>, Formats<'a>),
    /// When the workbooks use different date systems: what to add to each
    /// side's serials to put them on one scale.
    pub(super) date_offsets: Option<(f64, f64)>,
    /// The base workbook's external links in the other's numbering.
    pub(super) links: &'a LinkMap,
}

/// What to add to a workbook's date serials to get 1900-system serials: the
/// 1904 system counts from 1904-01-01, which is 1,462 in the 1900 system.
pub(super) fn date_offset(book: &Workbook) -> f64 {
    if book.date1904 { 1462.0 } else { 0.0 }
}

pub(super) fn sheet_changes(
    id: &str,
    gb: &Grid,
    go: &Grid,
    al: &Aligned,
    (this, back): (&SheetMap, &SheetMap),
    shared: &Shared,
) -> (Vec<Change>, usize, usize) {
    let Shared { options, formats, .. } = shared;
    let (col_al, row_al) = (&al.cols, &al.rows);
    let mut changes = Vec::new();
    for &c in &col_al.lone_base {
        changes.push(axis_change(id, Kind::Column, Status::OnlyBase, Some(c), None, None));
    }
    for &c in &col_al.lone_other {
        changes.push(axis_change(id, Kind::Column, Status::OnlyOther, None, Some(c), None));
    }
    for &(cb, co) in &col_al.moves {
        changes.push(axis_change(id, Kind::Column, Status::Moved, Some(cb), Some(co), None));
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
    // Value changes a formula produced rather than a person typed: a cell
    // with a formula on either side, or one inside an array formula's or
    // data table's range, whose cells other than the anchor carry no
    // formula of their own.
    let mut computed = 0;
    let (spilled_b, spilled_o) = (computed_ranges(gb), computed_ranges(go));
    let is_computed = |cell: Option<&crate::model::Cell>, spilled: &[Range], r: u32, c: u32| {
        cell.is_some_and(|c| c.formula.is_some()) || spilled.iter().any(|x| x.contains(r, c))
    };
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
            // Under different date systems the same date has two serials,
            // so two date cells compare by the date they show, and their
            // difference is in days on one scale. A time of day alone
            // (`06:00:00`, no date part) is the same serial in both systems.
            // (Alignment still reads the serials: see `value_bytes`.)
            let days_apart = shared.date_offsets.and_then(|(ob, oo)| {
                let (Value::Number(x), Value::Number(y)) = (vb, vo) else { return None };
                let shown_b = formats.0.iso_date(cell_b?.style, vb)?;
                let shown_o = formats.1.iso_date(cell_o?.style, vo)?;
                let on_one_scale = |shown: &str, v: f64, offset: f64| if shown.contains('-') { v + offset } else { v };
                let days = on_one_scale(&shown_o, *y, oo) - on_one_scale(&shown_b, *x, ob);
                // Adding the offset rounds in the last binary place: under
                // a ten-thousandth of a second, the serials are the same.
                Some(if days.abs() < 1e-9 { 0.0 } else { days })
            });
            let same = match days_apart {
                Some(days) => days.abs() <= options.tolerance,
                None => same_value(vb, vo, options.tolerance),
            };
            if same {
                continue;
            }
            if is_computed(cell_b, &spilled_b, rb, cb) || is_computed(cell_o, &spilled_o, ro, co) {
                computed += 1;
            }
            let status = match (value_bytes(vb, 0.0), value_bytes(vo, 0.0)) {
                (None, _) => Status::OnlyOther,
                (_, None) => Status::OnlyBase,
                _ => Status::Changed,
            };
            let delta = match (vb, vo) {
                _ if days_apart.is_some() => days_apart,
                (Value::Number(x), Value::Number(y)) => Some(y - x),
                (Value::Text(x), Value::Text(y)) => numeric_text(x).zip(numeric_text(y)).map(|(x, y)| y - x),
                _ => None,
            };
            changes.push(Change {
                id: format!("{id}:value:{}:{}", cell_name(rb, cb), cell_name(ro, co)),
                kind: Kind::Value,
                status,
                base: Some(cell_location((rb, cb), vb, cell_b.map_or(0, |c| c.style), &formats.0)),
                other: Some(cell_location((ro, co), vo, cell_o.map_or(0, |c| c.style), &formats.1)),
                delta,
                expected: None,
                labels: Some(labels_for(rb, cb, ro, co)),
                properties: Vec::new(),
            });
        }
    }
    changes.extend(fold_formula_edits(id, edits, &labels_for));
    changes.sort_by_key(order_key);
    (changes, adjusted, computed)
}

/// The ranges of a sheet's array formulas and data tables: cells whose
/// values those formulas produce. A single-cell range is left out: its one
/// cell carries the formula itself.
fn computed_ranges(g: &Grid) -> Vec<Range> {
    g.sheet
        .cells()
        .filter_map(|(_, c)| c.formula.as_ref())
        .filter(|f| matches!(f.kind, FormulaKind::Array | FormulaKind::DataTable))
        .filter_map(|f| f.range.as_deref().and_then(Range::parse))
        .filter(|r| (r.first_row, r.first_col) != (r.last_row, r.last_col))
        .collect()
}

/// Folds formula edits that are the same change filled down a column:
/// neighbouring cells in alignment order whose base, other, and adjusted
/// formulas have the same position-free form.
pub(super) fn fold_formula_edits(
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
        // The run's first cell, or its cells as a block.
        let span = |r1: u32, c1: u32, r2: u32, c2: u32| {
            Place::block(Range {
                first_row: r1.min(r2),
                first_col: c1.min(c2),
                last_row: r1.max(r2),
                last_col: c1.max(c2),
            })
        };
        let base = span(first.rb, first.cb, last.rb, last.cb);
        let other = span(first.ro, first.co, last.ro, last.co);
        let reference = |place: &Place| place.reference().unwrap_or_default();
        out.push(Change {
            id: format!("{id}:formula:{}:{}", reference(&base), reference(&other)),
            kind: Kind::Formula,
            status: first.status,
            base: Some(Location { place: base, formula: first.base.clone(), ..Location::default() }),
            other: Some(Location { place: other, formula: first.other.clone(), ..Location::default() }),
            delta: None,
            expected: first.expected.clone(),
            labels: Some(labels_for(first.rb, first.cb, first.ro, first.co)),
            properties: Vec::new(),
        });
    }
    out
}

pub(super) fn cell_location((row, col): (u32, u32), v: &Value, style: u32, formats: &Formats) -> Location {
    let kind = (!matches!(v, Value::Empty)).then(|| v.type_name());
    Location {
        place: Place::Cell { row, col },
        kind,
        value: json_value(v),
        display: formats.display(style, v),
        date: formats.iso_date(style, v),
        ..Location::default()
    }
}

pub(super) fn axis_change(
    id: &str,
    kind: Kind,
    status: Status,
    base: Option<u32>,
    other: Option<u32>,
    label: Option<String>,
) -> Change {
    let reference = |x: Option<u32>| x.map_or_else(|| "-".to_owned(), col_name);
    let loc = |c: Option<u32>| c.map(|c| Location { place: Place::column(c), ..Location::default() });
    Change {
        id: format!("{id}:{}:{}:{}", kind.as_str(), reference(base), reference(other)),
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

pub(super) fn row_change(
    id: &str,
    status: Status,
    base: Option<u32>,
    other: Option<u32>,
    label: Option<String>,
) -> Change {
    let reference = |x: Option<u32>| x.map_or_else(|| "-".to_owned(), |r| r.to_string());
    let loc = |r: Option<u32>| r.map(|r| Location { place: Place::row(r), ..Location::default() });
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
pub(super) fn order_key(c: &Change) -> (u8, u32, u32, u8) {
    let parse = |l: &Option<Location>| -> (u32, u32) {
        let Some(l) = l else { return (u32::MAX, u32::MAX) };
        match &l.place {
            Place::Cell { row, col } => (*row, *col),
            Place::Block(range) => (range.first_row, range.first_col),
            Place::Rows { first, .. } => (*first, 0),
            Place::Columns { first, .. } => (0, *first),
            Place::Ranges(_) | Place::Nowhere => (0, 0),
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
