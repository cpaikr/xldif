//! Prints every valued cell of a workbook as one JSON line: sheet, cell,
//! number format code, and the text xldif says Excel shows. Used by
//! `scripts/display_parity.py` to check display text against Excel.
//!
//! `cargo run -p xldif --example shown -- FILE`

use std::io::Write as _;

use xldif::cellref::cell_name;
use xldif::model::Value;
use xldif::numfmt::Formats;

fn main() {
    let path = std::env::args().nth(1).expect("usage: shown FILE");
    let book = xldif::read_path(std::path::Path::new(&path)).unwrap_or_else(|e| panic!("{e}"));
    let formats = Formats::new(&book.styles, book.date1904);
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for sheet in &book.sheets {
        for (row, cell) in sheet.cells() {
            if cell.value == Value::Empty {
                continue;
            }
            let line = serde_json::json!({
                "sheet": sheet.name,
                "cell": cell_name(row, cell.col),
                "format": book.styles.display_number_format(cell.style),
                "text": formats.shown(cell.style, &cell.value),
                "date": formats.get(cell.style).is_date(),
            });
            writeln!(out, "{line}").expect("stdout");
        }
    }
}
