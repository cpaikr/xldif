//! What the reader makes of workbooks Excel itself saved.
//!
//! `fixtures/excel/` holds one invented workbook that Excel built and saved
//! in the usual format and as Strict Open XML (`scripts/excel_fixture.py`).
//! Each assertion here is a file-format fact observed in those files, which
//! is what makes it verified (AGENTS.md).

use std::path::PathBuf;

use xldif::cellref::Range;
use xldif::diff::{CompareOptions, compare};
use xldif::model::{FormulaKind, Sheet, Value, Visibility, Workbook};
use xldif::numfmt::Formats;

fn book(name: &str) -> Workbook {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/excel").join(name);
    xldif::read_path(&path).unwrap()
}

fn ledger(book: &Workbook) -> &Sheet {
    book.sheet("Ledger").unwrap()
}

fn value(sheet: &Sheet, cell: &str) -> Value {
    let (row, col) = xldif::cellref::parse_cell(cell).unwrap();
    sheet.cells().find(|(r, c)| *r == row && c.col == col).map_or(Value::Empty, |(_, c)| c.value.clone())
}

fn text(sheet: &Sheet, cell: &str) -> String {
    match value(sheet, cell) {
        Value::Text(t) => t.to_string(),
        other => panic!("{cell} is {other:?}"),
    }
}

#[test]
fn values_and_escapes_read_as_excel_shows_them() {
    let wb = book("excel-saved.xlsx");
    let s = ledger(&wb);
    assert_eq!(value(s, "C2"), Value::Number(1200.5));
    assert_eq!(value(s, "E3"), Value::Bool(false));
    assert_eq!(value(s, "B9"), Value::Error("#DIV/0!".into()));
    // A line break typed in a cell is stored as `_x000D_` plus the break,
    // and typed text that looks like an escape as `_x005F_x000D_`.
    assert_eq!(text(s, "A7"), "line one\r\nline two");
    assert_eq!(text(s, "A8"), "_x000D_ as typed");
    // A number typed as text stays text.
    assert_eq!(text(s, "A9"), "0042");
    assert_eq!(wb.sheet("Workings").unwrap().visibility, Visibility::Hidden);
    let names: Vec<(&str, &str)> = wb.defined_names.iter().map(|n| (n.name.as_str(), n.refers_to.as_str())).collect();
    assert_eq!(names, [("Balances", "Ledger!$C$2:$C$4")]);
}

#[test]
fn shared_array_and_dynamic_array_formulas_are_read() {
    let wb = book("excel-saved.xlsx");
    let s = ledger(&wb);
    let formula = |cell: &str| {
        let (row, col) = xldif::cellref::parse_cell(cell).unwrap();
        let (_, c) = s.cells().find(|(r, c)| *r == row && c.col == col).unwrap();
        c.formula.clone().map(|f| (f.kind, f.text, f.range))
    };
    // Filling a formula down three rows: Excel writes the first as its own
    // formula and shares the rest from the second.
    assert_eq!(formula("F2"), Some((FormulaKind::Normal, "C2*D2".into(), None)));
    assert_eq!(formula("F3"), Some((FormulaKind::Shared, "C3*D3".into(), Some("F3:F4".into()))));
    assert_eq!(formula("F4").map(|f| (f.0, f.1)), Some((FormulaKind::Shared, "C4*D4".into())));
    assert_eq!(formula("G2"), Some((FormulaKind::Array, "C2:C4*2".into(), Some("G2:G4".into()))));
    // A dynamic array is an array formula over its spill range, marked with
    // cell metadata (`cm`); the function carries its future-function prefix.
    assert_eq!(formula("H2"), Some((FormulaKind::Array, "_xlfn._xlws.SORT(C2:C4)".into(), Some("H2:H4".into()))));
    assert_eq!((value(s, "H2"), value(s, "H4")), (Value::Number(-310.25), Value::Number(98000.0)));
    assert_eq!(formula("H3"), None);
}

#[test]
fn formats_layout_and_objects_are_read() {
    let wb = book("excel-saved.xlsx");
    let s = ledger(&wb);
    let formats = Formats::new(&wb.styles, wb.date1904);
    let shown = |cell: &str| {
        let (row, col) = xldif::cellref::parse_cell(cell).unwrap();
        let (_, c) = s.cells().find(|(r, c)| *r == row && c.col == col).unwrap();
        formats.shown(c.style, &c.value)
    };
    assert_eq!((shown("B2"), shown("C3"), shown("D2")), ("2024-01-01".into(), "-310.25".into(), "3.50%".into()));
    // A whole row formatted: `customFormat` with a style on the row.
    let row = |n: u32| s.layout.rows.iter().find(|r| r.index <= n && n <= r.last);
    assert!(row(5).is_some_and(|r| r.style.is_some()), "{:?}", s.layout.rows);
    assert!(row(8).is_some_and(|r| r.hidden));
    assert_eq!(row(1).and_then(|r| r.size.as_deref()), Some("24"));
    assert_eq!(s.merges, [Range::parse("A11:C11").unwrap()]);
    assert_eq!(s.hyperlinks.iter().map(|h| h.target.as_str()).collect::<Vec<_>>(), ["https://example.com/policy"]);
    assert_eq!(s.validations.len(), 1);
    // The rule's fill is a `dxf` whose colour is `bgColor`.
    assert_eq!(s.conditional_formats.len(), 1);
    assert!(s.conditional_formats[0].rule.contains("FFFFCEC7"), "{}", s.conditional_formats[0].rule);
    // A note, and a threaded comment: Excel also writes a note for the
    // thread that says older versions cannot show it, and only the thread
    // is kept.
    let comments: Vec<(u32, &str)> = s.comments.iter().map(|c| (c.row, c.text.as_str())).collect();
    assert_eq!(comments.len(), 2, "{comments:?}");
    assert!(comments[0] == (2, "Agreed to the bank letter"), "{comments:?}");
    assert!(comments[1].0 == 3 && comments[1].1.ends_with("Aged listing attached"), "{comments:?}");
    assert!(!comments[1].1.contains("[Threaded comment]"), "{comments:?}");
    // The table and the chart are named as not compared.
    let warning = wb.warnings.iter().find(|w| w.code == "not_compared").unwrap();
    assert!(warning.message.contains("tables (1)") && warning.message.contains("drawings"), "{}", warning.message);
}

#[test]
fn a_strict_save_reads_like_the_usual_one() {
    let (usual, strict) = (book("excel-saved.xlsx"), book("excel-strict.xlsx"));
    // A Strict save writes a date as ISO text (`t="d"`), which reads as the
    // serial the usual save stores.
    assert_eq!(value(ledger(&strict), "B2"), Value::Number(45292.0));
    let c = compare("usual", &usual, "strict", &strict, CompareOptions::default());
    assert_eq!(
        c.summary,
        xldif::diff::Summary::default(),
        "{:?}",
        c.sheets.iter().map(|s| &s.changes).collect::<Vec<_>>()
    );
    assert!(c.objects.is_empty());
}
