//! `xldif series`: lists a folder, hands the file names to the library's
//! grouping and walk, and reports each version's verdict with its size.
//! Nothing is deleted.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use xldif::diff::{AlignMode, CompareOptions, Summary};
use xldif::series::{self, Reason, SkipReason};
use xldif::{Error, ErrorCode};

use crate::cli::Format;
use crate::commands::{Output, escape, json, write_warnings};
use crate::envelope::{Failure, Warning};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    folder: String,
    min_edits: usize,
    series: Vec<SeriesView>,
    skipped: Vec<SkippedView>,
    totals: Totals,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SeriesView {
    /// The subfolder, `/`-separated; empty for the folder itself.
    folder: String,
    name: String,
    extension: String,
    versions: Vec<VersionView>,
    kept: usize,
    removable: usize,
    removable_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionView {
    file: String,
    stamp: String,
    /// Absent when the file's size could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    keep: bool,
    reason: Reason,
    /// The file it was compared with: the last version kept before it.
    #[serde(skip_serializing_if = "Option::is_none")]
    compared_with: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edits: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorView>,
}

#[derive(Serialize)]
struct ErrorView {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkippedView {
    /// Relative to the folder, `/`-separated.
    file: String,
    reason: SkipReason,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Totals {
    series: usize,
    versions: usize,
    kept: usize,
    removable: usize,
    removable_bytes: u64,
}

pub fn run(
    folder: &Path,
    recursive: bool,
    min_edits: usize,
    tolerance: f64,
    format: Format,
    unsaved: bool,
) -> Result<Output, Failure> {
    if !(tolerance >= 0.0 && tolerance.is_finite()) {
        return Err(Error::new(ErrorCode::InvalidArgument, "--tolerance must be a finite number of 0 or more").into());
    }
    if !folder.exists() {
        return Err(Error::new(ErrorCode::FileNotFound, format!("{} does not exist", folder.display())).into());
    }
    if !folder.is_dir() {
        return Err(Error::new(ErrorCode::InvalidArgument, format!("{} is not a folder", folder.display()))
            .with_hint("give series the folder that holds the versions; compare two files with xldif compare")
            .into());
    }
    let options = CompareOptions { tolerance, align: AlignMode::Content };
    let mut report = Report {
        folder: folder.display().to_string(),
        min_edits,
        series: Vec::new(),
        skipped: Vec::new(),
        totals: Totals::default(),
    };
    let mut notes = Notes::default();
    let mut unlisted: Vec<String> = Vec::new();
    for (relative, dir) in folders(folder, recursive, &mut unlisted)? {
        let names = match file_names(&dir) {
            Ok(names) => names,
            // The folder itself must be listable; a subfolder that is not
            // (a protected system folder) is reported and passed over.
            Err(_) if !relative.is_empty() => {
                unlisted.push(relative);
                continue;
            }
            Err(e) => return Err(e),
        };
        let grouping = series::group(names.iter().map(String::as_str));
        let path_of = |file: &str| if relative.is_empty() { file.to_owned() } else { format!("{relative}/{file}") };
        for s in grouping.skipped {
            report.skipped.push(SkippedView { file: path_of(&s.file), reason: s.reason });
        }
        for s in grouping.series {
            let files: Vec<PathBuf> = s.versions.iter().map(|v| dir.join(&v.file)).collect();
            // Asking Excel for each of dozens of versions would take minutes
            // for nothing: only a file held open can have unsaved edits.
            let read = |i: usize| crate::excel::read(&files[i], unsaved && crate::excel::in_use(&files[i]));
            let steps = series::walk(files.len(), read, options, min_edits);
            let mut view = SeriesView {
                folder: relative.clone(),
                name: s.name,
                extension: s.extension,
                versions: Vec::new(),
                kept: 0,
                removable: 0,
                removable_bytes: 0,
            };
            for (i, (version, step)) in s.versions.iter().zip(steps).enumerate() {
                let bytes = std::fs::metadata(&files[i]).ok().map(|m| m.len());
                let keep = step.reason.keep();
                if keep {
                    view.kept += 1;
                } else {
                    view.removable += 1;
                    view.removable_bytes += bytes.unwrap_or(0);
                }
                if bytes.is_none() {
                    notes.add(
                        "unknown_size",
                        "its size could not be read and is not in removableBytes".into(),
                        path_of(&version.file),
                    );
                }
                let compared_with = step.compared_with.map(|k| s.versions[k].file.clone());
                let here = path_of(&version.file);
                for w in step.warnings {
                    notes.add(w.code, w.message, here.clone());
                }
                for w in step.pair_warnings {
                    let base = compared_with.as_deref().map(path_of).unwrap_or_default();
                    notes.add(w.code, w.message, format!("{here} vs {base}"));
                }
                view.versions.push(VersionView {
                    file: version.file.clone(),
                    stamp: version.stamp.clone(),
                    bytes,
                    keep,
                    reason: step.reason,
                    compared_with,
                    edits: step.summary.as_ref().map(series::edits),
                    summary: step.summary,
                    error: step.error.map(|f| ErrorView { code: f.code, message: f.message, hint: f.hint }),
                });
            }
            report.totals.series += 1;
            report.totals.versions += view.versions.len();
            report.totals.kept += view.kept;
            report.totals.removable += view.removable;
            report.totals.removable_bytes += view.removable_bytes;
            report.series.push(view);
        }
    }
    for folder in unlisted {
        notes.add(
            "unreadable_folder",
            "the folder could not be listed, so its versions were not judged".into(),
            folder,
        );
    }
    let mut warnings = notes.warnings();
    let unreadable = report.series.iter().flat_map(|s| &s.versions).filter(|v| v.reason == Reason::Unreadable).count();
    if unreadable > 0 {
        warnings.push(Warning {
            code: "unreadable_version".to_owned(),
            message: format!(
                "{unreadable} version(s) could not be read and are kept; later versions are compared with the last \
                 version that could"
            ),
        });
    }
    Ok(match format {
        Format::Json => Output::Json { result: json(&report), warnings },
        Format::Text => Output::Text(text(&report, &warnings)),
    })
}

/// Warnings gathered across versions: versions of one workbook mostly share
/// them, so each distinct warning is reported once with the files it
/// applies to.
#[derive(Default)]
struct Notes(Vec<(&'static str, String, Vec<String>)>);

impl Notes {
    fn add(&mut self, code: &'static str, message: String, file: String) {
        match self.0.iter_mut().find(|(c, m, _)| *c == code && *m == message) {
            Some((_, _, files)) => files.push(file),
            None => self.0.push((code, message, vec![file])),
        }
    }

    fn warnings(self) -> Vec<Warning> {
        self.0
            .into_iter()
            .map(|(code, message, files)| {
                let whom = match files.as_slice() {
                    [one] => one.clone(),
                    [first, rest @ ..] => format!("{first} and {} more", rest.len()),
                    [] => String::new(),
                };
                Warning { code: code.to_owned(), message: format!("{whom}: {message}") }
            })
            .collect()
    }
}

/// `(relative path, folder)` for the folder and, with `recursive`, every
/// folder below it, in name order. Links and junctions are not followed (one
/// pointing above itself would never end); subfolders that cannot be listed
/// go to `unlisted`.
fn folders(root: &Path, recursive: bool, unlisted: &mut Vec<String>) -> Result<Vec<(String, PathBuf)>, Failure> {
    let mut out = vec![(String::new(), root.to_path_buf())];
    let mut next = 0;
    while recursive && next < out.len() {
        let (relative, dir) = out[next].clone();
        next += 1;
        let listed = match entries(&dir) {
            Ok(listed) => listed,
            Err(_) if !relative.is_empty() => {
                unlisted.push(relative);
                continue;
            }
            Err(e) => return Err(e),
        };
        let mut children: Vec<(String, PathBuf)> = listed
            .into_iter()
            .filter(|(_, path)| {
                std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
            })
            .map(|(name, path)| (if relative.is_empty() { name.clone() } else { format!("{relative}/{name}") }, path))
            .collect();
        children.sort();
        out.extend(children);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The names of the regular files in `dir`.
fn file_names(dir: &Path) -> Result<Vec<String>, Failure> {
    Ok(entries(dir)?.into_iter().filter(|(_, path)| path.is_file()).map(|(name, _)| name).collect())
}

/// `(name, path)` for each entry with a Unicode name; other names cannot
/// carry a stamp this command reads.
fn entries(dir: &Path) -> Result<Vec<(String, PathBuf)>, Failure> {
    let io =
        |e: std::io::Error| Failure::from(Error::new(ErrorCode::IoError, format!("reading {}: {e}", dir.display())));
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        if let Ok(name) = entry.file_name().into_string() {
            out.push((name, entry.path()));
        }
    }
    Ok(out)
}

#[allow(clippy::cast_precision_loss)]
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn text(r: &Report, warnings: &[Warning]) -> String {
    let mut out = String::new();
    let t = &r.totals;
    let _ = writeln!(
        out,
        "{}: {} series, {} version(s); keep {}, remove {} ({}); min edits {}",
        escape(&r.folder),
        t.series,
        t.versions,
        t.kept,
        t.removable,
        megabytes(t.removable_bytes),
        r.min_edits
    );
    for s in &r.series {
        let folder = if s.folder.is_empty() { String::new() } else { format!("{}/", escape(&s.folder)) };
        let _ = writeln!(
            out,
            "\n[{folder}{}.{}] keep {}, remove {} ({})",
            escape(&s.name),
            s.extension,
            s.kept,
            s.removable,
            megabytes(s.removable_bytes)
        );
        let _ =
            writeln!(out, "stamp\tverdict\treason\tedits\tinput\tformula\tlines\tcomputed\tformat\tobjects\tvs\tfile");
        for v in &s.versions {
            let verdict = if v.keep { "keep" } else { "remove" };
            let vs = v
                .compared_with
                .as_ref()
                .and_then(|f| s.versions.iter().find(|x| &x.file == f))
                .map_or("-", |x| x.stamp.as_str());
            let counts = v.summary.as_ref().map_or_else(
                || "-\t-\t-\t-\t-\t-\t-".to_owned(),
                |m| {
                    let lines = m.rows_only_base
                        + m.rows_only_other
                        + m.rows_moved
                        + m.columns_only_base
                        + m.columns_only_other
                        + m.columns_moved
                        + m.sheets_only_base
                        + m.sheets_only_other
                        + m.sheets_renamed;
                    format!(
                        "{}\t{}\t{}\t{lines}\t{}\t{}\t{}",
                        series::edits(m),
                        m.value_input,
                        m.formula,
                        m.value_computed,
                        m.format,
                        m.objects
                    )
                },
            );
            let _ = write!(out, "{}\t{verdict}\t{}\t{counts}\t{vs}\t{}", v.stamp, v.reason.as_str(), escape(&v.file));
            if let Some(e) = &v.error {
                let _ = write!(out, "\t{}: {}", e.code, escape(&e.message));
            }
            out.push('\n');
        }
    }
    if !r.skipped.is_empty() {
        out.push('\n');
        for s in &r.skipped {
            let reason = match s.reason {
                SkipReason::LockFile => "lockFile",
                SkipReason::NoStamp => "noStamp",
            };
            let _ = writeln!(out, "skipped\t{reason}\t{}", escape(&s.file));
        }
    }
    write_warnings(&mut out, warnings);
    out
}

#[cfg(test)]
mod tests {
    use super::Notes;

    #[test]
    fn a_warning_shared_by_versions_is_reported_once() {
        let mut notes = Notes::default();
        notes.add("manual_calculation", "stale".into(), "a.xlsx".into());
        notes.add("not_compared", "charts".into(), "a.xlsx".into());
        notes.add("manual_calculation", "stale".into(), "b.xlsx".into());
        notes.add("manual_calculation", "stale".into(), "c.xlsx".into());
        let messages: Vec<String> = notes.warnings().into_iter().map(|w| format!("{} {}", w.code, w.message)).collect();
        assert_eq!(messages, ["manual_calculation a.xlsx and 2 more: stale", "not_compared a.xlsx: charts"]);
    }
}
