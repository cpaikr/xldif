//! Prints one written cell for each distinct cell format of a workbook as
//! a JSON line: sheet, cell, the format as xldif resolves it (the
//! properties `compare` reports), and the border each written neighbour
//! has on the edge it shares with the cell. Used by
//! `scripts/format_parity.py` to check resolved formats against Excel.
//!
//! A cell outside every merge stands for its format when there is one:
//! Excel reports a merged cell's alignment for the whole merged area, so
//! the check skips those. Two cells share an edge and Excel reports one
//! border for it, so the check needs both cells' borders.
//!
//! `cargo run -p xldif --example formats -- FILE`

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;

use xldif::cellref::cell_name;

fn main() {
    let path = std::env::args().nth(1).expect("usage: formats FILE");
    let book = xldif::read_path(std::path::Path::new(&path)).unwrap_or_else(|e| panic!("{e}"));
    // style → (sheet index, row, column, inside a merge), the first cell
    // seen, or the first outside every merge.
    let mut samples: BTreeMap<u32, (usize, u32, u32, bool)> = BTreeMap::new();
    let mut styles: Vec<HashMap<(u32, u32), u32>> = Vec::new();
    for (index, sheet) in book.sheets.iter().enumerate() {
        styles.push(sheet.cells().map(|(row, cell)| ((row, cell.col), cell.style)).collect());
        for (row, cell) in sheet.cells() {
            let merged = sheet
                .merges
                .iter()
                .any(|m| (m.first_row..=m.last_row).contains(&row) && (m.first_col..=m.last_col).contains(&cell.col));
            match samples.get(&cell.style) {
                Some((.., false)) => {}
                Some(_) if merged => {}
                _ => {
                    samples.insert(cell.style, (index, row, cell.col, merged));
                }
            }
        }
    }
    let properties =
        |style: u32| -> HashMap<&'static str, String> { book.styles.format(style).properties().into_iter().collect() };
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for (style, (index, row, col, _)) in samples {
        let format: serde_json::Map<String, serde_json::Value> =
            book.styles.format(style).properties().into_iter().map(|(k, v)| (k.to_owned(), v.into())).collect();
        // The cell across each edge, and the edge of its own that faces back.
        let across = [
            ("border.left", (row, col.wrapping_sub(1)), "border.right"),
            ("border.right", (row, col + 1), "border.left"),
            ("border.top", (row.wrapping_sub(1), col), "border.bottom"),
            ("border.bottom", (row + 1, col), "border.top"),
        ];
        let neighbours: serde_json::Map<String, serde_json::Value> = across
            .into_iter()
            .filter_map(|(edge, at, facing)| {
                let style = styles[index].get(&at)?;
                Some((edge.to_owned(), properties(*style).remove(facing)?.into()))
            })
            .collect();
        let line = serde_json::json!({
            "sheet": book.sheets[index].name,
            "cell": cell_name(row, col),
            "format": format,
            "neighbours": neighbours,
        });
        writeln!(out, "{line}").expect("stdout");
    }
}
