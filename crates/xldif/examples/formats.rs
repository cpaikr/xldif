//! Prints one written cell for each distinct cell format of a workbook as
//! a JSON line: sheet, cell, and the format as xldif resolves it (the
//! properties `compare` reports). Used by `scripts/format_parity.py` to
//! check resolved formats against Excel.
//!
//! `cargo run -p xldif --example formats -- FILE`

use std::collections::HashSet;
use std::io::Write as _;

use xldif::cellref::cell_name;

fn main() {
    let path = std::env::args().nth(1).expect("usage: formats FILE");
    let book = xldif::read_path(std::path::Path::new(&path)).unwrap_or_else(|e| panic!("{e}"));
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let mut seen = HashSet::new();
    for sheet in &book.sheets {
        for (row, cell) in sheet.cells() {
            if !seen.insert(cell.style) {
                continue;
            }
            let properties: serde_json::Map<String, serde_json::Value> = book
                .styles
                .format(cell.style)
                .properties()
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.into()))
                .collect();
            let line = serde_json::json!({
                "sheet": sheet.name,
                "cell": cell_name(row, cell.col),
                "format": properties,
            });
            writeln!(out, "{line}").expect("stdout");
        }
    }
}
