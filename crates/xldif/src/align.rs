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
/// The most token comparisons the similarity pass spends on one gap.
pub const MAX_GAP_TOKEN_WORK: usize = 200_000_000;

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
    // Pass 1: whole content (label and tokens). Rows that did not change at
    // all anchor first, so repeated labels (a currency or entity column) and
    // unlabelled tables still line up around an insertion.
    let (cb, co) = (content_keys(base), content_keys(other));
    let mut pairs = match_keys(&cb, &co, (0, base.len()), (0, other.len()));
    // Pass 2: labels alone, inside the gaps pass 1 left, for rows whose
    // values changed.
    let (lb, lo) = (label_keys(base), label_keys(other));
    let mut by_label = Vec::new();
    for (a, b) in gaps(&pairs, base.len(), other.len()) {
        by_label.extend(match_keys(&lb, &lo, a, b));
    }
    pairs.extend(by_label);
    pairs.sort_unstable();
    // Pass 3: similarity inside what is still unpaired.
    let mut out = Alignment::default();
    let mut filled = Vec::new();
    for (a, b) in gaps(&pairs, base.len(), other.len()) {
        fill_gap(base, other, a, b, options, &mut filled, &mut out.fell_back);
    }
    pairs.extend(filled);
    pairs.sort_unstable();
    out.moves = find_moves(base, other, &pairs);
    out.pairs = pairs;
    out
}

/// The unpaired regions between ordered pairs: `((alo, ahi), (blo, bhi))`.
fn gaps(pairs: &[(usize, usize)], n: usize, m: usize) -> Vec<((usize, usize), (usize, usize))> {
    let mut out = Vec::new();
    let (mut a, mut b) = (0, 0);
    for &(i, j) in pairs.iter().chain(std::iter::once(&(n, m))) {
        if i > a || j > b {
            out.push(((a, i), (b, j)));
        }
        (a, b) = (i + 1, j + 1);
    }
    out
}

/// A deterministic 64-bit FNV-1a hash.
pub(crate) fn fnv(parts: &[&[u8]]) -> u64 {
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

/// Each item's label and tokens together; `None` for an item with neither.
fn content_keys(items: &[Item]) -> Vec<Option<u64>> {
    items
        .iter()
        .map(|it| {
            (it.key.is_some() || !it.tokens.is_empty()).then(|| {
                let tokens: Vec<u8> = it.tokens.iter().flat_map(|t| t.to_le_bytes()).collect();
                fnv(&[it.key.as_deref().unwrap_or("").as_bytes(), &[u8::from(it.key.is_some())], &tokens])
            })
        })
        .collect()
}

fn label_keys(items: &[Item]) -> Vec<Option<u64>> {
    items.iter().map(|it| it.key.as_deref().map(|k| fnv(&[k.as_bytes()]))).collect()
}

/// Histogram matching on keys within one region: trims equal ends, anchors
/// on keys unique on both sides (patience), else on the rarest shared key
/// with its longest run, and recurses on both sides of each anchor.
fn match_keys(
    a: &[Option<u64>],
    b: &[Option<u64>],
    (alo, ahi): (usize, usize),
    (blo, bhi): (usize, usize),
) -> Vec<(usize, usize)> {
    let same = |i: usize, j: usize| a[i].is_some() && a[i] == b[j];
    let mut pairs = Vec::new();
    let mut regions = vec![(alo, ahi, blo, bhi)];
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
        let anchors = patience_anchors(a, b, (alo, ahi), (blo, bhi));
        if !anchors.is_empty() {
            let (mut x, mut y) = (alo, blo);
            for &(i, j) in &anchors {
                regions.push((x, i, y, j));
                pairs.push((i, j));
                (x, y) = (i + 1, j + 1);
            }
            regions.push((x, ahi, y, bhi));
            continue;
        }
        // Otherwise the rarest repeated key anchors, with its longest run.
        let mut positions: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, k) in a.iter().enumerate().take(ahi).skip(alo) {
            if let Some(k) = k {
                positions.entry(*k).or_default().push(i);
            }
        }
        // (occurrences, -run length, i, j): the rarest, then longest, then first.
        let mut best: Option<(usize, std::cmp::Reverse<usize>, usize, usize, usize)> = None;
        for j in blo..bhi {
            let Some(found) = b[j].and_then(|k| positions.get(&k)) else { continue };
            if found.len() > MAX_ANCHOR_OCCURRENCES {
                continue;
            }
            for &i in found {
                // Measure each run once, from its start: a candidate inside a
                // run would only re-measure it (quadratic on long runs).
                if i > alo && j > blo && same(i - 1, j - 1) {
                    continue;
                }
                let mut len = 0;
                while i + len < ahi && j + len < bhi && same(i + len, j + len) {
                    len += 1;
                }
                let candidate = (found.len(), std::cmp::Reverse(len), i, j, len);
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
/// reduced to a longest subsequence increasing on both sides. When several
/// subsequences are longest, the choice must not depend on which side is
/// the base: both orientations are tried, and the one that moves items
/// least (then the one with the smaller keys) wins.
fn patience_anchors(
    a: &[Option<u64>],
    b: &[Option<u64>],
    (alo, ahi): (usize, usize),
    (blo, bhi): (usize, usize),
) -> Vec<(usize, usize)> {
    // key → (count in base, last base index, count in other, last other index)
    let mut seen: HashMap<u64, (usize, usize, usize, usize)> = HashMap::new();
    for (i, k) in a.iter().enumerate().take(ahi).skip(alo) {
        if let Some(k) = k {
            let e = seen.entry(*k).or_default();
            e.0 += 1;
            e.1 = i;
        }
    }
    for (j, k) in b.iter().enumerate().take(bhi).skip(blo) {
        if let Some(k) = k
            && let Some(e) = seen.get_mut(k)
        {
            e.2 += 1;
            e.3 = j;
        }
    }
    let mut unique: Vec<(usize, usize)> =
        seen.into_values().filter(|e| e.0 == 1 && e.2 == 1).map(|e| (e.1, e.3)).collect();
    if unique.is_empty() {
        return unique;
    }
    unique.sort_unstable();
    let by_base: Vec<(usize, usize)> = {
        let order: Vec<usize> = unique.iter().map(|p| p.1).collect();
        longest_increasing(&order).into_iter().map(|k| unique[k]).collect()
    };
    let by_other: Vec<(usize, usize)> = {
        let mut flipped: Vec<(usize, usize)> = unique.iter().map(|&(i, j)| (j, i)).collect();
        flipped.sort_unstable();
        let order: Vec<usize> = flipped.iter().map(|p| p.1).collect();
        let mut kept: Vec<(usize, usize)> =
            longest_increasing(&order).into_iter().map(|k| (flipped[k].1, flipped[k].0)).collect();
        kept.sort_unstable();
        kept
    };
    let cost = |set: &[(usize, usize)]| {
        let displacement: usize = set.iter().map(|&(i, j)| (i - alo).abs_diff(j - blo)).sum();
        let mut keys: Vec<u64> = set.iter().filter_map(|&(i, _)| a[i]).collect();
        keys.sort_unstable();
        (std::cmp::Reverse(set.len()), displacement, keys)
    };
    if cost(&by_other) < cost(&by_base) { by_other } else { by_base }
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

/// A label's sorted character bigrams, for Dice similarity.
fn bigrams(s: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = s.chars().collect();
    let mut g: Vec<(char, char)> = chars.windows(2).map(|w| (w[0], w[1])).collect();
    g.sort_unstable();
    g
}

/// Bigram Dice similarity of two labels' bigrams, 0..=1.
fn dice(ga: &[(char, char)], gb: &[(char, char)]) -> f64 {
    if ga.is_empty() || gb.is_empty() {
        return 0.0;
    }
    2.0 * common(ga, gb) as f64 / (ga.len() + gb.len()) as f64
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

/// Token Jaccard averaged with label likeness when both have labels;
/// `grams` are the labels' bigrams (empty without a label).
fn similarity_with(a: &Item, ga: &[(char, char)], b: &Item, gb: &[(char, char)]) -> f64 {
    let union = a.tokens.len() + b.tokens.len();
    let jaccard = if union == 0 {
        0.0
    } else {
        let shared = common(&a.tokens, &b.tokens);
        shared as f64 / (union - shared) as f64
    };
    match (&a.key, &b.key) {
        (Some(ka), Some(kb)) => {
            let label = if ka == kb { 1.0 } else { dice(ga, gb) };
            0.5 * label + 0.5 * jaccard
        }
        _ => jaccard,
    }
}

/// How alike two items are, 0..=1: token Jaccard, averaged with label
/// likeness when both have labels.
pub fn similarity(a: &Item, b: &Item) -> f64 {
    let grams = |it: &Item| it.key.as_deref().map(bigrams).unwrap_or_default();
    similarity_with(a, &grams(a), b, &grams(b))
}

/// Labels this alike count as evidence that two same-layout rows are one
/// row edited, not a row replaced by an unrelated one.
const MIN_LABEL_EVIDENCE: f64 = 0.25;

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
    // Each similarity merges two token lists, so the work is the pairs plus
    // every item's tokens once per item on the other side.
    let tokens = |items: &[Item]| items.iter().map(|it| it.tokens.len()).sum::<usize>();
    let work = n
        .saturating_mul(m)
        .saturating_add(tokens(&base[alo..ahi]).saturating_mul(m))
        .saturating_add(tokens(&other[blo..bhi]).saturating_mul(n));
    if n.saturating_mul(m) > MAX_GAP_WORK || work > MAX_GAP_TOKEN_WORK {
        *fell_back = true;
        out.extend((0..n.min(m)).map(|k| (alo + k, blo + k)));
        return;
    }
    let grams = |items: &[Item]| -> Vec<Vec<(char, char)>> {
        items.iter().map(|it| it.key.as_deref().map(bigrams).unwrap_or_default()).collect()
    };
    let (gb, go) = (grams(&base[alo..ahi]), grams(&other[blo..bhi]));
    let width = m + 1;
    let mut score = vec![0.0f64; (n + 1) * width];
    let mut sims = vec![0.0f64; n * m];
    for i in 0..n {
        for j in 0..m {
            let s = similarity_with(&base[alo + i], &gb[i], &other[blo + j], &go[j]);
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
    // pairs by position when the options allow (columns), or when every row
    // keeps its layout and shows some sign of being the same row (a shared
    // value or a similar label): a row whose label alone was edited.
    found.reverse();
    let mut positional = Vec::new();
    let (mut a, mut b) = (alo, blo);
    for &(i, j) in found.iter().chain(std::iter::once(&(ahi, bhi))) {
        let len = i - a;
        let edited_in_place = || {
            (0..len).all(|k| {
                let (x, y) = (&base[a + k], &other[b + k]);
                let evidence =
                    common(&x.tokens, &y.tokens) > 0 || dice(&gb[a + k - alo], &go[b + k - blo]) >= MIN_LABEL_EVIDENCE;
                x.shape != 0 && x.shape == y.shape && evidence
            })
        };
        if len > 0 && len == j - b && (options.positional_equal_gaps || edited_in_place()) {
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
        let mut a = vec![item(Some("cash"), &[1]), item(Some("netprofit"), &[]), item(Some("total"), &[9])];
        let mut b = vec![item(Some("cash"), &[1]), item(Some("netloss"), &[]), item(Some("total"), &[9])];
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (2, 2)]);
        a[1].shape = 7;
        b[1].shape = 7;
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (1, 1), (2, 2)]);
        // The same layout alone is no evidence: an unrelated row replacing
        // another stays a deletion and an insertion.
        b[1].key = Some("goodwill".into());
        assert_eq!(align(&a, &b, ROWS).pairs, [(0, 0), (2, 2)]);
    }

    #[test]
    fn repeated_labels_do_not_shift_rows_after_an_insertion() {
        // Every row says "krw"; their values differ. One row is inserted at the top.
        let a: Vec<Item> = (0..50).map(|i| item(Some("krw"), &[1000 + i, 2000 + i])).collect();
        let mut b = a.clone();
        b.insert(0, item(Some("krw"), &[9999]));
        let al = align(&a, &b, ROWS);
        assert_eq!(al.pairs, (0..50).map(|i| (i, i + 1)).collect::<Vec<_>>());
    }

    #[test]
    fn unlabelled_tables_anchor_on_content() {
        let a: Vec<Item> = (0..3000).map(|i| item(None, &[i, i + 10_000])).collect();
        let mut b = a.clone();
        b.insert(1500, item(None, &[77_777]));
        let al = align(&a, &b, ROWS);
        assert!(!al.fell_back);
        assert_eq!(al.pairs.len(), 3000);
        assert!(al.pairs.iter().all(|&(i, j)| j == if i < 1500 { i } else { i + 1 }));
    }

    #[test]
    fn swapped_rows_mirror_when_the_sides_swap() {
        let a = rows(&["h", "a", "b", "c"]);
        let b = vec![a[0].clone(), a[2].clone(), a[1].clone(), a[3].clone()];
        let ab = align(&a, &b, ROWS);
        let ba = align(&b, &a, ROWS);
        let flip = |v: &[(usize, usize)]| {
            let mut f: Vec<(usize, usize)> = v.iter().map(|&(i, j)| (j, i)).collect();
            f.sort_unstable();
            f
        };
        assert_eq!(flip(&ab.pairs), ba.pairs);
        assert_eq!(flip(&ab.moves), ba.moves);
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
