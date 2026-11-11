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

#[test]
fn rows_under_one_label_pair_by_their_values() {
    // A ledger whose every row is labelled KRW. Each row's second figure was
    // edited and one row was inserted in the middle. Pairing rows by label
    // in file order would shift every row after the insertion.
    let rows = |edit: u32, extra: bool| -> Vec<Vec<String>> {
        let mut out = vec![vec!["Currency".to_owned(), "Opening".to_owned(), "Closing".to_owned()]];
        for i in 0..30 {
            if extra && i == 10 {
                out.push(vec!["KRW".to_owned(), "777777".to_owned(), "888888".to_owned()]);
            }
            out.push(vec!["KRW".to_owned(), (1000 + 7 * i).to_string(), (5000 + 13 * i + edit).to_string()]);
        }
        out
    };
    let grid = |rows: &[Vec<String>]| -> Vec<u8> {
        let rows: Vec<Vec<&str>> = rows.iter().map(|r| r.iter().map(String::as_str).collect()).collect();
        let rows: Vec<&[&str]> = rows.iter().map(Vec::as_slice).collect();
        grid_book(&[("Ledger", &rows)])
    };
    let (b, o) = (read_bytes(grid(&rows(0, false))).unwrap(), read_bytes(grid(&rows(1, true))).unwrap());
    let c = compare("a", &b, "b", &o, CompareOptions::default());
    assert_eq!((c.summary.rows_only_other, c.summary.rows_only_base, c.summary.value), (1, 0, 30));
    assert!(brief(sheet(&c, "Ledger")).contains(&"row OnlyOther -→12".to_owned()));
}

#[test]
fn a_tolerance_reaches_the_alignment() {
    // Unlabelled figures, each off by float noise, with a row inserted.
    // Without the tolerance in the alignment no row matches another.
    let rows = |noise: f64, extra: bool| -> String {
        let mut out = String::new();
        let mut r = 1;
        for i in 0..40 {
            if extra && i == 15 {
                out.push_str(&format!(
                    r#"<row r="{r}"><c r="A{r}"><v>4242.5</v></c><c r="B{r}"><v>17.25</v></c></row>"#
                ));
                r += 1;
            }
            let (x, y) = (f64::from(i) * 100.5 + noise, f64::from(i) * 3.25 + 1000.0 + noise);
            out.push_str(&format!(r#"<row r="{r}"><c r="A{r}"><v>{x}</v></c><c r="B{r}"><v>{y}</v></c></row>"#));
            r += 1;
        }
        out
    };
    let book = |xml: &str| read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", xml).build()).unwrap();
    let (b, o) = (book(&rows(0.0, false)), book(&rows(1e-9, true)));
    let exact = compare("a", &b, "b", &o, CompareOptions::default());
    assert_eq!((exact.summary.rows_only_other, exact.summary.rows_only_base), (41, 40));
    let c = compare("a", &b, "b", &o, CompareOptions { tolerance: 1e-6, ..CompareOptions::default() });
    assert_eq!((c.summary.rows_only_other, c.summary.rows_only_base, c.summary.value), (1, 0, 0));
}

#[test]
fn a_tolerance_sized_for_amounts_leaves_rates_telling_rows_apart() {
    // Rows under one label, told apart only by two rates below 1, with a
    // row inserted. A tolerance of 0.5 must not make every rate one token.
    let rows = |extra: bool| -> Vec<Vec<String>> {
        let mut out = vec![vec!["Currency".to_owned(), "Spot".to_owned(), "Forward".to_owned()]];
        for i in 0..40u32 {
            if extra && i == 16 {
                out.push(vec!["KRW".to_owned(), "0.9731".to_owned(), "0.9642".to_owned()]);
            }
            out.push(vec!["KRW".to_owned(), format!("0.{:04}", 1000 + 137 * i), format!("0.{:04}", 2000 + 191 * i)]);
        }
        out
    };
    let book = |rows: &[Vec<String>]| {
        let rows: Vec<Vec<&str>> = rows.iter().map(|r| r.iter().map(String::as_str).collect()).collect();
        let rows: Vec<&[&str]> = rows.iter().map(Vec::as_slice).collect();
        read_bytes(grid_book(&[("Rates", &rows)])).unwrap()
    };
    let (b, o) = (book(&rows(false)), book(&rows(true)));
    let c = compare("a", &b, "b", &o, CompareOptions { tolerance: 0.5, ..CompareOptions::default() });
    assert_eq!((c.summary.rows_only_other, c.summary.rows_only_base, c.summary.value), (1, 0, 0));
    assert!(brief(sheet(&c, "Rates")).contains(&"row OnlyOther -→18".to_owned()));
}

#[test]
fn the_report_draws_the_rows_the_comparison_paired() {
    // Row 3 holds only an empty string: nothing to align on, so not a row
    // of the comparison. The report once worked its rows out again from the
    // workbooks and showed this one as removed and added.
    let xml = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Cash</t></is></c><c r="B1"><v>100</v></c></row>
                 <row r="2"><c r="A2" t="inlineStr"><is><t>Total</t></is></c><c r="B2"><v>100</v></c></row>
                 <row r="3"><c r="A3" t="str"><v></v></c></row>"#;
    let book = || read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", xml).build()).unwrap();
    let c = compare("a", &book(), "b", &book(), CompareOptions::default());
    assert_eq!(c.summary, xldif::diff::Summary::default());
    let axes = c.sheets[0].axes.clone().unwrap();
    assert_eq!((axes.rows.pairs, axes.rows.lone_base, axes.rows.lone_other), (vec![(1, 1), (2, 2)], vec![], vec![]));
    let listing = c.list(&Filter { all: true, ..Filter::default() });
    let html = xldif::html::render_html(&listing, &book(), &book(), &[]);
    assert!(html.contains("<tr class=\"r\"") && !html.contains("<tr class=\"r r") && !html.contains(" chg"));
}

#[test]
fn a_moved_row_that_was_also_resized_has_two_ids() {
    let rows = |order: &[u32], tall: bool| -> String {
        let mut out = String::new();
        for (r, n) in (1..).zip(order) {
            let height = if tall && *n == 1 { r#" ht="40" customHeight="1""# } else { "" };
            out.push_str(&format!(
                r#"<row r="{r}"{height}><c r="A{r}" t="inlineStr"><is><t>Line {n}</t></is></c><c r="B{r}"><v>{}</v></c></row>"#,
                n * 100
            ));
        }
        out
    };
    let book = |xml: &str| read_bytes(xldif_fixtures::raw::RawBook::new().sheet("S", xml).build()).unwrap();
    let (b, o) = (book(&rows(&[0, 1, 2, 3, 4], false)), book(&rows(&[0, 2, 3, 4, 1], true)));
    let c = compare("a", &b, "b", &o, CompareOptions::default());
    let ids: Vec<&str> = c.sheets[0].changes.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["s0:row:2:5", "s0:row:2:5:layout"]);
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

/// Invoices with a column of due dates (records, not headers), and
/// optionally an amount column inserted before them.
/// Entries without a date are `filler`: text, or a number when given.
fn invoice_book(due: &[Option<f64>], filler: Option<f64>, amounts: bool) -> Vec<u8> {
    use xldif_fixtures::rust_xlsxwriter::{Format, Workbook};
    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name("S").unwrap();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let due_col = if amounts { 2 } else { 1 };
    sheet.write_string(0, 0, "Invoice").unwrap();
    sheet.write_string(0, due_col, "Due").unwrap();
    if amounts {
        sheet.write_string(0, 1, "Amount").unwrap();
    }
    for (r, d) in (1u32..).zip(due) {
        sheet.write_string(r, 0, format!("INV-{r}")).unwrap();
        match d {
            Some(d) => sheet.write_number_with_format(r, due_col, *d, &date).unwrap(),
            None => match filler {
                Some(n) => sheet.write_number(r, due_col, n).unwrap(),
                None => sheet.write_string(r, due_col, "on request").unwrap(),
            },
        };
        if amounts {
            sheet.write_number(r, 1, f64::from(r) * 10.0).unwrap();
        }
    }
    book.save_to_buffer().unwrap()
}

#[test]
fn date_records_do_not_key_their_column() {
    // Every due date moves a year and an amount column is inserted: the
    // due column still pairs, so its cells compare as value changes. Also
    // when the dates sit among other entries ("on request"), and when the
    // column holds a single date among them.
    // A single date over numbers is shaped like a period column under a
    // caption; it pairs through the caption's label even when it keys the
    // column.
    // Which rows hold a date, and what the other rows hold.
    type Shape = (fn(u32) -> bool, Option<f64>);
    let shapes: [Shape; 4] = [(|_| true, None), (|k| k % 2 == 0, None), (|k| k == 0, None), (|k| k == 0, Some(42.0))];
    for (mixed, (shape, filler)) in shapes.into_iter().enumerate() {
        let base: Vec<Option<f64>> = (0..12).map(|k| shape(k).then_some(45_300.0 + f64::from(k))).collect();
        let other: Vec<Option<f64>> = base.iter().map(|d| d.map(|d| d + 366.0)).collect();
        let (b, o) = (
            read_bytes(invoice_book(&base, filler, false)).unwrap(),
            read_bytes(invoice_book(&other, filler, true)).unwrap(),
        );
        let c = compare("a", &b, "b", &o, CompareOptions::default());
        let s = &c.sheets[0];
        let columns: Vec<String> = s.changes.iter().filter(|c| c.kind == Kind::Column).map(describe).collect();
        assert_eq!(columns.len(), 1, "mixed {mixed}: only the amount column is new: {columns:?}");
        let values = s.changes.iter().filter(|c| c.kind == Kind::Value).count();
        assert_eq!(values, base.iter().flatten().count(), "mixed {mixed}: {:?}", brief(s));
        let first = s.changes.iter().find(|c| c.kind == Kind::Value).unwrap();
        assert_eq!(first.labels.as_ref().unwrap().column.as_deref(), Some("Due"));
    }
}

/// A list of deliveries: a date (style 1), a time of day (style 2), and a
/// count, in the 1900 or the 1904 date system.
fn delivery_book(date1904: bool, rows: &[(&str, f64, f64, f64)]) -> Vec<u8> {
    let main = xldif_fixtures::raw::MAIN_NS;
    let styles = format!(
        r#"<styleSheet xmlns="{main}"><numFmts count="1"><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="164" applyNumberFormat="1"/><xf numFmtId="20" applyNumberFormat="1"/></cellXfs></styleSheet>"#
    );
    let mut xml = String::from(
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Delivery</t></is></c><c r="B1" t="inlineStr"><is><t>Due</t></is></c><c r="C1" t="inlineStr"><is><t>At</t></is></c><c r="D1" t="inlineStr"><is><t>Boxes</t></is></c></row>"#,
    );
    for (r, (label, due, at, boxes)) in (2..).zip(rows) {
        xml.push_str(&format!(
            r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>{label}</t></is></c><c r="B{r}" s="1"><v>{due}</v></c><c r="C{r}" s="2"><v>{at}</v></c><c r="D{r}"><v>{boxes}</v></c></row>"#
        ));
    }
    let book = xldif_fixtures::raw::RawBook::new()
        .workbook_rel("styles", "styles.xml")
        .part("xl/styles.xml", styles.as_bytes());
    let book = if date1904 { book.workbook_before(r#"<workbookPr date1904="1"/>"#) } else { book };
    book.sheet("Deliveries", &xml).build()
}

#[test]
fn dates_in_different_date_systems_compare_by_the_date_they_show() {
    // 2024-01-01 is 45292 in the 1900 system and 43830 in the 1904 system.
    let base = delivery_book(
        true,
        &[
            ("North depot", 43_830.0, 0.25, 12.0),
            ("South depot", 43_831.0, 0.5, 43_830.0),
            ("East depot", 43_840.0, 0.5, 7.0),
            ("West depot", 43_841.3, 0.5, 7.0),
            ("Port", 43_842.0, 0.5, 7.0),
        ],
    );
    let other = delivery_book(
        false,
        &[
            ("North depot", 45_292.0, 0.25, 12.0),
            ("South depot", 45_294.0, 0.75, 45_292.0),
            ("East depot", 45_302.0, 0.5, 7.0),
            // The same moment, where adding the offset rounds; and a fifth
            // of a second later, which the date shown does not reveal.
            ("West depot", 45_303.3, 0.5, 7.0),
            ("Port", 45_304.0 + 0.2 / 86_400.0, 0.5, 7.0),
        ],
    );
    let (b, o) = (read_bytes(base).unwrap(), read_bytes(other).unwrap());
    let c = compare("base.xlsx", &b, "other.xlsx", &o, CompareOptions::default());
    assert!(c.warnings.iter().any(|w| w.code == "date_system_mismatch"), "{:?}", c.warnings);
    let s = &c.sheets[0];
    // The same dates are the same; the count that happens to equal a
    // serial is a plain number and still compares as one.
    assert_eq!(brief(s), ["value Changed B3→B3", "value Changed C3→C3", "value Changed D3→D3", "value Changed B6→B6"]);
    assert!(s.changes[3].delta.is_some_and(|d| (d * 86_400.0 - 0.2).abs() < 1e-3), "{:?}", s.changes[3].delta);
    let side = |c: &Change| (c.base.as_ref().unwrap().date.clone(), c.other.as_ref().unwrap().date.clone());
    let (due, at, boxes) = (&s.changes[0], &s.changes[1], &s.changes[2]);
    assert_eq!(side(due), (Some("2024-01-02".into()), Some("2024-01-03".into())));
    assert_eq!(due.delta, Some(1.0), "one day later, not the serials' difference");
    // A time of day is the same serial in both systems.
    assert_eq!(side(at), (Some("12:00:00".into()), Some("18:00:00".into())));
    assert_eq!(at.delta, Some(0.25));
    assert_eq!(boxes.delta, Some(1462.0));

    // Swapped, the same cells differ the other way.
    let back = compare("other.xlsx", &o, "base.xlsx", &b, CompareOptions::default());
    assert_eq!(brief(&back.sheets[0]), brief(s));
    assert_eq!(back.sheets[0].changes[0].delta, Some(-1.0));
    // In one date system the serials are compared as before.
    let same = compare("base.xlsx", &b, "base.xlsx", &b, CompareOptions::default());
    assert!(same.sheets[0].changes.is_empty() && same.warnings.iter().all(|w| w.code != "date_system_mismatch"));
}
