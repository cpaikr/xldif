//! Writes a demonstration pair of workbooks (every value invented) that
//! exercises each kind of change, for viewing the HTML report.
//!
//! Usage: `cargo run -p xldif-fixtures --example write_demo -- <dir>`

use std::path::PathBuf;

use rust_xlsxwriter::{Format, Note, Workbook, XlsxError};
use xldif_fixtures::formula;

fn book(later: bool) -> Result<Workbook, XlsxError> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let money = Format::new().set_num_format("#,##0");
    let ws = wb.add_worksheet().set_name("Balance sheet")?;
    ws.write_string_with_format(0, 0, "Account", &bold)?;
    ws.write_string_with_format(0, 1, "2025", &bold)?;
    ws.write_string_with_format(0, 2, "2024", &bold)?;
    let mut rows: Vec<(&str, f64, f64)> = vec![
        ("Cash", 1_200.0, 900.0),
        ("Trade receivables", 3_400.0, 3_100.0),
        ("Inventory", 2_100.0, 2_000.0),
        ("Prepaid expenses", 300.0, 250.0),
        ("Other assets", 150.0, 150.0),
    ];
    if later {
        rows[1].1 = 3_650.0; // a value change
        rows.insert(3, ("Short-term loans", 500.0, 0.0)); // an inserted row
        let moved = rows.remove(0); // a moved row
        rows.insert(3, moved);
    }
    for (i, (label, now, before)) in rows.iter().enumerate() {
        let r = 1 + i as u32;
        ws.write_string(r, 0, *label)?;
        ws.write_number_with_format(r, 1, *now, &money)?;
        ws.write_number_with_format(r, 2, *before, &money)?;
    }
    let total = 1 + rows.len() as u32;
    let sum = |col: &str, value: f64| formula(&format!("=SUM({col}2:{col}{total})"), &value.to_string());
    let (now, before): (f64, f64) = (rows.iter().map(|r| r.1).sum(), rows.iter().map(|r| r.2).sum());
    ws.write_string_with_format(total, 0, "Total assets", &bold)?;
    let total_format = if later { money.clone().set_bold() } else { money.clone() };
    ws.write_formula_with_format(total, 1, sum("B", now), &total_format)?;
    // In the later version the prior-year total was typed over its formula.
    if later {
        ws.write_number_with_format(total, 2, before, &money)?;
    } else {
        ws.write_formula_with_format(total, 2, sum("C", before), &money)?;
    }
    let note =
        Note::new(if later { "Agreed to ledger; loan added" } else { "Agreed to ledger" }).set_author("Reviewer");
    ws.insert_note(total, 1, &note)?;
    ws.set_column_width(0, 24)?;

    let notes = wb.add_worksheet().set_name(if later { "Notes 2025" } else { "Notes" })?;
    notes.write_string(0, 0, "Accounting policies")?;
    notes.write_string(1, 0, "Inventory is measured at the lower of cost and net realisable value.")?;
    notes.write_formula(2, 0, formula("='Balance sheet'!B5", "2100"))?;
    Ok(wb)
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: write_demo <dir>"));
    std::fs::create_dir_all(&dir).expect("create the output directory");
    for (name, later) in [("demo-v1.xlsx", false), ("demo-v2.xlsx", true)] {
        book(later).and_then(|mut b| b.save(dir.join(name))).expect("write a demo workbook");
    }
}
