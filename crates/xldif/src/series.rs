//! A series of saved versions of one workbook, and which of them to keep.
//!
//! Versions are files whose names differ only by a `_YYMMDD_HHMM` stamp
//! (`DCF_261123_1102.xlsx`, `DCF_261123_1115_review.xlsx`), ordered by that
//! stamp. Walking a series in order, each version is compared with the last
//! version kept, so small edits add up until they amount to a revision worth
//! keeping. What counts is what a person edited ([`edits`]), not the values
//! recalculation changed after one input moved.
//!
//! This module decides; it neither lists folders nor deletes files.

use serde::Serialize;

use crate::diff::{self, CompareOptions, Summary};
use crate::model::{Warning, Workbook};
use crate::views::stale_value_warnings;

/// One folder's workbook file names sorted into series.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Grouping {
    /// By name, then extension.
    pub series: Vec<Series>,
    /// Workbooks that belong to no series, by file name.
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// The file name before the stamp: `DCF` for `DCF_261123_1102.xlsx`.
    pub name: String,
    /// Lowercase, without the dot: `xlsx` or `xlsm`.
    pub extension: String,
    /// Oldest first.
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub file: String,
    /// `YYMMDD_HHMM`, as the name writes it.
    pub stamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub file: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    /// `~$name.xlsx`: the owner file Excel writes while a workbook is open.
    LockFile,
    /// No `_YYMMDD_HHMM` stamp in the name.
    NoStamp,
}

/// Sorts file names into series: names alike, ignoring case, before their
/// last `_YYMMDD_HHMM` stamp. Names that are not `.xlsx` or `.xlsm`
/// are ignored; workbooks without a stamp and Excel's lock files are
/// listed as skipped.
pub fn group<'a>(files: impl IntoIterator<Item = &'a str>) -> Grouping {
    let mut out = Grouping::default();
    for file in files {
        let Some((stem, extension)) = workbook_stem(file) else { continue };
        if file.starts_with("~$") {
            out.skipped.push(Skipped { file: file.to_owned(), reason: SkipReason::LockFile });
            continue;
        }
        let Some((name, stamp)) = split_stamp(stem) else {
            out.skipped.push(Skipped { file: file.to_owned(), reason: SkipReason::NoStamp });
            continue;
        };
        let version = Version { file: file.to_owned(), stamp };
        // Windows names are case-insensitive, so `dcf_…` and `DCF_…` are
        // one series, named as its first file found spells it.
        let key = name.to_lowercase();
        match out.series.iter_mut().find(|s| s.name.to_lowercase() == key && s.extension == extension) {
            Some(series) => series.versions.push(version),
            None => out.series.push(Series { name, extension, versions: vec![version] }),
        }
    }
    for series in &mut out.series {
        // The stamp orders versions; the full name breaks a tie between
        // two saved in the same minute.
        series.versions.sort_by(|a, b| a.stamp.cmp(&b.stamp).then_with(|| a.file.cmp(&b.file)));
    }
    out.series.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.extension.cmp(&b.extension)));
    out.skipped.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// `(stem, lowercase extension)` for an `.xlsx` or `.xlsm` name.
fn workbook_stem(file: &str) -> Option<(&str, String)> {
    let (stem, extension) = file.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    (matches!(extension.as_str(), "xlsx" | "xlsm") && !stem.is_empty()).then_some((stem, extension))
}

/// `(name, "YYMMDD_HHMM")` from a stem whose last stamp is `_YYMMDD_HHMM`,
/// followed by the stem's end or by `_` and any suffix.
fn split_stamp(stem: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = stem.split('_').collect();
    (1..parts.len().saturating_sub(1)).rev().find_map(|i| {
        let (date, time) = (parts[i], parts[i + 1]);
        let name = parts[..i].join("_");
        (is_date(date) && is_time(time) && !name.is_empty()).then(|| (name, format!("{date}_{time}")))
    })
}

fn digits(text: &str, n: usize) -> Option<Vec<u32>> {
    (text.len() == n).then(|| text.chars().map(|c| c.to_digit(10)).collect::<Option<Vec<u32>>>()).flatten()
}

fn is_date(text: &str) -> bool {
    digits(text, 6).is_some_and(|d| {
        let (month, day) = (d[2] * 10 + d[3], d[4] * 10 + d[5]);
        (1..=12).contains(&month) && (1..=31).contains(&day)
    })
}

fn is_time(text: &str) -> bool {
    digits(text, 4).is_some_and(|d| d[0] * 10 + d[1] < 24 && d[2] * 10 + d[3] < 60)
}

/// What a person edited between two versions: values typed, changed, or
/// cleared (not recalculated), formula changes (a fill counted once, Excel's
/// adjustment for moved rows not at all), and rows, columns, and sheets
/// added, removed, moved, or renamed. Formats, layout, and workbook objects
/// are left out: they rarely make a version worth keeping on their own.
pub const fn edits(s: &Summary) -> usize {
    s.value_input
        + s.formula
        + s.rows_only_base
        + s.rows_only_other
        + s.rows_moved
        + s.columns_only_base
        + s.columns_only_other
        + s.columns_moved
        + s.sheets_only_base
        + s.sheets_only_other
        + s.sheets_renamed
}

/// Why a version is kept or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// The first readable version: what later ones are measured from.
    First,
    /// The newest version that could be read, always kept.
    Latest,
    /// At least the threshold of edits since the last version kept.
    Edited,
    /// Fewer edits than the threshold since the last version kept.
    FewEdits,
    /// It could not be read, so nothing is known about it.
    Unreadable,
}

impl Reason {
    pub const fn keep(self) -> bool {
        !matches!(self, Self::FewEdits)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Latest => "latest",
            Self::Edited => "edited",
            Self::FewEdits => "fewEdits",
            Self::Unreadable => "unreadable",
        }
    }
}

/// One version's verdict.
#[derive(Debug)]
pub struct Step<E> {
    pub reason: Reason,
    /// The index of the version it was compared with: the last one kept.
    pub compared_with: Option<usize>,
    /// The comparison with that version.
    pub summary: Option<Summary>,
    /// Why it could not be read.
    pub error: Option<E>,
    /// The workbook's own warnings (stale values, content not compared).
    pub warnings: Vec<Warning>,
    /// Warnings about the comparison with `compared_with`.
    pub pair_warnings: Vec<Warning>,
}

/// Walks `count` versions oldest first, reading each once with `read`, and
/// decides which to keep: the first readable one, the newest readable one,
/// every one
/// with at least `min_edits` [`edits`] since the last one kept, and any it
/// could not read. A version that cannot be read is never measured from.
pub fn walk<E>(
    count: usize,
    mut read: impl FnMut(usize) -> Result<Workbook, E>,
    options: CompareOptions,
    min_edits: usize,
) -> Vec<Step<E>> {
    let mut steps = Vec::with_capacity(count);
    let mut baseline: Option<(usize, Workbook)> = None;
    for i in 0..count {
        let book = match read(i) {
            Ok(book) => book,
            Err(e) => {
                steps.push(Step {
                    reason: Reason::Unreadable,
                    compared_with: None,
                    summary: None,
                    error: Some(e),
                    warnings: Vec::new(),
                    pair_warnings: Vec::new(),
                });
                continue;
            }
        };
        let warnings: Vec<Warning> = book.warnings.iter().cloned().chain(stale_value_warnings(&book)).collect();
        let Some((base_index, base)) = &baseline else {
            steps.push(Step {
                reason: Reason::First,
                compared_with: None,
                summary: None,
                error: None,
                warnings,
                pair_warnings: Vec::new(),
            });
            baseline = Some((i, book));
            continue;
        };
        let comparison = diff::compare("", base, "", &book, options);
        let reason = if edits(&comparison.summary) >= min_edits { Reason::Edited } else { Reason::FewEdits };
        steps.push(Step {
            reason,
            compared_with: Some(*base_index),
            summary: Some(comparison.summary),
            error: None,
            warnings,
            pair_warnings: comparison.pair_warnings,
        });
        if reason.keep() {
            baseline = Some((i, book));
        }
    }
    // The newest version that could be read is kept, whatever follows it:
    // a newer file that is truncated or corrupt must not leave only an old
    // version and a broken one.
    if let Some(newest) = steps.iter_mut().rev().find(|s| s.reason != Reason::Unreadable)
        && newest.reason != Reason::First
    {
        newest.reason = Reason::Latest;
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(g: &Grouping) -> Vec<(String, Vec<&str>)> {
        g.series
            .iter()
            .map(|s| (format!("{}.{}", s.name, s.extension), s.versions.iter().map(|v| v.file.as_str()).collect()))
            .collect()
    }

    #[test]
    fn groups_by_the_name_before_the_stamp_and_orders_by_stamp() {
        let g = group([
            "DCF_261123_1102.xlsx",
            "DCF_261120_1637.xlsx",
            "DCF_261123_1115_review.xlsx",
            "FS (Q3)_261123_1112.XLSX",
            "DCF_261124_0918.xlsm",
            "notes.txt",
            "chart.png",
        ]);
        assert_eq!(
            names(&g),
            [
                ("DCF.xlsm".into(), vec!["DCF_261124_0918.xlsm"]),
                (
                    "DCF.xlsx".into(),
                    vec!["DCF_261120_1637.xlsx", "DCF_261123_1102.xlsx", "DCF_261123_1115_review.xlsx"]
                ),
                ("FS (Q3).xlsx".into(), vec!["FS (Q3)_261123_1112.XLSX"]),
            ]
        );
        assert!(g.skipped.is_empty());
    }

    #[test]
    fn lists_lock_files_and_unstamped_workbooks_as_skipped() {
        let g = group(["~$DCF_261126_0953.xlsx", "template.xlsx", "DCF_261399_1200.xlsx", "_261123_1102.xlsx"]);
        assert!(g.series.is_empty());
        let skipped: Vec<(&str, SkipReason)> = g.skipped.iter().map(|s| (s.file.as_str(), s.reason)).collect();
        assert_eq!(
            skipped,
            [
                ("DCF_261399_1200.xlsx", SkipReason::NoStamp),
                ("_261123_1102.xlsx", SkipReason::NoStamp),
                ("template.xlsx", SkipReason::NoStamp),
                ("~$DCF_261126_0953.xlsx", SkipReason::LockFile),
            ]
        );
    }

    #[test]
    fn names_differing_only_in_case_are_one_series() {
        let g = group(["DCF_261121_0900.xlsx", "dcf_261120_1637.xlsx"]);
        assert_eq!(names(&g).len(), 1);
        assert_eq!(g.series[0].versions[0].file, "dcf_261120_1637.xlsx");
    }

    #[test]
    fn the_last_stamp_in_a_name_is_the_version() {
        let g = group(["Plan_260101_0900_261123_1102.xlsx"]);
        assert_eq!(g.series[0].name, "Plan_260101_0900");
        assert_eq!(g.series[0].versions[0].stamp, "261123_1102");
    }
}
