//! Content-aligned comparison against synthetic workbooks: each alignment
//! case in PLAN.md gives exactly the expected changes.

use xldif::diff::{AlignMode, Change, CompareOptions, Comparison, Filter, Kind, SheetComparison, Status, compare};
use xldif::read_bytes;
use xldif_fixtures::grid_book;

type Grid<'a> = &'a [&'a [&'a str]];

const BS: Grid = &[
    &["Statement of financial position"],
    &["Account", "2025", "2024"],
    &["Cash", "100", "90"],
    &["Receivables", "200", "180"],
    &["Inventory", "300", "310"],
    &["Total", "600", "580"],
];

fn run(base: &[(&str, Grid)], other: &[(&str, Grid)], options: CompareOptions) -> Comparison {
    let (b, o) = (read_bytes(grid_book(base)).unwrap(), read_bytes(grid_book(other)).unwrap());
    compare("base.xlsx", &b, "other.xlsx", &o, options)
}

fn content(base: &[(&str, Grid)], other: &[(&str, Grid)]) -> Comparison {
    run(base, other, CompareOptions::default())
}

fn sheet<'a>(c: &'a Comparison, name: &str) -> &'a SheetComparison {
    c.sheets.iter().find(|s| s.base.as_deref() == Some(name) || s.other.as_deref() == Some(name)).unwrap()
}

/// `kind status base→other` for each change, in order.
fn brief(s: &SheetComparison) -> Vec<String> {
    s.changes.iter().map(describe).collect()
}

fn describe(c: &Change) -> String {
    let r = |l: &Option<xldif::diff::Location>| {
        l.as_ref()
            .and_then(|l| l.cell.clone().or_else(|| l.row.map(|r| r.to_string())).or_else(|| l.column.clone()))
            .unwrap_or_else(|| "-".into())
    };
    format!("{} {:?} {}→{}", c.kind.as_str(), c.status, r(&c.base), r(&c.other))
}

#[test]
fn a_workbook_compared_with_itself_has_no_changes() {
    let c = content(&[("BS", BS), ("Notes", BS)], &[("BS", BS), ("Notes", BS)]);
    assert!(c.sheets.iter().all(|s| s.status == Status::Same && s.changes.is_empty()));
    assert_eq!(c.summary, xldif::diff::Summary::default());
    assert_eq!(sheet(&c, "BS").alignment.as_ref().unwrap().rows, [[1, 1, 6]]);
}

#[test]
fn an_inserted_row_is_one_change() {
    let other: Grid = &[BS[0], BS[1], BS[2], BS[3], &["Prepaid", "50", "40"], BS[4], &["Total", "650", "620"]];
    let c = content(&[("BS", BS)], &[("BS", other)]);
    let s = sheet(&c, "BS");
    assert_eq!(brief(s), ["row OnlyOther -→5", "value Changed B6→B7", "value Changed C6→C7"]);
    let total = &s.changes[1];
    assert_eq!(total.delta, Some(50.0));
    let labels = total.labels.as_ref().unwrap();
    assert_eq!((labels.row.as_deref(), labels.column.as_deref()), (Some("Total"), Some("2025")));
    assert_eq!(s.changes[0].labels.as_ref().unwrap().row.as_deref(), Some("Prepaid"));
    assert_eq!(s.alignment.as_ref().unwrap().rows, [[1, 1, 4], [5, 6, 2]]);
}

#[test]
fn a_deleted_row_mirrors_an_inserted_one() {
    let other: Grid = &[BS[0], BS[1], BS[2], BS[4], BS[5]];
    let c = content(&[("BS", BS)], &[("BS", other)]);
    assert_eq!(brief(sheet(&c, "BS")), ["row OnlyBase 4→-"]);
    let back = content(&[("BS", other)], &[("BS", BS)]);
    assert_eq!(brief(sheet(&back, "BS")), ["row OnlyOther -→4"]);
}

#[test]
fn an_inserted_column_and_inserted_rows_together() {
    // A new 2026 column, and a row added at the top of the table.
    let other: Grid = &[
        &["Statement of financial position"],
        &["Unaudited draft"],
        &["Account", "2026", "2025", "2024"],
        &["Cash", "120", "100", "90"],
        &["Receivables", "210", "200", "180"],
        &["Inventory", "330", "300", "310"],
        &["Total", "660", "600", "580"],
    ];
    let c = content(&[("BS", BS)], &[("BS", other)]);
    assert_eq!(brief(sheet(&c, "BS")), ["column OnlyOther -→B", "row OnlyOther -→2"]);
}

#[test]
fn a_moved_row_is_one_move() {
    let other: Grid = &[BS[0], BS[1], BS[3], BS[4], BS[2], BS[5]];
    let c = content(&[("BS", BS)], &[("BS", other)]);
    let s = sheet(&c, "BS");
    assert_eq!(brief(s), ["row Moved 3→5"]);
    assert_eq!(s.alignment.as_ref().unwrap().row_moves, [[3, 5, 1]]);
    assert_eq!(c.summary.rows_moved, 1);
}

#[test]
fn blank_spacer_rows_are_not_changes() {
    let other: Grid = &[BS[0], &[], BS[1], &[], BS[2], BS[3], BS[4], &[], &[], BS[5]];
    let c = content(&[("BS", BS)], &[("BS", other)]);
    assert!(sheet(&c, "BS").changes.is_empty());
}

#[test]
fn renumbered_and_respaced_labels_still_pair() {
    let base: Grid = &[&["Account", "2025"], &["Ⅰ. 유동자산", "100"], &["(1) 당좌자산", "60"], &["자 산 총 계", "100"]];
    let other: Grid = &[&["Account", "2025"], &["1. 유동자산", "110"], &["당좌자산", "70"], &["자산총계", "110"]];
    let c = content(&[("S", base)], &[("S", other)]);
    // The rows pair, so each edited label is one value change next to its
    // amount, not a deleted and an inserted row.
    let expected: Vec<String> =
        (2..=4).flat_map(|r| [format!("value Changed A{r}→A{r}"), format!("value Changed B{r}→B{r}")]).collect();
    assert_eq!(brief(sheet(&c, "S")), expected);
    assert!(sheet(&c, "S").alignment.as_ref().unwrap().rows == [[1, 1, 4]]);
}

#[test]
fn a_relabelled_row_with_the_same_layout_is_a_value_change() {
    let base: Grid = &[&["Account", "2025"], &["Cash", "1"], &["Net profit"], &["Total", "9"]];
    let other: Grid = &[&["Account", "2025"], &["Cash", "1"], &["Net loss"], &["Total", "9"]];
    let c = content(&[("S", base)], &[("S", other)]);
    assert_eq!(brief(sheet(&c, "S")), ["value Changed A3→A3"]);
}

#[test]
fn sheets_pair_by_name_then_by_content() {
    let notes: Grid = &[&["Note 1"], &["Policy", "text that stays the same"], &["Rate", "0.05"]];
    let c = content(
        &[("BS", BS), ("Notes", notes), ("Old", &[&["gone"]])],
        &[("Notes 2025", notes), ("BS", BS), ("New", &[&["fresh"]])],
    );
    let renamed = sheet(&c, "Notes");
    assert!(renamed.renamed && renamed.changes.is_empty());
    assert_eq!(renamed.other.as_deref(), Some("Notes 2025"));
    assert_eq!(sheet(&c, "Old").status, Status::OnlyBase);
    assert_eq!(sheet(&c, "New").status, Status::OnlyOther);
    // BS moved from first to second place relative to Notes.
    assert_eq!(c.sheets.iter().filter(|s| s.moved).count(), 1);
    assert_eq!((c.summary.sheets_renamed, c.summary.sheets_only_base, c.summary.sheets_only_other), (1, 1, 1));
}

#[test]
fn numbers_written_as_text_get_a_delta() {
    let base: Grid = &[&["Account", "Amount"], &["Cash", "1,000"]];
    let other: Grid = &[&["Account", "Amount"], &["Cash", "(1,500)"]];
    let c = content(&[("S", base)], &[("S", other)]);
    assert_eq!(sheet(&c, "S").changes[0].delta, Some(-2500.0));
}

#[test]
fn tolerance_and_position_mode() {
    let other: Grid = &[BS[0], BS[1], &["Cash", "100.004", "90"], BS[3], BS[4], BS[5]];
    let tolerant =
        run(&[("BS", BS)], &[("BS", other)], CompareOptions { tolerance: 0.01, ..CompareOptions::default() });
    assert!(sheet(&tolerant, "BS").changes.is_empty());

    let inserted: Grid = &[BS[0], BS[1], &["Prepaid", "1", "1"], BS[2], BS[3], BS[4], BS[5]];
    let positional = run(
        &[("BS", BS)],
        &[("BS", inserted)],
        CompareOptions { align: AlignMode::Position, ..CompareOptions::default() },
    );
    let s = sheet(&positional, "BS");
    assert_eq!(s.alignment.as_ref().unwrap().method, "position");
    assert!(s.changes.iter().all(|c| c.kind == Kind::Value) && s.changes.len() > 10);
}

#[test]
fn swapping_the_sides_mirrors_the_changes() {
    let other: Grid = &[BS[0], BS[1], BS[3], &["Prepaid", "50", "40"], BS[4], BS[2], &["Total", "650", "620"]];
    let ab = content(&[("BS", BS)], &[("BS", other)]);
    let ba = content(&[("BS", other)], &[("BS", BS)]);
    let flip = |s: &str| {
        let (kind_status, refs) = s.rsplit_once(' ').unwrap();
        let (b, o) = refs.split_once('→').unwrap();
        let kind_status =
            kind_status.replace("OnlyBase", "X").replace("OnlyOther", "OnlyBase").replace('X', "OnlyOther");
        format!("{kind_status} {o}→{b}")
    };
    let mut mirrored: Vec<String> = brief(sheet(&ba, "BS")).iter().map(|s| flip(s)).collect();
    let mut forward = brief(sheet(&ab, "BS"));
    mirrored.sort();
    forward.sort();
    assert_eq!(forward, mirrored);
}

#[test]
fn listing_filters_and_counts_what_it_leaves_out() {
    let other: Grid = &[BS[0], BS[1], &["Cash", "101", "91"], &["Receivables", "201", "181"], BS[4], BS[5]];
    let c = content(&[("BS", BS), ("Same", BS)], &[("BS", other), ("Same", BS)]);
    let listing = c.clone().list(&Filter { limit: Some(3), ..Filter::default() });
    assert_eq!(listing.listed, 3);
    assert_eq!(listing.omitted["BS"]["value"], 1);
    assert_eq!(listing.sheets.len(), 1, "same sheets are left out by default");
    let all = c.clone().list(&Filter { all: true, ..Filter::default() });
    assert_eq!(all.sheets.len(), 2);
    let rows_only = c.list(&Filter { kinds: vec![Kind::Row], ..Filter::default() });
    assert_eq!((rows_only.listed, rows_only.omitted["BS"]["value"]), (0, 4));
}

#[test]
fn ids_are_stable_across_filters() {
    let other: Grid = &[BS[0], BS[1], &["Cash", "101", "90"], BS[3], BS[4], BS[5]];
    let c = content(&[("BS", BS)], &[("BS", other)]);
    assert_eq!(sheet(&c, "BS").changes[0].id, "s0:value:B3:B3");
}
