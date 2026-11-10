//! Comparison of what lies beside the cell values: formats, row and column
//! layout, merged ranges, comments, data validation, conditional
//! formatting, and hyperlinks. Everything tied to a cell or range is
//! carried through the sheet's alignment first, so an inserted row does
//! not flag every comment or merge below it.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::cellref::{Range, cell_name, col_name};
use crate::diff::{Change, Kind, Location, Property, Status};
use crate::formula::{SheetMap, map_range};
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

fn line_at(records: &[LineProps], n: u32) -> Option<&LineProps> {
    records.iter().find(|l| (l.index..=l.last).contains(&n))
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

/// Row or column records compared through the alignment: hand-set size and
/// hidden state. Neighbouring lines with the same change fold together.
fn layout_changes(
    id: &str,
    kind: Kind,
    base: &[LineProps],
    other: &[LineProps],
    fwd: &crate::formula::AxisMap,
    inv: &crate::formula::AxisMap,
) -> Vec<Change> {
    let props = |l: Option<&LineProps>| (l.and_then(|l| l.size.clone()), l.is_some_and(|l| l.hidden));
    let lines = |records: &[LineProps]| -> Vec<u32> {
        // Records covering the rest of the sheet (hidden unused columns) are
        // compared line by line up to a bound.
        records.iter().flat_map(|l| l.index..=l.last.min(l.index + 16_384)).collect()
    };
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
    let mut out: Vec<(u32, u32, u32, u32, Vec<Property>)> = Vec::new();
    for (no, nb) in pairs {
        let (pb, po) = (props(line_at(base, nb)), props(line_at(other, no)));
        if pb == po {
            continue;
        }
        let show = |s: &Option<String>| s.clone().unwrap_or_else(|| "default".to_owned());
        let mut properties = Vec::new();
        if pb.0 != po.0 {
            properties.push(prop(size_name, show(&pb.0), show(&po.0)));
        }
        if pb.1 != po.1 {
            properties.push(prop("hidden", if pb.1 { "yes" } else { "no" }, if po.1 { "yes" } else { "no" }));
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
            let mut c = change(
                format!("{id}:{}:{}:{}", kind.as_str(), name(b1, b2), name(o1, o2)),
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

/// Rules over ranges (validation, conditional formatting): a rule on the
/// same (mapped) ranges with different content is a change; others are
/// one-sided.
fn rule_changes(id: &str, kind: Kind, base: &[RangeRule], other: &[RangeRule], fwd: &SheetMap) -> Vec<Change> {
    let map_all = |r: &RangeRule| -> Option<Vec<Range>> {
        let mut m: Vec<Range> = r.ranges.iter().filter_map(|&x| map_range(x, fwd)).collect();
        m.sort_unstable();
        (!m.is_empty()).then_some(m)
    };
    let sorted = |r: &RangeRule| {
        let mut m = r.ranges.clone();
        m.sort_unstable();
        m
    };
    let mut left: Vec<(Vec<Range>, &RangeRule)> = base.iter().filter_map(|r| map_all(r).map(|m| (m, r))).collect();
    let mut right: Vec<(Vec<Range>, &RangeRule)> = other.iter().map(|r| (sorted(r), r)).collect();
    // Exact matches first.
    left.retain(|(m, r)| {
        if let Some(i) = right.iter().position(|(n, s)| n == m && s.rule == r.rule) {
            right.remove(i);
            false
        } else {
            true
        }
    });
    let loc = |ranges: &[Range], rule: &str| Location {
        range: Some(ranges_name(ranges)),
        text: Some(rule.to_owned()),
        ..Location::default()
    };
    let mut out = Vec::new();
    // The same rule over different ranges: its ranges changed.
    left.retain(|(m, r)| {
        let Some(i) = right.iter().position(|(_, s)| s.rule == r.rule) else { return true };
        let (n, s) = right.remove(i);
        let mut c = change(
            format!("{id}:{}:{}:{}", kind.as_str(), ranges_name(&r.ranges), ranges_name(&n)),
            kind,
            Status::Changed,
            Some(loc(&r.ranges, &r.rule)),
            Some(loc(&n, &s.rule)),
        );
        c.properties = vec![prop("ranges", ranges_name(m), ranges_name(&n))];
        out.push(c);
        false
    });
    for (m, r) in left {
        if let Some(i) = right.iter().position(|(n, _)| *n == m) {
            let (n, s) = right.remove(i);
            let mut c = change(
                format!("{id}:{}:{}:{}", kind.as_str(), ranges_name(&r.ranges), ranges_name(&n)),
                kind,
                Status::Changed,
                Some(loc(&r.ranges, &r.rule)),
                Some(loc(&n, &s.rule)),
            );
            c.properties = vec![prop("rule", r.rule.clone(), s.rule.clone())];
            out.push(c);
        } else {
            out.push(change(
                format!("{id}:{}:{}:-", kind.as_str(), ranges_name(&r.ranges)),
                kind,
                Status::OnlyBase,
                Some(loc(&r.ranges, &r.rule)),
                None,
            ));
        }
    }
    for (n, s) in right {
        out.push(change(
            format!("{id}:{}:-:{}", kind.as_str(), ranges_name(&n)),
            kind,
            Status::OnlyOther,
            None,
            Some(loc(&n, &s.rule)),
        ));
    }
    out
}

fn hyperlink_changes(id: &str, b: Side, o: Side, fwd: &SheetMap) -> Vec<Change> {
    let to_rules = |links: &[crate::model::Hyperlink]| -> Vec<RangeRule> {
        links.iter().map(|h| RangeRule { ranges: vec![h.range], rule: h.target.clone() }).collect()
    };
    rule_changes(id, Kind::Hyperlink, &to_rules(&b.sheet.hyperlinks), &to_rules(&o.sheet.hyperlinks), fwd)
}

/// Every non-cell change of one sheet pair.
pub fn sheet_object_changes(id: &str, b: Side, o: Side, fwd: &SheetMap, inv: &SheetMap) -> Vec<Change> {
    let mut out = format_changes(id, b, o, fwd, inv);
    out.extend(layout_changes(id, Kind::Row, &b.sheet.layout.rows, &o.sheet.layout.rows, &fwd.rows, &inv.rows));
    out.extend(layout_changes(id, Kind::Column, &b.sheet.layout.cols, &o.sheet.layout.cols, &fwd.cols, &inv.cols));
    out.extend(merge_changes(id, b, o, fwd));
    out.extend(comment_changes(id, b, o, fwd));
    out.extend(rule_changes(id, Kind::Validation, &b.sheet.validations, &o.sheet.validations, fwd));
    out.extend(rule_changes(
        id,
        Kind::ConditionalFormat,
        &b.sheet.conditional_formats,
        &o.sheet.conditional_formats,
        fwd,
    ));
    out.extend(hyperlink_changes(id, b, o, fwd));
    if b.sheet.visibility != o.sheet.visibility {
        let mut c = change(format!("{id}:sheet:visibility"), Kind::Sheet, Status::Changed, None, None);
        c.properties = vec![prop(
            "visibility",
            format!("{:?}", b.sheet.visibility).to_lowercase(),
            format!("{:?}", o.sheet.visibility).to_lowercase(),
        )];
        out.push(c);
    }
    out
}
