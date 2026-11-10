//! Content-aligned comparison against synthetic workbooks: each alignment
//! case in PLAN.md gives exactly the expected changes.
#![allow(clippy::format_collect, clippy::format_push_string)]

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

#[test]
fn rows_without_values_still_match_themselves() {
    // A formula with no cached result, and a cell holding an empty string.
    let rows = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Label</t></is></c></row>
        <row r="2"><c r="B2"><f>A1&amp;"x"</f></c></row>
        <row r="3"><c r="B3" t="inlineStr"><is><t></t></is></c></row>
        <row r="4"><c r="A4"><v>4</v></c></row>"#;
    let book = read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", rows).build()).unwrap();
    let c = compare("a", &book, "b", &book, CompareOptions::default());
    assert!(c.sheets[0].changes.is_empty(), "{:?}", c.sheets[0].changes);
}

#[test]
fn alignment_runs_stop_at_moved_rows() {
    let base: Grid =
        &[&["a", "1"], &["b", "2", "x"], &["c", "3"], &["d", "4"], &["e", "5"], &["f", "6", "", "y"], &["g", "7"]];
    let other: Grid =
        &[&["a", "1"], &["f", "6", "", "y"], &["c", "3"], &["d", "4"], &["e", "5"], &["b", "2", "x"], &["g", "7"]];
    let c = content(&[("S", base)], &[("S", other)]);
    let al = sheet(&c, "S").alignment.clone().unwrap();
    assert_eq!(al.row_moves, [[2, 6, 1], [6, 2, 1]]);
    // No run may cover rows 2 or 6, which moved elsewhere.
    for [b, o, len] in &al.rows {
        assert!(!(*b..b + len).any(|r| r == 2 || r == 6), "{:?}", al.rows);
        assert!(!(*o..o + len).any(|r| r == 2 || r == 6), "{:?}", al.rows);
    }
}

#[test]
fn sparse_sheets_compare_in_time_proportional_to_cells() {
    // A diagonal: every row and column populated, one cell each.
    let n = 3000;
    let rows: String = (1..=n)
        .map(|i| format!(r#"<row r="{i}"><c r="{}{i}"><v>{i}</v></c></row>"#, xldif::cellref::col_name(i)))
        .collect();
    let book = read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", &rows).build()).unwrap();
    let start = std::time::Instant::now();
    let c = compare("a", &book, "b", &book, CompareOptions { align: AlignMode::Position, ..CompareOptions::default() });
    assert!(c.sheets[0].changes.is_empty());
    assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
}

#[test]
fn column_labels_prefer_the_header_over_the_record_above() {
    let base: Grid = &[&["Id", "Name"], &["1", "Alice"], &["2", "Bob"], &["3", "Carol"]];
    let other: Grid = &[&["Id", "Name"], &["1", "Alice"], &["2", "Bob"], &["3", "Caroline"]];
    let c = content(&[("S", base)], &[("S", other)]);
    let change = &sheet(&c, "S").changes[0];
    assert_eq!(change.labels.as_ref().unwrap().column.as_deref(), Some("Name"));
}

#[test]
fn years_typed_as_text_are_headers() {
    let rows = |amount: &str| {
        format!(
            r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Account</t></is></c><c r="B1" t="inlineStr"><is><t>2025</t></is></c></row>
               <row r="2"><c r="A2" t="inlineStr"><is><t>Cash</t></is></c><c r="B2"><v>{amount}</v></c></row>"#
        )
    };
    let (b, o) = (
        read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", &rows("1")).build()).unwrap(),
        read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", &rows("2")).build()).unwrap(),
    );
    let c = compare("a", &b, "b", &o, CompareOptions::default());
    assert_eq!(c.sheets[0].changes[0].labels.as_ref().unwrap().column.as_deref(), Some("2025"));
}

#[test]
fn many_changes_and_repeated_labels_stay_fast() {
    // Every value changes (labels are computed for each change), and labels
    // repeat every 12 rows (run search must stay linear).
    let n = 4000;
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let grid = |shift: u32, extra: bool| -> String {
        let mut rows = String::from(r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Month</t></is></c></row>"#);
        let mut r = 2;
        if extra {
            rows.push_str(&format!(r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>Opening</t></is></c></row>"#));
            r += 1;
        }
        for i in 0..n {
            let m = months[i as usize % 12];
            rows.push_str(&format!(
                r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>{m}</t></is></c><c r="B{r}"><v>{}</v></c></row>"#,
                i + shift
            ));
            r += 1;
        }
        rows
    };
    let b = read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", &grid(0, false)).build()).unwrap();
    let o = read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", &grid(1, true)).build()).unwrap();
    let start = std::time::Instant::now();
    let c = compare("a", &b, "b", &o, CompareOptions::default());
    assert!(start.elapsed().as_secs() < 10, "took {:?}", start.elapsed());
    assert_eq!(c.summary.rows_only_other, 1);
    assert_eq!(c.summary.value, n as usize);
}

/// Account rows under period columns headed by dates (serials, formatted).
fn period_book(periods: &[f64], rows: &[(&str, &[f64])]) -> Vec<u8> {
    use xldif_fixtures::rust_xlsxwriter::{Format, Workbook};
    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name("S").unwrap();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    sheet.write_string(0, 0, "Account").unwrap();
    for (c, p) in (1u16..).zip(periods) {
        sheet.write_number_with_format(0, c, *p, &date).unwrap();
    }
    for (r, (label, values)) in (1u32..).zip(rows) {
        sheet.write_string(r, 0, *label).unwrap();
        for (c, v) in (1u16..).zip(*values) {
            sheet.write_number(r, c, *v).unwrap();
        }
    }
    book.save_to_buffer().unwrap()
}

#[test]
fn date_headers_label_and_pair_period_columns() {
    // Periods roll forward: 2025 is added in front, 2023 drops off, and the
    // 2024 figures are restated.
    let (d23, d24, d25) = (45_291.0, 45_657.0, 46_022.0);
    let base = period_book(&[d24, d23], &[("Cash", &[100.0, 90.0]), ("Receivables", &[200.0, 180.0])]);
    let other = period_book(&[d25, d24], &[("Cash", &[120.0, 105.0]), ("Receivables", &[230.0, 200.0])]);
    let (b, o) = (read_bytes(base).unwrap(), read_bytes(other).unwrap());
    let c = compare("a", &b, "b", &o, CompareOptions::default());
    let s = &c.sheets[0];
    let columns: Vec<String> = s.changes.iter().filter(|c| c.kind == Kind::Column).map(describe).collect();
    assert_eq!(columns.len(), 2, "one period added, one dropped: {:?}", brief(s));
    let restated: Vec<&Change> = s.changes.iter().filter(|c| c.kind == Kind::Value).collect();
    assert_eq!(restated.len(), 1, "{:?}", brief(s));
    let labels = restated[0].labels.as_ref().unwrap();
    assert_eq!((labels.row.as_deref(), labels.column.as_deref()), (Some("Cash"), Some("2024-12-31")));
}
