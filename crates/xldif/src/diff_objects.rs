//! Comparison of what lies beside the cell values: formats, row and column
//! layout, merged ranges, comments, data validation, conditional
//! formatting, and hyperlinks. Everything tied to a cell or range is
//! carried through the sheet's alignment first, so an inserted row does
//! not flag every comment or merge below it.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::cellref::{Range, cell_name, col_name};
use crate::diff::{Change, Kind, Location, Property, Status};
use crate::formula::{SheetMap, map_range};
use crate::links::LinkMap;
use crate::model::{LineProps, RangeRule, Sheet};
use crate::styles::Styles;

/// One side of a sheet pair.
#[derive(Clone, Copy)]
pub struct Side<'a> {
    pub sheet: &'a Sheet,
    pub styles: &'a Styles,
}

fn change(id: String, kind: Kind, status: Status, base: Option<Location>, other: Option<Location>) -> Change {
    Change { id, kind, status, base, other, delta: None, expected: None, labels: None, properties: Vec::new() }
}

fn range_location(r: Range) -> Location {
    Location { range: Some(r.name()), ..Location::default() }
}

fn prop(name: &str, base: impl Into<String>, other: impl Into<String>) -> Property {
    Property { name: name.to_owned(), base: base.into(), other: other.into() }
}

/// The style a cell shows: its own when written, else its row's (when the
/// row sets one), else its column's, else the default (ECMA-376 Part 1,
/// 18.3.1.73 and 18.3.1.13).
fn effective_style(sheet: &Sheet, row: u32, col: u32) -> u32 {
    if let Ok(r) = sheet.rows.binary_search_by_key(&row, |r| r.index)
        && let Ok(c) = sheet.rows[r].cells.binary_search_by_key(&col, |c| c.col)
    {
        return sheet.rows[r].cells[c].style;
    }
    if let Some(style) = line_at(&sheet.layout.rows, row).and_then(|l| l.style) {
        return style;
    }
    line_at(&sheet.layout.cols, col).and_then(|l| l.style).unwrap_or(0)
}

/// The record covering `n`: the reader leaves records sorted and disjoint,
/// so it is the last one starting at or before `n`, if it reaches `n`.
fn line_at(records: &[LineProps], n: u32) -> Option<&LineProps> {
    let k = records.partition_point(|l| l.index <= n);
    k.checked_sub(1).map(|k| &records[k]).filter(|l| l.last >= n)
}

/// Property changes between two styles: `(property, base, other)`.
type StyleDiff = Vec<(&'static str, String, String)>;
/// A changed cell: other row, other column, base row, base column, and the
/// `(base, other)` style pair.
type FormatCell = (u32, u32, u32, u32, (u32, u32));
/// A horizontal run: other first and last column, base first and last
/// column, and the style pair.
type FormatRun = (u32, u32, u32, u32, (u32, u32));

/// Format changes on every written cell of either side, folded into
/// rectangles that share the same property changes.
fn format_changes(id: &str, b: Side, o: Side, fwd: &SheetMap, inv: &SheetMap) -> Vec<Change> {
    let mut cache: HashMap<(u32, u32), StyleDiff> = HashMap::new();
    let mut diffs = |sb: u32, so: u32| -> bool {
        !cache.entry((sb, so)).or_insert_with(|| b.styles.format(sb).differences(&o.styles.format(so))).is_empty()
    };
    let mut cells: Vec<FormatCell> = Vec::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    for (rb, cell) in b.sheet.cells() {
        let (Some(ro), Some(co)) = (fwd.rows.map(rb), fwd.cols.map(cell.col)) else { continue };
        seen.insert((ro, co));
        let key = (cell.style, effective_style(o.sheet, ro, co));
        if diffs(key.0, key.1) {
            cells.push((ro, co, rb, cell.col, key));
        }
    }
    for (ro, cell) in o.sheet.cells() {
        if seen.contains(&(ro, cell.col)) {
            continue;
        }
        let (Some(rb), Some(cb)) = (inv.rows.map(ro), inv.cols.map(cell.col)) else { continue };
        let key = (effective_style(b.sheet, rb, cb), cell.style);
        if diffs(key.0, key.1) {
            cells.push((ro, cell.col, rb, cb, key));
        }
    }
    cells.sort_unstable();

    // Horizontal runs, then stacked into rectangles.
    let mut runs: Vec<(u32, u32, FormatRun)> = Vec::new(); // (ro, rb, run)
    for &(ro, co, rb, cb, key) in &cells {
        if let Some((r, rbb, run)) = runs.last_mut()
            && *r == ro
            && *rbb == rb
            && run.4 == key
            && run.1 + 1 == co
            && run.3 + 1 == cb
        {
            run.1 = co;
            run.3 = cb;
            continue;
        }
        runs.push((ro, rb, (co, co, cb, cb, key)));
    }
    // Open rectangles by run shape: (ro_start, ro_end, rb_start, rb_end).
    let mut open: BTreeMap<FormatRun, (u32, u32, u32, u32)> = BTreeMap::new();
    let mut done: Vec<(FormatRun, (u32, u32, u32, u32))> = Vec::new();
    for (ro, rb, run) in runs {
        match open.get_mut(&run) {
            Some(rect) if rect.1 + 1 == ro && rect.3 + 1 == rb => {
                rect.1 = ro;
                rect.3 = rb;
            }
            _ => {
                if let Some(rect) = open.insert(run, (ro, ro, rb, rb)) {
                    done.push((run, rect));
                }
            }
        }
    }
    done.extend(open);
    done.sort_by_key(|(run, rect)| (rect.0, run.0));
    done.into_iter()
        .map(|((co1, co2, cb1, cb2, key), (ro1, ro2, rb1, rb2))| {
            let other = Range { first_row: ro1, first_col: co1, last_row: ro2, last_col: co2 };
            let base = Range { first_row: rb1, first_col: cb1, last_row: rb2, last_col: cb2 };
            let loc = |r: Range| Location {
                cell: Some(cell_name(r.first_row, r.first_col)),
                range: (r.first_row != r.last_row || r.first_col != r.last_col).then(|| r.name()),
                ..Location::default()
            };
            let mut c = change(
                format!("{id}:format:{}:{}", base.name(), other.name()),
                Kind::Format,
                Status::Changed,
                Some(loc(base)),
                Some(loc(other)),
            );
            c.properties = cache[&key].iter().map(|(n, x, y)| prop(n, x.clone(), y.clone())).collect();
            c
        })
        .collect()
}

/// Row or column records compared through the alignment: hand-set size,
/// hidden state, and the style the line gives its unwritten cells.
/// Neighbouring lines with the same change fold together.
fn layout_changes(
    id: &str,
    kind: Kind,
    b: Side,
    o: Side,
    fwd: &crate::formula::AxisMap,
    inv: &crate::formula::AxisMap,
) -> Vec<Change> {
    let (base, other) = if kind == Kind::Row {
        (&b.sheet.layout.rows, &o.sheet.layout.rows)
    } else {
        (&b.sheet.layout.cols, &o.sheet.layout.cols)
    };
    // Lines named by any record, in order; the records are disjoint, so
    // these are at most the grid's extent.
    let lines = |records: &[LineProps]| -> Vec<u32> { records.iter().flat_map(|l| l.index..=l.last).collect() };
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    let mut seen = HashSet::new();
    for nb in lines(base) {
        if let Some(no) = fwd.map(nb) {
            seen.insert(no);
            pairs.push((no, nb));
        }
    }
    for no in lines(other) {
        if !seen.contains(&no)
            && let Some(nb) = inv.map(no)
        {
            pairs.push((no, nb));
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let size_name = if kind == Kind::Row { "height" } else { "width" };
    let mut style_cache: HashMap<(u32, u32), Vec<Property>> = HashMap::new();
    let mut out: Vec<(u32, u32, u32, u32, Vec<Property>)> = Vec::new();
    for (no, nb) in pairs {
        let (lb, lo) = (line_at(base, nb), line_at(other, no));
        let size = |l: Option<&LineProps>| l.and_then(|l| l.size.clone());
        let hidden = |l: Option<&LineProps>| l.is_some_and(|l| l.hidden);
        let style = |l: Option<&LineProps>| l.and_then(|l| l.style).unwrap_or(0);
        let show = |s: &Option<String>| s.clone().unwrap_or_else(|| "default".to_owned());
        let mut properties = Vec::new();
        if size(lb) != size(lo) {
            properties.push(prop(size_name, show(&size(lb)), show(&size(lo))));
        }
        if hidden(lb) != hidden(lo) {
            properties.push(prop(
                "hidden",
                if hidden(lb) { "yes" } else { "no" },
                if hidden(lo) { "yes" } else { "no" },
            ));
        }
        let (sb, so) = (style(lb), style(lo));
        properties.extend(
            style_cache
                .entry((sb, so))
                .or_insert_with(|| {
                    b.styles
                        .format(sb)
                        .differences(&o.styles.format(so))
                        .into_iter()
                        .map(|(n, x, y)| prop(&format!("style.{n}"), x, y))
                        .collect()
                })
                .iter()
                .cloned(),
        );
        if properties.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut()
            && last.1 + 1 == no
            && last.3 + 1 == nb
            && last.4 == properties
        {
            last.1 = no;
            last.3 = nb;
            continue;
        }
        out.push((no, no, nb, nb, properties));
    }
    out.into_iter()
        .map(|(o1, o2, b1, b2, properties)| {
            let name = |a: u32, b: u32| {
                let one = |n: u32| if kind == Kind::Row { n.to_string() } else { col_name(n) };
                if a == b { one(a) } else { format!("{}:{}", one(a), one(b)) }
            };
            let loc = |a: u32, b: u32| {
                let (row, column) = if kind == Kind::Row { (Some(a), None) } else { (None, Some(col_name(a))) };
                Location { row, column, range: (a != b).then(|| name(a, b)), ..Location::default() }
            };
            // `:layout` keeps the id apart from the move of the same row or
            // column, which names the same two numbers.
            let mut c = change(
                format!("{id}:{}:{}:{}:layout", kind.as_str(), name(b1, b2), name(o1, o2)),
                kind,
                Status::Changed,
                Some(loc(b1, b2)),
                Some(loc(o1, o2)),
            );
            c.properties = properties;
            c
        })
        .collect()
}

fn merge_changes(id: &str, b: Side, o: Side, fwd: &SheetMap) -> Vec<Change> {
    let mapped: Vec<(Range, Range)> =
        b.sheet.merges.iter().filter_map(|&r| map_range(r, fwd).map(|m| (r, m))).collect();
    let other: HashSet<Range> = o.sheet.merges.iter().copied().collect();
    let moved: HashSet<Range> = mapped.iter().map(|p| p.1).collect();
    let mut out = Vec::new();
    for (r, m) in &mapped {
        if !other.contains(m) {
            out.push(change(
                format!("{id}:merge:{}:-", r.name()),
                Kind::Merge,
                Status::OnlyBase,
                Some(range_location(*r)),
                None,
            ));
        }
    }
    for r in &o.sheet.merges {
        if !moved.contains(r) {
            out.push(change(
                format!("{id}:merge:-:{}", r.name()),
                Kind::Merge,
                Status::OnlyOther,
                None,
                Some(range_location(*r)),
            ));
        }
    }
    out
}

fn comment_changes(id: &str, b: Side, o: Side, fwd: &SheetMap) -> Vec<Change> {
    let at = |row: u32, col: u32| Location { cell: Some(cell_name(row, col)), ..Location::default() };
    let other: HashMap<(u32, u32), &crate::model::Comment> =
        o.sheet.comments.iter().map(|c| ((c.row, c.col), c)).collect();
    let mut matched = HashSet::new();
    let mut out = Vec::new();
    for cb in &b.sheet.comments {
        let target = fwd.rows.map(cb.row).zip(fwd.cols.map(cb.col));
        let base_loc = Location { text: Some(cb.text.clone()), ..at(cb.row, cb.col) };
        match target.and_then(|t| other.get(&t).map(|c| (t, c))) {
            Some((t, co)) => {
                matched.insert(t);
                let mut properties = Vec::new();
                if cb.text != co.text {
                    properties.push(prop("text", cb.text.clone(), co.text.clone()));
                }
                if cb.author != co.author {
                    properties.push(prop("author", cb.author.clone(), co.author.clone()));
                }
                if !properties.is_empty() {
                    let mut c = change(
                        format!("{id}:comment:{}:{}", cell_name(cb.row, cb.col), cell_name(t.0, t.1)),
                        Kind::Comment,
                        Status::Changed,
                        Some(base_loc),
                        Some(Location { text: Some(co.text.clone()), ..at(t.0, t.1) }),
                    );
                    c.properties = properties;
                    out.push(c);
                }
            }
            None if target.is_some() => out.push(change(
                format!("{id}:comment:{}:-", cell_name(cb.row, cb.col)),
                Kind::Comment,
                Status::OnlyBase,
                Some(base_loc),
                None,
            )),
            // The comment's row or column was deleted; that change covers it.
            None => {}
        }
    }
    for co in &o.sheet.comments {
        if !matched.contains(&(co.row, co.col)) {
            out.push(change(
                format!("{id}:comment:-:{}", cell_name(co.row, co.col)),
                Kind::Comment,
                Status::OnlyOther,
                None,
                Some(Location { text: Some(co.text.clone()), ..at(co.row, co.col) }),
            ));
        }
    }
    out
}

fn ranges_name(ranges: &[Range]) -> String {
    ranges.iter().map(Range::name).collect::<Vec<_>>().join(" ")
}

/// A rule with its (mapped, sorted) ranges, as matched.
type Placed<'a> = (Vec<Range>, &'a RangeRule);
/// How rules are keyed in one matching pass.
type RuleKey<'k> = &'k dyn Fn(&Placed<'_>, Option<Carry<'_>>) -> String;

/// What carries a base rule's formulas to the other workbook: the sheet
/// pair's alignment and the workbooks' external links.
type Carry<'a> = (&'a SheetMap, &'a LinkMap);

/// Rules over ranges (validation, conditional formatting, hyperlinks),
/// matched in three passes: the same (mapped) ranges and rule is no change;
/// then the same ranges with a different rule is a rule change; then the
/// same rule on different ranges is a ranges change. What is left is
/// one-sided.
/// A rule as compared: its description and its formulas, normalized (and,
/// for the base side, carried through the alignment).
fn rule_text(rule: &RangeRule, carry: Option<Carry<'_>>) -> String {
    let mut text = rule.rule.clone();
    for f in &rule.formulas {
        let tokens = crate::formula::tokenize(f);
        let tokens = match carry {
            // The other workbook's link numbers first, then its rows and columns.
            Some((map, links)) => {
                let relinked = links.relink(&tokens);
                crate::formula::remap(relinked.as_deref().unwrap_or(&tokens), map, |_| None)
            }
            None => tokens,
        };
        text.push_str("; formula=");
        text.push_str(&crate::formula::render(&tokens).to_uppercase());
    }
    text
}

fn rule_changes(id: &str, kind: Kind, base: &[RangeRule], other: &[RangeRule], (fwd, links): Carry<'_>) -> Vec<Change> {
    let sorted = |ranges: &[Range]| {
        let mut m = ranges.to_vec();
        m.sort_unstable();
        m
    };
    let mapped = |r: &RangeRule| -> Option<Vec<Range>> {
        let m: Vec<Range> = r.ranges.iter().filter_map(|&x| map_range(x, fwd)).collect();
        (!m.is_empty()).then(|| sorted(&m))
    };
    // Base rules whose ranges survive, and every other-side rule, by index.
    let left: Vec<Placed<'_>> = base.iter().filter_map(|r| mapped(r).map(|m| (m, r))).collect();
    let right: Vec<Placed<'_>> = other.iter().map(|r| (sorted(&r.ranges), r)).collect();
    let mut left_done = vec![false; left.len()];
    let mut right_done = vec![false; right.len()];
    // Index of other-side rules by a key, consumed in order.
    let index = |key: RuleKey<'_>| {
        let mut map: HashMap<String, std::collections::VecDeque<usize>> = HashMap::new();
        for (j, r) in right.iter().enumerate() {
            map.entry(key(r, None)).or_default().push_back(j);
        }
        map
    };
    let both = |r: &Placed<'_>, carry: Option<Carry<'_>>| format!("{}|{}", ranges_name(&r.0), rule_text(r.1, carry));
    let by_ranges = |r: &Placed<'_>, _: Option<Carry<'_>>| ranges_name(&r.0);
    let by_rule = |r: &Placed<'_>, carry: Option<Carry<'_>>| rule_text(r.1, carry);
    let loc = |ranges: &[Range], rule: &RangeRule| Location {
        range: Some(ranges_name(ranges)),
        text: Some(rule_text(rule, None)),
        ..Location::default()
    };
    let mut out = Vec::new();
    for (pass, key) in [(0, &both as RuleKey<'_>), (1, &by_ranges), (2, &by_rule)] {
        let mut available = index(key);
        for (i, l) in left.iter().enumerate() {
            if left_done[i] {
                continue;
            }
            let Some(queue) = available.get_mut(&key(l, Some((fwd, links)))) else { continue };
            let Some(j) = std::iter::from_fn(|| queue.pop_front()).find(|&j| !right_done[j]) else { continue };
            left_done[i] = true;
            right_done[j] = true;
            if pass == 0 {
                continue;
            }
            let (n, s) = &right[j];
            let (name, before, after) = if pass == 1 {
                ("rule", rule_text(l.1, None), rule_text(s, None))
            } else {
                ("ranges", ranges_name(&l.0), ranges_name(n))
            };
            let mut c = change(
                format!("{id}:{}:{}:{}", kind.as_str(), ranges_name(&l.1.ranges), ranges_name(n)),
                kind,
                Status::Changed,
                Some(loc(&l.1.ranges, l.1)),
                Some(loc(n, s)),
            );
            c.properties = vec![prop(name, before, after)];
            out.push(c);
        }
    }
    for (_, (_, r)) in left.iter().enumerate().filter(|(i, _)| !left_done[*i]) {
        out.push(change(
            format!("{id}:{}:{}:-", kind.as_str(), ranges_name(&r.ranges)),
            kind,
            Status::OnlyBase,
            Some(loc(&r.ranges, r)),
            None,
        ));
    }
    for (_, (n, s)) in right.iter().enumerate().filter(|(j, _)| !right_done[*j]) {
        out.push(change(
            format!("{id}:{}:-:{}", kind.as_str(), ranges_name(n)),
            kind,
            Status::OnlyOther,
            None,
            Some(loc(n, s)),
        ));
    }
    out
}

fn hyperlink_changes(id: &str, b: Side, o: Side, carry: Carry<'_>) -> Vec<Change> {
    let to_rules = |links: &[crate::model::Hyperlink]| -> Vec<RangeRule> {
        links
            .iter()
            .map(|h| RangeRule { ranges: vec![h.range].into(), rule: h.target.clone(), formulas: Vec::new() })
            .collect()
    };
    rule_changes(id, Kind::Hyperlink, &to_rules(&b.sheet.hyperlinks), &to_rules(&o.sheet.hyperlinks), carry)
}

/// Every non-cell change of one sheet pair.
pub fn sheet_object_changes(
    id: &str,
    b: Side,
    o: Side,
    fwd: &SheetMap,
    inv: &SheetMap,
    links: &LinkMap,
) -> Vec<Change> {
    let mut out = format_changes(id, b, o, fwd, inv);
    out.extend(layout_changes(id, Kind::Row, b, o, &fwd.rows, &inv.rows));
    out.extend(layout_changes(id, Kind::Column, b, o, &fwd.cols, &inv.cols));
    out.extend(merge_changes(id, b, o, fwd));
    out.extend(comment_changes(id, b, o, fwd));
    out.extend(rule_changes(id, Kind::Validation, &b.sheet.validations, &o.sheet.validations, (fwd, links)));
    out.extend(rule_changes(
        id,
        Kind::ConditionalFormat,
        &b.sheet.conditional_formats,
        &o.sheet.conditional_formats,
        (fwd, links),
    ));
    out.extend(hyperlink_changes(id, b, o, (fwd, links)));
    if b.sheet.visibility != o.sheet.visibility {
        let mut c = change(format!("{id}:sheet:visibility"), Kind::Sheet, Status::Changed, None, None);
        c.properties = vec![prop(
            "visibility",
            format!("{:?}", b.sheet.visibility).to_lowercase(),
            format!("{:?}", o.sheet.visibility).to_lowercase(),
        )];
        out.push(c);
    }
    // Several objects can share a location (two rules on one range); their
    // ids stay unique, and stable as long as the rules keep their order.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for c in &mut out {
        let n = seen.entry(c.id.clone()).or_default();
        *n += 1;
        if *n > 1 {
            c.id = format!("{}#{n}", c.id);
        }
    }
    out
}
