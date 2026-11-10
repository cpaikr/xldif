//! Sequence alignment for rows and columns.
//!
//! An [`Item`] is one row or column: an optional label key and a sorted
//! multiset of content tokens. Alignment runs in three passes:
//!
//! 1. **Keys.** Histogram matching on label keys pairs items whose keys
//!    agree, anchoring on the rarest keys first so repeated labels such as
//!    "subtotal" cannot pull unrelated sections together. Pairs never cross.
//! 2. **Gaps.** Between key anchors, items are paired by similarity (label
//!    likeness and token overlap) with a weighted longest-common-subsequence,
//!    again without crossing. Below [`MIN_SIMILARITY`] items stay unpaired.
//! 3. **Moves.** Unpaired items on both sides with the same key and the same
//!    tokens, unique among the unpaired, are reported as moved.
//!
//! Work is bounded: a gap too large for the similarity pass is paired by
//! position and the result says so.
//!
//! Every tie is broken by index order, so the result depends only on the
//! input, never on hashing or iteration order.

// Index names (i, j for base and other positions) read best in this algorithm.
#![allow(clippy::many_single_char_names)]

use std::collections::HashMap;

/// Pairs below this similarity are not considered the same row or column.
pub const MIN_SIMILARITY: f64 = 0.5;
/// Keys occurring more often than this in a region are not used as anchors.
const MAX_ANCHOR_OCCURRENCES: usize = 64;
/// The largest gap (base items × other items) the similarity pass examines.
pub const MAX_GAP_WORK: usize = 4_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The normalized label, when the item has one.
    pub key: Option<String>,
    /// Content tokens, sorted; equal tokens repeat.
    pub tokens: Vec<u64>,
    /// Which positions hold content (for a row: its aligned columns), as a
    /// hash; 0 when unknown. Equal-sized gaps whose items share shapes pair
    /// by position, so a row whose label alone was edited stays one row.
    pub shape: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Pair a gap of equal size by position when similarity finds nothing.
    /// Suits columns, whose headers often change wholesale between periods.
    pub positional_equal_gaps: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alignment {
    /// Ordered, non-crossing `(base, other)` index pairs.
    pub pairs: Vec<(usize, usize)>,
    /// Items that moved: `(base, other)`, outside the ordered pairs.
    pub moves: Vec<(usize, usize)>,
    /// A gap exceeded [`MAX_GAP_WORK`] and was paired by position.
    pub fell_back: bool,
}

impl Alignment {
    /// Pairs by position: the alignment of `--align position`.
    pub fn positional(base: usize, other: usize) -> Self {
        Self { pairs: (0..base.min(other)).map(|i| (i, i)).collect(), ..Self::default() }
    }
}

pub fn align(base: &[Item], other: &[Item], options: Options) -> Alignment {
    let mut out = Alignment { pairs: match_keys(base, other), ..Alignment::default() };
    let anchors = out.pairs.clone();
    let mut filled = Vec::new();
    let (mut a, mut b) = (0, 0);
    for &(i, j) in anchors.iter().chain(std::iter::once(&(base.len(), other.len()))) {
        if i > a || j > b {
            fill_gap(base, other, (a, i), (b, j), options, &mut filled, &mut out.fell_back);
        }
        (a, b) = (i + 1, j + 1);
    }
    out.pairs.extend(filled);
    out.pairs.sort_unstable();
    out.moves = find_moves(base, other, &out.pairs);
    out
}

/// Histogram matching on keys: trims equal ends, then anchors each region on
/// its rarest shared key and recurses on both sides of the anchor.
fn match_keys(base: &[Item], other: &[Item]) -> Vec<(usize, usize)> {
    fn key(items: &[Item], i: usize) -> Option<&str> {
        items[i].key.as_deref()
    }
    let same = |i: usize, j: usize| key(base, i).is_some() && key(base, i) == key(other, j);
    let mut pairs = Vec::new();
    let mut regions = vec![(0, base.len(), 0, other.len())];
    while let Some((mut alo, mut ahi, mut blo, mut bhi)) = regions.pop() {
        while alo < ahi && blo < bhi && same(alo, blo) {
            pairs.push((alo, blo));
            alo += 1;
            blo += 1;
        }
        while alo < ahi && blo < bhi && same(ahi - 1, bhi - 1) {
            pairs.push((ahi - 1, bhi - 1));
            ahi -= 1;
            bhi -= 1;
        }
        if alo == ahi || blo == bhi {
            continue;
        }
        // Patience: keys unique on both sides anchor the region, keeping the
        // longest run of them that stays in order, so one moved row cannot
        // pull the rows around it out of line.
        let anchors = patience_anchors(base, other, (alo, ahi), (blo, bhi));
        if !anchors.is_empty() {
            let (mut a, mut b) = (alo, blo);
            for &(i, j) in &anchors {
                regions.push((a, i, b, j));
                pairs.push((i, j));
                (a, b) = (i + 1, j + 1);
            }
            regions.push((a, ahi, b, bhi));
            continue;
        }
        // Otherwise the rarest repeated key anchors, with its longest run.
        let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
        for i in alo..ahi {
            if let Some(k) = key(base, i) {
                positions.entry(k).or_default().push(i);
            }
        }
        // (occurrences, -run length, i, j): the rarest, then longest, then first.
        let mut best: Option<(usize, std::cmp::Reverse<usize>, usize, usize, usize)> = None;
        for j in blo..bhi {
            let Some(found) = key(other, j).and_then(|k| positions.get(k)) else { continue };
            if found.len() > MAX_ANCHOR_OCCURRENCES {
                continue;
            }
            for &i in found {
                let (mut s, mut t) = (i, j);
                while s > alo && t > blo && same(s - 1, t - 1) {
                    s -= 1;
                    t -= 1;
                }
                let mut len = 0;
                while s + len < ahi && t + len < bhi && same(s + len, t + len) {
                    len += 1;
                }
                let candidate = (found.len(), std::cmp::Reverse(len), s, t, len);
                if best.is_none_or(|b| (candidate.0, candidate.1, candidate.2, candidate.3) < (b.0, b.1, b.2, b.3)) {
                    best = Some(candidate);
                }
            }
        }
        if let Some((_, _, s, t, len)) = best {
            pairs.extend((0..len).map(|k| (s + k, t + k)));
            regions.push((alo, s, blo, t));
            regions.push((s + len, ahi, t + len, bhi));
        }
    }
    pairs.sort_unstable();
    pairs
}

/// Pairs of items whose key occurs exactly once on each side of the region,
/// reduced to the longest subsequence increasing on both sides.
fn patience_anchors(
    base: &[Item],
    other: &[Item],
    (alo, ahi): (usize, usize),
    (blo, bhi): (usize, usize),
) -> Vec<(usize, usize)> {
    // key → (count in base, last base index, count in other, last other index)
    let mut seen: HashMap<&str, (usize, usize, usize, usize)> = HashMap::new();
    for (i, item) in base.iter().enumerate().take(ahi).skip(alo) {
        if let Some(k) = item.key.as_deref() {
            let e = seen.entry(k).or_default();
            e.0 += 1;
            e.1 = i;
        }
    }
    for (j, item) in other.iter().enumerate().take(bhi).skip(blo) {
        if let Some(k) = item.key.as_deref()
            && let Some(e) = seen.get_mut(k)
        {
            e.2 += 1;
            e.3 = j;
        }
    }
    let mut unique: Vec<(usize, usize)> =
        seen.into_values().filter(|e| e.0 == 1 && e.2 == 1).map(|e| (e.1, e.3)).collect();
    unique.sort_unstable();
    let order: Vec<usize> = unique.iter().map(|p| p.1).collect();
    let mut keep = longest_increasing(&order);
    keep.sort_unstable();
    keep.into_iter().map(|k| unique[k]).collect()
}

/// Positions of one longest strictly increasing subsequence of `seq`.
pub fn longest_increasing(seq: &[usize]) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &x) in seq.iter().enumerate() {
        let pos = tails.partition_point(|&t| seq[t] < x);
        if pos > 0 {
            prev[i] = tails[pos - 1];
        }
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut keep = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(i) = at {
        keep.push(i);
        at = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    keep.reverse();
    keep
}

/// Bigram Dice similarity of two labels, 0..=1.
fn label_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let grams = |s: &str| {
        let chars: Vec<char> = s.chars().collect();
        let mut g: Vec<(char, char)> = chars.windows(2).map(|w| (w[0], w[1])).collect();
        g.sort_unstable();
        g
    };
    let (ga, gb) = (grams(a), grams(b));
    if ga.is_empty() || gb.is_empty() {
        return 0.0;
    }
    2.0 * common(&ga, &gb) as f64 / (ga.len() + gb.len()) as f64
}

/// The size of the multiset intersection of two sorted slices.
fn common<T: Ord>(a: &[T], b: &[T]) -> usize {
    let (mut i, mut j, mut n) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    n
}

/// How alike two items are, 0..=1: token Jaccard, averaged with label
/// likeness when both have labels.
pub fn similarity(a: &Item, b: &Item) -> f64 {
    let union = a.tokens.len() + b.tokens.len();
    let jaccard = if union == 0 {
        0.0
    } else {
        let shared = common(&a.tokens, &b.tokens);
        shared as f64 / (union - shared) as f64
    };
    match (&a.key, &b.key) {
        (Some(ka), Some(kb)) => 0.5 * label_similarity(ka, kb) + 0.5 * jaccard,
        _ => jaccard,
    }
}

/// Pairs the items of one gap by similarity, without crossing, maximizing
/// total similarity (weighted LCS). Ties prefer the earlier base item.
fn fill_gap(
    base: &[Item],
    other: &[Item],
    (alo, ahi): (usize, usize),
    (blo, bhi): (usize, usize),
    options: Options,
    out: &mut Vec<(usize, usize)>,
    fell_back: &mut bool,
) {
    let (n, m) = (ahi - alo, bhi - blo);
    if n == 0 || m == 0 {
        return;
    }
    if n.saturating_mul(m) > MAX_GAP_WORK {
        *fell_back = true;
        out.extend((0..n.min(m)).map(|k| (alo + k, blo + k)));
        return;
    }
    let width = m + 1;
    let mut score = vec![0.0f64; (n + 1) * width];
    let mut sims = vec![0.0f64; n * m];
    for i in 0..n {
        for j in 0..m {
            let s = similarity(&base[alo + i], &other[blo + j]);
            sims[i * m + j] = s;
            let diag = if s >= MIN_SIMILARITY { score[i * width + j] + s } else { f64::NEG_INFINITY };
            score[(i + 1) * width + j + 1] = score[i * width + j + 1].max(score[(i + 1) * width + j]).max(diag);
        }
    }
    let mut found = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let here = score[i * width + j];
        let s = sims[(i - 1) * m + j - 1];
        if s >= MIN_SIMILARITY && (here - (score[(i - 1) * width + j - 1] + s)).abs() < 1e-12 {
            found.push((alo + i - 1, blo + j - 1));
            i -= 1;
            j -= 1;
        } else if (here - score[(i - 1) * width + j]).abs() < 1e-12 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    // Between the pairs found, an equal-sized run left over on both sides
    // pairs by position when the options allow (columns) or when every item
    // keeps its layout (a row whose label alone was edited).
    found.reverse();
    let mut positional = Vec::new();
    let (mut a, mut b) = (alo, blo);
    for &(i, j) in found.iter().chain(std::iter::once(&(ahi, bhi))) {
        let len = i - a;
        let same_shapes = || (0..len).all(|k| base[a + k].shape != 0 && base[a + k].shape == other[b + k].shape);
        if len > 0 && len == j - b && (options.positional_equal_gaps || same_shapes()) {
            positional.extend((0..len).map(|k| (a + k, b + k)));
        }
        (a, b) = (i + 1, j + 1);
    }
    out.extend(found);
    out.extend(positional);
}

/// What makes two unpaired items the same item moved: label and content.
type Signature = (Option<String>, Vec<u64>);

/// Unpaired items with the same key and tokens, unique on each side.
fn find_moves(base: &[Item], other: &[Item], pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut paired_base = vec![false; base.len()];
    let mut paired_other = vec![false; other.len()];
    for &(i, j) in pairs {
        paired_base[i] = true;
        paired_other[j] = true;
    }
    let signature = |item: &Item| -> Signature { (item.key.clone(), item.tokens.clone()) };
    let mut groups: HashMap<Signature, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for (i, item) in base.iter().enumerate().filter(|(i, it)| !paired_base[*i] && !it.tokens.is_empty()) {
        groups.entry(signature(item)).or_default().0.push(i);
    }
    for (j, item) in other.iter().enumerate().filter(|(j, it)| !paired_other[*j] && !it.tokens.is_empty()) {
        if let Some(group) = groups.get_mut(&signature(item)) {
            group.1.push(j);
        }
    }
    let mut moves: Vec<(usize, usize)> =
        groups.into_values().filter(|(a, b)| a.len() == 1 && b.len() == 1).map(|(a, b)| (a[0], b[0])).collect();
    moves.sort_unstable();
    moves
}

#[cfg(test)]
mod tests {
    use super::{Alignment, Item, Options, align, similarity};

    fn item(key: Option<&str>, tokens: &[u64]) -> Item {
        let mut tokens = tokens.to_vec();
        tokens.sort_unstable();
        Item { key: key.map(str::to_owned), tokens, shape: 0 }
    }

    fn rows(keys: &[&str]) -> Vec<Item> {
        keys.iter().enumerate().map(|(i, k)| item(Some(k), &[i as u64 + 100])).collect()
    }

    const ROWS: Options = Options { positional_equal_gaps: false };

    #[test]
    fn an_inserted_row_is_one_unpaired_item() {
        let a = rows(&["cash", "receivables", "inventory", "total"]);
        let b = rows(&["cash", "receivables", "prepaid", "inventory", "total"]);
        let al = align(&a, &b, ROWS);
        assert_eq!(al.pairs, [(0, 0), (1, 1), (2, 3), (3, 4)]);
        assert!(al.moves.is_empty() && !al.fell_back);
    }

    #[test]
    fn repeated_labels_anchor_on_rare_ones() {
        // Two sections, each ending in "subtotal"; the first section loses a row.
        let a = rows(&["A", "a1", "a2", "subtotal", "B", "b1", "subtotal"]);
        let b = rows(&["A", "a1", "subtotal", "B", "b1", "subtotal"]);
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (1, 1), (3, 2), (4, 3), (5, 4), (6, 5)]);
    }

    #[test]
    fn renamed_labels_pair_by_similarity() {
        let a = vec![item(Some("cash"), &[1]), item(Some("tradereceivables"), &[2, 3]), item(Some("total"), &[9])];
        let b = vec![item(Some("cash"), &[1]), item(Some("tradereceivablesnet"), &[2, 3]), item(Some("total"), &[9])];
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn unrelated_rows_stay_unpaired_unless_positional() {
        let a = vec![item(Some("x"), &[1, 2])];
        let b = vec![item(Some("yy"), &[7, 8])];
        assert!(align(&a, &b, ROWS).pairs.is_empty());
        let cols = Options { positional_equal_gaps: true };
        assert_eq!(align(&a, &b, cols).pairs, [(0, 0)]);
    }

    #[test]
    fn equal_gaps_of_the_same_shape_pair_by_position() {
        let mut a = vec![item(Some("cash"), &[1]), item(Some("profit"), &[]), item(Some("total"), &[9])];
        let mut b = vec![item(Some("cash"), &[1]), item(Some("loss"), &[]), item(Some("total"), &[9])];
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (2, 2)]);
        a[1].shape = 7;
        b[1].shape = 7;
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn moved_rows_are_found() {
        let a = rows(&["cash", "receivables", "inventory", "total"]);
        let mut b = a.clone();
        let moved = b.remove(1);
        b.insert(3, moved);
        let al = align(&a, &b, ROWS);
        assert_eq!(al.pairs, [(0, 0), (2, 1), (3, 2)]);
        assert_eq!(al.moves, [(1, 3)]);
    }

    #[test]
    fn unlabeled_rows_pair_by_tokens() {
        let a = vec![item(None, &[1, 2, 3]), item(None, &[4, 5, 6]), item(None, &[7, 8, 9])];
        let b = vec![item(None, &[1, 2, 3]), item(None, &[10, 11]), item(None, &[4, 5, 6]), item(None, &[7, 8, 0])];
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (1, 2), (2, 3)]);
    }

    #[test]
    fn identical_input_pairs_everything() {
        let a = rows(&["a", "b", "a", "b"]);
        let al = align(&a, &a, ROWS);
        assert_eq!(al, Alignment { pairs: vec![(0, 0), (1, 1), (2, 2), (3, 3)], ..Alignment::default() });
    }

    #[test]
    fn similarity_is_symmetric_and_bounded() {
        let a = item(Some("receivables"), &[1, 2, 2, 3]);
        let b = item(Some("receivable"), &[2, 3, 4]);
        let (ab, ba) = (similarity(&a, &b), similarity(&b, &a));
        assert!((ab - ba).abs() < 1e-12 && (0.0..=1.0).contains(&ab));
    }
}
