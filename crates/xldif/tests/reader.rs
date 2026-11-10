//! The reader against synthetic workbooks: `rust_xlsxwriter` output and
//! hand-assembled parts in shapes Excel and other generators write.

use xldif::model::{BookFormat, CalcMode, FormulaKind, SheetKind, Value, Visibility, Workbook};
use xldif::{ErrorCode, read_bytes};
use xldif_fixtures::raw::{RawBook, zip_parts};

fn cell<'a>(book: &'a Workbook, sheet: &str, row: u32, col: u32) -> &'a xldif::model::Cell {
    let sheet = book.sheet(sheet).unwrap();
    let r = sheet.rows.iter().find(|r| r.index == row).unwrap_or_else(|| panic!("row {row}"));
    r.cells.iter().find(|c| c.col == col).unwrap_or_else(|| panic!("col {col}"))
}

#[test]
fn rust_xlsxwriter_fixture_reads_values_and_formulas() {
    let bytes = xldif_fixtures::basic().unwrap().save_to_buffer().unwrap();
    let book = read_bytes(bytes).unwrap();
    assert_eq!(book.format, BookFormat::Xlsx);
    assert_eq!(book.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["BS", "Notes"]);
    assert_eq!(cell(&book, "BS", 2, 1).value, Value::Text("Cash".into()));
    assert_eq!(cell(&book, "BS", 3, 2).value, Value::Number(200.0));
    let total = cell(&book, "BS", 4, 2);
    assert_eq!(total.value, Value::Number(300.0));
    let f = total.formula.as_ref().unwrap();
    assert_eq!((f.kind, f.text.as_str()), (FormulaKind::Normal, "SUM(B2:B3)"));
    assert_eq!(book.sheet("BS").unwrap().used_range().unwrap().name(), "A1:B4");
    // rust_xlsxwriter asks Excel to recalculate on open.
    assert!(book.calculation.full_calc_on_load);
}

#[test]
fn shared_strings_drop_phonetic_runs_and_decode_escapes() {
    let bytes = RawBook::new()
        .shared_strings(
            "<si><t>plain</t></si>\
             <si><r><t>rich </t></r><r><rPr><b/></rPr><t>text</t></r><rPh sb=\"0\" eb=\"1\"><t>ふりがな</t></rPh></si>\
             <si><t xml:space=\"preserve\">line_x000D_\nnext</t></si>",
        )
        .sheet(
            "S",
            r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c></row>"#,
        )
        .build();
    let book = read_bytes(bytes).unwrap();
    assert_eq!(cell(&book, "S", 1, 1).value, Value::Text("plain".into()));
    assert_eq!(cell(&book, "S", 1, 2).value, Value::Text("rich text".into()));
    assert_eq!(cell(&book, "S", 1, 3).value, Value::Text("line\r\nnext".into()));
}

#[test]
fn every_value_type_is_typed() {
    let rows = r#"<row r="1">
        <c r="A1"><v>1.5</v></c>
        <c r="B1" t="b"><v>1</v></c>
        <c r="C1" t="e"><v>#DIV/0!</v></c>
        <c r="D1" t="str"><f>"a"&amp;"b"</f><v>ab</v></c>
        <c r="E1" t="inlineStr"><is><t>inline</t></is></c>
        <c r="F1" s="3"/>
        <c r="G1"><f>1/0</f></c>
        <c r="H1" t="s"><v>99</v></c>
    </row>"#;
    let book = read_bytes(RawBook::new().sheet("S", rows).build()).unwrap();
    assert_eq!(cell(&book, "S", 1, 1).value, Value::Number(1.5));
    assert_eq!(cell(&book, "S", 1, 2).value, Value::Bool(true));
    assert_eq!(cell(&book, "S", 1, 3).value, Value::Error("#DIV/0!".into()));
    let d = cell(&book, "S", 1, 4);
    assert_eq!(d.value, Value::Text("ab".into()));
    assert_eq!(d.formula.as_ref().unwrap().text, r#""a"&"b""#);
    assert_eq!(cell(&book, "S", 1, 5).value, Value::Text("inline".into()));
    let styled = cell(&book, "S", 1, 6);
    assert_eq!((styled.style, styled.has_content()), (3, false));
    assert_eq!(cell(&book, "S", 1, 7).value, Value::Empty);
    // A shared-string index past the table is kept as raw text and reported.
    assert_eq!(cell(&book, "S", 1, 8).value, Value::Text("99".into()));
    assert!(book.warnings.iter().any(|w| w.code == "unreadable_value"));
}

#[test]
fn shared_and_array_formulas_keep_their_anchor_data() {
    let rows = r#"<row r="1"><c r="A1"><f t="shared" ref="A1:A3" si="0">B1*2</f><v>2</v></c></row>
        <row r="2"><c r="A2"><f t="shared" si="0"/><v>4</v></c></row>
        <row r="5"><c r="C5"><f t="array" ref="C5:C6">B1:B2*10</f><v>10</v></c></row>
        <row r="6"><c r="C6"><v>20</v></c></row>"#;
    let book = read_bytes(RawBook::new().sheet("S", rows).build()).unwrap();
    let anchor = cell(&book, "S", 1, 1).formula.clone().unwrap();
    assert_eq!(
        (anchor.kind, anchor.shared_index, anchor.range.as_deref()),
        (FormulaKind::Shared, Some(0), Some("A1:A3"))
    );
    let follower = cell(&book, "S", 2, 1).formula.clone().unwrap();
    assert_eq!((follower.kind, follower.text.as_str(), follower.shared_index), (FormulaKind::Shared, "", Some(0)));
    let array = cell(&book, "S", 5, 3).formula.clone().unwrap();
    assert_eq!((array.kind, array.range.as_deref()), (FormulaKind::Array, Some("C5:C6")));
    // Cells inside an array range hold values only.
    assert!(cell(&book, "S", 6, 3).formula.is_none());
}

#[test]
fn omitted_references_and_prefixes_are_inferred() {
    let part = format!(
        r#"<x:worksheet xmlns:x="{}"><x:sheetData><x:row><x:c><x:v>1</x:v></x:c><x:c><x:v>2</x:v></x:c></x:row><x:row r="4"><x:c r="C4"><x:v>3</x:v></x:c><x:c><x:v>4</x:v></x:c></x:row></x:sheetData></x:worksheet>"#,
        xldif_fixtures::raw::MAIN_NS
    );
    let book = read_bytes(RawBook::new().sheet_part("S", &part).build()).unwrap();
    assert_eq!(cell(&book, "S", 1, 1).value, Value::Number(1.0));
    assert_eq!(cell(&book, "S", 1, 2).value, Value::Number(2.0));
    assert_eq!(cell(&book, "S", 4, 3).value, Value::Number(3.0));
    assert_eq!(cell(&book, "S", 4, 4).value, Value::Number(4.0));
}

#[test]
fn out_of_order_rows_and_duplicate_cells_normalize() {
    let rows = r#"<row r="3"><c r="B3"><v>3</v></c><c r="A3"><v>1</v></c></row>
        <row r="1"><c r="A1"><v>0</v></c><c r="A1"><v>9</v></c></row>"#;
    let book = read_bytes(RawBook::new().sheet("S", rows).build()).unwrap();
    let sheet = book.sheet("S").unwrap();
    assert_eq!(sheet.rows.iter().map(|r| r.index).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(sheet.rows[1].cells.iter().map(|c| c.col).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(cell(&book, "S", 1, 1).value, Value::Number(9.0));
}

#[test]
fn workbook_settings_names_and_visibility_are_read() {
    let bytes = RawBook::new()
        .workbook_before(r#"<workbookPr date1904="1"/>"#)
        .sheet("Visible", "")
        .sheet_with_state("Hidden", Some("hidden"), "")
        .sheet_with_state("Secret", Some("veryHidden"), "")
        .workbook_after(
            r#"<definedNames><definedName name="Rate">Visible!$B$2</definedName><definedName name="Local" localSheetId="1" hidden="1">Hidden!$A$1:$A$3</definedName></definedNames><calcPr calcMode="manual"/>"#,
        )
        .macro_enabled()
        .build();
    let book = read_bytes(bytes).unwrap();
    assert!(book.date1904);
    assert_eq!(book.format, BookFormat::Xlsm);
    assert_eq!(book.calculation.mode, CalcMode::Manual);
    assert!(!book.calculation.full_calc_on_load);
    let vis: Vec<Visibility> = book.sheets.iter().map(|s| s.visibility).collect();
    assert_eq!(vis, [Visibility::Visible, Visibility::Hidden, Visibility::VeryHidden]);
    assert!(book.sheets.iter().all(|s| s.kind == SheetKind::Worksheet));
    assert_eq!(book.defined_names[0].scope, None);
    assert_eq!(book.defined_names[0].refers_to, "Visible!$B$2");
    assert_eq!(book.defined_names[1].scope.as_deref(), Some("Hidden"));
    assert!(book.defined_names[1].hidden);
}

#[test]
fn binary_workbooks_are_unsupported() {
    let types = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/></Types>"#;
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.bin"/></Relationships>"#;
    let bytes = zip_parts(&[
        ("[Content_Types].xml".into(), types.as_bytes().to_vec()),
        ("_rels/.rels".into(), rels.as_bytes().to_vec()),
        ("xl/workbook.bin".into(), vec![0, 1, 2]),
    ]);
    assert_eq!(read_bytes(bytes).unwrap_err().code, ErrorCode::UnsupportedFormat);
}

#[test]
fn malformed_parts_fail_with_stable_codes() {
    let not_xml = RawBook::new().sheet_part("S", "<worksheet><sheetData><row></worksheet>").build();
    assert_eq!(read_bytes(not_xml).unwrap_err().code, ErrorCode::InvalidXml);
    let bad_ref = RawBook::new().sheet("S", r#"<row r="1"><c r="1A"><v>1</v></c></row>"#).build();
    assert_eq!(read_bytes(bad_ref).unwrap_err().code, ErrorCode::InvalidXml);
    let no_workbook = zip_parts(&[("hello.txt".into(), b"hi".to_vec())]);
    assert_eq!(read_bytes(no_workbook).unwrap_err().code, ErrorCode::InvalidXml);
}
