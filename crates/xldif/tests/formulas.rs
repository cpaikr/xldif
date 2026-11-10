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
    assert_eq!(f[0].base.as_ref().unwrap().range.as_deref(), Some("D2:D4"));
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
