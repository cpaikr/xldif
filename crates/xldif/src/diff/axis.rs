//! The alignment of one sheet pair's rows and columns, by sheet number.

use std::collections::{HashMap, HashSet};

use super::grid::Grid;
use super::{AlignMode, AlignmentView, CompareOptions, Run};
use crate::align::{self, Alignment, Options};
use crate::cellref::{MAX_COL, MAX_ROW};
use crate::formula::{AxisMap, SheetMap};

/// Runs of `(base, other)` pairs, by sheet row or column number, that keep a
/// constant offset and have no one-sided item between them.
pub(super) fn runs(pairs: &[(u32, u32)], lone_base: &[u32], lone_other: &[u32]) -> Vec<Run> {
    let between =
        |lone: &[u32], lo: u32, hi: u32| lone.partition_point(|&x| x <= lo) < lone.partition_point(|&x| x < hi);
    let mut out: Vec<Run> = Vec::new();
    for &(b, o) in pairs {
        if let Some(last) = out.last_mut() {
            let (lb, lo) = (last[0] + last[2] - 1, last[1] + last[2] - 1);
            if b > lb && b - lb == o.wrapping_sub(lo) && !between(lone_base, lb, b) && !between(lone_other, lo, o) {
                last[2] += b - lb;
                continue;
            }
        }
        out.push([b, o, 1]);
    }
    out
}

/// The rows, or the columns, of one sheet pair that hold content, by sheet
/// number: every one is in exactly one of the four lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Axis {
    /// `(base number, other number)` pairs in order.
    pub pairs: Vec<(u32, u32)>,
    /// Moved items: `(base number, other number)`, outside the ordered pairs.
    pub moves: Vec<(u32, u32)>,
    /// Numbers only the base sheet has, in order.
    pub lone_base: Vec<u32>,
    /// Numbers only the other sheet has, in order.
    pub lone_other: Vec<u32>,
}

impl Axis {
    pub(super) fn new(al: &Alignment, base: &[u32], other: &[u32]) -> Self {
        let pairs: Vec<(u32, u32)> = al.pairs.iter().map(|&(i, j)| (base[i], other[j])).collect();
        let moves: Vec<(u32, u32)> = al.moves.iter().map(|&(i, j)| (base[i], other[j])).collect();
        let used_b: HashSet<u32> = pairs.iter().chain(&moves).map(|p| p.0).collect();
        let used_o: HashSet<u32> = pairs.iter().chain(&moves).map(|p| p.1).collect();
        let lone_base = base.iter().copied().filter(|x| !used_b.contains(x)).collect();
        let lone_other = other.iter().copied().filter(|x| !used_o.contains(x)).collect();
        Self { pairs, moves, lone_base, lone_other }
    }

    /// A1 against A1: every row or column number pairs with itself.
    pub(super) fn identity(base: &[u32], other: &[u32]) -> Self {
        let all: std::collections::BTreeSet<u32> = base.iter().chain(other).copied().collect();
        Self {
            pairs: all.into_iter().map(|n| (n, n)).collect(),
            moves: Vec::new(),
            lone_base: Vec::new(),
            lone_other: Vec::new(),
        }
    }

    /// The same axis seen from the other side.
    pub(super) fn flipped(self) -> Self {
        let flip = |v: Vec<(u32, u32)>| {
            let mut v: Vec<(u32, u32)> = v.into_iter().map(|(b, o)| (o, b)).collect();
            v.sort_unstable();
            v
        };
        Self {
            pairs: flip(self.pairs),
            moves: flip(self.moves),
            lone_base: self.lone_other,
            lone_other: self.lone_base,
        }
    }

    pub(super) fn view_runs(&self) -> (Vec<Run>, Vec<Run>) {
        let mut moves: Vec<Run> = self.moves.iter().map(|&(b, o)| [b, o, 1]).collect();
        moves.sort_unstable();
        // A run must not span a moved item either, or it would map that
        // number to itself as well as to where it moved.
        let barrier = |lone: &[u32], moved: &mut dyn Iterator<Item = u32>| {
            let mut all: Vec<u32> = lone.iter().copied().chain(moved).collect();
            all.sort_unstable();
            all
        };
        let base = barrier(&self.lone_base, &mut self.moves.iter().map(|m| m.0));
        let other = barrier(&self.lone_other, &mut self.moves.iter().map(|m| m.1));
        (runs(&self.pairs, &base, &other), moves)
    }
}

/// One sheet pair's row and column alignment.
pub(super) struct Aligned {
    pub(super) cols: Axis,
    pub(super) rows: Axis,
    pub(super) method: &'static str,
    pub(super) fell_back: bool,
}

impl Aligned {
    pub(super) fn view(&self) -> AlignmentView {
        let (rows, row_moves) = self.rows.view_runs();
        let (cols, col_moves) = self.cols.view_runs();
        AlignmentView { method: self.method, fell_back: self.fell_back, rows, row_moves, cols, col_moves }
    }

    pub(super) fn sheet_map(&self, renamed_to: Option<String>) -> SheetMap {
        let axis = |a: &Axis, max: u32| {
            AxisMap::new(a.pairs.clone(), a.moves.clone(), a.lone_base.clone(), a.lone_other.clone(), max)
        };
        SheetMap { renamed_to, rows: axis(&self.rows, MAX_ROW), cols: axis(&self.cols, MAX_COL) }
    }

    /// The map from the other side back to the base side.
    pub(super) fn inverse_map(&self) -> SheetMap {
        let flip = |v: &[(u32, u32)]| v.iter().map(|&(b, o)| (o, b)).collect::<Vec<_>>();
        let axis = |a: &Axis, max: u32| {
            AxisMap::new(flip(&a.pairs), flip(&a.moves), a.lone_other.clone(), a.lone_base.clone(), max)
        };
        SheetMap { renamed_to: None, rows: axis(&self.rows, MAX_ROW), cols: axis(&self.cols, MAX_COL) }
    }
}

/// Aligns one sheet pair. Alignment breaks its ties by position, so which
/// grid is the base would otherwise decide them: the grids go in in a fixed
/// order and the result is turned round, and swapped files give the
/// mirrored result.
pub(super) fn align_grids(gb: &Grid, go: &Grid, options: CompareOptions) -> Aligned {
    if options.align == AlignMode::Content && gb.digest() > go.digest() {
        let al = align_in_order(go, gb, options);
        return Aligned { cols: al.cols.flipped(), rows: al.rows.flipped(), ..al };
    }
    align_in_order(gb, go, options)
}

pub(super) fn align_in_order(gb: &Grid, go: &Grid, options: CompareOptions) -> Aligned {
    let col_numbers = |g: &Grid| g.cols.iter().map(|(c, _)| *c).collect::<Vec<u32>>();
    let row_numbers = |g: &Grid| g.rows.iter().map(|r| r.index).collect::<Vec<u32>>();
    let (bc, oc, br, or) = (col_numbers(gb), col_numbers(go), row_numbers(gb), row_numbers(go));
    match options.align {
        AlignMode::Position => Aligned {
            cols: Axis::identity(&bc, &oc),
            rows: Axis::identity(&br, &or),
            method: "position",
            fell_back: false,
        },
        AlignMode::Content => {
            let col_al = align::align(&gb.column_items(), &go.column_items(), Options { positional_equal_gaps: true });
            let cols = Axis::new(&col_al, &bc, &oc);
            // Paired columns share an id in row tokens; moved columns too.
            let mut base_ids = HashMap::new();
            let mut other_ids = HashMap::new();
            for (k, &(cb, co)) in cols.pairs.iter().chain(&cols.moves).enumerate() {
                base_ids.insert(cb, k as u32);
                other_ids.insert(co, k as u32);
            }
            let row_al = align::align(
                &gb.row_items(&base_ids),
                &go.row_items(&other_ids),
                Options { positional_equal_gaps: false },
            );
            let fell_back = col_al.fell_back || row_al.fell_back;
            Aligned { cols, rows: Axis::new(&row_al, &br, &or), method: "content", fell_back }
        }
    }
}
