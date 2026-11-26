//! Version series: what counts as an edit, and which versions the walk
//! keeps, on synthetic workbooks.

use xldif::diff::{CompareOptions, compare};
use xldif::model::Workbook;
use xldif::read_bytes;
use xldif::series::{Reason, edits, walk};
use xldif_fixtures::grid_book;

type Grid<'a> = &'a [&'a [&'a str]];

/// Revenue grows by a rate in B1; every year's figure is computed from it.
fn model(rate: &str, years: [&str; 3], note: &str) -> Workbook {
    let grid: Grid = &[
        &["Growth", rate],
        &["Base revenue", "1000"],
        &["Year 1", years[0]],
        &["Year 2", years[1]],
        &["Year 3", years[2]],
        &["Note", note],
    ];
    read_bytes(grid_book(&[("DCF", grid)])).unwrap()
}

fn v1() -> Workbook {
    model("0.1", ["=B2*(1+B1)|1100", "=B3*(1+B1)|1210", "=B4*(1+B1)|1331"], "draft")
}

/// Only the rate changed: one typed value, three recalculated.
fn rate_changed() -> Workbook {
    model("0.2", ["=B2*(1+B1)|1200", "=B3*(1+B1)|1440", "=B4*(1+B1)|1728"], "draft")
}

#[test]
fn a_changed_input_counts_once_and_its_recalculation_separately() {
    let c = compare("a", &v1(), "b", &rate_changed(), CompareOptions::default());
    let s = &c.summary;
    assert_eq!((s.value, s.value_input, s.value_computed), (4, 1, 3));
    assert_eq!(edits(s), 1);
}

#[test]
fn a_formula_replaced_by_a_typed_value_is_one_edit() {
    let typed = model("0.1", ["=B2*(1+B1)|1100", "1300", "=B4*(1+B1)|1430"], "draft");
    let s = compare("a", &v1(), "b", &typed, CompareOptions::default()).summary;
    // The formula change is the edit; both value changes follow from it.
    assert_eq!((s.formula, s.value_input, s.value_computed), (1, 0, 2));
    assert_eq!(edits(&s), 1);
}

#[test]
fn array_formula_cells_count_as_computed() {
    // C2 is inside C1's array formula but carries no formula of its own.
    let book = |second: &str| {
        xldif_fixtures::raw::RawBook::new()
            .sheet(
                "S",
                &format!(
                    r#"<row r="1"><c r="A1" t="inlineStr"><is><t>One</t></is></c><c r="B1"><v>1</v></c><c r="C1"><f t="array" ref="C1:C2">B1:B2*10</f><v>10</v></c><c r="D1" t="inlineStr"><is><t>kept</t></is></c></row>
                       <row r="2"><c r="A2" t="inlineStr"><is><t>Two</t></is></c><c r="B2"><v>{second}</v></c><c r="C2"><v>{}</v></c><c r="D2" t="inlineStr"><is><t>kept</t></is></c></row>"#,
                    second.parse::<f64>().unwrap() * 10.0
                ),
            )
            .build()
    };
    let (b, o) = (read_bytes(book("2")).unwrap(), read_bytes(book("3")).unwrap());
    let s = compare("a", &b, "b", &o, CompareOptions::default()).summary;
    assert_eq!((s.value_input, s.value_computed), (1, 1));
}

/// Steps as `(reason, compared with)`.
fn verdicts(books: Vec<Option<Workbook>>, min_edits: usize) -> Vec<(Reason, Option<usize>)> {
    let mut books: Vec<Option<Workbook>> = books;
    let steps = walk(books.len(), |i| books[i].take().ok_or("unreadable"), CompareOptions::default(), min_edits);
    steps.iter().map(|s| (s.reason, s.compared_with)).collect()
}

#[test]
fn small_edits_add_up_against_the_last_version_kept() {
    // Each version changes the note; the third also changes the rate.
    let note = |n: &str| model("0.1", ["=B2*(1+B1)|1100", "=B3*(1+B1)|1210", "=B4*(1+B1)|1331"], n);
    let rate_and_note = model("0.2", ["=B2*(1+B1)|1200", "=B3*(1+B1)|1440", "=B4*(1+B1)|1728"], "b");
    let books = vec![Some(v1()), Some(note("a")), Some(note("b")), Some(rate_and_note), Some(note("c"))];
    assert_eq!(
        verdicts(books, 2),
        [
            (Reason::First, None),
            // One edit since v1.
            (Reason::FewEdits, Some(0)),
            // Still one: the note again, measured from v1.
            (Reason::FewEdits, Some(0)),
            // The rate and the note differ from v1: two edits.
            (Reason::Edited, Some(0)),
            (Reason::Latest, Some(3)),
        ]
    );
}

#[test]
fn an_unreadable_version_is_kept_and_never_measured_from() {
    let books = vec![None, Some(v1()), None, Some(v1()), Some(rate_changed())];
    assert_eq!(
        verdicts(books, 1),
        [
            (Reason::Unreadable, None),
            (Reason::First, None),
            (Reason::Unreadable, None),
            (Reason::FewEdits, Some(1)),
            (Reason::Latest, Some(1)),
        ]
    );
    assert!(Reason::Unreadable.keep() && !Reason::FewEdits.keep());
}

#[test]
fn the_newest_readable_version_is_kept_when_a_newer_one_is_broken() {
    let books = vec![Some(v1()), Some(rate_changed()), None];
    assert_eq!(verdicts(books, 5), [(Reason::First, None), (Reason::Latest, Some(0)), (Reason::Unreadable, None)]);
}
