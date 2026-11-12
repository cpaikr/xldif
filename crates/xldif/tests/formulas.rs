//! Formula comparison through the alignment: Excel's own adjustments for
//! inserted and deleted rows are not changes; real rewrites are, with the
//! formula Excel would have produced as `expected`.

use xldif::diff::{Change, CompareOptions, Comparison, Kind, Status, compare};
use xldif::read_bytes;
use xldif_fixtures::grid_book;

type Grid<'a> = &'a [&'a [&'a str]];

fn run(base: &[(&str, Grid)], other: &[(&str, Grid)]) -> Comparison {
    let (b, o) = (read_bytes(grid_book(base)).unwrap(), read_bytes(grid_book(other)).unwrap());
    compare("base.xlsx", &b, "other.xlsx", &o, CompareOptions::default())
}

fn formulas(c: &Comparison) -> Vec<&Change> {
    c.sheets.iter().flat_map(|s| &s.changes).filter(|c| c.kind == Kind::Formula).collect()
}

const BS: Grid = &[
    &["Account", "2025"],
    &["Cash", "100"],
    &["Receivables", "200"],
    &["Total", "=SUM(B2:B3)|300"],
    &["Double", "=B4*2|600"],
];

#[test]
fn an_inserted_row_adjusts_formulas_without_changing_them() {
    let other: Grid = &[
        &["Account", "2025"],
        &["Cash", "100"],
        &["Prepaid", "50"],
        &["Receivables", "200"],
        &["Total", "=SUM(B2:B4)|350"],
        &["Double", "=B5*2|700"],
    ];
    let c = run(&[("BS", BS)], &[("BS", other)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
    assert_eq!(c.summary.formulas_adjusted, 2);
}

#[test]
fn a_rewritten_formula_is_a_change() {
    let other: Grid = &[BS[0], BS[1], BS[2], &["Total", "=SUM(B2:B2)|100"], &["Double", "=B4*2|200"]];
    let c = run(&[("BS", BS)], &[("BS", other)]);
    let f = formulas(&c);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].id, "s0:formula:B4:B4");
    assert_eq!(f[0].base.as_ref().unwrap().formula.as_deref(), Some("SUM(B2:B3)"));
    assert_eq!(f[0].other.as_ref().unwrap().formula.as_deref(), Some("SUM(B2:B2)"));
    assert_eq!(f[0].expected, None, "nothing moved, so the base formula is what Excel would keep");
    assert_eq!(f[0].labels.as_ref().unwrap().row.as_deref(), Some("Total"));
}

#[test]
fn a_rewrite_after_an_insertion_shows_the_expected_formula() {
    let other: Grid = &[
        &["Account", "2025"],
        &["Prepaid", "50"],
        &["Cash", "100"],
        &["Receivables", "200"],
        &["Total", "=SUM(B3:B3)|100"],
        &["Double", "=B5*2|200"],
    ];
    let c = run(&[("BS", BS)], &[("BS", other)]);
    let f = formulas(&c);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].expected.as_deref(), Some("SUM(B3:B4)"));
}

#[test]
fn a_formula_replaced_by_a_value_is_reported() {
    let other: Grid = &[BS[0], BS[1], BS[2], &["Total", "300"], BS[4]];
    let c = run(&[("BS", BS)], &[("BS", other)]);
    let f = formulas(&c);
    assert_eq!((f.len(), f[0].status), (1, Status::OnlyBase));
    // The value is unchanged, so only the formula is listed.
    assert_eq!(c.summary.value, 0);
}

#[test]
fn the_same_change_filled_down_folds_into_one() {
    let base: Grid = &[
        &["Item", "Qty", "Price", "Amount"],
        &["a", "1", "10", "=B2*C2|10"],
        &["b", "2", "10", "=B3*C3|20"],
        &["c", "3", "10", "=B4*C4|30"],
        &["d", "4", "10", "=B5*C5|40"],
    ];
    let other: Grid = &[
        &["Item", "Qty", "Price", "Amount"],
        &["a", "1", "10", "=B2*C2*1.1|11"],
        &["b", "2", "10", "=B3*C3*1.1|22"],
        &["c", "3", "10", "=B4*C4*1.1|33"],
        &["d", "4", "10", "=B5*C5|40"],
    ];
    let c = run(&[("S", base)], &[("S", other)]);
    let f = formulas(&c);
    // D2:D4 changed the same way; D5 kept its formula.
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].id, "s0:formula:D2:D4:D2:D4");
    assert_eq!(f[0].base.as_ref().unwrap().range().as_deref(), Some("D2:D4"));
    assert_eq!(c.summary.formula, 1);
}

#[test]
fn references_to_other_sheets_follow_their_alignment_and_renames() {
    let notes: Grid = &[&["Total assets", "=BS!B4|300"]];
    let bs_inserted: Grid = &[
        &["Account", "2025"],
        &["Cash", "100"],
        &["Prepaid", "50"],
        &["Receivables", "200"],
        &["Total", "=SUM(B2:B4)|350"],
        &["Double", "=B5*2|700"],
    ];
    let moved_notes: Grid = &[&["Total assets", "=BS!B5|350"]];
    let c = run(&[("BS", BS), ("Notes", notes)], &[("BS", bs_inserted), ("Notes", moved_notes)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
    assert_eq!(c.summary.formulas_adjusted, 3);

    // A renamed sheet: references to it are renamed too, as Excel does.
    let renamed_ref: Grid = &[&["Total assets", "='Balance sheet'!B4|300"]];
    let c = run(&[("BS", BS), ("Notes", notes)], &[("Balance sheet", BS), ("Notes", renamed_ref)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
    assert_eq!((c.summary.sheets_renamed, c.summary.formulas_adjusted), (1, 1));
}

fn as_grid(v: &[Vec<String>]) -> Vec<Vec<&str>> {
    v.iter().map(|r| r.iter().map(String::as_str).collect()).collect()
}

#[test]
fn unlabelled_formula_rows_align_after_an_insertion() {
    let rows = |offset: usize| -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = Vec::new();
        if offset == 1 {
            out.push(vec!["999".into(), "=A1*2|1998".into()]);
        }
        for i in 1..=30 {
            let r = i + offset;
            out.push(vec![format!("{}", i * 10), format!("=A{r}*2|{}", i * 20)]);
        }
        out
    };
    let (b, o) = (rows(0), rows(1));
    let (gb, go) = (as_grid(&b), as_grid(&o));
    let rb: Vec<&[&str]> = gb.iter().map(Vec::as_slice).collect();
    let ro: Vec<&[&str]> = go.iter().map(Vec::as_slice).collect();
    let c = run(&[("S", &rb)], &[("S", &ro)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
    assert_eq!((c.summary.rows_only_other, c.summary.value, c.summary.formulas_adjusted), (1, 0, 30));
}

#[test]
fn ref_errors_from_deleted_rows_are_adjustments() {
    let base: Grid = &[&["A", "1"], &["B", "2"], &["C", "3", "=B3*2|6"], &["D", "4", "=B3+1|4"]];
    // Row 3 (C) deleted: Excel turns references to it into #REF!.
    let other: Grid = &[&["A", "1"], &["B", "2"], &["D", "4", "=#REF!+1|0"]];
    let notes: Grid = &[&["x", "=BS!B3|3"]];
    let notes_after: Grid = &[&["x", "=BS!#REF!|0"]];
    let c = run(&[("BS", base), ("Notes", notes)], &[("BS", other), ("Notes", notes_after)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
    assert_eq!(c.summary.formulas_adjusted, 2);
}

#[test]
fn blank_spacer_rows_next_to_an_insertion_map_with_it() {
    let base: Grid = &[&["Item", "Amount"], &["a", "1"], &["b", "2"], &["c", "3"], &[], &["Total", "=SUM(B2:B5)|6"]];
    // A line item added above the spacer row.
    let inserted: Grid =
        &[&["Item", "Amount"], &["a", "1"], &["b", "2"], &["c", "3"], &["d", "4"], &[], &["Total", "=SUM(B2:B6)|10"]];
    let c = run(&[("S", base)], &[("S", inserted)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
    // A line item deleted above the spacer row.
    let deleted: Grid = &[&["Item", "Amount"], &["a", "1"], &["b", "2"], &[], &["Total", "=SUM(B2:B4)|3"]];
    let c = run(&[("S", base)], &[("S", deleted)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
}

#[test]
fn a_total_left_unchanged_over_an_inserted_row_is_a_change() {
    let other: Grid =
        &[BS[0], BS[1], &["Prepaid", "50"], BS[2], &["Total", "=SUM(B2:B3)|150"], &["Double", "=B5*2|300"]];
    let c = run(&[("BS", BS)], &[("BS", other)]);
    let f = formulas(&c);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].expected.as_deref(), Some("SUM(B2:B4)"));
}

#[test]
fn folded_ranges_cover_only_changed_cells() {
    // The same edit on both sides of an inserted row: a folded range would
    // cover the inserted row too, so the edits stay separate, each with the
    // same adjusted formula.
    let base: Grid = &[&["k", "v", "r"], &["a", "1", "=B2*2|2"], &["b", "2", "=B3*2|4"]];
    let other: Grid = &[&["k", "v", "r"], &["a", "1", "=B2*3|3"], &["new", "9"], &["b", "2", "=B4*3|6"]];
    let c = run(&[("S", base)], &[("S", other)]);
    let f = formulas(&c);
    assert_eq!(f.len(), 2, "{f:?}");
    assert!(f.iter().all(|c| c.base.as_ref().unwrap().range().is_none()));
}

#[test]
fn unchanged_text_that_now_reads_a_moved_cell_is_a_change() {
    let base: Grid = &[&["a", "1"], &["b", "2"], &["ref", "=B2|2"]];
    // A row inserted above "b": B2's content moved to B3, but the formula still reads B2.
    let other: Grid = &[&["a", "1"], &["new", "5"], &["b", "2"], &["ref", "=B2|5"]];
    let c = run(&[("S", base)], &[("S", other)]);
    let f = formulas(&c);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].expected.as_deref(), Some("B3"));
}

const CHECKS: Grid =
    &[&["Item", "Amount"], &["Revenue", "100"], &["Cost", "100"], &["Tax", "5"], &["Check", "=B2|100"]];

#[test]
fn unchanged_text_over_a_deleted_row_reads_the_row_that_took_its_place() {
    // Revenue was deleted and Cost moved up into row 2: `=B2` reads Cost now,
    // and its cached value hides that.
    let other: Grid = &[CHECKS[0], CHECKS[2], CHECKS[3], &["Check", "=B2|100"]];
    let c = run(&[("PL", CHECKS)], &[("PL", other)]);
    let f = formulas(&c);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].id, "s0:formula:B5:B4");
    assert_eq!(f[0].expected.as_deref(), Some("#REF!"));
    assert_eq!(c.summary.value, 0);
}

#[test]
fn unchanged_text_over_a_row_replaced_in_place_is_not_a_change() {
    // Row 2 holds an unrelated line on each side: nothing else moved, so the
    // formula still reads row 2 and was not touched.
    let other: Grid = &[CHECKS[0], &["Grants", "7"], CHECKS[2], CHECKS[3], &["Check", "=B2|7"]];
    let c = run(&[("PL", CHECKS)], &[("PL", other)]);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
}

#[test]
fn a_malformed_formula_compares_as_written() {
    let base: Grid = &[&["x", "1"], &["y", "=A1+1|2"]];
    for broken in ["=A1+[|2", "=A1+[é|2", "='|2", "=\"|2", "=A1+#|2"] {
        let other: Grid = &[&["x", "1"], &["y", broken]];
        let c = run(&[("S", base)], &[("S", other)]);
        assert_eq!(formulas(&c).len(), 1, "{broken}");
        let same = run(&[("S", other)], &[("S", other)]);
        assert!(formulas(&same).is_empty(), "{broken}");
    }
}

#[test]
fn unchanged_text_over_a_removed_blank_row_is_a_change() {
    // The spacer in row 5 was removed and D moved up into it: `=B5` read
    // nothing and now reads D; the total now covers D as well.
    let base: Grid = &[
        &["Item", "Amount", "", "=B5|0"],
        &["A", "1"],
        &["B", "2"],
        &["C", "3", "", "=SUM(B2:B5)|6"],
        &[],
        &["D", "4"],
    ];
    let other: Grid =
        &[&["Item", "Amount", "", "=B5|4"], &["A", "1"], &["B", "2"], &["C", "3", "", "=SUM(B2:B5)|10"], &["D", "4"]];
    let c = run(&[("S", base)], &[("S", other)]);
    let f = formulas(&c);
    assert_eq!(f.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["s0:formula:D1:D1", "s0:formula:D4:D4"]);
    // The removed row is a deleted row to the formulas over it, as to Excel.
    assert_eq!(f[0].expected.as_deref(), Some("#REF!"));
    assert_eq!(f[1].expected.as_deref(), Some("SUM(B2:B4)"));
}

#[test]
fn unchanged_text_reading_another_sheet_follows_that_sheets_deleted_row() {
    let notes: Grid = &[&["Revenue per PL", "=PL!B2|100"]];
    let other: Grid = &[CHECKS[0], CHECKS[2], CHECKS[3], &["Check", "=B3|5"]];
    let c = run(&[("PL", CHECKS), ("Notes", notes)], &[("PL", other), ("Notes", notes)]);
    let f: Vec<&Change> = formulas(&c).into_iter().filter(|c| c.id.starts_with("s1:")).collect();
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].expected.as_deref(), Some("PL!#REF!"));
}

#[test]
fn unchanged_range_over_rows_reordered_inside_it_is_not_a_change() {
    // A moved from row 2 to row 4. The total still covers the same four
    // rows; the single reference to B2 now reads B's amount.
    let base: Grid = &[
        &["Item", "Amount", "", "=SUM(B2:B5)|10", "=B2|1"],
        &["Alpha", "1"],
        &["Bravo", "2"],
        &["Charlie", "3"],
        &["Delta", "4"],
    ];
    let other: Grid = &[
        &["Item", "Amount", "", "=SUM(B2:B5)|10", "=B2|2"],
        &["Bravo", "2"],
        &["Charlie", "3"],
        &["Alpha", "1"],
        &["Delta", "4"],
    ];
    let c = run(&[("S", base)], &[("S", other)]);
    let f = formulas(&c);
    assert_eq!(f.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["s0:formula:E1:E1"], "{f:?}");
}

/// A sheet of labelled formulas (`label`, `formula`, cached result) in a
/// workbook that links to `links`, in that order, with one defined name.
fn linked_book(links: &[&str], rows: &[(&str, &str, f64)], rate: &str) -> xldif::model::Workbook {
    use std::fmt::Write as _;
    let mut xml = String::new();
    for (r, (label, formula, result)) in (1..).zip(rows) {
        let _ = write!(
            xml,
            r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>{label}</t></is></c><c r="B{r}"><f>{formula}</f><v>{result}</v></c></row>"#
        );
    }
    let mut book = xldif_fixtures::raw::RawBook::new()
        .sheet("Summary", &xml)
        .workbook_after(&format!(r#"<definedNames><definedName name="Rate">{rate}</definedName></definedNames>"#));
    for target in links {
        book = book.external_link(target);
    }
    read_bytes(book.build()).unwrap()
}

fn compare_books(base: &xldif::model::Workbook, other: &xldif::model::Workbook) -> Comparison {
    compare("base.xlsx", base, "other.xlsx", other, CompareOptions::default())
}

/// `id status base→other` for each workbook-level change.
fn objects(c: &Comparison) -> Vec<String> {
    let text = |l: &Option<xldif::diff::Location>| l.as_ref().and_then(|l| l.text.clone()).unwrap_or_default();
    c.objects.iter().map(|o| format!("{} {:?} {}→{}", o.id, o.status, text(&o.base), text(&o.other))).collect()
}

#[test]
fn a_renumbered_external_link_is_not_a_formula_change() {
    // The other workbook gained a link to Budget.xlsx as its first, so
    // Prices.xlsx is [2] there and Rates.xlsx is [3].
    let base = linked_book(
        &["Prices.xlsx", "Rates.xlsx"],
        &[
            ("Widgets", "[1]Sheet1!A1*2", 20.0),
            ("Rate", "'[2]Year end'!$B$2+[2]!Margin", 3.0),
            ("Total", "SUM(B1:B2)", 23.0),
        ],
        "[2]Sheet1!$A$1",
    );
    let other = linked_book(
        &["Budget.xlsx", "prices.xlsx", "Rates.xlsx"],
        &[
            ("Widgets", "[2]Sheet1!A1*2", 20.0),
            ("Rate", "'[3]Year end'!$B$2+[3]!Margin", 3.0),
            ("Total", "SUM(B1:B2)", 23.0),
        ],
        "[3]Sheet1!$A$1",
    );
    for (c, added) in [
        (compare_books(&base, &other), "externalLink:-:1 OnlyOther →Budget.xlsx"),
        (compare_books(&other, &base), "externalLink:1:- OnlyBase Budget.xlsx→"),
    ] {
        assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
        assert_eq!(c.summary.formulas_adjusted, 0);
        // The defined name follows its link too; only the new link is listed.
        assert_eq!(objects(&c), [added]);
        assert_eq!(c.objects[0].kind, Kind::ExternalLink);
        assert_eq!(c.summary.objects, 1);
    }
}

#[test]
fn a_link_pointed_at_another_file_is_one_change() {
    // The same formula text on both sides, reading another file: the link
    // is the change, not each formula through it.
    let rows: &[(&str, &str, f64)] = &[("Opening", "[1]Sheet1!A1", 10.0), ("Closing", "[1]Sheet1!A2+1", 12.0)];
    let base = linked_book(&["FY2024.xlsx"], rows, "Summary!$B$1");
    let other = linked_book(&["FY2025.xlsx"], rows, "Summary!$B$1");
    let c = compare_books(&base, &other);
    assert_eq!(objects(&c), ["externalLink:1:1 Changed FY2024.xlsx→FY2025.xlsx"]);
    assert_eq!(c.objects[0].properties[0].name, "target");
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
    // A link removed outright leaves its formulas reading a file the other
    // workbook has no number for: those are listed, with the file named.
    let removed = linked_book(&[], &[("Opening", "A5", 10.0), ("Closing", "A6+1", 12.0)], "Summary!$B$1");
    let c = compare_books(&base, &removed);
    assert_eq!(objects(&c), ["externalLink:1:- OnlyBase FY2024.xlsx→"]);
    let f = formulas(&c);
    assert_eq!(f.len(), 2, "{f:?}");
    assert_eq!(f[0].expected.as_deref(), Some("[FY2024.xlsx]Sheet1!A1"));
    // The same workbook against itself still has nothing to report.
    let same = compare_books(&base, &base);
    assert!(formulas(&same).is_empty() && same.objects.is_empty());
}

#[test]
fn a_rewrite_through_a_renumbered_link_shows_the_expected_formula() {
    let base = linked_book(&["Prices.xlsx", "Rates.xlsx"], &[("Widgets", "[1]Sheet1!A1+1", 11.0)], "[1]Sheet1!$A$1");
    let other = linked_book(&["Rates.xlsx", "Prices.xlsx"], &[("Widgets", "[2]Sheet1!A1+2", 12.0)], "[1]Sheet1!$A$1");
    let c = compare_books(&base, &other);
    let f = formulas(&c);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].base.as_ref().unwrap().formula.as_deref(), Some("[1]Sheet1!A1+1"));
    assert_eq!(f[0].expected.as_deref(), Some("[2]Sheet1!A1+1"));
    // Both files are linked on both sides, so no link is listed; the name
    // kept its text and so now reads the other file.
    assert_eq!(objects(&c), ["name:Rate Changed [1]Sheet1!$A$1→[1]Sheet1!$A$1"]);
    assert_eq!(c.objects[0].expected.as_deref(), Some("[2]Sheet1!$A$1"));
}

#[test]
fn a_validation_list_follows_a_renumbered_link() {
    // A list validation whose source is in another workbook. The other
    // workbook links to one more file first, so the same source is `[2]`.
    let book = |links: &[&str], source: &str| {
        let main = xldif_fixtures::raw::MAIN_NS;
        let xml = format!(
            r#"<worksheet xmlns="{main}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><dataValidations count="1"><dataValidation type="list" sqref="A1"><formula1>{source}</formula1></dataValidation></dataValidations></worksheet>"#
        );
        let mut book = xldif_fixtures::raw::RawBook::new().sheet_part("Entry", &xml);
        for target in links {
            book = book.external_link(target);
        }
        read_bytes(book.build()).unwrap()
    };
    let validations = |c: &Comparison| c.sheets[0].changes.iter().filter(|c| c.kind == Kind::Validation).count();
    let base = book(&["Lists.xlsx"], "[1]Codes!$A$1:$A$9");
    let renumbered = book(&["Budget.xlsx", "Lists.xlsx"], "[2]Codes!$A$1:$A$9");
    assert_eq!(validations(&compare_books(&base, &renumbered)), 0);
    // The same number for the new first link reads another file.
    let another = book(&["Budget.xlsx", "Lists.xlsx"], "[1]Codes!$A$1:$A$9");
    assert_eq!(validations(&compare_books(&base, &another)), 1);
}

#[test]
fn rows_of_linked_formulas_pair_through_a_renumbered_link() {
    // Rows with no labels, each a formula into another workbook, and a row
    // inserted. The link is `[1]` in one workbook and `[2]` in the other, so
    // a row's formula is the same token on both sides only when read in one
    // numbering; otherwise each row shares its value alone, too little to
    // pair.
    let book = |links: &[&str], link: u32, bump: u32, extra: bool| {
        use std::fmt::Write as _;
        let mut xml = String::new();
        let mut r = 1;
        for i in 0..12u32 {
            if extra && i == 5 {
                let _ = write!(xml, r#"<row r="{r}"><c r="A{r}"><v>999</v></c></row>"#);
                r += 1;
            }
            let _ = write!(
                xml,
                r#"<row r="{r}"><c r="A{r}"><f>[{link}]Data!$A${}</f><v>{}</v></c></row>"#,
                i + 1,
                i * 10 + bump
            );
            r += 1;
        }
        let mut book = xldif_fixtures::raw::RawBook::new().sheet("Pull", &xml);
        for target in links {
            book = book.external_link(target);
        }
        read_bytes(book.build()).unwrap()
    };
    let (base, other) = (book(&["Data.xlsx"], 1, 0, false), book(&["Budget.xlsx", "Data.xlsx"], 2, 0, true));
    let c = compare_books(&base, &other);
    assert_eq!((c.summary.rows_only_other, c.summary.rows_only_base, c.summary.value), (1, 0, 0), "{:?}", c.summary);
    assert!(formulas(&c).is_empty(), "{:?}", formulas(&c));
}
