//! Excel formulas as tokens, for comparing formulas across a structural edit.
//!
//! A formula is split into references and everything else. References are
//! what move: [`shift`] offsets their relative parts (expanding a shared
//! formula to the cells after its anchor), and [`remap`] carries them
//! through a row and column alignment, the way Excel adjusts a formula when
//! rows or columns are inserted or deleted. Two formulas are the same when
//! their tokens agree, ignoring spacing and the case of names.
//!
//! The tokenizer is deliberately forgiving: anything it does not recognize
//! (structured table references, external workbook references, 3-D sheet
//! ranges) passes through as opaque text, which compares as written but is
//! never adjusted. One odd formula never fails a comparison.

// Short names (a, b for two formulas; r, c for row and column) read best here.
#![allow(clippy::many_single_char_names)]

use std::fmt::Write as _;

use crate::cellref::{MAX_COL, MAX_ROW, col_name, parse_col};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    Cell {
        row: u32,
        col: u32,
        row_abs: bool,
        col_abs: bool,
    },
    /// A whole row, as in `3:5`.
    Row {
        row: u32,
        abs: bool,
    },
    /// A whole column, as in `B:D`.
    Col {
        col: u32,
        abs: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The sheet as written before `!`, unquoted; `None` for this sheet.
    pub sheet: Option<String>,
    pub start: Point,
    /// The far corner of a range; both points have the same kind.
    pub end: Option<Point>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Ref(Reference),
    /// A string literal's content, compared exactly.
    Str(String),
    /// Anything else: functions, names, numbers, operators. Compared without
    /// case; whitespace tokens are ignored.
    Text(String),
    /// A reference that no longer exists (`#REF!`, or `Sheet!#REF!` with its
    /// sheet): its target was deleted or shifted off the grid.
    RefError(Option<String>),
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\') || (!c.is_ascii() && !c.is_whitespace())
}

/// Splits `$A$1`, `A1`, `$A`, `3` into a point; `None` if it is not one.
fn parse_point(word: &str) -> Option<Point> {
    let (col_abs, rest) = word.strip_prefix('$').map_or((false, word), |r| (true, r));
    let letters = rest.chars().take_while(char::is_ascii_alphabetic).count();
    let (col_part, rest) = rest.split_at(letters);
    let (row_abs, digits) = rest.strip_prefix('$').map_or((false, rest), |r| (true, r));
    let row = (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && !digits.starts_with('0'))
        .then(|| digits.parse::<u32>().ok())
        .flatten()
        .filter(|r| *r <= MAX_ROW);
    match (col_part.is_empty(), digits.is_empty()) {
        (false, false) => Some(Point::Cell { row: row?, col: parse_col(col_part)?, row_abs, col_abs }),
        (false, true) if !row_abs => Some(Point::Col { col: parse_col(col_part)?, abs: col_abs }),
        (true, false) if !col_abs || !row_abs => {
            // `$3` is an absolute row; a bare `3` alone is a number, which
            // only becomes a row inside a range (the caller checks that).
            Some(Point::Row { row: row?, abs: col_abs || row_abs })
        }
        _ => None,
    }
}

fn same_kind(a: &Point, b: &Point) -> bool {
    matches!(
        (a, b),
        (Point::Cell { .. }, Point::Cell { .. })
            | (Point::Row { .. }, Point::Row { .. })
            | (Point::Col { .. }, Point::Col { .. })
    )
}

/// Parses `A1`, `A1:B2`, `A:C`, or `1:3` from the start of `s`, returning
/// the reference's points and the bytes it used.
fn parse_reference(s: &str) -> Option<(Point, Option<Point>, usize)> {
    let first_len: usize = s.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '$').map(char::len_utf8).sum();
    let first = parse_point(&s[..first_len])?;
    // A reference cannot run into more word characters (`A1B`, `A1.x`).
    if s[first_len..].chars().next().is_some_and(is_word_char) {
        return None;
    }
    if let Some(rest) = s[first_len..].strip_prefix(':') {
        let second_len: usize =
            rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '$').map(char::len_utf8).sum();
        if let Some(second) = parse_point(&rest[..second_len])
            && same_kind(&first, &second)
            && !rest[second_len..].chars().next().is_some_and(is_word_char)
        {
            return Some((first, Some(second), first_len + 1 + second_len));
        }
    }
    // Whole rows and columns only exist as ranges.
    matches!(first, Point::Cell { .. }).then_some((first, None, first_len))
}

/// What follows a sheet prefix and `!`: a reference, or `#REF!`.
fn qualified(sheet: String, target: &str) -> Option<(Token, usize)> {
    if target.get(..5).is_some_and(|t| t.eq_ignore_ascii_case("#REF!")) {
        return Some((Token::RefError(Some(sheet)), 5));
    }
    let (start, end, used) = parse_reference(target)?;
    Some((Token::Ref(Reference { sheet: Some(sheet), start, end }), used))
}

/// The length of the word starting `s` (sheet names, names, references).
fn word_len(s: &str) -> usize {
    s.chars().take_while(|c| is_word_char(*c)).map(char::len_utf8).sum()
}

/// Tokenizes a formula as stored (without the leading `=`).
pub fn tokenize(formula: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut rest = formula;
    while let Some(c) = rest.chars().next() {
        if c == '"' {
            let (lit, used) = quoted(rest, '"');
            out.push(Token::Str(lit));
            rest = &rest[used..];
        } else if c == '\'' {
            let (sheet, used) = quoted(rest, '\'');
            if let Some((token, len)) = rest[used..].strip_prefix('!').and_then(|t| qualified(sheet, t)) {
                out.push(token);
                rest = &rest[used + 1 + len..];
            } else {
                out.push(Token::Text(rest[..used].to_owned()));
                rest = &rest[used..];
            }
        } else if c == '#' && rest.get(..5).is_some_and(|t| t.eq_ignore_ascii_case("#REF!")) {
            out.push(Token::RefError(None));
            rest = &rest[5..];
        } else if c == '[' || c == '#' {
            // An external workbook's sheet ([1]Prices!A1) is a reference that
            // shifts with a shared formula but never follows this workbook's
            // alignment; anything else (error literals, unusual brackets)
            // stays opaque.
            let book = bracketed_len(rest);
            let name = word_len(&rest[book..]);
            // An unterminated `[` has no index to read.
            let index = rest[..book].strip_prefix('[').and_then(|r| r.strip_suffix(']'));
            let external = index.is_some_and(|i| i.bytes().all(|b| b.is_ascii_digit()))
                && name > 0
                && rest[book + name..].starts_with('!');
            if let Some((token, len)) =
                external.then(|| qualified(rest[..book + name].to_owned(), &rest[book + name + 1..])).flatten()
            {
                out.push(token);
                rest = &rest[book + name + 1 + len..];
            } else {
                let len = opaque_len(rest);
                out.push(Token::Text(rest[..len].to_owned()));
                rest = &rest[len..];
            }
        } else if c.is_whitespace() {
            let len: usize = rest.chars().take_while(|c| c.is_whitespace()).map(char::len_utf8).sum();
            out.push(Token::Text(rest[..len].to_owned()));
            rest = &rest[len..];
        } else if is_word_char(c) {
            let len: usize = rest.chars().take_while(|c| is_word_char(*c)).map(char::len_utf8).sum();
            let word = &rest[..len];
            let after = &rest[len..];
            if after.starts_with('[') {
                // A structured reference: Table1[Column], Table1[[#This Row],[A]].
                let total = len + bracketed_len(after);
                out.push(Token::Text(rest[..total].to_owned()));
                rest = &rest[total..];
            } else if let Some(target) = after.strip_prefix('!')
                && let Some((token, used)) = qualified(word.to_owned(), target)
            {
                out.push(token);
                rest = &target[used..];
            } else if let Some(second) = after.strip_prefix(':')
                && let n = word_len(second)
                && n > 0
                && let Some(target) = second[n..].strip_prefix('!')
                && let Some((token, used)) = qualified(format!("{word}:{}", &second[..n]), target)
            {
                // A 3-D reference across sheets (Jan:Dec!B5): one reference
                // whose "sheet" names the span. It shifts in shared formulas
                // and is otherwise left as written.
                out.push(token);
                rest = &target[used..];
            } else if after.starts_with('(') {
                out.push(Token::Text(word.to_owned()));
                rest = after;
            } else if let Some((start, end, used)) = parse_reference(rest) {
                out.push(Token::Ref(Reference { sheet: None, start, end }));
                rest = &rest[used..];
            } else {
                out.push(Token::Text(word.to_owned()));
                rest = after;
            }
        } else {
            out.push(Token::Text(c.to_string()));
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// A `q`-quoted run with doubled quotes as escapes: its content and the
/// bytes used, including both quotes (or to the end when unterminated).
fn quoted(s: &str, q: char) -> (String, usize) {
    let mut content = String::new();
    let mut chars = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if c == q {
            if chars.peek().is_some_and(|(_, n)| *n == q) {
                content.push(q);
                chars.next();
            } else {
                return (content, i + c.len_utf8());
            }
        } else {
            content.push(c);
        }
    }
    (content, s.len())
}

/// The length of a balanced `[...]` group at the start of `s`.
fn bracketed_len(s: &str) -> usize {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    s.len()
}

/// An opaque run starting with `[` or `#`: brackets, then word characters
/// and the `!`, `?`, `/` that error literals and external references use.
fn opaque_len(s: &str) -> usize {
    let mut len = if s.starts_with('[') { bracketed_len(s) } else { 1 };
    for c in s[len..].chars() {
        if is_word_char(c) || matches!(c, '!' | '?' | '/' | ':' | '\'') {
            len += c.len_utf8();
        } else {
            break;
        }
    }
    len
}

fn render_point(out: &mut String, p: &Point) {
    let dollar = |abs: bool| if abs { "$" } else { "" };
    match *p {
        Point::Cell { row, col, row_abs, col_abs } => {
            let _ = write!(out, "{}{}{}{row}", dollar(col_abs), col_name(col), dollar(row_abs));
        }
        Point::Row { row, abs } => {
            let _ = write!(out, "{}{row}", dollar(abs));
        }
        Point::Col { col, abs } => {
            let _ = write!(out, "{}{}", dollar(abs), col_name(col));
        }
    }
}

/// Whether Excel would quote this sheet name in a reference: names that
/// are not plain words, start with a digit, or read as a cell, an R1C1
/// reference, or a boolean.
fn needs_quotes(sheet: &str) -> bool {
    let upper = sheet.to_ascii_uppercase();
    let r1c1 = {
        let rest =
            upper.strip_prefix('R').map_or(upper.as_str(), |r| r.trim_start_matches(|c: char| c.is_ascii_digit()));
        let rest = rest.strip_prefix('C').map_or(rest, |r| r.trim_start_matches(|c: char| c.is_ascii_digit()));
        rest.is_empty() && (upper.starts_with('R') || upper.starts_with('C'))
    };
    sheet.is_empty()
        || sheet.chars().next().is_some_and(|c| c.is_ascii_digit())
        || !sheet.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        || matches!(parse_point(sheet), Some(Point::Cell { .. }))
        || r1c1
        || upper == "TRUE"
        || upper == "FALSE"
}

/// `Sheet!`, `'My sheet'!`, `Jan:Dec!`, or `[1]Prices!` as Excel writes it.
fn write_sheet(out: &mut String, sheet: &str) {
    // An external workbook index or a 3-D span is quoted as a whole only
    // when one of its names needs it.
    let (book, names) = match sheet.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((n, rest)) => (format!("[{n}]"), rest),
        None => (String::new(), sheet),
    };
    if names.split(':').any(needs_quotes) {
        let _ = write!(out, "'{}'!", sheet.replace('\'', "''"));
    } else {
        let _ = write!(out, "{book}{names}!");
    }
}

/// Formula text from tokens, as Excel would store it.
pub fn render(tokens: &[Token]) -> String {
    let mut out = String::new();
    for t in tokens {
        match t {
            Token::Ref(r) => {
                if let Some(sheet) = &r.sheet {
                    write_sheet(&mut out, sheet);
                }
                render_point(&mut out, &r.start);
                if let Some(end) = &r.end {
                    out.push(':');
                    render_point(&mut out, end);
                }
            }
            Token::Str(s) => {
                let _ = write!(out, "\"{}\"", s.replace('"', "\"\""));
            }
            Token::Text(t) => out.push_str(t),
            Token::RefError(sheet) => {
                if let Some(sheet) = sheet {
                    write_sheet(&mut out, sheet);
                }
                out.push_str("#REF!");
            }
        }
    }
    out
}

/// Whether two token streams are the same formula: spacing ignored, names
/// compared without case, string literals and references exactly.
pub fn same(a: &[Token], b: &[Token]) -> bool {
    let significant = |t: &&Token| !matches!(t, Token::Text(s) if s.trim().is_empty());
    let mut x = a.iter().filter(significant);
    let mut y = b.iter().filter(significant);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(Token::Text(p)), Some(Token::Text(q)))
                if p.eq_ignore_ascii_case(q) || p.to_lowercase() == q.to_lowercase() => {}
            (Some(Token::Ref(p)), Some(Token::Ref(q))) => {
                let sheets = match (&p.sheet, &q.sheet) {
                    (Some(s), Some(t)) => s.to_lowercase() == t.to_lowercase(),
                    (None, None) => true,
                    _ => false,
                };
                if !(sheets && p.start == q.start && p.end == q.end) {
                    return false;
                }
            }
            (Some(Token::RefError(p)), Some(Token::RefError(q))) => {
                if p.as_ref().map(|s| s.to_lowercase()) != q.as_ref().map(|s| s.to_lowercase()) {
                    return false;
                }
            }
            (Some(p), Some(q)) if p == q => {}
            _ => return false,
        }
    }
}

/// Whether a reference in the formula points at a row or column that exists
/// on both sides (paired or moved) but under a different number: the cell
/// it read moved, so unchanged text now reads something else. With the maps
/// from the other side back to the base side, the same question asks whether
/// the address the formula reads now was another row or column before.
///
/// A range is judged by its ends, and an end that stays inside the range
/// does not count: rows reordered within a total leave it reading the same
/// cells.
pub fn retargets<'a>(tokens: &[Token], this: &SheetMap, sheet: impl Fn(&str) -> Option<&'a SheetMap>) -> bool {
    let row = |p: &Point| match *p {
        Point::Cell { row, .. } | Point::Row { row, .. } => Some(row),
        Point::Col { .. } => None,
    };
    let col = |p: &Point| match *p {
        Point::Cell { col, .. } | Point::Col { col, .. } => Some(col),
        Point::Row { .. } => None,
    };
    // Whether an end of the span `a..=b` went outside it. A single reference
    // is a span of one, so any other number is outside.
    let leaves = |a: Option<u32>, b: Option<u32>, axis: &AxisMap| {
        let (Some(a), Some(b)) = (a.or(b), b.or(a)) else { return false };
        let span = a.min(b)..=a.max(b);
        [a, b].iter().any(|&n| axis.exact(n).is_some_and(|m| !span.contains(&m)))
    };
    tokens.iter().any(|t| {
        let Token::Ref(r) = t else { return false };
        let Some(map) = (match &r.sheet {
            None => Some(this),
            Some(s) => sheet(s),
        }) else {
            return false;
        };
        let end = r.end.as_ref().unwrap_or(&r.start);
        leaves(row(&r.start), row(end), &map.rows) || leaves(col(&r.start), col(end), &map.cols)
    })
}

/// Whether carrying a formula through an alignment (`moved`, from
/// [`remap`] of `base`) made some range cover more rows or columns: rows or
/// columns were inserted inside it, which Excel would have absorbed.
pub fn widens(base: &[Token], moved: &[Token]) -> bool {
    let span = |p: &Point, q: &Point| -> (u32, u32) {
        match (*p, *q) {
            (Point::Cell { row: r1, col: c1, .. }, Point::Cell { row: r2, col: c2, .. }) => {
                (r1.abs_diff(r2), c1.abs_diff(c2))
            }
            (Point::Row { row: a, .. }, Point::Row { row: b, .. }) => (a.abs_diff(b), 0),
            (Point::Col { col: a, .. }, Point::Col { col: b, .. }) => (0, a.abs_diff(b)),
            _ => (0, 0),
        }
    };
    base.iter().zip(moved).any(|(b, m)| match (b, m) {
        (Token::Ref(rb), Token::Ref(rm)) => match (&rb.end, &rm.end) {
            (Some(eb), Some(em)) => {
                let (before, after) = (span(&rb.start, eb), span(&rm.start, em));
                after.0 > before.0 || after.1 > before.1
            }
            _ => false,
        },
        _ => false,
    })
}

/// The formula as written in the cell at `row`, `col`, in a position-free
/// form: relative parts become offsets (`R[-1]C[0]`) and `$` parts stay
/// absolute. Equal forms in neighbouring cells mean "the same formula
/// filled down or across", which is how a block of changes folds into one.
pub fn relative_form(tokens: &[Token], row: u32, col: u32) -> String {
    let point = |out: &mut String, p: &Point| match *p {
        Point::Cell { row: r, col: c, row_abs, col_abs } => {
            let rr = if row_abs { format!("R{r}") } else { format!("R[{}]", i64::from(r) - i64::from(row)) };
            let cc = if col_abs { format!("C{c}") } else { format!("C[{}]", i64::from(c) - i64::from(col)) };
            out.push_str(&rr);
            out.push_str(&cc);
        }
        Point::Row { row: r, abs } => {
            let _ = if abs { write!(out, "R{r}") } else { write!(out, "R[{}]", i64::from(r) - i64::from(row)) };
        }
        Point::Col { col: c, abs } => {
            let _ = if abs { write!(out, "C{c}") } else { write!(out, "C[{}]", i64::from(c) - i64::from(col)) };
        }
    };
    let mut out = String::new();
    for t in tokens {
        match t {
            Token::Ref(r) => {
                if let Some(sheet) = &r.sheet {
                    let _ = write!(out, "{:?}!", sheet.to_lowercase());
                }
                point(&mut out, &r.start);
                if let Some(end) = &r.end {
                    out.push(':');
                    point(&mut out, end);
                }
            }
            // Debug quoting escapes quotes, so distinct literals stay distinct.
            Token::Str(s) => {
                let _ = write!(out, "{s:?}");
            }
            Token::Text(s) if s.trim().is_empty() => {}
            Token::Text(s) => out.push_str(&s.to_lowercase()),
            Token::RefError(sheet) => {
                let _ = write!(out, "{:?}!#ref!", sheet.as_deref().map(str::to_lowercase));
            }
        }
    }
    out
}

fn offset(n: u32, by: i64, max: u32) -> Option<u32> {
    u32::try_from(i64::from(n) + by).ok().filter(|v| (1..=max).contains(v))
}

fn shift_point(p: &Point, rows: i64, cols: i64) -> Option<Point> {
    Some(match *p {
        Point::Cell { row, col, row_abs, col_abs } => Point::Cell {
            row: if row_abs { row } else { offset(row, rows, MAX_ROW)? },
            col: if col_abs { col } else { offset(col, cols, MAX_COL)? },
            row_abs,
            col_abs,
        },
        Point::Row { row, abs } => Point::Row { row: if abs { row } else { offset(row, rows, MAX_ROW)? }, abs },
        Point::Col { col, abs } => Point::Col { col: if abs { col } else { offset(col, cols, MAX_COL)? }, abs },
    })
}

/// The formula as it reads `rows` down and `cols` right of where it was
/// written: relative parts move, `$` parts stay. This is how a shared
/// formula reads in each cell after its anchor.
pub fn shift(tokens: &[Token], rows: i64, cols: i64) -> Vec<Token> {
    tokens
        .iter()
        .map(|t| match t {
            Token::Ref(r) => {
                let start = shift_point(&r.start, rows, cols);
                let end = r.end.as_ref().map(|e| shift_point(e, rows, cols));
                match (start, end) {
                    (Some(start), None) => Token::Ref(Reference { sheet: r.sheet.clone(), start, end: None }),
                    (Some(start), Some(Some(end))) => {
                        Token::Ref(Reference { sheet: r.sheet.clone(), start, end: Some(end) })
                    }
                    _ => Token::RefError(r.sheet.clone()),
                }
            }
            other => other.clone(),
        })
        .collect()
}

/// Maps base row (or column) numbers to the other side's numbers, from an
/// alignment of content rows.
///
/// Numbers between two paired rows (blank rows, cells no row item covered)
/// are matched in order with the other side's blank rows in the same gap,
/// skipping rows that were deleted, inserted, or moved there; when the two
/// gaps hold different numbers of blank rows, a number keeps its offset from
/// the pair before it, less the deletions in between.
#[derive(Debug, Clone, Default)]
pub struct AxisMap {
    /// Ordered `(base, other)` pairs.
    pairs: Vec<(u32, u32)>,
    /// Moved items: their own destination, outside the ordered pairs.
    moves: Vec<(u32, u32)>,
    /// Base numbers whose item was deleted, sorted.
    deleted: Vec<u32>,
    /// Base numbers that are not blank but unpaired: deleted or moved away.
    base_taken: Vec<u32>,
    /// Other numbers that are not blank but unpaired: inserted or moved here.
    other_taken: Vec<u32>,
    max: u32,
}

impl AxisMap {
    pub fn new(
        mut pairs: Vec<(u32, u32)>,
        mut moves: Vec<(u32, u32)>,
        mut deleted: Vec<u32>,
        inserted: Vec<u32>,
        max: u32,
    ) -> Self {
        pairs.sort_unstable();
        moves.sort_unstable();
        deleted.sort_unstable();
        let mut base_taken: Vec<u32> = deleted.iter().copied().chain(moves.iter().map(|m| m.0)).collect();
        let mut other_taken: Vec<u32> = inserted.into_iter().chain(moves.iter().map(|m| m.1)).collect();
        base_taken.sort_unstable();
        other_taken.sort_unstable();
        Self { pairs, moves, deleted, base_taken, other_taken, max }
    }

    /// The identity: every number maps to itself.
    pub fn identity(max: u32) -> Self {
        Self { max, ..Self::default() }
    }

    /// Where `n` went when its own row or column was paired or moved; `None`
    /// for numbers the alignment only interpolates or deleted.
    pub fn exact(&self, n: u32) -> Option<u32> {
        if let Ok(i) = self.pairs.binary_search_by_key(&n, |p| p.0) {
            return Some(self.pairs[i].1);
        }
        self.moves.binary_search_by_key(&n, |m| m.0).ok().map(|i| self.moves[i].1)
    }

    /// How many of `sorted` lie strictly between `lo` and `hi`.
    fn between(sorted: &[u32], lo: u32, hi: u32) -> u32 {
        let a = sorted.partition_point(|&x| x <= lo);
        let b = sorted.partition_point(|&x| x < hi);
        u32::try_from(b.saturating_sub(a)).unwrap_or(u32::MAX)
    }

    /// Where base number `n` is on the other side, or `None` if it was deleted.
    pub fn map(&self, n: u32) -> Option<u32> {
        if self.deleted.binary_search(&n).is_ok() {
            return None;
        }
        if let Ok(i) = self.moves.binary_search_by_key(&n, |m| m.0) {
            return Some(self.moves[i].1);
        }
        if let Ok(i) = self.pairs.binary_search_by_key(&n, |p| p.0) {
            return Some(self.pairs[i].1);
        }
        if self.pairs.is_empty() && self.base_taken.is_empty() && self.other_taken.is_empty() {
            return Some(n);
        }
        let k = self.pairs.partition_point(|p| p.0 < n);
        let (b1, o1) = if k == 0 { (0, 0) } else { self.pairs[k - 1] };
        // n's place among the base gap's blank numbers, 0-based.
        let index = n - b1 - 1 - Self::between(&self.base_taken, b1, n);
        if let Some(&(b2, o2)) = self.pairs.get(k) {
            let base_blanks = (b2 - b1 - 1) - Self::between(&self.base_taken, b1, b2);
            let other_blanks = (o2.saturating_sub(o1 + 1)).saturating_sub(Self::between(&self.other_taken, o1, o2));
            if base_blanks == other_blanks {
                // The index-th other number after o1 that is not taken.
                let mut x = o1 + 1 + index;
                loop {
                    let next = o1 + 1 + index + Self::between(&self.other_taken, o1, x + 1);
                    if next == x {
                        return Some(x);
                    }
                    x = next;
                }
            }
        }
        offset(o1, i64::from(index) + 1, self.max)
    }

    /// A range corner: like [`map`](Self::map), but a row that moved
    /// elsewhere leaves the range as a deleted one does, rather than
    /// dragging the corner to where it went.
    fn corner(&self, n: u32) -> Option<u32> {
        if self.moves.binary_search_by_key(&n, |m| m.0).is_ok() { None } else { self.map(n) }
    }

    /// The first surviving number at or after `n`, up to `limit`, mapped.
    fn map_forward(&self, n: u32, limit: u32) -> Option<u32> {
        (n..=limit).take(self.base_taken.len() + 1).find_map(|k| self.corner(k))
    }

    /// The last surviving number at or before `n`, down to `limit`, mapped.
    fn map_backward(&self, n: u32, limit: u32) -> Option<u32> {
        (limit..=n).rev().take(self.base_taken.len() + 1).find_map(|k| self.corner(k))
    }
}

/// How references to one base sheet carry over to the other workbook.
#[derive(Debug, Clone)]
pub struct SheetMap {
    /// The sheet's name in the other workbook, when it was renamed.
    pub renamed_to: Option<String>,
    pub rows: AxisMap,
    pub cols: AxisMap,
}

/// A cell range carried through an alignment the way Excel moves a range:
/// its corners follow their rows and columns, a deleted corner moves inward,
/// and a range with nothing left is `None`.
pub fn map_range(r: crate::cellref::Range, map: &SheetMap) -> Option<crate::cellref::Range> {
    let corner = |row, col| Point::Cell { row, col, row_abs: false, col_abs: false };
    let (a, b) = remap_range(&corner(r.first_row, r.first_col), &corner(r.last_row, r.last_col), &map.rows, &map.cols)?;
    match (a, b) {
        (Point::Cell { row: r1, col: c1, .. }, Point::Cell { row: r2, col: c2, .. }) => Some(crate::cellref::Range {
            first_row: r1.min(r2),
            first_col: c1.min(c2),
            last_row: r1.max(r2),
            last_col: c1.max(c2),
        }),
        _ => None,
    }
}

fn remap_point(p: &Point, rows: &AxisMap, cols: &AxisMap) -> Option<Point> {
    Some(match *p {
        Point::Cell { row, col, row_abs, col_abs } => {
            Point::Cell { row: rows.map(row)?, col: cols.map(col)?, row_abs, col_abs }
        }
        Point::Row { row, abs } => Point::Row { row: rows.map(row)?, abs },
        Point::Col { col, abs } => Point::Col { col: cols.map(col)?, abs },
    })
}

/// A range's corners after the alignment: a deleted corner moves inward to
/// the nearest surviving row or column, as Excel shrinks a range when rows
/// at its edge are deleted; a range with nothing left is `#REF!`.
fn remap_range(start: &Point, end: &Point, rows: &AxisMap, cols: &AxisMap) -> Option<(Point, Point)> {
    let span = |a: u32, b: u32, axis: &AxisMap| -> Option<(u32, u32)> {
        let (lo, hi) = (a.min(b), a.max(b));
        Some((axis.map_forward(lo, hi)?, axis.map_backward(hi, lo)?))
    };
    match (*start, *end) {
        (
            Point::Cell { row: r1, col: c1, row_abs: ra1, col_abs: ca1 },
            Point::Cell { row: r2, col: c2, row_abs: ra2, col_abs: ca2 },
        ) => {
            let (r1, r2) = span(r1, r2, rows)?;
            let (c1, c2) = span(c1, c2, cols)?;
            Some((
                Point::Cell { row: r1, col: c1, row_abs: ra1, col_abs: ca1 },
                Point::Cell { row: r2, col: c2, row_abs: ra2, col_abs: ca2 },
            ))
        }
        (Point::Row { row: a, abs: x }, Point::Row { row: b, abs: y }) => {
            let (a, b) = span(a, b, rows)?;
            Some((Point::Row { row: a, abs: x }, Point::Row { row: b, abs: y }))
        }
        (Point::Col { col: a, abs: x }, Point::Col { col: b, abs: y }) => {
            let (a, b) = span(a, b, cols)?;
            Some((Point::Col { col: a, abs: x }, Point::Col { col: b, abs: y }))
        }
        _ => None,
    }
}

/// The formula as Excel would have adjusted it for the structural changes
/// an alignment describes. `this` is how references without a sheet carry
/// over; `sheet` looks up other sheets by their name in the base workbook
/// (case-insensitive). References to sheets it does not know stay as they
/// are.
pub fn remap<'a>(tokens: &[Token], this: &SheetMap, sheet: impl Fn(&str) -> Option<&'a SheetMap>) -> Vec<Token> {
    tokens
        .iter()
        .map(|t| {
            let Token::Ref(r) = t else { return t.clone() };
            let (map, name) = match &r.sheet {
                None => (Some(this), None),
                Some(s) => {
                    let found = sheet(s);
                    (found, Some(found.and_then(|m| m.renamed_to.clone()).unwrap_or_else(|| s.clone())))
                }
            };
            let Some(map) = map else { return t.clone() };
            let moved = match &r.end {
                None => remap_point(&r.start, &map.rows, &map.cols).map(|p| (p, None)),
                Some(end) => remap_range(&r.start, end, &map.rows, &map.cols).map(|(s, e)| (s, Some(e))),
            };
            match moved {
                Some((start, end)) => Token::Ref(Reference { sheet: name, start, end }),
                None => Token::RefError(name),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{AxisMap, Point, Reference, SheetMap, Token, remap, render, same, shift, tokenize};

    fn round_trip(f: &str) {
        assert_eq!(render(&tokenize(f)), f, "{f}");
    }

    /// Every string of up to four of the characters the tokenizer branches
    /// on: none may panic, in the tokenizer or in what reads its tokens.
    #[test]
    fn no_formula_text_panics() {
        let alphabet = ['[', ']', '\'', '"', '!', '#', ':', '$', 'A', '1', '(', ' ', 'é'];
        let mut texts = vec![String::new()];
        for _ in 0..4 {
            let longer: Vec<String> =
                texts.iter().flat_map(|t| alphabet.iter().map(move |c| format!("{t}{c}"))).collect();
            texts.extend(longer);
            texts.sort_unstable();
            texts.dedup();
        }
        let map = SheetMap {
            renamed_to: None,
            rows: AxisMap::new(vec![(1, 2)], Vec::new(), Vec::new(), vec![1], 100),
            cols: AxisMap::identity(100),
        };
        for text in &texts {
            let tokens = tokenize(text);
            let _ = render(&tokens);
            let _ = super::relative_form(&tokens, 1, 1);
            let _ = render(&shift(&tokens, -1, -1));
            let _ = render(&remap(&tokens, &map, |_| None));
        }
    }

    #[test]
    fn formulas_round_trip() {
        for f in [
            "SUM(D10:D19)",
            "$A$1+B$2*$C3",
            "'Sheet 1'!A1+재무상태표!$B$5",
            "VLOOKUP(A2,Rates!$A:$C,3,FALSE)",
            "SUM(3:5)",
            "IF(A1=\"a \"\"quoted\"\" text\",#N/A,1.5E+3)",
            "Table1[[#This Row],[Amount]]*2",
            "[1]Prices!A1",
            "LOG10(A1)+ATAN2(B1,C1)",
            "_xlfn.XLOOKUP(A1,B:B,C:C)",
            "SUM(A1:A5 B2:B6)",
        ] {
            round_trip(f);
        }
    }

    #[test]
    fn references_are_recognized() {
        let t = tokenize("SUM('My Sheet'!B2:C3)+Rate+A1");
        let refs: Vec<&Reference> =
            t.iter().filter_map(|t| if let Token::Ref(r) = t { Some(r) } else { None }).collect();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].sheet.as_deref(), Some("My Sheet"));
        assert!(matches!(refs[0].end, Some(Point::Cell { row: 3, col: 3, .. })));
        assert!(t.contains(&Token::Text("Rate".into())));
        // Bare numbers and names that only look like references are not.
        assert!(!tokenize("2+3").iter().any(|t| matches!(t, Token::Ref(_))));
        assert!(!tokenize("A+B").iter().any(|t| matches!(t, Token::Ref(_))));
    }

    #[test]
    fn shift_moves_relative_parts_only() {
        let t = tokenize("A1*$B$1+C$2+$D3+SUM(E1:E3)");
        assert_eq!(render(&shift(&t, 2, 1)), "B3*$B$1+D$2+$D5+SUM(F3:F5)");
        assert_eq!(render(&shift(&tokenize("A1"), -1, 0)), "#REF!");
    }

    #[test]
    fn three_d_and_external_references_shift_but_never_remap() {
        for f in ["SUM(Jan:Dec!B5)", "[1]Data!B1*2", "SUM('Sheet 1:Sheet 3'!A1)", "Notes!#REF!+#REF!"] {
            round_trip(f);
        }
        assert_eq!(render(&shift(&tokenize("SUM(Jan:Dec!B5)+[1]Data!B1"), 1, 0)), "SUM(Jan:Dec!B6)+[1]Data!B2");
        let map = rows_inserted_at(1, 1);
        assert_eq!(
            render(&remap(&tokenize("SUM(Jan:Dec!B5)+[1]Data!B1+B1"), &map, |_| None)),
            "SUM(Jan:Dec!B5)+[1]Data!B1+B2"
        );
    }

    #[test]
    fn sheet_names_that_read_as_references_are_quoted() {
        let r = |sheet: &str| {
            render(&[Token::Ref(Reference {
                sheet: Some(sheet.into()),
                start: Point::Cell { row: 1, col: 1, row_abs: false, col_abs: false },
                end: None,
            })])
        };
        assert_eq!(r("R2C3"), "'R2C3'!A1");
        assert_eq!(r("TRUE"), "'TRUE'!A1");
        assert_eq!(r("Notes"), "Notes!A1");
    }

    #[test]
    fn a_moved_row_at_a_range_edge_leaves_the_range() {
        // Base rows 2..=6 summed; row 2 moved below the total (to 9); rows 3..=6 move up one.
        let pairs = vec![(1, 1), (3, 2), (4, 3), (5, 4), (6, 5), (7, 6)];
        let map = SheetMap {
            renamed_to: None,
            rows: AxisMap::new(pairs, vec![(2, 9)], vec![], vec![], 1_048_576),
            cols: AxisMap::identity(16_384),
        };
        // The corner skips the moved row instead of following it to row 9.
        assert_eq!(render(&remap(&tokenize("SUM(B2:B6)"), &map, |_| None)), "SUM(B2:B5)");
        // A single reference to the moved row follows it.
        assert_eq!(render(&remap(&tokenize("B2"), &map, |_| None)), "B9");
    }

    #[test]
    fn blank_rows_follow_the_gap_they_sit_in() {
        // Base rows 1..=4 content, 5 blank, 6 content; other inserts a row at 5.
        let map = AxisMap::new(vec![(1, 1), (2, 2), (3, 3), (4, 4), (6, 7)], vec![], vec![], vec![5], 100);
        assert_eq!(map.map(5), Some(6));
        // Base row 4 deleted; blank row 5 closes up.
        let map = AxisMap::new(vec![(1, 1), (2, 2), (3, 3), (6, 5)], vec![], vec![4], vec![], 100);
        assert_eq!(map.map(5), Some(4));
        assert_eq!(map.map(4), None);
    }

    #[test]
    fn sameness_ignores_spacing_and_case() {
        assert!(same(&tokenize("sum( a1 , B2 )"), &tokenize("SUM(A1,B2)")));
        assert!(!same(&tokenize("SUM(A1,B2)"), &tokenize("SUM(A1,B3)")));
        assert!(!same(&tokenize("\"a\""), &tokenize("\"A\"")));
    }

    fn rows_inserted_at(at: u32, count: u32) -> SheetMap {
        // Content rows 1..=50 on the base side; `count` rows inserted before `at`.
        let pairs = (1..=50).map(|r| (r, if r >= at { r + count } else { r })).collect();
        SheetMap {
            renamed_to: None,
            rows: AxisMap::new(pairs, vec![], vec![], vec![], 1_048_576),
            cols: AxisMap::identity(16_384),
        }
    }

    #[test]
    fn remap_follows_inserted_rows() {
        let map = rows_inserted_at(10, 1);
        let adjust = |f: &str| render(&remap(&tokenize(f), &map, |_| None));
        assert_eq!(adjust("$D$5+D30"), "$D$5+D31");
        // A range spanning the insertion grows, as in Excel.
        assert_eq!(adjust("SUM(D5:D19)"), "SUM(D5:D20)");
        assert_eq!(adjust("SUM(2:20)"), "SUM(2:21)");
    }

    #[test]
    fn remap_shrinks_or_breaks_on_deleted_rows() {
        let pairs = (1..=20).filter(|r| *r != 10).map(|r| (r, if r > 10 { r - 1 } else { r })).collect();
        let map = SheetMap {
            renamed_to: None,
            rows: AxisMap::new(pairs, vec![], vec![10], vec![], 1_048_576),
            cols: AxisMap::identity(16_384),
        };
        let adjust = |f: &str| render(&remap(&tokenize(f), &map, |_| None));
        assert_eq!(adjust("A10"), "#REF!");
        assert_eq!(adjust("SUM(A10:A15)"), "SUM(A10:A14)");
        assert_eq!(adjust("SUM(A5:A10)"), "SUM(A5:A9)");
        assert_eq!(adjust("A15"), "A14");
    }

    #[test]
    fn remap_follows_other_sheets_and_renames() {
        let notes = SheetMap { renamed_to: Some("Notes 2025".into()), ..rows_inserted_at(1, 2) };
        let here = SheetMap { renamed_to: None, rows: AxisMap::identity(1_048_576), cols: AxisMap::identity(16_384) };
        let lookup = |s: &str| (s.eq_ignore_ascii_case("notes")).then_some(&notes);
        assert_eq!(render(&remap(&tokenize("Notes!B2+A1+Other!A1"), &here, lookup)), "'Notes 2025'!B4+A1+Other!A1");
    }
}
