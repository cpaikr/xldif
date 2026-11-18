//! Which parts of two texts differ, so that the report can mark the changed
//! words of a long cell instead of colouring all of it.
//!
//! Texts are compared token by token: a run of letters and digits, a run of
//! whitespace, or one other character. That keeps a word of prose and a
//! reference or name of a formula whole.

use serde::Serialize;

/// The token table a comparison may fill, in cells (four bytes each).
/// Beyond it, only what the texts share at their start and end is found,
/// and everything between is marked.
const MAX_TABLE: usize = 4_000_000;
/// The table cells all of one report's comparisons may fill: about a
/// hundred of the largest. A workbook with thousands of long changed
/// texts would otherwise take minutes to report. Once it is spent, the
/// remaining texts keep only their shared start and end.
const REPORT_TABLE: usize = 400_000_000;

/// Texts that share fewer tokens than this, whitespace aside, are not
/// marked: a short text found whole inside an unrelated longer one (`Q1`
/// in a sentence that mentions it) would otherwise count as shared.
const MIN_SHARED: usize = 3;

/// The changed parts of two texts, in order: `[start, end)` ranges in UTF-16
/// code units, which is how the report's script indexes a string.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Marks {
    pub base: Vec<(u32, u32)>,
    pub other: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

struct Token<'a> {
    text: &'a str,
    class: Class,
    /// Where the token starts and ends, in UTF-16 code units.
    span: (u32, u32),
}

impl Token<'_> {
    const fn len(&self) -> u32 {
        self.span.1 - self.span.0
    }
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out: Vec<Token<'_>> = Vec::new();
    let (mut start, mut units) = (0, 0u32);
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let class = if c.is_alphanumeric() {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        };
        if out.last().is_none_or(|t| t.class != class || class == Class::Other) {
            start = at;
            out.push(Token { text: "", class, span: (units, units) });
        }
        // A text's length in UTF-16 units is at most its length in bytes,
        // and a cell's text is far below 4 GiB.
        units += u32::try_from(c.len_utf16()).expect("one or two units");
        let end = chars.peek().map_or(text.len(), |(next, _)| *next);
        let token = out.last_mut().expect("pushed above");
        token.text = &text[start..end];
        token.span.1 = units;
    }
    out
}

/// For each token of each text, whether the other text has it too, in
/// the same order: a longest common subsequence, after what the texts share
/// at their start and end. The table may have `limit` cells; the last value
/// is how many it had.
fn kept(base: &[Token<'_>], other: &[Token<'_>], limit: usize) -> (Vec<bool>, Vec<bool>, usize) {
    let same = |x: &Token<'_>, y: &Token<'_>| x.text == y.text;
    let head = base.iter().zip(other).take_while(|(x, y)| same(x, y)).count();
    let tail = base[head..].iter().rev().zip(other[head..].iter().rev()).take_while(|(x, y)| same(x, y)).count();
    let (mid_a, mid_b) = (&base[head..base.len() - tail], &other[head..other.len() - tail]);
    let (mut keep_a, mut keep_b) = (vec![true; base.len()], vec![true; other.len()]);
    keep_a[head..head + mid_a.len()].fill(false);
    keep_b[head..head + mid_b.len()].fill(false);
    let (rows, cols) = (mid_a.len(), mid_b.len());
    let table = rows.saturating_mul(cols);
    if table == 0 || table > limit {
        return (keep_a, keep_b, 0);
    }
    // common[i * width + j]: the longest common subsequence of
    // mid_a[i..] and mid_b[j..].
    let width = cols + 1;
    let mut common = vec![0u32; (rows + 1) * width];
    for i in (0..rows).rev() {
        for j in (0..cols).rev() {
            common[i * width + j] = if same(&mid_a[i], &mid_b[j]) {
                common[(i + 1) * width + j + 1] + 1
            } else {
                common[(i + 1) * width + j].max(common[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < rows && j < cols {
        if same(&mid_a[i], &mid_b[j]) {
            keep_a[head + i] = true;
            keep_b[head + j] = true;
            i += 1;
            j += 1;
        } else if common[(i + 1) * width + j] >= common[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (keep_a, keep_b, table)
}

/// Moves each run of changed tokens as far towards the end of the text as
/// it can go while the tokens kept stay the same text: an added `,0)`
/// reads better than the equally true `),0`, and an added word takes the
/// space after it, not the one before.
fn slide(tokens: &[Token<'_>], kept: &mut [bool]) {
    let mut start = 0;
    while start < tokens.len() {
        if kept[start] {
            start += 1;
            continue;
        }
        let mut end = start + kept[start..].iter().take_while(|k| !**k).count();
        while end < tokens.len() && kept[end] && tokens[start].text == tokens[end].text {
            kept.swap(start, end);
            start += 1;
            end += 1;
        }
        start = end;
    }
}

/// The changed tokens as ranges. Whitespace both texts have, between two
/// changed tokens, is marked with them: a rewritten phrase reads as one
/// mark, not one per word.
fn ranges(tokens: &[Token<'_>], kept: &[bool]) -> Vec<(u32, u32)> {
    let changed = |i: usize| {
        !kept[i]
            || (tokens[i].class == Class::Space && i > 0 && !kept[i - 1] && kept.get(i + 1).is_some_and(|next| !next))
    };
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !changed(i) {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.1 == token.span.0 => last.1 = token.span.1,
            _ => out.push(token.span),
        }
    }
    out
}

/// The parts of `base` and `other` that differ, or `None` when marking
/// parts would not help a reader: the texts are equal, one is empty, or
/// they share less than half the words of the shorter one or fewer than
/// [`MIN_SHARED`] tokens (two different texts, where the few tokens they
/// happen to share would only be noise).
///
/// Words count by their length, so that a shared `on the` does not make
/// two sentences alike.
fn marks_within(base: &str, other: &str, left: &mut usize) -> Option<Marks> {
    if base == other {
        return None;
    }
    let (a, b) = (tokens(base), tokens(other));
    let (mut keep_a, mut keep_b, used) = kept(&a, &b, MAX_TABLE.min(*left));
    *left -= used;
    slide(&a, &mut keep_a);
    slide(&b, &mut keep_b);
    let words = |tokens: &[Token<'_>], keep: Option<&[bool]>| -> u32 {
        tokens
            .iter()
            .enumerate()
            .filter(|(i, t)| t.class == Class::Word && keep.is_none_or(|k| k[*i]))
            .map(|(_, t)| t.len())
            .sum()
    };
    let fewer = words(&a, None).min(words(&b, None));
    let shared = a.iter().zip(&keep_a).filter(|(t, keep)| **keep && t.class != Class::Space).count();
    if fewer == 0 || words(&a, Some(&keep_a)) * 2 < fewer || shared < MIN_SHARED {
        return None;
    }
    Some(Marks { base: ranges(&a, &keep_a), other: ranges(&b, &keep_b) })
}

/// Marks the texts of one report, within [`REPORT_TABLE`].
pub struct Marker {
    left: usize,
}

impl Marker {
    pub const fn new() -> Self {
        Self { left: REPORT_TABLE }
    }

    /// See [`marks_within`].
    pub fn marks(&mut self, base: &str, other: &str) -> Option<Marks> {
        marks_within(base, other, &mut self.left)
    }
}

#[cfg(test)]
mod tests {
    use super::{Marker, Marks};

    fn marks(base: &str, other: &str) -> Option<Marks> {
        Marker::new().marks(base, other)
    }

    /// The marked parts of each text, as text.
    fn marked(base: &str, other: &str) -> Option<(Vec<String>, Vec<String>)> {
        let cut = |text: &str, runs: &[(u32, u32)]| -> Vec<String> {
            let units: Vec<u16> = text.encode_utf16().collect();
            runs.iter().map(|(s, e)| String::from_utf16(&units[*s as usize..*e as usize]).unwrap()).collect()
        };
        marks(base, other).map(|m| (cut(base, &m.base), cut(other, &m.other)))
    }

    fn texts(base: &[&str], other: &[&str]) -> (Vec<String>, Vec<String>) {
        let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
        (own(base), own(other))
    }

    #[test]
    fn an_appended_paragraph_is_the_only_mark() {
        let base = "1. Scope\nThe note covers bonds.";
        let other = "1. Scope\nThe note covers bonds.\n\n2. Model\nA tree is used.";
        assert_eq!(marked(base, other), Some(texts(&[], &["\n\n2. Model\nA tree is used."])));
        assert_eq!(marked(other, base), Some(texts(&["\n\n2. Model\nA tree is used."], &[])));
    }

    #[test]
    fn changed_words_are_marked_inside_the_text() {
        assert_eq!(
            marked("The rate is 4.5% at year end", "The rate was 4.7% at the year end"),
            Some(texts(&["is", "5"], &["was", "7", "the "]))
        );
    }

    #[test]
    fn a_rewritten_phrase_is_one_mark() {
        assert_eq!(
            marked("Paid in cash by the buyer on closing", "Paid by bank transfer by the buyer on closing"),
            Some(texts(&["in cash"], &["by bank transfer"]))
        );
    }

    #[test]
    fn a_formula_is_marked_by_reference_and_name() {
        assert_eq!(
            marked("=SUM(B2:B10)*Rates!$C$4", "=SUM(B2:B11)*Rates!$C$5"),
            Some(texts(&["B10", "4"], &["B11", "5"]))
        );
        assert_eq!(marked("=SUM(B2:B10)", "=ROUND(SUM(B2:B10),0)"), Some(texts(&[], &["ROUND(", ",0)"])));
    }

    #[test]
    fn ranges_count_utf16_units() {
        // Hangul is three bytes and one unit; the emoji is four bytes and two units.
        let m = marks("전환권 😀 평가 완료", "전환권 😀 재평가 완료").unwrap();
        assert_eq!(m, Marks { base: vec![(7, 9)], other: vec![(7, 10)] });
        assert_eq!(marked("가 나 😀 다", "가 나 😃 다"), Some(texts(&["😀"], &["😃"])));
    }

    #[test]
    fn whitespace_changes_are_marked() {
        assert_eq!(marked("total due\nnet", "total due net"), Some(texts(&["\n"], &[" "])));
    }

    #[test]
    fn different_texts_are_not_marked() {
        assert_eq!(marks("same", "same"), None);
        assert_eq!(marks("", "added"), None);
        assert_eq!(marks("1,234", "1,235"), None);
        assert_eq!(marks("The cat sat on the mat", "A dog lay on the rug"), None);
        assert_eq!(marks("---", "n/a"), None);
        // The shorter text is all in the longer one, and is still another text.
        assert_eq!(marks("Q1", "Revenue for Q1 rose sharply on export demand"), None);
        assert_eq!(marks("N/A", "A new note"), None);
    }

    #[test]
    fn a_report_that_has_spent_its_work_keeps_shared_ends() {
        let (base, other) =
            ("alpha beta gamma delta one two three four five six", "alpha beta gamma delta one 2 three four 5 six");
        let mut marker = Marker { left: 60 };
        // 7 × 7 tokens between the shared ends: within the bound once.
        assert_eq!(marker.marks(base, other).unwrap().base, [(27, 30), (42, 46)]);
        assert_eq!(marker.left, 11);
        assert_eq!(marker.marks(base, other).unwrap().base, [(27, 46)]);
        assert_eq!(marker.left, 11);
    }

    #[test]
    fn texts_too_long_to_compare_keep_their_shared_ends() {
        let word = |i: usize, salt: &str| format!("w{i}{salt} ");
        let middle = |salt: &str| (0..1500).map(|i| word(i, salt)).collect::<String>();
        let (base, other) = (format!("start {}end", middle("a")), format!("start {}end", middle("b")));
        let m = marks(&format!("{base} {base}"), &format!("{base} {other}")).unwrap();
        assert_eq!((m.base.len(), m.other.len()), (1, 1));
        let first = u32::try_from(base.len() + 1 + "start ".len()).unwrap();
        assert_eq!(m.base[0].0, first);
        assert_eq!(m.other[0].0, first);
    }
}
