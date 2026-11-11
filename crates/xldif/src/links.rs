//! External workbook links across two workbooks.
//!
//! A formula names another workbook by its position in its own workbook's
//! list of links: `[1]Prices!A1` reads the first link's file. The position
//! says nothing by itself: adding or removing a link renumbers the others.
//! So links pair by the file they name, and a base formula is read in the
//! other workbook's numbering before it is compared: `[2]` on one side and
//! `[1]` on the other are the same reference when both name `Prices.xlsx`.
//!
//! A link pointed at another file (last year's trial balance replaced by
//! this year's) is one change to the link. The formulas through it are not
//! listed one by one: their text did not change, and the link's change says
//! what they all read now.

use std::collections::{HashMap, VecDeque};

use crate::diff::{Change, Kind, Location, Property, Status};
use crate::formula::{Reference, Token};

/// For each base link, the other workbook's link that names the same file
/// (0-based indexes). Names compare without case, as Windows paths do. A
/// link without a target pairs only with one at the same position.
pub fn pair(base: &[String], other: &[String]) -> Vec<Option<usize>> {
    let mut by_target: HashMap<String, VecDeque<usize>> = HashMap::new();
    for (j, target) in other.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
        by_target.entry(target.to_lowercase()).or_default().push_back(j);
    }
    base.iter()
        .enumerate()
        .map(|(i, target)| {
            if target.is_empty() {
                other.get(i).is_some_and(String::is_empty).then_some(i)
            } else {
                by_target.get_mut(&target.to_lowercase()).and_then(VecDeque::pop_front)
            }
        })
        .collect()
}

/// For each base link that names a file the other workbook does not link
/// to: the other workbook's link at the same position, when that one names
/// a file the base does not link to either. The link was pointed at
/// another file.
fn repointed(base: &[String], other: &[String], paired: &[Option<usize>]) -> Vec<Option<usize>> {
    let mut other_paired = vec![false; other.len()];
    for &j in paired.iter().flatten() {
        other_paired[j] = true;
    }
    (0..base.len()).map(|i| (paired[i].is_none() && i < other.len() && !other_paired[i]).then_some(i)).collect()
}

/// Changes to the links themselves, for the workbook-level list. A file
/// linked on one side only is `onlyBase` or `onlyOther`; where both sides
/// have such a link at the same position, the link was pointed at another
/// file and is one `changed`. The same file at another position is not a
/// change. Ids carry the positions formulas use
/// (`externalLink:<base>:<other>`), so `[1]` in a formula can be looked up
/// here.
pub fn changes(base: &[String], other: &[String]) -> Vec<Change> {
    let paired = pair(base, other);
    let repointed = repointed(base, other, &paired);
    let mut other_done = vec![false; other.len()];
    for &j in paired.iter().chain(&repointed).flatten() {
        other_done[j] = true;
    }
    let shown = |target: &str| if target.is_empty() { "(no workbook file)".to_owned() } else { target.to_owned() };
    let loc = |target: &str| Some(Location { text: Some(shown(target)), ..Location::default() });
    let position = |i: Option<usize>| i.map_or_else(|| "-".to_owned(), |i| (i + 1).to_string());
    let change = |b: Option<usize>, o: Option<usize>| Change {
        id: format!("externalLink:{}:{}", position(b), position(o)),
        kind: Kind::ExternalLink,
        status: match (b, o) {
            (Some(_), Some(_)) => Status::Changed,
            (Some(_), None) => Status::OnlyBase,
            _ => Status::OnlyOther,
        },
        base: b.and_then(|i| loc(&base[i])),
        other: o.and_then(|j| loc(&other[j])),
        delta: None,
        expected: None,
        labels: None,
        properties: match (b, o) {
            (Some(i), Some(j)) => {
                vec![Property { name: "target".to_owned(), base: shown(&base[i]), other: shown(&other[j]) }]
            }
            _ => Vec::new(),
        },
    };
    let mut out = Vec::new();
    for i in (0..base.len()).filter(|&i| paired[i].is_none()) {
        out.push(change(Some(i), repointed[i]));
    }
    out.extend((0..other.len()).filter(|&j| !other_done[j]).map(|j| change(None, Some(j))));
    out
}

/// What each base link index becomes in the other workbook's formulas.
#[derive(Debug, Clone)]
pub struct LinkMap {
    /// By base position: the text between the brackets on the other side.
    names: Vec<String>,
    /// No index changes, so formulas compare as written.
    identity: bool,
}

impl LinkMap {
    pub fn new(base: &[String], other: &[String]) -> Self {
        let paired = pair(base, other);
        let repointed = repointed(base, other, &paired);
        let names: Vec<String> = paired
            .iter()
            .zip(&repointed)
            .zip(base)
            .enumerate()
            // A link pointed at another file keeps its number: the change
            // is the link's, reported once, not each formula's through it.
            .map(|(i, ((paired, repointed), target))| match paired.or(*repointed) {
                Some(j) => (j + 1).to_string(),
                // The other workbook has no link to this file, so no index
                // there can mean it: the file's name stands in, as Excel
                // shows a link (`[Prices.xlsx]Sheet1!A1`). A name that reads
                // as an index is marked so it cannot be taken for one.
                None if target.is_empty() => format!("?{}", i + 1),
                None if target.bytes().all(|b| b.is_ascii_digit()) => format!("./{target}"),
                None => target.clone(),
            })
            .collect();
        let identity = names.iter().enumerate().all(|(i, n)| *n == (i + 1).to_string());
        Self { names, identity }
    }

    /// `[2]Prices` as the other workbook writes it; `None` when `sheet` has
    /// no link index or the index names no link (`[0]` is the workbook
    /// itself).
    fn renumber(&self, sheet: &str) -> Option<String> {
        let (index, rest) = sheet.strip_prefix('[')?.split_once(']')?;
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let name = self.names.get(index.parse::<usize>().ok()?.checked_sub(1)?)?;
        Some(format!("[{name}]{rest}"))
    }

    /// A base formula's tokens in the other workbook's link numbering, or
    /// `None` when they read the same there.
    ///
    /// Besides references (`[1]Prices!A1`), this covers what the tokenizer
    /// keeps as text with a `!` after the index: an external defined name
    /// (`[1]!Rate`) and a 3-D span (`[1]Jan:Dec!B5`). A bare `[1]` is a
    /// table column, not a link.
    pub fn relink(&self, tokens: &[Token]) -> Option<Vec<Token>> {
        if self.identity {
            return None;
        }
        let mut changed = false;
        let mut renumber = |sheet: &str| {
            let new = self.renumber(sheet).filter(|n| n != sheet);
            changed |= new.is_some();
            new.unwrap_or_else(|| sheet.to_owned())
        };
        let out = tokens
            .iter()
            .map(|t| match t {
                Token::Ref(Reference { sheet: Some(s), start, end }) => {
                    Token::Ref(Reference { sheet: Some(renumber(s)), start: *start, end: *end })
                }
                Token::RefError(Some(s)) => Token::RefError(Some(renumber(s))),
                Token::Text(s) if s.contains('!') => Token::Text(renumber(s)),
                other => other.clone(),
            })
            .collect();
        changed.then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::{render, tokenize};

    fn links(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    fn relinked(map: &LinkMap, formula: &str) -> String {
        map.relink(&tokenize(formula)).map_or_else(|| formula.to_owned(), |t| render(&t))
    }

    #[test]
    fn links_pair_by_the_file_they_name() {
        let base = links(&["Prices.xlsx", "Rates.xlsx", "Old.xlsx", ""]);
        let other = links(&["rates.xlsx", "New.xlsx", "Prices.xlsx", ""]);
        assert_eq!(pair(&base, &other), [Some(2), Some(0), None, Some(3)]);
        assert_eq!(pair(&other, &base), [Some(1), None, Some(0), Some(3)]);
        // A link that names no file is the same link only in the same place.
        assert_eq!(pair(&links(&["", "A.xlsx"]), &links(&["A.xlsx", ""])), [None, Some(0)]);
    }

    #[test]
    fn a_formula_is_read_in_the_other_workbooks_numbering() {
        let map =
            LinkMap::new(&links(&["Prices.xlsx", "Rates.xlsx", "Old.xlsx"]), &links(&["Rates.xlsx", "Prices.xlsx"]));
        // A link pointed at another file keeps its number.
        let moved = LinkMap::new(&links(&["FY2024.xlsx"]), &links(&["FY2025.xlsx"]));
        assert!(moved.relink(&tokenize("[1]Sheet1!A1")).is_none());
        assert_eq!(relinked(&map, "[1]Prices!A1*[2]Rates!$B$2"), "[2]Prices!A1*[1]Rates!$B$2");
        assert_eq!(relinked(&map, "'[1]Price list'!A1"), "'[2]Price list'!A1");
        assert_eq!(relinked(&map, "[2]!Rate+[1]Jan:Dec!B5"), "[1]!Rate+[2]Jan:Dec!B5");
        assert_eq!(relinked(&map, "[1]Prices!#REF!"), "[2]Prices!#REF!");
        // A file the other workbook does not link to keeps its name.
        assert_eq!(relinked(&map, "SUM([3]Data!A1:A9)"), "SUM([Old.xlsx]Data!A1:A9)");
        // Not links: the workbook itself, an index past the list, a table
        // column, and this workbook's own sheets.
        for same in ["[0]!Rate", "[9]Prices!A1", "Table1[1]", "SUM([1])", "Prices!A1+B2"] {
            assert!(map.relink(&tokenize(same)).is_none(), "{same}");
        }
    }

    #[test]
    fn link_changes_name_both_files() {
        // Prices moved from [1] to [2]: no change. Old was removed, New
        // added. [3] names another file on each side.
        let base = links(&["Prices.xlsx", "Old.xlsx", "FY2024.xlsx"]);
        let other = links(&["New.xlsx", "Prices.xlsx", "FY2025.xlsx"]);
        let found: Vec<(String, Status, Option<String>, Option<String>)> = changes(&base, &other)
            .into_iter()
            .map(|c| (c.id, c.status, c.base.and_then(|l| l.text), c.other.and_then(|l| l.text)))
            .collect();
        let text = |t: &str| Some(t.to_owned());
        assert_eq!(
            found,
            [
                ("externalLink:2:-".to_owned(), Status::OnlyBase, text("Old.xlsx"), None),
                ("externalLink:3:3".to_owned(), Status::Changed, text("FY2024.xlsx"), text("FY2025.xlsx")),
                ("externalLink:-:1".to_owned(), Status::OnlyOther, None, text("New.xlsx")),
            ]
        );
        assert!(changes(&base, &base).is_empty());
    }

    #[test]
    fn the_same_links_in_the_same_order_change_nothing() {
        let both = links(&["Prices.xlsx", "Rates.xlsx"]);
        assert!(LinkMap::new(&both, &both).relink(&tokenize("[1]Prices!A1+[2]Rates!A1")).is_none());
    }

    #[test]
    fn a_file_named_like_an_index_cannot_pass_for_one() {
        let map = LinkMap::new(&links(&["A.xlsx", "2", ""]), &links(&["A.xlsx"]));
        assert_eq!(relinked(&map, "[2]S!A1+[3]S!A1"), "[./2]S!A1+[?3]S!A1");
    }
}
