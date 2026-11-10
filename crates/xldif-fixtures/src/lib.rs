//! Synthetic workbooks for tests, written with `rust_xlsxwriter`.
//!
//! Every value is invented. Formulas go through [`formula`], which requires
//! the cached result: `rust_xlsxwriter` otherwise stores 0 for every formula,
//! and xldif compares stored values, so a fixture without real results would
//! make value comparisons meaningless.

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
