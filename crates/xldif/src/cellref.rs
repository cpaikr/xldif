//! A1-style cell references: `B5` is row 5, column 2 (both 1-based).

/// Excel's grid: 1,048,576 rows by 16,384 columns (XFD).
pub const MAX_ROW: u32 = 1_048_576;
pub const MAX_COL: u32 = 16_384;

/// Column letters for a 1-based column: 1 → `A`, 27 → `AA`.
pub fn col_name(mut col: u32) -> String {
    let mut out = Vec::new();
    while col > 0 {
        let rem = (col - 1) % 26;
        out.push(b'A' + rem as u8);
        col = (col - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII letters")
}

pub fn cell_name(row: u32, col: u32) -> String {
    format!("{}{row}", col_name(col))
}

/// Parses column letters (case-insensitive) into a 1-based column.
pub fn parse_col(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut col = 0u32;
    for b in letters.bytes() {
        if !b.is_ascii_alphabetic() {
            return None;
        }
        col = col * 26 + u32::from(b.to_ascii_uppercase() - b'A' + 1);
    }
    (col <= MAX_COL).then_some(col)
}

/// Parses `B5` (optionally with `$` markers) into `(row, col)`.
pub fn parse_cell(text: &str) -> Option<(u32, u32)> {
    let text = text.replace('$', "");
    let split = text.find(|c: char| c.is_ascii_digit())?;
    let col = parse_col(&text[..split])?;
    let digits = &text[split..];
    if !digits.bytes().all(|b| b.is_ascii_digit()) || digits.starts_with('0') {
        return None;
    }
    let row: u32 = digits.parse().ok()?;
    (row <= MAX_ROW).then_some((row, col))
}

/// A rectangle of cells, inclusive on both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Range {
    pub first_row: u32,
    pub first_col: u32,
    pub last_row: u32,
    pub last_col: u32,
}

impl Range {
    /// Parses `A1:C9` or a single cell `B5`; corners may come in any order.
    pub fn parse(text: &str) -> Option<Self> {
        let (a, b) = text.split_once(':').unwrap_or((text, text));
        let (r1, c1) = parse_cell(a.trim())?;
        let (r2, c2) = parse_cell(b.trim())?;
        Some(Self { first_row: r1.min(r2), first_col: c1.min(c2), last_row: r1.max(r2), last_col: c1.max(c2) })
    }

    pub fn contains(&self, row: u32, col: u32) -> bool {
        (self.first_row..=self.last_row).contains(&row) && (self.first_col..=self.last_col).contains(&col)
    }

    pub fn name(&self) -> String {
        let first = cell_name(self.first_row, self.first_col);
        if self.first_row == self.last_row && self.first_col == self.last_col {
            first
        } else {
            format!("{first}:{}", cell_name(self.last_row, self.last_col))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_COL, Range, col_name, parse_cell, parse_col};

    #[test]
    fn columns_round_trip() {
        for col in [1, 26, 27, 52, 702, 703, MAX_COL] {
            assert_eq!(parse_col(&col_name(col)), Some(col), "{col}");
        }
        assert_eq!(col_name(MAX_COL), "XFD");
        assert_eq!(parse_col("XFE"), None);
    }

    #[test]
    fn cells_parse_strictly() {
        assert_eq!(parse_cell("B5"), Some((5, 2)));
        assert_eq!(parse_cell("$AA$10"), Some((10, 27)));
        assert_eq!(parse_cell("b5"), Some((5, 2)));
        for bad in ["", "5", "B", "B0", "B05", "B5C", "B1048577"] {
            assert_eq!(parse_cell(bad), None, "{bad}");
        }
    }

    #[test]
    fn ranges_normalize_corners() {
        let r = Range::parse("C9:A1").unwrap();
        assert_eq!(r.name(), "A1:C9");
        assert!(r.contains(5, 2) && !r.contains(10, 1));
        assert_eq!(Range::parse("B5").unwrap().name(), "B5");
    }
}
