//! Writes the synthetic fixture workbooks into a directory, for checks that
//! run outside Rust (the openpyxl parity script).
//!
//! Usage: `cargo run -p xldif-fixtures --example write_fixtures -- <dir>`

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: write_fixtures <dir>"));
    std::fs::create_dir_all(&dir).expect("create the output directory");
    for (name, bytes) in xldif_fixtures::all() {
        std::fs::write(dir.join(name), bytes).expect("write a fixture");
    }
}
