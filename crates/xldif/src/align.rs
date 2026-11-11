//! Sequence alignment for rows and columns.
//!
//! An [`Item`] is one row or column: an optional label key and a sorted
//! multiset of content tokens. Two items are the same row or column to the
//! degree [`similarity`] says, and an alignment is the set of pairs, never
//! crossing, with the greatest total similarity; a pair below
//! [`MIN_SIMILARITY`] is never made. Two steps find it without comparing
//! every item with every other:
//!
//! 1. **Anchors.** Items whose whole content (label and tokens) occurs
//!    exactly once on each side of a region are candidates to pair, and so
//!    are items whose label does; the longest run of candidates in order
//!    anchors, so one line that moved cannot split the rest. Equal items at
//!    a region's two ends pair as well. Each anchor splits its region and
//!    the parts are searched again.
//! 2. **Gaps.** Between anchors a weighted longest-common-subsequence picks
//!    the pairs.
//!
//! Unpaired items on both sides with the same label and tokens, unique among
//! the unpaired, are then reported as moved.
//!
//! A label shared by several items never pairs two of them ahead of their
//! content: rows under one label (a currency or entity column) are told
//! apart by their values in step 2.
//!
//! Step 2 compares every item of a gap with every other, which a large gap
//! cannot afford. A region with nothing unique that is too large for it is
//! split on weaker evidence first, each kind tried in turn:
//!
//! - a run of consecutive items whose contents occur once, as a run, on
//!   each side (rows that each repeat, as in a table stacked twice);
//! - a token found once on each side, which votes for the two items holding
//!   it: items that are each other's first choice, and similar enough to
//!   pair, anchor (every row edited, but not in every cell);
//! - a run of consecutive labels that occurs once on each side (sections
//!   of a schedule under names that repeat while every figure changed);
//! - failing all of those, items at the region's two ends that are similar
//!   enough to pair, taken in file order from each end until one is not
//!   (labels repeating in a cycle, with a block inserted in the middle).
//!
//! A gap still too large is searched only near its diagonal (rows mostly
//! keep their order and few come or go), and one too large even for that is
//! paired by position. Pairing from the ends, the search near the diagonal,
//! and pairing by position can each miss the best alignment, and the result
//! says when one was used.
//!
//! The result depends only on the input, never on hashing or iteration
//! order. Ties are broken by index order, so swapping the two sides need not
//! mirror the result; a caller that needs that puts the sides in a fixed
//! order first, as the comparison does.

// Index names (i, j for base and other positions) read best in this algorithm.
#![allow(clippy::many_single_char_names)]

use std::collections::HashMap;

/// Pairs below this similarity are not considered the same row or column.
pub const MIN_SIMILARITY: f64 = 0.5;
/// The largest region (base items × other items) step 2 takes without
/// first looking for weaker evidence to split it.
const DIRECT_GAP_WORK: usize = 1_000_000;
/// The most pairs of items the similarity pass compares in one gap: every
/// pair when the gap has no more, else those near its diagonal.
pub const MAX_GAP_WORK: usize = 4_000_000;
/// The most token comparisons the similarity pass spends on one gap.
pub const MAX_GAP_TOKEN_WORK: usize = 200_000_000;
/// Lengths of the runs tried as anchors, shortest first: a short run that
/// is unique leaves the least unanchored around an edit.
const RUNS: [usize; 5] = [2, 4, 8, 16, 32];
/// The fewest spare diagonals worth searching a banded gap with; with
/// fewer the gap is paired by position.
const MIN_BAND: usize = 16;
/// How many times over the anchor search may read the whole input. Each
/// search reads its region once and regions nest, so only input built to
/// split one item at a time comes near this.
const ANCHOR_PASSES: usize = 64;

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
    /// A gap exceeded the work bounds: it was searched only near its
    /// diagonal, or paired by position, and the best alignment may have
    /// been missed.
    pub fell_back: bool,
}

impl Alignment {
    /// Pairs by position: the alignment of `--align position`.
    pub fn positional(base: usize, other: usize) -> Self {
        Self { pairs: (0..base.min(other)).map(|i| (i, i)).collect(), ..Self::default() }
    }
}

pub fn align(base: &[Item], other: &[Item], options: Options) -> Alignment {
    let mut out = Alignment::default();
    let mut pairs = anchors(base, other, &mut out.fell_back);
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

/// Step 1: the pairs certain enough to split the input before any
/// similarity is computed, in order. `fell_back` is set when a region too
/// large for step 2, with nothing else to go on, was paired from its ends.
fn anchors(base: &[Item], other: &[Item], fell_back: &mut bool) -> Vec<(usize, usize)> {
    let (a, b) = (content_keys(base), content_keys(other));
    let (la, lb) = (label_keys(base), label_keys(other));
    let same = |i: usize, j: usize| a[i].is_some() && a[i] == b[j];
    let unique = |keys: (&[Option<u64>], &[Option<u64>]), (alo, ahi, blo, bhi): (usize, usize, usize, usize)| {
        once_on_each_side(
            (alo..ahi).filter_map(|i| keys.0[i].map(|k| (k, i))),
            (blo..bhi).filter_map(|j| keys.1[j].map(|k| (k, j))),
        )
    };
    let size = |items: &[Item]| items.iter().map(|it| it.tokens.len() + 1).sum::<usize>();
    let mut budget = ANCHOR_PASSES.saturating_mul(size(base) + size(other));
    let mut spend = |cost: usize| {
        let affordable = cost <= budget;
        budget = budget.saturating_sub(cost);
        affordable
    };
    let mut pairs = Vec::new();
    let mut regions = vec![(0, base.len(), 0, other.len())];
    while let Some((mut alo, mut ahi, mut blo, mut bhi)) = regions.pop() {
        // Equal items at the two ends belong to some best alignment: any
        // other partner for either could be exchanged for this one at no loss.
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
        if alo == ahi || blo == bhi || !spend(ahi - alo + bhi - blo) {
            continue;
        }
        // An item unique by content and by label names one pair both ways
        // (its partner carries the same label), so the candidates never
        // offer one item two partners. Taking the longest run of them all,
        // not of the content pairs first, keeps one unchanged line that
        // moved from anchoring across every line it passed.
        let region = (alo, ahi, blo, bhi);
        let mut candidates = unique((&a, &b), region);
        candidates.extend(unique((&la, &lb), region));
        candidates.sort_unstable();
        candidates.dedup();
        let mut found = in_order(&candidates);
        let (n, m) = (ahi - alo, bhi - blo);
        let large = n.saturating_mul(m) > DIRECT_GAP_WORK.min(full_search(&base[alo..ahi], &other[blo..bhi]));
        if found.is_empty() && large {
            let runs = |keys: (&[Option<u64>], &[Option<u64>]), spend: &mut dyn FnMut(usize) -> bool| {
                RUNS.iter()
                    .filter(|&&k| k <= n.min(m))
                    .map_while(|&k| spend((n + m) * k).then(|| run_anchors(keys, region, k)))
                    .find(|found| !found.is_empty())
                    .unwrap_or_default()
            };
            found = runs((&a, &b), &mut spend);
            if found.is_empty() && spend(size(&base[alo..ahi]) + size(&other[blo..bhi])) {
                found = evidence_anchors(base, other, (alo, ahi), (blo, bhi));
            }
            if found.is_empty() {
                found = runs((&la, &lb), &mut spend);
            }
            if found.is_empty() && spend(n + m) {
                // Nothing places any item. Rows in a large table mostly keep
                // their order, so the two ends are paired in file order for
                // as long as the items there could be the same item, and
                // what is left between them is searched again.
                let similar = |i: usize, j: usize| similarity(&base[i], &other[j]) >= MIN_SIMILARITY;
                let before = (alo, ahi);
                while alo < ahi && blo < bhi && similar(alo, blo) {
                    pairs.push((alo, blo));
                    alo += 1;
                    blo += 1;
                }
                while alo < ahi && blo < bhi && similar(ahi - 1, bhi - 1) {
                    pairs.push((ahi - 1, bhi - 1));
                    ahi -= 1;
                    bhi -= 1;
                }
                if (alo, ahi) != before {
                    *fell_back = true;
                    regions.push((alo, ahi, blo, bhi));
                }
                continue;
            }
        }
        if found.is_empty() {
            continue;
        }
        let (mut x, mut y) = (alo, blo);
        for &(i, j) in &found {
            regions.push((x, i, y, j));
            (x, y) = (i + 1, j + 1);
        }
        regions.push((x, ahi, y, bhi));
        pairs.extend(found);
    }
    pairs.sort_unstable();
    pairs
}

/// `(base index, other index)` for every key that occurs exactly once among
/// `base` and once among `other`, sorted.
fn once_on_each_side(
    base: impl Iterator<Item = (u64, usize)>,
    other: impl Iterator<Item = (u64, usize)>,
) -> Vec<(usize, usize)> {
    // key → (count in base, its last index there, the same for other)
    let mut seen: HashMap<u64, (usize, usize, usize, usize)> = HashMap::new();
    for (k, i) in base {
        let e = seen.entry(k).or_default();
        e.0 += 1;
        e.1 = i;
    }
    for (k, j) in other {
        if let Some(e) = seen.get_mut(&k) {
            e.2 += 1;
            e.3 = j;
        }
    }
    let mut found: Vec<(usize, usize)> =
        seen.into_values().filter(|e| e.0 == 1 && e.2 == 1).map(|e| (e.1, e.3)).collect();
    found.sort_unstable();
    found
}

/// Anchors on runs of `k` consecutive keys that occur once, as a run, on
/// each side of the region: the items the two runs start at. Keys that
/// repeat say little one at a time, but a run of them in an order found
/// nowhere else places its items. An item without a key breaks a run.
fn run_anchors(
    (a, b): (&[Option<u64>], &[Option<u64>]),
    (alo, ahi, blo, bhi): (usize, usize, usize, usize),
    k: usize,
) -> Vec<(usize, usize)> {
    let runs = |keys: &[Option<u64>], lo: usize, hi: usize| -> Vec<(u64, usize)> {
        (lo..(hi + 1).saturating_sub(k))
            .filter_map(|i| {
                let mut h = 0u64;
                for key in &keys[i..i + k] {
                    h = fnv(&[&h.to_le_bytes(), &(*key)?.to_le_bytes()]);
                }
                Some((h, i))
            })
            .collect()
    };
    in_order(&once_on_each_side(runs(a, alo, ahi).into_iter(), runs(b, blo, bhi).into_iter()))
}

/// How many pairs of items the similarity pass may compare in one gap.
/// Each comparison merges two token lists, so a pair costs itself plus an
/// average item's tokens on each side.
fn full_search(base: &[Item], other: &[Item]) -> usize {
    let tokens = |items: &[Item]| items.iter().map(|it| it.tokens.len()).sum::<usize>();
    let per_pair = 1 + tokens(base).div_ceil(base.len().max(1)) + tokens(other).div_ceil(other.len().max(1));
    MAX_GAP_WORK.min(MAX_GAP_TOKEN_WORK / per_pair)
}

/// The longest run of `pairs` (sorted, no index repeated) that is in order
/// on both sides.
fn in_order(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let order: Vec<usize> = pairs.iter().map(|p| p.1).collect();
    longest_increasing(&order).into_iter().map(|k| pairs[k]).collect()
}

/// Anchors for a region with no item whose whole content or label is
/// unique on both sides: every row was edited and none is named apart from
/// the rest. A token that occurs once on each side is one vote for the two
/// items holding it. Two items anchor when each has more votes for the
/// other than for any third item and they are similar enough to pair at all.
fn evidence_anchors(
    base: &[Item],
    other: &[Item],
    (alo, ahi): (usize, usize),
    (blo, bhi): (usize, usize),
) -> Vec<(usize, usize)> {
    let votes = once_on_each_side(
        (alo..ahi).flat_map(|i| base[i].tokens.iter().map(move |&k| (k, i))),
        (blo..bhi).flat_map(|j| other[j].tokens.iter().map(move |&k| (k, j))),
    );
    // (base index, other index, votes), sorted.
    let mut tally: Vec<(usize, usize, usize)> = Vec::new();
    for (i, j) in votes {
        match tally.last_mut() {
            Some(t) if (t.0, t.1) == (i, j) => t.2 += 1,
            _ => tally.push((i, j, 1)),
        }
    }
    // Each item's most votes for one partner, and how many partners have that many.
    let lead = |lead: &mut (usize, usize), votes: usize| match votes.cmp(&lead.0) {
        std::cmp::Ordering::Greater => *lead = (votes, 1),
        std::cmp::Ordering::Equal => lead.1 += 1,
        std::cmp::Ordering::Less => {}
    };
    let mut lead_base = vec![(0, 0); ahi - alo];
    let mut lead_other = vec![(0, 0); bhi - blo];
    for &(i, j, votes) in &tally {
        lead(&mut lead_base[i - alo], votes);
        lead(&mut lead_other[j - blo], votes);
    }
    let chosen: Vec<(usize, usize)> = tally
        .iter()
        .filter(|&&(i, j, votes)| lead_base[i - alo] == (votes, 1) && lead_other[j - blo] == (votes, 1))
        .filter(|&&(i, j, _)| similarity(&base[i], &other[j]) >= MIN_SIMILARITY)
        .map(|&(i, j, _)| (i, j))
        .collect();
    in_order(&chosen)
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
///
/// A gap too large to compare every item with every other is searched only
/// near its diagonal. An alignment that inserts or deletes more than the
/// band allows in one place is out of reach, and nothing found inside the
/// band can show that a better one lies outside it, so `fell_back` is set
/// whenever the band is used.
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
    let cells = full_search(&base[alo..ahi], &other[blo..bhi]);
    // Diagonals are `other index - base index`. The two corners sit on 0
    // and `m - n`; a band keeps `spare` more on each side of those.
    let (n_, m_) = (n.cast_signed(), m.cast_signed());
    let band = if n.saturating_mul(m) <= cells {
        None
    } else {
        match (cells / n).checked_sub(n.abs_diff(m) + 1).map(|free| free / 2) {
            Some(spare) if spare >= MIN_BAND => Some(spare.cast_signed()),
            _ => {
                *fell_back = true;
                out.extend((0..n.min(m)).map(|k| (alo + k, blo + k)));
                return;
            }
        }
    };
    let (dmin, dmax) = match band {
        None => (-n_, m_),
        Some(spare) => (0.min(m_ - n_) - spare, 0.max(m_ - n_) + spare),
    };
    // Row `i` of the score table holds columns `first(i)..=last(i)`.
    let first = |i: usize| (i.cast_signed() + dmin).max(0).cast_unsigned();
    let last = |i: usize| (i.cast_signed() + dmax).min(m_).cast_unsigned();
    let mut start = Vec::with_capacity(n + 2);
    start.push(0);
    for i in 0..=n {
        start.push(start[i] + last(i) + 1 - first(i));
    }
    let mut score = vec![0.0f64; start[n + 1]];
    let at = |score: &[f64], i: usize, j: usize| {
        if j < first(i) || j > last(i) { f64::NEG_INFINITY } else { score[start[i] + j - first(i)] }
    };
    let grams = |items: &[Item]| -> Vec<Vec<(char, char)>> {
        items.iter().map(|it| it.key.as_deref().map(bigrams).unwrap_or_default()).collect()
    };
    let (gb, go) = (grams(&base[alo..ahi]), grams(&other[blo..bhi]));
    let sim = |i: usize, j: usize| similarity_with(&base[alo + i], &gb[i], &other[blo + j], &go[j]);
    for i in 1..=n {
        for j in first(i).max(1)..=last(i) {
            let s = sim(i - 1, j - 1);
            let diag = if s >= MIN_SIMILARITY { at(&score, i - 1, j - 1) + s } else { f64::NEG_INFINITY };
            score[start[i] + j - first(i)] = at(&score, i - 1, j).max(at(&score, i, j - 1)).max(diag);
        }
    }
    *fell_back |= band.is_some();
    let mut found = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let here = at(&score, i, j);
        let s = sim(i - 1, j - 1);
        if s >= MIN_SIMILARITY && (here - (at(&score, i - 1, j - 1) + s)).abs() < 1e-12 {
            found.push((alo + i - 1, blo + j - 1));
            i -= 1;
            j -= 1;
        } else if (here - at(&score, i - 1, j)).abs() < 1e-12 {
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
    fn rows_sharing_a_label_pair_by_their_values() {
        // Every row says "krw" and every row had one value edited; a row was
        // inserted among them. The label must not pair them in file order.
        // (Rows after the insertion once paired with the row above their own.)
        let a: Vec<Item> = (0..30).map(|i| item(Some("krw"), &[1000 + i, 2000 + i])).collect();
        let mut b: Vec<Item> = (0..30).map(|i| item(Some("krw"), &[1000 + i, 3000 + i])).collect();
        b.insert(10, item(Some("krw"), &[5000, 6000]));
        let al = align(&a, &b, ROWS);
        assert_eq!(al.pairs, (0..30).map(|i| (i, if i < 10 { i } else { i + 1 })).collect::<Vec<_>>());
        assert!(al.moves.is_empty());
    }

    #[test]
    fn a_large_table_with_every_row_edited_still_aligns() {
        // No row is the same on both sides and none has a label, so nothing
        // anchors on whole content, and the table is too large to compare
        // every row with every other.
        let row = |i: u64, edit: u64| item(None, &[i, 100_000 + i, 200_000 + i, 300_000 + i + edit]);
        let a: Vec<Item> = (0..2500).map(|i| row(i, 0)).collect();
        let mut b: Vec<Item> = (0..2500).map(|i| row(i, 1_000_000)).collect();
        b.insert(1200, item(None, &[77_777, 88_888]));
        let al = align(&a, &b, ROWS);
        assert!(!al.fell_back);
        assert_eq!(al.pairs, (0..2500).map(|i| (i, if i < 1200 { i } else { i + 1 })).collect::<Vec<_>>());
    }

    /// A row under one of twelve labels in a cycle: its tokens are its
    /// label cell and its value.
    fn month(i: u64, value: u64) -> Item {
        item(Some(&format!("m{}", i % 12)), &[1_000_000 + i % 12, value])
    }

    #[test]
    fn a_large_table_under_repeating_labels_pairs_from_its_ends() {
        // Labels repeat in a cycle, every value changed, and one row was
        // inserted at the top: nothing anchors, and the table is too large
        // to compare every row with every other.
        let a: Vec<Item> = (0..3000).map(|i| month(i, i)).collect();
        let mut b: Vec<Item> = (0..3000).map(|i| month(i, i + 1)).collect();
        b.insert(0, item(Some("opening"), &[]));
        let al = align(&a, &b, ROWS);
        // Right here, but pairing in file order cannot know that.
        assert!(al.fell_back);
        assert_eq!(al.pairs, (0..3000).map(|i| (i, i + 1)).collect::<Vec<_>>());
    }

    #[test]
    fn a_large_table_with_unlike_ends_is_searched_near_its_diagonal() {
        // As above, but a row was inserted in the middle and the first and
        // last rows were replaced by unrelated ones, so nothing pairs from
        // the ends either.
        let a: Vec<Item> = (0..3000).map(|i| month(i, i)).collect();
        let mut b: Vec<Item> = (0..3000).map(|i| month(i, i + 1)).collect();
        b[0] = item(Some("opening"), &[7]);
        b[2999] = item(Some("closing"), &[8]);
        b.insert(1500, item(Some("adjustment"), &[9]));
        let al = align(&a, &b, ROWS);
        assert!(al.fell_back);
        let expected: Vec<(usize, usize)> = (1..2999).map(|i| (i, if i < 1500 { i } else { i + 1 })).collect();
        assert_eq!(al.pairs, expected);
    }

    #[test]
    fn a_table_stacked_twice_anchors_on_runs_of_rows() {
        // Every row occurs twice, so none anchors alone; 700 rows went from
        // the top and 700 new ones came at the bottom, too far for a search
        // near the diagonal.
        let row = |r: u64| item(Some(&format!("m{}", r % 12)), &[1_000_000 + r % 12, 5_000 + r]);
        let a: Vec<Item> = (0..3000).map(|i| row(i % 1500)).collect();
        let mut b: Vec<Item> = a[700..].to_vec();
        b.extend((0..700).map(|k| item(Some("new"), &[900_000 + k])));
        let al = align(&a, &b, ROWS);
        assert!(!al.fell_back);
        assert_eq!(al.pairs, (700..3000).map(|i| (i, i - 700)).collect::<Vec<_>>());
    }

    #[test]
    fn sections_under_repeating_names_anchor_on_runs_of_labels() {
        // Lines under seven names in no pattern, every figure changed, and
        // the first third moved to the end: no content, label, or token is
        // unique, the ends do not pair, and the move is too large for a
        // search near the diagonal. Only the order of the names places a line.
        let mut state = 7u64;
        let names: Vec<u64> = (0..2700)
            .map(|_| {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                (state >> 33) % 7
            })
            .collect();
        let row = |i: usize, v: u64| item(Some(&format!("n{}", names[i])), &[1_000_000 + names[i], v + i as u64]);
        let a: Vec<Item> = (0..2700).map(|i| row(i, 10_000_000)).collect();
        let b: Vec<Item> = (900..2700).chain(0..900).map(|i| row(i, 50_000_000)).collect();
        let al = align(&a, &b, ROWS);
        // The larger part stays in order; the part that crossed it does not pair.
        let right = al.pairs.iter().filter(|&&(i, j)| i >= 900 && j == i - 900).count();
        assert!(right >= 1780, "{right} right of {} pairs", al.pairs.len());
    }

    #[test]
    fn a_large_ledger_with_a_block_inserted_pairs_from_both_ends() {
        // Account names in a near-cycle, every amount changed, and a block
        // inserted half-way that is too large for a search near the
        // diagonal: no content, label, token, or run is unique.
        let account = |i: u64| (i.wrapping_mul(2_654_435_761) >> 7) % 37;
        let row = |i: u64, v: u64| item(Some(&format!("acct{}", account(i))), &[1_000_000 + account(i), v]);
        let a: Vec<Item> = (0..6000).map(|i| row(i, 10_000_000 + i)).collect();
        let mut b: Vec<Item> = (0..6000).map(|i| row(i, 50_000_000 + i)).collect();
        b.splice(3000..3000, (0..700).map(|k| row(1_000_000 + k, 90_000_000 + k)));
        let al = align(&a, &b, ROWS);
        let right = al.pairs.iter().filter(|&&(i, j)| j == if i < 3000 { i } else { i + 700 }).count();
        // All but the odd row beside the block whose name happens to match.
        assert!(right >= 5990 && al.pairs.len() <= 6000, "{right} right of {} pairs", al.pairs.len());
        assert!(al.fell_back);
    }

    #[test]
    fn one_unchanged_line_that_moved_does_not_split_a_statement() {
        // Every line keeps its name and changes its figure, except one,
        // which is unchanged and moved far down.
        let a: Vec<Item> = (0..60).map(|i| item(Some(&format!("line{i}")), &[i, 100 + i])).collect();
        let mut b: Vec<Item> = (0..60).map(|i| item(Some(&format!("line{i}")), &[i, 900 + i])).collect();
        b.remove(2);
        b.insert(40, a[2].clone());
        let al = align(&a, &b, ROWS);
        assert_eq!(al.pairs.len(), 59);
        assert_eq!(al.moves, [(2, 40)]);
    }

    #[test]
    fn evidence_shared_with_two_rows_anchors_neither() {
        // Too large for the similarity pass, and each base row's tokens are
        // split evenly between two other rows: no first choice, no anchor.
        let a: Vec<Item> = (0..2100).map(|i| item(None, &[i, 10_000 + i])).collect();
        let b: Vec<Item> =
            (0..2100).flat_map(|i| [item(None, &[i, 50_000 + i]), item(None, &[10_000 + i, 60_000 + i])]).collect();
        let al = align(&a, &b, ROWS);
        assert!(al.fell_back);
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

    mod properties {
        use proptest::prelude::*;

        use super::super::{Item, MIN_SIMILARITY, Options, align, similarity};

        /// Items over a small alphabet, so labels and tokens repeat.
        fn items() -> impl Strategy<Value = Vec<Item>> {
            let item =
                (prop::option::of(0..4u8), prop::collection::vec(0..6u64, 0..4)).prop_map(|(key, mut tokens)| {
                    tokens.sort_unstable();
                    Item { key: key.map(|k| format!("label{k}")), tokens, shape: 0 }
                });
            prop::collection::vec(item, 0..24)
        }

        proptest! {
            #[test]
            fn pairs_keep_order_and_use_each_item_once(a in items(), b in items(), columns: bool) {
                let al = align(&a, &b, Options { positional_equal_gaps: columns });
                prop_assert!(al.pairs.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
                let mut base: Vec<usize> = al.pairs.iter().chain(&al.moves).map(|p| p.0).collect();
                let mut other: Vec<usize> = al.pairs.iter().chain(&al.moves).map(|p| p.1).collect();
                let count = base.len();
                base.sort_unstable();
                base.dedup();
                other.sort_unstable();
                other.dedup();
                prop_assert_eq!((base.len(), other.len()), (count, count));
                prop_assert!(base.last().is_none_or(|&i| i < a.len()) && other.last().is_none_or(|&j| j < b.len()));
                prop_assert!(!al.fell_back);
            }

            #[test]
            fn rows_pair_only_when_similar(a in items(), b in items()) {
                let al = align(&a, &b, Options { positional_equal_gaps: false });
                for &(i, j) in &al.pairs {
                    prop_assert!(a[i] == b[j] || similarity(&a[i], &b[j]) >= MIN_SIMILARITY, "{:?} with {:?}", a[i], b[j]);
                }
                for &(i, j) in &al.moves {
                    prop_assert_eq!(&a[i], &b[j]);
                }
            }

            #[test]
            fn a_sequence_aligns_with_itself_in_place(a in items()) {
                let al = align(&a, &a, Options { positional_equal_gaps: false });
                // An item with neither label nor tokens is nothing to pair.
                let content: Vec<(usize, usize)> =
                    (0..a.len()).filter(|&i| a[i].key.is_some() || !a[i].tokens.is_empty()).map(|i| (i, i)).collect();
                prop_assert_eq!(al.pairs, content);
                prop_assert!(al.moves.is_empty());
            }

        }
    }
}
