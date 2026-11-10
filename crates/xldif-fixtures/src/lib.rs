//! Synthetic workbooks for tests, written with `rust_xlsxwriter`.
//!
//! Every value is invented. Formulas go through [`formula`], which requires
//! the cached result: `rust_xlsxwriter` otherwise stores 0 for every formula,
//! and xldif compares stored values, so a fixture without real results would
//! make value comparisons meaningless.

pub mod raw;

use rust_xlsxwriter::Formula;
pub use rust_xlsxwriter::{Workbook, XlsxError};

/// A formula with its cached result, as Excel would have saved it.
pub fn formula(expression: &str, result: &str) -> Formula {
    Formula::new(expression).set_result(result)
}

/// A small two-sheet workbook: a labelled column of numbers with a total,
/// and a second sheet that refers to it.
pub fn basic() -> Result<Workbook, XlsxError> {
    let mut book = Workbook::new();
    let bs = book.add_worksheet().set_name("BS")?;
    bs.write_string(0, 0, "Account")?;
    bs.write_string(0, 1, "2025")?;
    for (row, (label, amount)) in [(1, ("Cash", 100.0)), (2, ("Receivables", 200.0))] {
        bs.write_string(row, 0, label)?;
        bs.write_number(row, 1, amount)?;
    }
    bs.write_string(3, 0, "Total")?;
    bs.write_formula(3, 1, formula("=SUM(B2:B3)", "300"))?;

    let notes = book.add_worksheet().set_name("Notes")?;
    notes.write_string(0, 0, "Total assets")?;
    notes.write_formula(0, 1, formula("=BS!B4", "300"))?;
    Ok(book)
}

/// Values of every type, Excel-style shared and array formulas, rich text
/// with phonetic runs, escapes, and a hidden sheet, assembled from raw XML.
pub fn edge_cases() -> Vec<u8> {
    raw::RawBook::new()
        .shared_strings(
            "<si><t>plain</t></si>             <si><r><t>rich </t></r><r><rPr><b/></rPr><t>text</t></r><rPh sb=\"0\" eb=\"1\"><t>yomi</t></rPh></si>             <si><t xml:space=\"preserve\"> padded &amp; escaped_x000D_</t></si>",
        )
        .sheet(
            "Values",
            r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c></row>
               <row r="2"><c r="A2"><v>1.25</v></c><c r="B2"><v>-3E-5</v></c><c r="C2" t="b"><v>0</v></c><c r="D2" t="e"><v>#N/A</v></c></row>
               <row r="3"><c r="A3" t="inlineStr"><is><t>inline</t></is></c><c r="B3" t="str"><f>A3&amp;"!"</f><v>inline!</v></c></row>"#,
        )
        .sheet_with_state(
            "Formulas",
            Some("hidden"),
            r#"<row r="1"><c r="A1"><v>1</v></c><c r="B1"><f t="shared" ref="B1:B3" si="0">A1*2</f><v>2</v></c></row>
               <row r="2"><c r="A2"><v>2</v></c><c r="B2"><f t="shared" si="0"/><v>4</v></c></row>
               <row r="3"><c r="A3"><v>3</v></c><c r="B3"><f t="shared" si="0"/><v>6</v></c></row>
               <row r="5"><c r="A5"><f>SUM(A1:A3)</f><v>6</v></c></row>
               <row r="7"><c r="A7"><f t="array" ref="A7:A8">A1:A2*10</f><v>10</v></c></row>
               <row r="8"><c r="A8"><v>20</v></c></row>"#,
        )
        .build()
}

/// A workbook from text grids, one per sheet: a cell that parses as a
/// number is written as one, `=FORMULA|result` is a formula with its cached
/// result, an empty string leaves the cell blank, and anything else is
/// text. Row 1 of the grid is sheet row 1.
pub fn grid_book(sheets: &[(&str, &[&[&str]])]) -> Vec<u8> {
    let mut book = Workbook::new();
    for (name, rows) in sheets {
        let sheet = book.add_worksheet().set_name(*name).expect("valid sheet name");
        for (r, row) in rows.iter().enumerate() {
            for (c, text) in row.iter().enumerate() {
                let (r, c) = (r as u32, c as u16);
                if text.is_empty() {
                    continue;
                }
                if text.starts_with('=') {
                    let (expression, result) = text.split_once('|').unwrap_or((text, "0"));
                    sheet.write_formula(r, c, formula(expression, result)).expect("cell in range");
                    continue;
                }
                match text.parse::<f64>() {
                    Ok(n) => sheet.write_number(r, c, n),
                    Err(_) => sheet.write_string(r, c, *text),
                }
                .expect("cell in range");
            }
        }
    }
    book.save_to_buffer().expect("workbook builds")
}

/// Every fixture, by file name, for checks outside Rust.
pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("basic.xlsx", basic().and_then(|mut b| b.save_to_buffer()).expect("basic fixture builds")),
        ("edge-cases.xlsx", edge_cases()),
    ]
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read as _};

    fn member(bytes: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        archive.by_name(name).unwrap().read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn basic_stores_formula_results() {
        let bytes = super::basic().unwrap().save_to_buffer().unwrap();
        let bs = member(&bytes, "xl/worksheets/sheet1.xml");
        assert!(bs.contains("<f>SUM(B2:B3)</f><v>300</v>"), "{bs}");
        let notes = member(&bytes, "xl/worksheets/sheet2.xml");
        assert!(notes.contains("<f>BS!B4</f><v>300</v>"), "{notes}");
    }
}
