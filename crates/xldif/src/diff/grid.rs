//! One sheet as alignment sees it: its content rows and columns, their
//! labels, and the tokens their cells contribute.

use std::collections::{HashMap, HashSet};

use crate::align::Item;
use crate::formula;
use crate::links::LinkMap;
use crate::model::{Cell, Row, Sheet, Value};
use crate::numfmt::Formats;

/// A deterministic 64-bit FNV-1a hash, so tokens never depend on a random seed.
pub(super) fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in *part {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// How many tolerances wide a bucket of numbers is in alignment tokens.
pub(super) const TOLERANCE_BUCKET: f64 = 8.0;
/// A number takes a bucket only when it is at least this many buckets from
/// zero. Smaller ones would nearly all share a bucket (rates under a
/// tolerance sized for amounts), and rows told apart only by them could no
/// longer be.
pub(super) const BUCKETS_FROM_ZERO: f64 = 8.0;
/// Where a bucket's edges sit within it. Off the round decimals that
/// amounts are typed in, so a figure and the same figure with float noise
/// do not straddle an edge.
pub(super) const BUCKET_PHASE: f64 = 0.381_966;

/// A value as bytes for an alignment token; `None` for nothing to align on.
///
/// With a tolerance, a number large against it is put in a bucket
/// [`TOLERANCE_BUCKET`] tolerances wide, so numbers the comparison will call
/// equal mostly share a token and float noise does not unpair their rows.
/// Two such numbers on either side of a bucket's edge still differ here,
/// and so do small numbers that differ at all; a row is paired on the rest
/// of its content then.
pub(super) fn value_bytes(v: &Value, tolerance: f64) -> Option<Vec<u8>> {
    Some(match v {
        Value::Empty => return None,
        Value::Number(n) => {
            let width = tolerance * TOLERANCE_BUCKET;
            if width > 0.0 && n.abs() >= width * BUCKETS_FROM_ZERO {
                [b"q".as_slice(), &(n / width + BUCKET_PHASE).floor().to_bits().to_le_bytes()].concat()
            } else {
                // Adding zero turns -0.0, which equals 0.0, into it.
                [b"n".as_slice(), &(n + 0.0).to_bits().to_le_bytes()].concat()
            }
        }
        Value::Text(t) if t.is_empty() => return None,
        Value::Text(t) => [b"t".as_slice(), t.as_bytes()].concat(),
        Value::Bool(b) => vec![b'b', u8::from(*b)],
        Value::Error(e) => [b"e".as_slice(), e.as_bytes()].concat(),
    })
}

/// A label made comparable: whitespace removed (Korean statements space out
/// characters, as in "자 산 총 계"), case folded, and a leading outline
/// marker dropped ("Ⅰ.", "(1)", "1.", "가."), since renumbering a section
/// does not make its rows different rows.
pub fn label_key(text: &str) -> Option<String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    let key = strip_outline_marker(&compact);
    let key = if key.is_empty() { compact.as_str() } else { key };
    (!key.is_empty()).then(|| key.to_owned())
}

pub(super) fn is_roman(c: char) -> bool {
    ('\u{2160}'..='\u{217F}').contains(&c)
}

pub(super) fn is_hangul_counter(c: char) -> bool {
    "가나다라마바사아자차카타파하".contains(c)
}

/// Drops one leading outline marker: `(1)`, `(가)`, `1.`, `1)`, `가.`, `Ⅰ.`, or
/// a bare Roman numeral directly before the label.
pub(super) fn strip_outline_marker(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix('(') {
        if let Some(end) = rest.find(')') {
            let inner = &rest[..end];
            let counter =
                inner.chars().all(|c| c.is_ascii_digit()) || inner.chars().all(|c| is_roman(c) || is_hangul_counter(c));
            if !inner.is_empty() && inner.chars().count() <= 3 && counter {
                return &rest[end + 1..];
            }
        }
        return s;
    }
    let digits: usize = s.chars().take_while(char::is_ascii_digit).map(char::len_utf8).sum();
    let romans: usize = s.chars().take_while(|c| is_roman(*c)).map(char::len_utf8).sum();
    let hangul = s.chars().next().filter(|c| is_hangul_counter(*c)).map_or(0, char::len_utf8);
    // Outline numbers stay short; "2025.12.31" is a date, not section 2025.
    for (lead, max) in [(digits, 2), (romans, 9), (hangul, 3)] {
        if lead == 0 || lead > max {
            continue;
        }
        let rest = &s[lead..];
        if let Some(stripped) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            return stripped;
        }
        if lead == romans && !rest.is_empty() {
            return rest;
        }
    }
    s
}

/// A number written as text, as exported statements do: "1,234", "(1,234)",
/// "-12.5", "12%". `None` for anything else.
pub fn numeric_text(text: &str) -> Option<f64> {
    let t = text.trim();
    let (negative, t) = match t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(inner) => (true, inner.trim()),
        None => (false, t),
    };
    let (scale, t) = match t.strip_suffix('%') {
        Some(x) => (0.01, x),
        None => (1.0, t),
    };
    let digits: String = t.chars().filter(|c| *c != ',').collect();
    if !digits.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let well_formed =
        digits.chars().enumerate().all(|(i, c)| c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+')));
    if !well_formed {
        return None;
    }
    let n: f64 = digits.parse().ok()?;
    Some(if negative { -n } else { n } * scale)
}

/// What tokens are read with besides the cells, so that two cells the
/// comparison will call equal are equal to the alignment too.
#[derive(Clone, Copy)]
pub(super) struct Reading<'a> {
    /// The comparison's tolerance, for number tokens.
    pub(super) tolerance: f64,
    /// When the two workbooks use different date systems: what to add to
    /// this workbook's date serials to put them on the 1900 scale.
    pub(super) date_offset: Option<f64>,
    /// For the base workbook: its external links in the other's numbering.
    pub(super) links: Option<&'a LinkMap>,
}

pub(super) fn display_label(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 60 { collapsed.chars().take(60).collect::<String>() + "…" } else { collapsed }
}

/// A cell's text as a column header: text that is not a number, or a year
/// stored as a number (period headers often are). Date headers are read in
/// [`Grid::new`], which needs the cell below them.
pub(super) fn header_text(cell: &Cell) -> Option<String> {
    let year = |n: f64| n.fract() == 0.0 && (1900.0..=2100.0).contains(&n);
    match &cell.value {
        Value::Number(n) if year(*n) => Some(format!("{n}")),
        // "2025" typed as text is a year too; "2,025" is an amount.
        Value::Text(t) if !t.contains(',') && numeric_text(t).is_some_and(year) => Some(t.trim().to_owned()),
        _ => text_of(cell).filter(|t| numeric_text(t).is_none()).map(display_label),
    }
}

pub(super) fn text_of(cell: &Cell) -> Option<&str> {
    match &cell.value {
        Value::Text(t) if !t.trim().is_empty() => Some(t),
        _ => None,
    }
}

/// How many content rows at the top of a sheet may hold column headers.
pub(super) const HEADER_ROWS: usize = 10;

/// Rows and columns of one sheet with content, for alignment.
pub(super) struct Grid<'a> {
    pub(super) sheet: &'a Sheet,
    /// Content rows, in order.
    pub(super) rows: Vec<&'a Row>,
    /// Columns with content, in order, each with its content cells by row.
    pub(super) cols: Vec<(u32, Vec<(u32, &'a Cell)>)>,
    /// Each column's header texts in the top content rows, in row order;
    /// the first is the column's label.
    pub(super) headers: HashMap<u32, Vec<String>>,
    reading: Reading<'a>,
    /// With a date offset: the styles that show a number as a date.
    date_styles: HashSet<u32>,
}

impl<'a> Grid<'a> {
    pub(super) fn new(sheet: &'a Sheet, formats: &Formats, reading: Reading<'a>) -> Self {
        let has_tokens =
            |c: &Cell| value_bytes(&c.value, 0.0).is_some() || c.formula.as_ref().is_some_and(|f| !f.text.is_empty());
        let rows: Vec<&Row> = sheet.rows.iter().filter(|r| r.cells.iter().any(has_tokens)).collect();
        let mut by_col: std::collections::BTreeMap<u32, Vec<(u32, &Cell)>> = std::collections::BTreeMap::new();
        for row in &rows {
            for cell in row.cells.iter().filter(|c| has_tokens(c)) {
                by_col.entry(cell.col).or_default().push((row.index, cell));
            }
        }
        let cols: Vec<(u32, Vec<(u32, &Cell)>)> = by_col.into_iter().collect();
        let top: HashSet<u32> = rows.iter().take(HEADER_ROWS).map(|r| r.index).collect();
        let date = |c: &Cell| formats.iso_date(c.style, &c.value);
        let headers = cols
            .iter()
            .map(|(col, cells)| {
                // A date heads a period column: the column's only date, over
                // figures. Where dates are the records (due dates, or dates
                // among other entries) they must not key the column.
                let one_date = cells.iter().filter(|(_, c)| date(c).is_some()).take(2).count() == 1;
                let figure = |c: &Cell| matches!(c.value, Value::Number(_)) && date(c).is_none();
                let texts = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, (r, _))| top.contains(r))
                    .filter_map(|(i, (_, c))| match date(c) {
                        Some(d) => (one_date && cells.get(i + 1).is_some_and(|(_, next)| figure(next))).then_some(d),
                        None => header_text(c),
                    })
                    .collect();
                (*col, texts)
            })
            .collect();
        let date_styles = if reading.date_offset.is_some() {
            let numeric = |c: &&Cell| matches!(c.value, Value::Number(_));
            let styles: HashSet<u32> =
                rows.iter().flat_map(|r| r.cells.iter().filter(numeric)).map(|c| c.style).collect();
            styles.into_iter().filter(|s| formats.get(*s).is_date()).collect()
        } else {
            HashSet::new()
        };
        Self { sheet, rows, cols, headers, reading, date_styles }
    }

    /// A cell's value as an alignment token. Under different date systems a
    /// date is read on one scale, as the comparison reads it; a time of day
    /// alone (below 1) is the same number in both.
    fn value_token(&self, cell: &Cell) -> Option<Vec<u8>> {
        match (&cell.value, self.reading.date_offset) {
            (Value::Number(n), Some(offset)) if *n >= 1.0 && self.date_styles.contains(&cell.style) => {
                value_bytes(&Value::Number(n + offset), self.reading.tolerance)
            }
            _ => value_bytes(&cell.value, self.reading.tolerance),
        }
    }

    /// What a cell at `row` contributes to alignment: its value and, when it
    /// has one, its formula in position-free form (so `=B5*2` in row 5 and
    /// `=B6*2` in row 6 are the same token, as they are after an insertion),
    /// with external links in the other workbook's numbering. A cell with
    /// neither (formatting only, or an empty string) contributes nothing, so
    /// its row or column is not an item at all.
    fn cell_tokens(&self, cell: &Cell, row: u32) -> impl Iterator<Item = Vec<u8>> {
        let formula = cell.formula.as_ref().filter(|f| !f.text.is_empty()).map(|f| {
            let tokens = formula::tokenize(&f.text);
            let relinked = self.reading.links.and_then(|links| links.relink(&tokens));
            let form = formula::relative_form(relinked.as_deref().unwrap_or(&tokens), row, cell.col);
            [b"f".as_slice(), form.as_bytes()].concat()
        });
        self.value_token(cell).into_iter().chain(formula)
    }

    /// A hash of everything alignment reads, to put two grids in a fixed
    /// order whichever is the base.
    pub(super) fn digest(&self) -> u64 {
        let mut h = 0u64;
        let mut mix = |parts: &[&[u8]]| h = fnv(&[&h.to_le_bytes(), &fnv(parts).to_le_bytes()]);
        for (col, _) in &self.cols {
            let header = self.headers.get(col).map(|h| h.join("/")).unwrap_or_default();
            mix(&[&col.to_le_bytes(), header.as_bytes()]);
        }
        for row in &self.rows {
            for c in &row.cells {
                let value = value_bytes(&c.value, self.reading.tolerance).unwrap_or_default();
                let formula = c.formula.as_ref().map_or("", |f| f.text.as_str());
                mix(&[&row.index.to_le_bytes(), &c.col.to_le_bytes(), &value, formula.as_bytes()]);
            }
        }
        h
    }

    /// The written cells of one row, in column order.
    pub(super) fn row_cells(&self, row: u32) -> &'a [Cell] {
        self.sheet.rows.binary_search_by_key(&row, |r| r.index).map_or(&[][..], |i| &self.sheet.rows[i].cells[..])
    }

    pub(super) fn cell(&self, row: u32, col: u32) -> Option<&'a Cell> {
        let r = self.sheet.rows.binary_search_by_key(&row, |r| r.index).ok()?;
        let cells = &self.sheet.rows[r].cells;
        cells.binary_search_by_key(&col, |c| c.col).ok().map(|i| &cells[i])
    }

    /// The leftmost text cell of a row: its label.
    pub(super) fn row_label(row: &Row) -> Option<&str> {
        row.cells.iter().find_map(text_of)
    }

    /// The column's header from the top rows; failing that, the nearest
    /// text above `row` in `col` that is not a number written as text. The
    /// header comes first because in a text column the nearest text above
    /// is usually the previous record, not a heading.
    pub(super) fn column_label(&self, row: u32, col: u32) -> Option<String> {
        if let Some(header) = self.headers.get(&col).and_then(|h| h.first()) {
            return Some(header.clone());
        }
        let below = self.sheet.rows.partition_point(|r| r.index < row);
        self.sheet.rows[..below]
            .iter()
            .rev()
            .take(30)
            .find_map(|r| {
                let cell = &r.cells[r.cells.binary_search_by_key(&col, |c| c.col).ok()?];
                text_of(cell).filter(|t| numeric_text(t).is_none())
            })
            .map(display_label)
    }

    pub(super) fn column_items(&self) -> Vec<Item> {
        self.cols
            .iter()
            .map(|(col, cells)| {
                let header = self.headers.get(col).map(Vec::as_slice).unwrap_or_default();
                let mut tokens: Vec<u64> = cells
                    .iter()
                    .flat_map(|(r, c)| {
                        // A column's formulas change form whenever a column they
                        // refer to moves, so they only stand in for a missing value.
                        let value = self.value_token(c);
                        let formula = if value.is_none() { self.cell_tokens(c, *r).collect() } else { Vec::new() };
                        value.into_iter().chain(formula)
                    })
                    .map(|b| fnv(&[&b]))
                    .collect();
                tokens.sort_unstable();
                Item { key: label_key(&header.join("/")), tokens, shape: 0 }
            })
            .collect()
    }

    /// Row items whose tokens tie each value to its aligned column, so a
    /// value only counts as shared when it sits in the matching column.
    pub(super) fn row_items(&self, col_ids: &HashMap<u32, u32>) -> Vec<Item> {
        self.rows
            .iter()
            .map(|row| {
                let mut tokens = Vec::new();
                let mut shape = Vec::new();
                for c in &row.cells {
                    let Some(id) = col_ids.get(&c.col) else { continue };
                    let before = tokens.len();
                    tokens.extend(self.cell_tokens(c, row.index).map(|bytes| fnv(&[&id.to_le_bytes(), &bytes])));
                    if tokens.len() > before {
                        shape.extend_from_slice(&id.to_le_bytes());
                    }
                }
                tokens.sort_unstable();
                // Cells arrive in column order, so the shape needs no sorting.
                let shape = if shape.is_empty() { 0 } else { fnv(&[&shape]) };
                Item { key: Self::row_label(row).and_then(label_key), tokens, shape }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{label_key, numeric_text};

    #[test]
    fn labels_drop_spacing_case_and_outline_markers() {
        for (text, key) in [
            ("자 산 총 계", "자산총계"),
            ("    Ⅰ. 유동자산", "유동자산"),
            ("Ⅱ비유동자산", "비유동자산"),
            ("(1)  당좌자산", "당좌자산"),
            ("1. Cash", "cash"),
            ("가. 현금", "현금"),
            ("2025", "2025"),
            ("2025.12.31", "2025.12.31"),
            ("가지급금", "가지급금"),
            ("(주)우성", "(주)우성"),
            ("(1)", "(1)"),
        ] {
            assert_eq!(label_key(text).as_deref(), Some(key), "{text}");
        }
        assert_eq!(label_key("   "), None);
    }

    #[test]
    fn number_tokens_follow_the_tolerance_only_for_large_numbers() {
        use super::{Value, value_bytes};
        let token = |n: f64, tolerance: f64| value_bytes(&Value::Number(n), tolerance);
        // Zero has one token, whatever its sign.
        assert_eq!(token(0.0, 0.0), token(-0.0, 0.0));
        assert_ne!(token(1.0, 0.0), token(1.0 + f64::EPSILON, 0.0));
        // Under a tolerance, an amount and the same amount with noise share
        // a token, also when it is typed to the cent.
        for amount in [1234.56, 99_999.99, -4321.04, 1_000_000.0] {
            assert_eq!(token(amount, 0.01), token(amount + 1e-9, 0.01), "{amount}");
        }
        assert_ne!(token(1234.56, 0.01), token(1235.56, 0.01));
        // Numbers small against the tolerance keep their own tokens: rates
        // under a tolerance sized for amounts still tell rows apart.
        assert_ne!(token(0.035, 0.5), token(0.0425, 0.5));
        assert_eq!(token(0.035, 0.5), token(0.035, 0.0));
    }

    #[test]
    fn numbers_written_as_text_parse() {
        assert_eq!(numeric_text("19,555,301,961"), Some(19_555_301_961.0));
        assert_eq!(numeric_text("(1,000)"), Some(-1000.0));
        assert_eq!(numeric_text("-12.5"), Some(-12.5));
        assert_eq!(numeric_text("50%"), Some(0.5));
        for not in ["", "-", "1-2", "abc", "12원", "1.2.3x"] {
            assert_eq!(numeric_text(not), None, "{not}");
        }
    }
}
