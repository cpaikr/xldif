//! Properties every comparison must have, over random small workbooks: a
//! workbook compared with itself shows nothing, and swapping the two files
//! gives the mirrored result.

use proptest::prelude::*;
use xldif::diff::{Change, CompareOptions, Comparison, Filter, Kind, Location, Run, Status, compare};
use xldif::read_bytes;
use xldif_fixtures::grid_book;

type Rows = Vec<Vec<String>>;

/// Cells over a small alphabet, so labels and values repeat within a sheet
/// and alignment has ties to break.
fn cell() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => Just(String::new()),
        4 => (0..6u8).prop_map(|n| n.to_string()),
        3 => prop::sample::select(vec!["Cash", "Total", "KRW", "Note"]).prop_map(str::to_owned),
        1 => prop::sample::select(vec!["=A1+1|2", "=SUM(B1:B3)|6"]).prop_map(str::to_owned),
    ]
}

fn row() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(cell(), 0..5)
}

fn rows() -> impl Strategy<Value = Rows> {
    prop::collection::vec(row(), 0..10)
}

#[derive(Debug, Clone)]
enum Edit {
    Insert(usize, Vec<String>),
    Delete(usize),
    Move(usize, usize),
    Set(usize, usize, String),
    InsertColumn(usize, String),
    DeleteColumn(usize),
}

fn edit() -> impl Strategy<Value = Edit> {
    let at = 0..12usize;
    prop_oneof![
        (at.clone(), row()).prop_map(|(i, r)| Edit::Insert(i, r)),
        at.clone().prop_map(Edit::Delete),
        (at.clone(), at.clone()).prop_map(|(i, j)| Edit::Move(i, j)),
        (at.clone(), 0..5usize, cell()).prop_map(|(i, c, v)| Edit::Set(i, c, v)),
        (0..5usize, cell()).prop_map(|(c, v)| Edit::InsertColumn(c, v)),
        (0..5usize).prop_map(Edit::DeleteColumn),
    ]
}

fn apply(rows: &mut Rows, edit: Edit) {
    match edit {
        Edit::Insert(i, row) => rows.insert(i.min(rows.len()), row),
        Edit::Delete(i) if i < rows.len() => drop(rows.remove(i)),
        Edit::Move(i, j) if i < rows.len() => {
            let row = rows.remove(i);
            rows.insert(j.min(rows.len()), row);
        }
        Edit::Set(i, c, value) if i < rows.len() => {
            let row = &mut rows[i];
            if row.len() <= c {
                row.resize(c + 1, String::new());
            }
            row[c] = value;
        }
        Edit::InsertColumn(c, value) => {
            for row in rows.iter_mut().filter(|r| r.len() >= c) {
                row.insert(c, value.clone());
            }
        }
        Edit::DeleteColumn(c) => {
            for row in rows.iter_mut().filter(|r| r.len() > c) {
                row.remove(c);
            }
        }
        Edit::Delete(_) | Edit::Move(..) | Edit::Set(..) => {}
    }
}

/// A sheet and a version of it a few edits later.
fn versions() -> impl Strategy<Value = (Rows, Rows)> {
    (rows(), prop::collection::vec(edit(), 0..5)).prop_map(|(base, edits)| {
        let mut other = base.clone();
        for e in edits {
            apply(&mut other, e);
        }
        (base, other)
    })
}

fn book(rows: &Rows) -> xldif::model::Workbook {
    let rows: Vec<Vec<&str>> = rows.iter().map(|r| r.iter().map(String::as_str).collect()).collect();
    let rows: Vec<&[&str]> = rows.iter().map(Vec::as_slice).collect();
    read_bytes(grid_book(&[("S", &rows)])).unwrap()
}

fn compared(base: &Rows, other: &Rows, tolerance: f64) -> Comparison {
    let options = CompareOptions { tolerance, ..CompareOptions::default() };
    compare("base.xlsx", &book(base), "other.xlsx", &book(other), options)
}

/// No tolerance, or one that merges some of the small numbers the cells hold.
fn tolerance() -> impl Strategy<Value = f64> {
    prop_oneof![Just(0.0), Just(0.3), Just(1.0)]
}

fn flip(runs: &[Run]) -> Vec<Run> {
    let mut flipped: Vec<Run> = runs.iter().map(|r| [r[1], r[0], r[2]]).collect();
    flipped.sort_unstable();
    flipped
}

/// A change's kind, status, and the two places it names, with the sides
/// exchanged when `mirrored`.
fn place(c: &Change, mirrored: bool) -> (&'static str, &'static str, String, String) {
    let at = |l: &Option<Location>| {
        l.as_ref().map_or_else(String::new, |l| {
            l.cell
                .clone()
                .or_else(|| l.range.clone())
                .or_else(|| l.row.map(|r| r.to_string()))
                .or_else(|| l.column.clone())
                .unwrap_or_default()
        })
    };
    let status = match (c.status, mirrored) {
        (Status::OnlyBase, true) => Status::OnlyOther,
        (Status::OnlyOther, true) => Status::OnlyBase,
        (status, _) => status,
    };
    let (base, other) = (at(&c.base), at(&c.other));
    let (base, other) = if mirrored { (other, base) } else { (base, other) };
    (c.kind.as_str(), status.as_str(), base, other)
}

/// Every change but the formula ones, which have a direction: a row
/// inserted inside a total is a change, the same row deleted is not.
fn places(c: &Comparison, mirrored: bool) -> Vec<(&'static str, &'static str, String, String)> {
    let mut all: Vec<_> = c
        .sheets
        .iter()
        .flat_map(|s| &s.changes)
        .filter(|c| c.kind != Kind::Formula)
        .map(|c| place(c, mirrored))
        .collect();
    all.sort_unstable();
    all
}

proptest! {
    // A failing case is printed, not saved: nothing is written beside the tests.
    #![proptest_config(ProptestConfig { cases: 192, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn a_workbook_against_itself_has_no_changes(rows in rows(), tolerance in tolerance()) {
        let c = compared(&rows, &rows, tolerance);
        prop_assert_eq!(&c.summary, &xldif::diff::Summary::default());
        prop_assert!(c.sheets.iter().all(|s| s.status == Status::Same && s.changes.is_empty()));
        prop_assert!(c.objects.is_empty());
        // The report draws the sheet when asked to show unchanged ones too,
        // and marks no row as changed, one-sided, or moved.
        let listing = c.list(&Filter { all: true, ..Filter::default() });
        let html = xldif::html::render_html(&listing, &book(&rows), &book(&rows), &[]);
        prop_assert!(!html.contains("<tr class=\"r chg") && !html.contains("<tr class=\"r r"));
    }

    #[test]
    fn swapping_the_files_mirrors_the_comparison((base, other) in versions(), tolerance in tolerance()) {
        let (ab, ba) = (compared(&base, &other, tolerance), compared(&other, &base, tolerance));
        let (sa, sb) = (&ab.sheets[0], &ba.sheets[0]);
        let (va, vb) = (sa.alignment.as_ref().unwrap(), sb.alignment.as_ref().unwrap());
        prop_assert_eq!(&va.rows, &flip(&vb.rows));
        prop_assert_eq!(&va.row_moves, &flip(&vb.row_moves));
        prop_assert_eq!(&va.cols, &flip(&vb.cols));
        prop_assert_eq!(&va.col_moves, &flip(&vb.col_moves));
        prop_assert_eq!(places(&ab, false), places(&ba, true));
        // No two changes share an id, and the pairing handed to renderers
        // is the one the alignment runs summarize.
        let mut ids: Vec<&str> = sa.changes.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        prop_assert!(ids.windows(2).all(|w| w[0] != w[1]), "{:?}", ids);
        // (A run also spans the blank rows between its pairs.)
        let axes = sa.axes.as_ref().unwrap();
        let in_a_run = |&(b, o): &(u32, u32)| {
            va.rows.iter().any(|r| (r[0]..r[0] + r[2]).contains(&b) && i64::from(o) - i64::from(b) == i64::from(r[1]) - i64::from(r[0]))
        };
        prop_assert!(axes.rows.pairs.iter().all(in_a_run), "{:?} not in {:?}", axes.rows.pairs, va.rows);
        prop_assert_eq!(va.row_moves.len(), axes.rows.moves.len());
    }
}
