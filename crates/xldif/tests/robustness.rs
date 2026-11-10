//! Hostile and unusual input: every bound fails with `invalid_archive`, and
//! shapes outside a normal workbook fail or warn instead of reading wrong.
#![allow(clippy::format_collect)]

use std::sync::Arc;

use xldif::model::{SheetKind, Value};
use xldif::{ErrorCode, Limits, read_bytes, read_bytes_with};
use xldif_fixtures::raw::{RawBook, zip_parts};

fn cells_xml(n: usize) -> String {
    (1..=n).map(|r| format!(r#"<row r="{r}"><c r="A{r}"><v>{r}</v></c></row>"#)).collect()
}

fn code(result: Result<xldif::model::Workbook, xldif::Error>) -> ErrorCode {
    result.expect_err("should fail").code
}

#[test]
fn record_budget_counts_cells_rows_and_strings() {
    let book = RawBook::new().sheet("S", &cells_xml(10)).build();
    let tight = Limits { max_records: 15, ..Limits::default() };
    // 1 sheet + 10 rows + 10 cells = 21 records.
    assert_eq!(code(read_bytes_with(book.clone(), &tight)), ErrorCode::InvalidArchive);
    assert!(read_bytes_with(book, &Limits { max_records: 21, ..Limits::default() }).is_ok());

    let empty_rows: String = (1..=50).map(|r| format!(r#"<row r="{r}"/>"#)).collect();
    let rows_only = RawBook::new().sheet("S", &empty_rows).build();
    assert_eq!(code(read_bytes_with(rows_only, &tight)), ErrorCode::InvalidArchive);

    let strings = RawBook::new().shared_strings(&"<si/>".repeat(50)).sheet("S", "").build();
    assert_eq!(code(read_bytes_with(strings, &tight)), ErrorCode::InvalidArchive);
}

#[test]
fn size_bounds_are_enforced() {
    let book = RawBook::new().sheet("S", &cells_xml(200)).build();
    let len = book.len() as u64;
    let file = Limits { max_file_bytes: len - 1, ..Limits::default() };
    assert_eq!(code(read_bytes_with(book.clone(), &file)), ErrorCode::InvalidArchive);
    let member = Limits { max_member_bytes: 2_000, ..Limits::default() };
    assert_eq!(code(read_bytes_with(book.clone(), &member)), ErrorCode::InvalidArchive);
    let total = Limits { max_total_bytes: 3_000, ..Limits::default() };
    assert_eq!(code(read_bytes_with(book.clone(), &total)), ErrorCode::InvalidArchive);
    let members = Limits { max_members: 3, ..Limits::default() };
    assert_eq!(code(read_bytes_with(book, &members)), ErrorCode::InvalidArchive);
}

#[test]
fn repeated_shared_strings_are_not_copied() {
    let rows: String = (1..=100).map(|r| format!(r#"<row r="{r}"><c r="A{r}" t="s"><v>0</v></c></row>"#)).collect();
    let book =
        read_bytes(RawBook::new().shared_strings("<si><t>a long string</t></si>").sheet("S", &rows).build()).unwrap();
    let texts: Vec<&Arc<str>> = book.sheets[0]
        .cells()
        .map(|(_, c)| match &c.value {
            Value::Text(t) => t,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(texts.windows(2).all(|w| Arc::ptr_eq(w[0], w[1])));
}

#[test]
fn indented_string_parts_drop_layout_whitespace() {
    let sst = "\n  <si>\n    <t>abc</t>\n  </si>\n  <si>\n    <r><t>rich </t></r>\n    <r><t>text</t></r>\n  </si>\n";
    let rows = r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is>
        <t xml:space="preserve"> kept </t>
    </is></c></row>"#;
    let book = read_bytes(RawBook::new().shared_strings(sst).sheet("S", rows).build()).unwrap();
    let values: Vec<Value> = book.sheets[0].cells().map(|(_, c)| c.value.clone()).collect();
    assert_eq!(values, [Value::Text("abc".into()), Value::Text("rich text".into()), Value::Text(" kept ".into())]);
}

#[test]
fn office_documents_that_are_not_workbooks_are_unsupported() {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let docx = zip_parts(&[
        ("_rels/.rels".into(), rels.as_bytes().to_vec()),
        ("word/document.xml".into(), br#"<w:document xmlns:w="urn:w"><w:body/></w:document>"#.to_vec()),
    ]);
    assert_eq!(code(read_bytes(docx)), ErrorCode::UnsupportedFormat);
}

#[test]
fn a_name_scoped_to_a_missing_sheet_is_dropped_with_a_warning() {
    let book = RawBook::new()
        .sheet("S", "")
        .workbook_after(r#"<definedNames><definedName name="Rate">S!$A$1</definedName><definedName name="Rate" localSheetId="7">S!$B$1</definedName></definedNames>"#)
        .build();
    let book = read_bytes(book).unwrap();
    assert_eq!(book.defined_names.len(), 1);
    assert_eq!(book.defined_names[0].refers_to, "S!$A$1");
    assert!(book.warnings.iter().any(|w| w.code == "defined_name_scope_invalid"));
}

#[test]
fn rows_past_the_grid_are_rejected() {
    let rows = r#"<row r="1048576"><c><v>1</v></c></row><row><c><v>2</v></c></row>"#;
    assert_eq!(code(read_bytes(RawBook::new().sheet("S", rows).build())), ErrorCode::InvalidXml);
}

#[test]
fn a_cell_naming_another_row_moves_to_that_row() {
    let rows =
        r#"<row r="1"><c r="A1"><v>1</v></c><c r="B3"><v>3</v></c></row><row r="3"><c r="A3"><v>2</v></c></row>"#;
    let book = read_bytes(RawBook::new().sheet("S", rows).build()).unwrap();
    let sheet = &book.sheets[0];
    assert_eq!(sheet.rows.iter().map(|r| r.index).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(sheet.rows[1].cells.iter().map(|c| c.col).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn macro_sheets_are_read() {
    let ns = xldif_fixtures::raw::MAIN_NS;
    let part = format!(
        r#"<xm:macrosheet xmlns="{ns}" xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><sheetData><row r="1"><c r="A1"><f>RETURN()</f></c></row></sheetData></xm:macrosheet>"#
    );
    let book = read_bytes(RawBook::new().sheet_part("M", &part).build()).unwrap();
    // The fixture links the part as a worksheet; the root element is what matters.
    assert_eq!(book.sheets[0].kind, SheetKind::Worksheet);
    assert_eq!(book.sheets[0].cells().count(), 1);
}
