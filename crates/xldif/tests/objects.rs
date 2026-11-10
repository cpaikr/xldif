//! Formats, layout, and workbook objects compared through the alignment.
// The test variant is a set of independent switches.
#![allow(clippy::struct_excessive_bools)]

use xldif::diff::{Change, CompareOptions, Comparison, Kind, Status, compare};
use xldif::read_bytes;
use xldif_fixtures::rust_xlsxwriter::{
    Color, ConditionalFormatCell, ConditionalFormatCellRule, DataValidation, Format, Note, Workbook,
};

/// What differs between the two generated workbooks.
#[derive(Default, Clone, Copy)]
struct Variant {
    /// Insert a row above the table, shifting everything down by one.
    insert_row: bool,
    bold_total: bool,
    merge_title_wider: bool,
    edit_note: bool,
    widen_validation_list: bool,
    retarget_link: bool,
    hide_row: bool,
    widen_column: bool,
    hide_sheet: bool,
    move_name: bool,
    change_conditional: bool,
}

fn book(v: Variant) -> Vec<u8> {
    let mut wb = Workbook::new();
    let o = u32::from(v.insert_row);
    let ws = wb.add_worksheet().set_name("BS").unwrap();
    ws.write_string(0, 0, "Balance sheet").unwrap();
    ws.merge_range(0, 0, 0, if v.merge_title_wider { 2 } else { 1 }, "Balance sheet", &Format::new()).unwrap();
    if v.insert_row {
        ws.write_string(1, 0, "Draft").unwrap();
    }
    ws.write_string(1 + o, 0, "Account").unwrap();
    ws.write_string(1 + o, 1, "2025").unwrap();
    for (i, (label, n)) in [("Cash", 100.0), ("Receivables", 200.0), ("Inventory", 300.0)].iter().enumerate() {
        let r = 2 + o + i as u32;
        ws.write_string(r, 0, *label).unwrap();
        ws.write_number(r, 1, *n).unwrap();
    }
    let total = Format::new().set_num_format("#,##0");
    let total = if v.bold_total { total.set_bold() } else { total };
    ws.write_string(5 + o, 0, "Total").unwrap();
    ws.write_number_with_format(5 + o, 1, 600.0, &total).unwrap();
    let note = Note::new(if v.edit_note { "checked twice" } else { "checked" }).set_author("Auditor");
    ws.insert_note(5 + o, 1, &note).unwrap();
    let list: &[&str] = if v.widen_validation_list { &["yes", "no", "n/a"] } else { &["yes", "no"] };
    ws.add_data_validation(2 + o, 2, 4 + o, 2, &DataValidation::new().allow_list_strings(list).unwrap()).unwrap();
    ws.write_url(6 + o, 0, if v.retarget_link { "https://example.com/new" } else { "https://example.com/old" })
        .unwrap();
    if v.hide_row {
        ws.set_row_hidden(3 + o).unwrap();
    }
    ws.set_column_width(0, if v.widen_column { 30 } else { 20 }).unwrap();
    let rule = if v.change_conditional {
        ConditionalFormatCellRule::GreaterThan(250)
    } else {
        ConditionalFormatCellRule::GreaterThan(150)
    };
    let cf = ConditionalFormatCell::new().set_rule(rule).set_format(Format::new().set_background_color(Color::Yellow));
    ws.add_conditional_format(2 + o, 1, 4 + o, 1, &cf).unwrap();

    let notes = wb.add_worksheet().set_name("Notes").unwrap();
    notes.write_string(0, 0, "Policy").unwrap();
    if v.hide_sheet {
        notes.set_hidden(true);
    }
    let rate = if v.move_name { "=Notes!$A$2" } else { "=Notes!$A$1" };
    wb.define_name("Policy", rate).unwrap();
    wb.save_to_buffer().unwrap()
}

fn run(base: Variant, other: Variant) -> Comparison {
    let (b, o) = (read_bytes(book(base)).unwrap(), read_bytes(book(other)).unwrap());
    compare("base.xlsx", &b, "other.xlsx", &o, CompareOptions::default())
}

fn of_kind(c: &Comparison, kind: Kind) -> Vec<&Change> {
    c.sheets.iter().flat_map(|s| &s.changes).chain(&c.objects).filter(|x| x.kind == kind).collect()
}

#[test]
fn identical_workbooks_have_no_object_changes() {
    let c = run(Variant::default(), Variant::default());
    assert!(c.sheets.iter().all(|s| s.changes.is_empty()), "{:?}", c.sheets);
    assert!(c.objects.is_empty());
}

#[test]
fn an_inserted_row_moves_every_object_with_it() {
    let c = run(Variant::default(), Variant { insert_row: true, ..Variant::default() });
    let kinds: Vec<Kind> = c.sheets.iter().flat_map(|s| &s.changes).map(|x| x.kind).collect();
    // Only the inserted row itself: merges, notes, validation, links,
    // conditional formats, and formats all followed the alignment.
    assert_eq!(kinds, [Kind::Row], "{:?}", c.sheets[0].changes);
}

#[test]
fn a_format_change_names_its_properties() {
    let c = run(Variant::default(), Variant { bold_total: true, ..Variant::default() });
    let f = of_kind(&c, Kind::Format);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].other.as_ref().unwrap().cell.as_deref(), Some("B6"));
    assert_eq!(f[0].properties.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["font.bold"]);
    assert_eq!(c.summary.format, 1);
}

#[test]
fn objects_report_what_changed() {
    let all = Variant {
        merge_title_wider: true,
        edit_note: true,
        widen_validation_list: true,
        retarget_link: true,
        hide_row: true,
        widen_column: true,
        hide_sheet: true,
        move_name: true,
        change_conditional: true,
        ..Variant::default()
    };
    let c = run(Variant::default(), all);
    let merges = of_kind(&c, Kind::Merge);
    assert_eq!(merges.iter().map(|m| m.status).collect::<Vec<_>>(), [Status::OnlyBase, Status::OnlyOther]);
    let note = &of_kind(&c, Kind::Comment)[0];
    // Excel (and rust_xlsxwriter) start a note's text with its author.
    assert_eq!(
        (note.properties[0].base.as_str(), note.properties[0].other.as_str()),
        (
            "Auditor:
checked",
            "Auditor:
checked twice"
        )
    );
    assert_eq!(of_kind(&c, Kind::Validation)[0].status, Status::Changed);
    assert_eq!(of_kind(&c, Kind::Hyperlink)[0].properties[0].other, "https://example.com/new");
    assert_eq!(of_kind(&c, Kind::ConditionalFormat).len(), 1);
    let rows = of_kind(&c, Kind::Row);
    assert!(rows.iter().any(|r| r.properties.iter().any(|p| p.name == "hidden")), "{rows:?}");
    let cols = of_kind(&c, Kind::Column);
    assert!(cols.iter().any(|r| r.properties.iter().any(|p| p.name == "width")), "{cols:?}");
    assert_eq!(of_kind(&c, Kind::Sheet)[0].properties[0].other, "hidden");
    let name = &of_kind(&c, Kind::Name)[0];
    assert_eq!(name.properties[0].other, "Notes!$A$2");
}

fn pair(make: impl Fn(&mut Workbook, bool)) -> Comparison {
    let build = |later: bool| {
        let mut wb = Workbook::new();
        make(&mut wb, later);
        read_bytes(wb.save_to_buffer().unwrap()).unwrap()
    };
    compare("a", &build(false), "b", &build(true), CompareOptions::default())
}

#[test]
fn conditional_formats_compare_their_style_not_its_index() {
    // The fill a rule applies changes: one rule change.
    let c = pair(|wb, later| {
        let ws = wb.add_worksheet();
        ws.write_number(0, 0, 1).unwrap();
        let color = if later { Color::Green } else { Color::Yellow };
        let cf = ConditionalFormatCell::new()
            .set_rule(ConditionalFormatCellRule::GreaterThan(0))
            .set_format(Format::new().set_background_color(color));
        ws.add_conditional_format(0, 0, 4, 0, &cf).unwrap();
    });
    assert_eq!(of_kind(&c, Kind::ConditionalFormat).len(), 1);
    // A rule added on the first sheet renumbers the second sheet's style
    // index; the second sheet's unchanged rule is not a change.
    let c = pair(|wb, later| {
        let red = || {
            ConditionalFormatCell::new()
                .set_rule(ConditionalFormatCellRule::LessThan(0))
                .set_format(Format::new().set_font_color(Color::Red))
        };
        let first = wb.add_worksheet();
        first.write_number(0, 0, 1).unwrap();
        if later {
            first.add_conditional_format(0, 0, 0, 0, &red().set_format(Format::new().set_bold())).unwrap();
        }
        let second = wb.add_worksheet();
        second.write_number(0, 0, 1).unwrap();
        second.add_conditional_format(0, 0, 0, 0, &red()).unwrap();
    });
    let cf = of_kind(&c, Kind::ConditionalFormat);
    assert_eq!(cf.len(), 1, "{cf:?}");
    assert_eq!(cf[0].status, Status::OnlyOther);
}

#[test]
fn row_and_column_styles_and_sizes_are_compared_and_counted() {
    let c = pair(|wb, later| {
        let ws = wb.add_worksheet();
        ws.write_number(0, 0, 1).unwrap();
        let fill = Format::new().set_background_color(if later { Color::Green } else { Color::Yellow });
        ws.set_column_format(3, &fill).unwrap();
        ws.set_row_format(5, &fill).unwrap();
        ws.set_row_height(8, if later { 30 } else { 20 }).unwrap();
    });
    let rows = of_kind(&c, Kind::Row);
    let cols = of_kind(&c, Kind::Column);
    assert!(cols.iter().any(|x| x.properties.iter().any(|p| p.name == "style.fill.fg")), "{cols:?}");
    assert!(rows.iter().any(|x| x.properties.iter().any(|p| p.name == "style.fill.fg")), "{rows:?}");
    assert!(rows.iter().any(|x| x.properties.iter().any(|p| p.name == "height")), "{rows:?}");
    assert_eq!((c.summary.rows_changed, c.summary.columns_changed), (rows.len(), cols.len()));
}

#[test]
fn rules_on_one_range_keep_unique_ids_and_edits_are_edits() {
    let c = pair(|wb, later| {
        let ws = wb.add_worksheet();
        ws.write_number(0, 0, 1).unwrap();
        if !later {
            for n in [1, 2] {
                let cf = ConditionalFormatCell::new()
                    .set_rule(ConditionalFormatCellRule::GreaterThan(n))
                    .set_format(Format::new().set_bold());
                ws.add_conditional_format(0, 0, 4, 0, &cf).unwrap();
            }
        }
        // Two links whose targets each move on by one.
        let (a, b) = if later { ("https://x/2", "https://x/3") } else { ("https://x/1", "https://x/2") };
        ws.write_string(1, 0, "first").unwrap();
        ws.write_string(2, 0, "second").unwrap();
        ws.write_url_with_text(1, 1, a, "link").unwrap();
        ws.write_url_with_text(2, 1, b, "link").unwrap();
    });
    let ids: Vec<&str> = c.sheets[0].changes.iter().map(|x| x.id.as_str()).collect();
    let unique: std::collections::HashSet<&&str> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    let links = of_kind(&c, Kind::Hyperlink);
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(links.iter().all(|l| l.status == Status::Changed && l.properties[0].name == "rule"), "{links:?}");
}

#[test]
fn many_custom_row_heights_compare_quickly() {
    let build = || {
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        for r in 0..20_000 {
            ws.set_row_height(r, 18).unwrap();
        }
        ws.write_number(0, 0, 1).unwrap();
        read_bytes(wb.save_to_buffer().unwrap()).unwrap()
    };
    let (a, b) = (build(), build());
    let start = std::time::Instant::now();
    let c = compare("a", &a, "b", &b, CompareOptions::default());
    assert!(c.sheets[0].changes.is_empty());
    assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
}
