//! The side-by-side HTML report: one self-contained file, no external
//! resources. Each sheet pair is shown as two grids whose rows and columns
//! are aligned the way the comparison paired them, with inserted, deleted,
//! and moved rows and columns marked, changed cells colored by kind, and a
//! details panel for the selected change.
//!
//! The report is built from a comparison [`Listing`] and the two workbooks;
//! it adds no comparison logic of its own.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::cellref::{Range, cell_name, col_name};
use crate::diff::{Change, Kind, Listing, SheetComparison, Status};
use crate::model::{Cell, Sheet, Value, Workbook};

/// Rows with a change, plus this many neighbours, stay visible in
/// "changes only" view.
const CONTEXT_ROWS: usize = 2;
/// Above this many cells in a sheet's grid, only changed rows and their
/// context are rendered.
const MAX_GRID_CELLS: usize = 250_000;

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// JSON text for embedding in `<script type="application/json">`: `<` is
/// escaped so the data can never close the script element.
fn json_for_script<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("report data serializes").replace('<', "\\u003c")
}

/// One position in an aligned axis: the base number, the other number, or
/// both, and what happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    base: Option<u32>,
    other: Option<u32>,
    status: Status,
}

/// A cell's text as shown: numbers with digit grouping when they are whole,
/// booleans and errors as Excel spells them.
fn shown(v: &Value) -> String {
    match v {
        Value::Empty => String::new(),
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => group(*n),
        Value::Number(n) => format!("{n}"),
        Value::Text(t) => t.to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Value::Error(e) => e.clone(),
    }
}

fn group(n: f64) -> String {
    // Callers pass whole numbers below 1e15, which a u64 holds exactly.
    #[allow(clippy::cast_sign_loss)]
    let digits = format!("{}", n.abs() as u64);
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0.0 { format!("-{out}") } else { out }
}

fn cell_value(sheet: &Sheet, row: u32, col: u32) -> Option<&crate::model::Cell> {
    let r = sheet.rows.binary_search_by_key(&row, |r| r.index).ok()?;
    let cells = &sheet.rows[r].cells;
    cells.binary_search_by_key(&col, |c| c.col).ok().map(|i| &cells[i])
}

/// Rebuilds an axis's slots in display order: the paired numbers from the
/// alignment runs, moved items at their place on the other side, and the
/// one-sided numbers, each between the pairs that surround it.
fn slots(runs: &[[u32; 3]], moves: &[[u32; 3]], lone_base: &[u32], lone_other: &[u32]) -> Vec<Slot> {
    let mut pairs: Vec<(u32, u32)> = runs.iter().flat_map(|&[b, o, n]| (0..n).map(move |k| (b + k, o + k))).collect();
    pairs.sort_unstable();
    // Moves and other-side inserts both sit at their other-side position.
    let mut other_side: Vec<Slot> = moves
        .iter()
        .flat_map(|&[b, o, n]| {
            (0..n).map(move |k| Slot { base: Some(b + k), other: Some(o + k), status: Status::Moved })
        })
        .chain(lone_other.iter().map(|&o| Slot { base: None, other: Some(o), status: Status::OnlyOther }))
        .collect();
    other_side.sort_unstable_by_key(|s| s.other);
    let mut lb = lone_base.to_vec();
    lb.sort_unstable();
    let (mut ib, mut io) = (0, 0);
    let mut out = Vec::new();
    let flush = |b_limit: u32, o_limit: u32, out: &mut Vec<Slot>, ib: &mut usize, io: &mut usize| {
        while *ib < lb.len() && lb[*ib] < b_limit {
            out.push(Slot { base: Some(lb[*ib]), other: None, status: Status::OnlyBase });
            *ib += 1;
        }
        while *io < other_side.len() && other_side[*io].other.is_some_and(|o| o < o_limit) {
            out.push(other_side[*io]);
            *io += 1;
        }
    };
    for &(b, o) in &pairs {
        flush(b, o, &mut out, &mut ib, &mut io);
        out.push(Slot { base: Some(b), other: Some(o), status: Status::Same });
    }
    flush(u32::MAX, u32::MAX, &mut out, &mut ib, &mut io);
    out
}

/// Every cell a change touches on one side: its cell, or each cell of its
/// ranges (`A1:B2 D5` too), up to a bound; a larger object marks no cells
/// and is listed in the details panel instead.
fn touched(loc: Option<&crate::diff::Location>) -> Vec<(u32, u32)> {
    const MAX_MARKED: usize = 10_000;
    let Some(l) = loc else { return Vec::new() };
    if let Some(ranges) = &l.range {
        let parsed: Vec<Range> = ranges.split_whitespace().filter_map(Range::parse).collect();
        let area: usize = parsed
            .iter()
            .map(|r| (r.last_row - r.first_row + 1) as usize * (r.last_col - r.first_col + 1) as usize)
            .sum();
        if !parsed.is_empty() {
            if area > MAX_MARKED {
                return Vec::new();
            }
            return parsed
                .iter()
                .flat_map(|r| {
                    (r.first_row..=r.last_row).flat_map(move |row| (r.first_col..=r.last_col).map(move |c| (row, c)))
                })
                .collect();
        }
    }
    l.cell.as_deref().and_then(crate::cellref::parse_cell).into_iter().collect()
}

/// The row (or column) numbers a row or column change covers on one side:
/// its number, or each number of a folded range such as `3:5` or `B:D`.
fn lines(loc: Option<&crate::diff::Location>, kind: Kind) -> Vec<u32> {
    let Some(l) = loc else { return Vec::new() };
    let one = |t: &str| if kind == Kind::Row { t.parse().ok() } else { crate::cellref::parse_col(t) };
    if let Some((a, b)) = l.range.as_deref().and_then(|r| r.split_once(':'))
        && let (Some(a), Some(b)) = (one(a), one(b))
    {
        return (a.min(b)..=a.max(b)).take(20_000).collect();
    }
    if kind == Kind::Row {
        l.row.into_iter().collect()
    } else {
        l.column.as_deref().and_then(crate::cellref::parse_col).into_iter().collect()
    }
}

fn kind_class(k: Kind) -> &'static str {
    match k {
        Kind::Value => "v",
        Kind::Formula => "f",
        Kind::Format => "m",
        _ => "o",
    }
}

fn join_ids(ids: &[usize]) -> String {
    ids.iter().map(usize::to_string).collect::<Vec<_>>().join(" ")
}

/// Content rows and columns of a sheet: those with a value or formula.
fn content_lines(sheet: &Sheet) -> (Vec<u32>, Vec<u32>) {
    let rows: Vec<u32> = sheet.rows.iter().filter(|r| r.cells.iter().any(Cell::has_content)).map(|r| r.index).collect();
    let mut cols: Vec<u32> = sheet.cells().filter(|(_, c)| c.has_content()).map(|(_, c)| c.col).collect();
    cols.sort_unstable();
    cols.dedup();
    (rows, cols)
}

/// Numbers in `content` that no pair or move of the alignment covers.
fn unpaired(content: &[u32], runs: &[[u32; 3]], moves: &[[u32; 3]], base_side: bool) -> Vec<u32> {
    let covered = |n: u32| {
        runs.iter().chain(moves).any(|&[b, o, len]| {
            let start = if base_side { b } else { o };
            (start..start + len).contains(&n)
        })
    };
    content.iter().copied().filter(|&n| !covered(n)).collect()
}

/// One sheet pair's two grids and its per-cell change index.
fn render_sheet(out: &mut String, s: &SheetComparison, base: Option<&Sheet>, other: Option<&Sheet>, index: usize) {
    let Some(al) = &s.alignment else {
        let what =
            if s.status == Status::OnlyBase { "only in the base workbook" } else { "only in the other workbook" };
        let _ = write!(
            out,
            "<section class=\"sheet\" data-s=\"{index}\"><p class=\"lone\">This sheet is {what}.</p></section>"
        );
        return;
    };
    let (Some(base), Some(other)) = (base, other) else { return };
    // The grids follow the alignment, not the listed changes, so filtering
    // the changes (--kind) never drops inserted rows or misaligns the sides.
    let ((b_rows, b_cols), (o_rows, o_cols)) = (content_lines(base), content_lines(other));
    let rows = slots(
        &al.rows,
        &al.row_moves,
        &unpaired(&b_rows, &al.rows, &al.row_moves, true),
        &unpaired(&o_rows, &al.rows, &al.row_moves, false),
    );
    let cols = slots(
        &al.cols,
        &al.col_moves,
        &unpaired(&b_cols, &al.cols, &al.col_moves, true),
        &unpaired(&o_cols, &al.cols, &al.col_moves, false),
    );
    // Only rows and columns with content on either side are drawn: a stray
    // far-away cell must not widen every row to the whole grid.
    let in_list = |list: &[u32], n: Option<u32>| n.is_some_and(|n| list.binary_search(&n).is_ok());
    let rows: Vec<Slot> = rows.into_iter().filter(|r| in_list(&b_rows, r.base) || in_list(&o_rows, r.other)).collect();
    let cols: Vec<Slot> = cols.into_iter().filter(|c| in_list(&b_cols, c.base) || in_list(&o_cols, c.other)).collect();

    // Which change ids touch each cell, row, and column, by side.
    let mut on_cell: [HashMap<(u32, u32), Vec<usize>>; 2] = [HashMap::new(), HashMap::new()];
    let mut on_row: [HashMap<u32, Vec<usize>>; 2] = [HashMap::new(), HashMap::new()];
    let mut on_col: [HashMap<u32, Vec<usize>>; 2] = [HashMap::new(), HashMap::new()];
    for (i, c) in s.changes.iter().enumerate() {
        for (side, loc) in [(0, c.base.as_ref()), (1, c.other.as_ref())] {
            match c.kind {
                Kind::Row => {
                    for n in lines(loc, Kind::Row) {
                        on_row[side].entry(n).or_default().push(i);
                    }
                }
                Kind::Column => {
                    for n in lines(loc, Kind::Column) {
                        on_col[side].entry(n).or_default().push(i);
                    }
                }
                _ => {
                    for p in touched(loc) {
                        on_cell[side].entry(p).or_default().push(i);
                    }
                }
            }
        }
    }
    let marked_rows: [std::collections::HashSet<u32>; 2] =
        [0, 1].map(|side| on_cell[side].keys().map(|k| k.0).collect());
    // Content in an inserted, deleted, or moved column is a change for every
    // row that has some.
    let changed_cols: [std::collections::HashSet<u32>; 2] = [0, 1].map(|side| {
        cols.iter()
            .filter(|c| c.status != Status::Same)
            .filter_map(|c| if side == 0 { c.base } else { c.other })
            .chain(on_col[side].keys().copied())
            .collect()
    });
    let touches_changed_col = |sheet: &Sheet, side: usize, r: Option<u32>| {
        r.and_then(|r| cell_row(sheet, r))
            .is_some_and(|cells| cells.iter().any(|c| c.has_content() && changed_cols[side].contains(&c.col)))
    };
    let row_changed: Vec<bool> = rows
        .iter()
        .map(|r| {
            r.status != Status::Same
                || r.base.is_some_and(|b| on_row[0].contains_key(&b) || marked_rows[0].contains(&b))
                || r.other.is_some_and(|o| on_row[1].contains_key(&o) || marked_rows[1].contains(&o))
                || touches_changed_col(base, 0, r.base)
                || touches_changed_col(other, 1, r.other)
        })
        .collect();
    let near: Vec<bool> = (0..rows.len())
        .map(|i| {
            (i.saturating_sub(CONTEXT_ROWS)..=(i + CONTEXT_ROWS).min(rows.len().saturating_sub(1)))
                .any(|j| row_changed[j])
        })
        .collect();
    let trimmed = rows.len() * cols.len().max(1) > MAX_GRID_CELLS;
    let drawn = if trimmed { near.iter().filter(|n| **n).count() } else { rows.len() };
    let _ = write!(out, "<section class=\"sheet\" data-s=\"{index}\">");
    if drawn * cols.len().max(1) > MAX_GRID_CELLS {
        let _ = write!(
            out,
            "<p class=\"warn\" data-nogrid=\"1\">This sheet's changes span too much of it to draw ({} rows × {} columns); every change is listed below.</p></section>",
            drawn,
            cols.len()
        );
        return;
    }
    if trimmed {
        out.push_str("<p class=\"warn\">This sheet is large; only changed rows and their neighbours are shown.</p>");
    }
    let col_class = |c: &Slot| match c.status {
        Status::OnlyBase => " cb",
        Status::OnlyOther => " co",
        Status::Moved => " cm",
        _ => "",
    };
    // One width per column slot, shared by both grids so they stay aligned
    // when scrolled together: from the longest text on either side.
    let widths: Vec<u32> = cols
        .iter()
        .map(|c| {
            let longest = rows
                .iter()
                .enumerate()
                .filter(|(i, _)| !trimmed || near[*i])
                .flat_map(|(_, r)| {
                    [(base, r.base, c.base), (other, r.other, c.other)].into_iter().filter_map(|(sheet, row, col)| {
                        cell_value(sheet, row?, col?).map(|cell| shown(&cell.value).chars().count())
                    })
                })
                .max()
                .unwrap_or(0);
            (u32::try_from(longest).unwrap_or(u32::MAX).saturating_mul(7).saturating_add(14)).clamp(56, 260)
        })
        .collect();
    let total: u32 = 48 + widths.iter().sum::<u32>();
    out.push_str("<div class=\"panes\">");
    for (side, sheet) in [(0usize, base), (1usize, other)] {
        let _ = write!(
            out,
            "<div class=\"pane {}\"><table class=\"grid\" style=\"width:{total}px\"><colgroup><col style=\"width:48px\">",
            if side == 0 { "b" } else { "o" }
        );
        for w in &widths {
            let _ = write!(out, "<col style=\"width:{w}px\">");
        }
        out.push_str("</colgroup><thead><tr><th class=\"rn\"></th>");
        for c in &cols {
            let n = if side == 0 { c.base } else { c.other };
            let ids = n.and_then(|n| on_col[side].get(&n));
            let _ = write!(out, "<th class=\"ch{}\"", col_class(c));
            if let Some(ids) = ids {
                let _ = write!(out, " data-c=\"{}\"", join_ids(ids));
            }
            let _ = write!(out, ">{}</th>", n.map(col_name).unwrap_or_default());
        }
        out.push_str("</tr></thead><tbody>");
        for (i, r) in rows.iter().enumerate() {
            if trimmed && !near[i] {
                continue;
            }
            let n = if side == 0 { r.base } else { r.other };
            let status = match r.status {
                Status::OnlyBase => " rb",
                Status::OnlyOther => " ro",
                Status::Moved => " rm",
                _ => "",
            };
            let flags = format!("{}{}", if row_changed[i] { " chg" } else { "" }, if near[i] { " ctx" } else { "" });
            let _ = write!(out, "<tr class=\"r{status}{flags}\" data-i=\"{i}\"");
            if let Some(ids) = n.and_then(|n| on_row[side].get(&n)) {
                let _ = write!(out, " data-c=\"{}\"", join_ids(ids));
            }
            let _ = write!(out, "><th class=\"rn\">{}</th>", n.map(|n| n.to_string()).unwrap_or_default());
            for c in &cols {
                let col = if side == 0 { c.base } else { c.other };
                let (Some(row), Some(col)) = (n, col) else {
                    out.push_str("<td class=\"void\"></td>");
                    continue;
                };
                let cell = cell_value(sheet, row, col);
                let text = cell.map(|c| shown(&c.value)).unwrap_or_default();
                let ids = on_cell[side].get(&(row, col));
                let mut class = String::from(col_class(c).trim());
                if let Some(ids) = ids {
                    let mut kinds: Vec<&str> = ids.iter().map(|&i| kind_class(s.changes[i].kind)).collect();
                    kinds.sort_unstable();
                    kinds.dedup();
                    for k in kinds {
                        let _ = write!(class, " k{k}");
                    }
                }
                if cell.is_some_and(|c| matches!(c.value, Value::Number(_))) {
                    class.push_str(" num");
                }
                let _ = write!(out, "<td class=\"{}\"", class.trim());
                if let Some(ids) = ids {
                    let _ = write!(out, " data-c=\"{}\" data-a=\"{}\"", join_ids(ids), cell_name(row, col));
                }
                let title = cell
                    .and_then(|c| c.formula.as_ref())
                    .map(|f| format!(" title=\"={}\"", esc(&f.text)))
                    .unwrap_or_default();
                let _ = write!(out, "{title}>{}</td>", esc(&text));
            }
            out.push_str("</tr>");
        }
        out.push_str("</tbody></table></div>");
    }
    out.push_str("</div></section>");
}

fn cell_row(sheet: &Sheet, row: u32) -> Option<&[Cell]> {
    sheet.rows.binary_search_by_key(&row, |r| r.index).ok().map(|i| &sheet.rows[i].cells[..])
}

/// The report's embedded data: every change, by sheet, for the details panel.
#[derive(serde::Serialize)]
struct Data<'a> {
    sheets: Vec<SheetData<'a>>,
    objects: &'a [Change],
}

#[derive(serde::Serialize)]
struct SheetData<'a> {
    changes: &'a [Change],
}

const STYLE: &str = r#"
:root{--bg:#eceef1;--paper:#fff;--ink:#1d2129;--muted:#6b7280;--line:#d3d7dd;--th:#e9ebee;
--del:#ffd5d8;--del-ink:#9b1c1c;--ins:#d3f2db;--ins-ink:#1e6b33;--mov:#fff1b8;--accent:#2f5fd0;--fx:#7c3aed}
*{box-sizing:border-box}
html,body{height:100%}
body{margin:0;display:flex;flex-direction:column;overflow:hidden;background:var(--bg);color:var(--ink);font:13px/1.5 "Malgun Gothic","맑은 고딕","Apple SD Gothic Neo",system-ui,sans-serif}
.bar{display:flex;flex-wrap:wrap;align-items:center;gap:6px 16px;padding:6px 12px;background:var(--paper);border-bottom:1px solid var(--line)}
.bar h1{font-size:15px;margin:0}
.chips span{display:inline-block;margin-right:6px;padding:1px 8px;border-radius:10px;background:var(--bg);font-size:12px}
.ctl{display:flex;flex-wrap:wrap;align-items:center;gap:4px}
.ctl button,.ctl label{font:inherit;font-size:12px;padding:3px 10px;border:1px solid var(--line);border-radius:6px;background:var(--paper);color:var(--ink);cursor:pointer}
.ctl button:hover,.ctl label:hover{border-color:var(--accent)}
.ctl button[aria-pressed=true]{border-color:var(--accent);color:var(--accent)}
kbd{display:inline-block;min-width:1.5em;margin:0 1px;padding:0 4px;border:1px solid #b9bfc8;border-bottom-width:2px;border-radius:4px;background:#f6f7f9;font:600 11px/1.5 ui-monospace,Consolas,monospace;text-align:center}
.ctl button kbd{margin-left:6px}.ctl button kbd:first-child{margin:0 6px 0 0}
#at{min-width:72px;text-align:center;font-variant-numeric:tabular-nums;color:var(--muted)}
.legend{display:flex;gap:10px;font-size:11px;color:var(--muted)}
.legend i{display:inline-block;width:10px;height:10px;margin-right:3px;vertical-align:-1px;border-radius:2px}
.main{flex:1;min-height:0;display:flex}
#toc{flex:none;width:220px;overflow:auto;padding:8px 6px;border-right:1px solid var(--line);font-size:12px}
body.notoc #toc{display:none}
#toc a{display:flex;justify-content:space-between;gap:6px;padding:3px 8px;border-radius:5px;color:var(--ink);text-decoration:none;cursor:pointer}
#toc a:hover{background:var(--paper)}
#toc a.on{background:var(--paper);box-shadow:inset 3px 0 var(--accent)}
#toc a b{font-weight:600;color:var(--accent)}
#toc a.clean b{color:var(--muted);font-weight:400}
.work{flex:1;min-width:0;display:flex;flex-direction:column}
.names{flex:none;display:flex;background:var(--paper);border-bottom:1px solid var(--line)}
.names div{flex:1 1 0;min-width:0;padding:6px 12px;font-weight:700;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.names div+div{border-left:1px solid var(--line)}
.names small{margin-right:8px;font-size:11px}
.names .b small{color:var(--del-ink)}.names .o small{color:var(--ins-ink)}
.sheet{display:none;flex:1;min-height:0}
.sheet.on{display:flex;flex-direction:column}
.panes{flex:1;min-height:0;display:flex}
.pane{flex:1 1 0;min-width:0;overflow:auto;background:var(--paper)}
.pane+.pane{border-left:1px solid var(--line)}
table.grid{border-collapse:separate;border-spacing:0;font-size:12px;table-layout:fixed}
.grid col:first-child{width:48px}
.grid th,.grid td{height:22px;padding:0 6px;border-right:1px solid #e3e6ea;border-bottom:1px solid #e3e6ea;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.grid thead th{position:sticky;top:0;z-index:2;background:var(--th);font-weight:500;color:var(--muted);text-align:center}
.grid th.rn{position:sticky;left:0;z-index:1;min-width:44px;background:var(--th);color:var(--muted);font-weight:400;text-align:right}
.grid thead th.rn{z-index:3}
.grid td.num{text-align:right;font-variant-numeric:tabular-nums}
.grid tr.rb td{background:var(--del)}.grid tr.ro td{background:var(--ins)}.grid tr.rm td{background:var(--mov)}
.grid tr td.void{background:repeating-linear-gradient(135deg,#f6f7f9 0 6px,#eceef1 6px 12px)}
.grid .cb{background:var(--del)}.grid .co{background:var(--ins)}.grid .cm{background:var(--mov)}
.pane.b td.kv{background:var(--del);color:var(--del-ink);font-weight:600}
.pane.o td.kv{background:var(--ins);color:var(--ins-ink);font-weight:600}
td.kf{box-shadow:inset 0 0 0 2px var(--fx)}
td.km{outline:1px dashed #b45309;outline-offset:-3px}
td.ko{box-shadow:inset 3px 0 #0e7490}
td[data-c],th[data-c]{cursor:pointer}
.grid thead th[data-c]{color:var(--ink);font-weight:700}
td.hot,th.hot{outline:2px solid var(--accent)!important;outline-offset:-2px}
body.only .grid tbody tr:not(.ctx){display:none}
.warn,.lone{margin:8px 12px;color:#8a5a00}
#info{display:none;flex:none;max-height:38vh;overflow:auto;padding:6px 14px 10px;background:var(--paper);border-top:2px solid var(--accent)}
body.info #info{display:block}
#info h3{margin:2px 0 6px;font-size:13px}
#info .k{display:inline-block;margin-right:6px;padding:0 6px;border-radius:8px;background:var(--bg);font-size:11px}
table.cl{border-collapse:collapse;width:100%;margin:4px 0;font-size:12px}
table.cl th,table.cl td{border-bottom:1px solid #e5e7eb;padding:2px 6px;text-align:left;vertical-align:top}
table.cl th{width:130px;color:var(--muted);font-weight:500}
table.cl td.d{text-align:right;font-variant-numeric:tabular-nums;font-weight:600}
table.cl tr[data-n]{cursor:pointer}
table.cl tr[data-n]:hover td{background:#f3f6fd}
code{font:12px ui-monospace,Consolas,monospace;white-space:pre-wrap;word-break:break-all}
@media (max-width:900px){.chips,.legend{display:none}#toc{width:150px}}
"#;

const SCRIPT: &str = r#"
(function(){
  var body=document.body, data=JSON.parse(document.getElementById('data').textContent);
  var info=document.getElementById('info'), counter=document.getElementById('at');
  var sheets=[].slice.call(document.querySelectorAll('section.sheet')), links=[].slice.call(document.querySelectorAll('#toc a'));
  var current=0, stops=[], at=-1;
  if(window.innerWidth<900) body.classList.add('notoc');
  function esc(s){return String(s).replace(/[&<>"]/g,function(c){return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]})}
  function val(l){ if(!l) return ''; if(l.formula!==undefined) return '='+l.formula; if(l.value!==undefined) return typeof l.value==='string'?l.value:String(l.value); return l.text||''; }
  function where(l){ if(!l) return '–'; return l.range||l.cell||(l.row!==undefined?'row '+l.row:'')||(l.column?'column '+l.column:'')||''; }
  function describe(c){
    var h='<h3><span class="k">'+esc(c.kind)+'</span>'+esc(c.status)+' · '+esc(where(c.base))+' → '+esc(where(c.other))+'</h3><table class="cl">';
    if(c.labels){ var lb=[c.labels.row,c.labels.column].filter(Boolean).join(' | '); if(lb) h+='<tr><th>label</th><td colspan="2">'+esc(lb)+'</td></tr>'; }
    if(c.kind==='value'||c.kind==='formula'){
      h+='<tr><th>base</th><td><code>'+esc(val(c.base))+'</code></td></tr><tr><th>other</th><td><code>'+esc(val(c.other))+'</code></td></tr>';
      if(c.delta!==undefined) h+='<tr><th>delta</th><td class="d">'+esc((c.delta>0?'+':'')+c.delta)+'</td></tr>';
      if(c.expected) h+='<tr><th>expected</th><td><code>='+esc(c.expected)+'</code></td></tr>';
    }
    (c.properties||[]).forEach(function(p){ h+='<tr><th>'+esc(p.name)+'</th><td><code>'+esc(p.base)+'</code></td><td><code>'+esc(p.other)+'</code></td></tr>'; });
    if(!(c.properties||[]).length && c.kind!=='value' && c.kind!=='formula'){ var t=(c.other&&c.other.text)||(c.base&&c.base.text); if(t) h+='<tr><th>content</th><td><code>'+esc(t)+'</code></td></tr>'; }
    return h+'</table>';
  }
  function show(ids,s){
    var sheet=data.sheets[s]; if(!sheet) return;
    info.innerHTML=ids.map(function(i){return describe(sheet.changes[i])}).join('') || '<p>No changes here.</p>';
    if(!body.classList.contains('info')) document.getElementById('infobtn').click();
  }
  function line(c,i){ return '<tr data-n="'+i+'"><td><span class="k">'+esc(c.kind)+'</span>'+esc(c.status)+'</td><td>'+esc(where(c.base))+' → '+esc(where(c.other))+'</td></tr>'; }
  function objects(s){
    // Every change the grid does not mark (objects, formatting on rows with
    // no values, changes too large to mark) is listed here.
    var sheet=data.sheets[s], sec=sheets[s], all=sec&&sec.querySelector('[data-nogrid]'), rest=[], marked={};
    if(sec) [].forEach.call(sec.querySelectorAll('[data-c]'),function(el){ el.getAttribute('data-c').split(' ').forEach(function(i){ marked[i]=1; }); });
    (sheet?sheet.changes:[]).forEach(function(c,i){ if(all||!marked[i]) rest.push(i); });
    var h='';
    if(rest.length) h+='<h3>'+(all?'Changes on this sheet':'Changes not marked in the grid')+'</h3><table class="cl">'+rest.map(function(i){return line(sheet.changes[i],i)}).join('')+'</table>';
    if(data.objects.length) h+='<h3>Workbook</h3><table class="cl">'+data.objects.map(function(c){
      var detail=(c.properties||[]).map(function(p){return p.name+': '+p.base+' → '+p.other}).join('; ')||val(c.other)||val(c.base);
      return '<tr><td><span class="k">'+esc(c.kind)+'</span>'+esc(c.status)+'</td><td>'+esc(c.id)+'</td><td>'+esc(detail)+'</td></tr>'}).join('')+'</table>';
    return h;
  }
  // Every place a change shows, on either side: column headers, row
  // headers, and cells. A place on both sides is one stop (the other side's
  // element); a change only on the base side (a removed comment) still gets
  // a stop. Stops run top to bottom, left to right.
  function collect(sec){
    var seen={}, list=[];
    function add(el,row,col){ var k=row+':'+col; if(seen[k]) return; seen[k]=1; list.push({el:el,row:row,col:col}); }
    ['.pane.o','.pane.b'].forEach(function(p){
      var pane=sec.querySelector(p); if(!pane) return;
      [].forEach.call(pane.querySelectorAll('thead th[data-c]'),function(th){ add(th,-1,[].indexOf.call(th.parentNode.children,th)); });
      [].forEach.call(pane.querySelectorAll('tbody tr'),function(tr){
        var i=Number(tr.getAttribute('data-i'));
        if(tr.hasAttribute('data-c')) add(tr.firstChild,i,0);
        [].forEach.call(tr.querySelectorAll('td[data-c]'),function(td){ add(td,i,[].indexOf.call(tr.children,td)); });
      });
    });
    list.sort(function(a,b){ return a.row-b.row||a.col-b.col; });
    return list;
  }
  function open(s){
    current=s;
    sheets.forEach(function(x,i){x.classList.toggle('on',i===s)});
    links.forEach(function(a,i){a.classList.toggle('on',i===s)});
    var sec=sheets[s];
    stops=sec?collect(sec):[];
    at=-1; counter.textContent=stops.length?stops.length+' changes':'no cell changes';
    info.innerHTML=objects(s)||'';
    var o=sec&&sec.querySelector('.pane.o'), b=sec&&sec.querySelector('.pane.b');
    if(o&&b&&!o.linked){ o.linked=true; follow(o,b); follow(b,o); }
  }
  function follow(src,dst){
    src.addEventListener('scroll',function(){
      var echo=src.echo; src.echo=null;
      if(echo&&echo[0]===src.scrollTop&&echo[1]===src.scrollLeft) return;
      if(dst.scrollTop!==src.scrollTop||dst.scrollLeft!==src.scrollLeft){ dst.scrollTop=src.scrollTop; dst.scrollLeft=src.scrollLeft; dst.echo=[dst.scrollTop,dst.scrollLeft]; }
    },{passive:true});
  }
  function hot(stop){
    [].forEach.call(document.querySelectorAll('.hot'),function(x){x.classList.remove('hot')});
    if(!stop) return;
    var sec=sheets[current];
    if(stop.row<0){ [].forEach.call(sec.querySelectorAll('thead tr'),function(tr){ var c=tr.children[stop.col]; if(c) c.classList.add('hot'); }); return; }
    [].forEach.call(sec.querySelectorAll('tr[data-i="'+stop.row+'"]'),function(row){ var c=row.children[stop.col]; if(c) c.classList.add('hot'); });
  }
  function ids(el){ return (el.getAttribute('data-c')||(el.parentNode&&el.parentNode.getAttribute('data-c'))||'').split(' ').filter(Boolean).map(Number); }
  function select(k){
    if(!stops.length) return;
    at=(k+stops.length)%stops.length;
    var stop=stops[at], el=stop.el, pane=el.closest('.pane');
    var tr=el.closest('tr');
    if(stop.row>=0&&body.classList.contains('only')&&!tr.classList.contains('ctx')) document.getElementById('only').click();
    var r=el.getBoundingClientRect(), p=pane.getBoundingClientRect();
    if(stop.row>=0) pane.scrollTop+=r.top-p.top-pane.clientHeight/3;
    if(r.left<p.left||r.right>p.right) pane.scrollLeft+=r.left-p.left-pane.clientWidth/3;
    hot(stop);
    counter.textContent=(at+1)+' / '+stops.length;
    show(ids(el),current);
  }
  document.getElementById('next').onclick=function(){select(at+1)};
  document.getElementById('prev').onclick=function(){select(at-1)};
  function toggle(button,cls,invert){
    button.onclick=function(){ var on=body.classList.toggle(cls); button.setAttribute('aria-pressed',String(invert?!on:on)); };
    button.setAttribute('aria-pressed',String(invert?!body.classList.contains(cls):body.classList.contains(cls)));
  }
  toggle(document.getElementById('tocbtn'),'notoc',true);
  toggle(document.getElementById('infobtn'),'info',false);
  document.getElementById('only').onchange=function(e){ body.classList.toggle('only',e.target.checked); };
  links.forEach(function(a,i){ a.onclick=function(e){e.preventDefault(); open(i);}; });
  document.addEventListener('click',function(e){
    var el=e.target.closest('td[data-c], th[data-c], tr[data-c] th.rn');
    if(el&&el.closest('section.sheet')){
      var k=-1; stops.forEach(function(s,j){ if(s.el===el) k=j; });
      if(k<0){ var tr=el.closest('tr'), row=el.closest('thead')?-1:Number(tr.getAttribute('data-i')), col=[].indexOf.call(tr.children,el);
        stops.forEach(function(s,j){ if(s.row===row&&s.col===col) k=j; }); }
      if(k>=0) select(k); else show(ids(el),current);
      return;
    }
    var row=e.target.closest('#info tr[data-n]');
    if(row) show([Number(row.getAttribute('data-n'))],current);
  });
  document.addEventListener('keydown',function(e){
    if(e.target.tagName==='INPUT'||e.ctrlKey||e.metaKey||e.altKey) return;
    if(e.key==='n'||e.key==='j') select(at+1);
    else if(e.key==='p'||e.key==='k') select(at-1);
    else if(e.key==='i') document.getElementById('infobtn').click();
  });
  var first=links.findIndex(function(a){return !a.classList.contains('clean')});
  open(first<0?0:first);
})();
"#;

/// The report for a comparison of `base` and `other`: every sheet and
/// change in `cmp` (the caller chooses its filters; a limit would hide
/// changes from the report).
pub fn render_html(cmp: &Listing, base: &Workbook, other: &Workbook) -> String {
    let mut out = String::with_capacity(1 << 16);
    let title = format!("{} ↔ {}", cmp.base.file, cmp.other.file);
    let _ = write!(
        out,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>xldif — {}</title><style>{STYLE}</style></head><body class=\"only\">",
        esc(&title)
    );
    let s = &cmp.summary;
    let _ = write!(out, "<div class=\"bar\"><h1>{}</h1><div class=\"chips\">", esc(&title));
    let chips = [
        (s.sheets_changed, "sheets changed"),
        (s.rows_only_other, "rows added"),
        (s.rows_only_base, "rows removed"),
        (s.rows_moved, "rows moved"),
        (s.columns_only_other + s.columns_only_base, "columns added or removed"),
        (s.value, "values"),
        (s.formula, "formulas"),
        (s.format, "formats"),
        (s.objects, "objects"),
    ];
    for (n, label) in chips.iter().filter(|(n, _)| *n > 0) {
        let _ = write!(out, "<span>{n} {label}</span>");
    }
    out.push_str("</div><div class=\"ctl\"><button id=\"prev\" title=\"previous change (p or k)\"><kbd>P</kbd>◀ prev</button><span id=\"at\"></span><button id=\"next\" title=\"next change (n or j)\">next ▶<kbd>N</kbd></button><label><input type=\"checkbox\" id=\"only\" checked> changes only</label><button id=\"tocbtn\">sheets</button><button id=\"infobtn\" title=\"show or hide details (i)\">details<kbd>I</kbd></button></div>");
    out.push_str("<div class=\"legend\"><span><i style=\"background:var(--del)\"></i>base / removed</span><span><i style=\"background:var(--ins)\"></i>other / added</span><span><i style=\"background:var(--mov)\"></i>moved</span><span><i style=\"box-shadow:inset 0 0 0 2px var(--fx)\"></i>formula</span><span><i style=\"outline:1px dashed #b45309\"></i>format</span></div></div>");

    out.push_str("<div class=\"main\"><nav id=\"toc\">");
    for sheet in &cmp.sheets {
        let name = match (&sheet.base, &sheet.other) {
            (Some(b), Some(o)) if b != o => format!("{b} → {o}"),
            (Some(n), _) | (None, Some(n)) => n.clone(),
            _ => String::new(),
        };
        let count = match sheet.status {
            Status::OnlyBase => "removed".to_owned(),
            Status::OnlyOther => "added".to_owned(),
            _ => sheet.changes.len().to_string(),
        };
        let clean = if sheet.status == Status::Same { " class=\"clean\"" } else { "" };
        let _ = write!(out, "<a href=\"#\"{clean}><span>{}</span><b>{count}</b></a>", esc(&name));
    }
    if !cmp.objects.is_empty() {
        let _ = write!(
            out,
            "<p style=\"padding:4px 8px;color:var(--muted)\">{} workbook change(s): see details</p>",
            cmp.objects.len()
        );
    }
    out.push_str("</nav><div class=\"work\">");
    let _ = write!(
        out,
        "<div class=\"names\"><div class=\"b\"><small>BASE</small>{}</div><div class=\"o\"><small>OTHER</small>{}</div></div>",
        esc(&cmp.base.file),
        esc(&cmp.other.file)
    );
    let find = |book: &'_ Workbook, name: &Option<String>| -> Option<usize> {
        name.as_ref().and_then(|n| book.sheets.iter().position(|s| &s.name == n))
    };
    for (i, sheet) in cmp.sheets.iter().enumerate() {
        let b = find(base, &sheet.base).map(|k| &base.sheets[k]);
        let o = find(other, &sheet.other).map(|k| &other.sheets[k]);
        render_sheet(&mut out, sheet, b, o, i);
    }
    out.push_str("<div id=\"info\"></div></div></div>");
    let data =
        Data { sheets: cmp.sheets.iter().map(|s| SheetData { changes: &s.changes }).collect(), objects: &cmp.objects };
    let _ = write!(out, "<script type=\"application/json\" id=\"data\">{}</script>", json_for_script(&data));
    let _ = write!(out, "<script>{SCRIPT}</script></body></html>");
    out
}

#[cfg(test)]
mod tests {
    use super::{Slot, slots};
    use crate::diff::Status;

    #[test]
    fn moves_and_inserts_in_one_gap_keep_the_other_sides_order() {
        // The demo: pairs 1-1, 3-2, 4-3, 5-6, 6-7, 7-8; base 2 moved to other 5; other 4 inserted.
        let got = slots(&[[1, 1, 1], [3, 2, 2], [5, 6, 3]], &[[2, 5, 1]], &[], &[4]);
        let others: Vec<Option<u32>> = got.iter().map(|s| s.other).collect();
        assert_eq!(others, [Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7), Some(8)]);
    }

    #[test]
    fn slots_interleave_one_sided_and_moved_rows() {
        // Base 1,2,3,5 paired with other 1,2,4,5; base 4 deleted, other 3 inserted; base 6 moved to other 0? none.
        let got = slots(&[[1, 1, 2], [3, 4, 1], [5, 5, 1]], &[], &[4], &[3]);
        let s = |b: Option<u32>, o: Option<u32>, st| Slot { base: b, other: o, status: st };
        assert_eq!(
            got,
            [
                s(Some(1), Some(1), Status::Same),
                s(Some(2), Some(2), Status::Same),
                s(None, Some(3), Status::OnlyOther),
                s(Some(3), Some(4), Status::Same),
                s(Some(4), None, Status::OnlyBase),
                s(Some(5), Some(5), Status::Same),
            ]
        );
    }
}
