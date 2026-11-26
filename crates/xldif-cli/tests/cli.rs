//! The CLI as an external process: the process contract.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn xldif<const N: usize>(args: [&str; N]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_xldif")).args(args).output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// A per-test scratch directory under the system temp dir, removed on drop.
struct Work(PathBuf);

impl Work {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("xldif-cli-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn basic(&self) -> PathBuf {
        self.file("basic.xlsx", &xldif_fixtures::basic().unwrap().save_to_buffer().unwrap())
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn envelope(run: &Run) -> Value {
    assert!(run.stdout.ends_with('\n') && run.stdout.matches('\n').count() == 1, "one line: {:?}", run.stdout);
    serde_json::from_str(&run.stdout).unwrap()
}

#[test]
fn bare_invocation_prints_root_help() {
    let bare = xldif([]);
    let help = xldif(["--help"]);
    assert_eq!((bare.code, help.code), (0, 0));
    assert_eq!(bare.stdout, help.stdout);
    assert!(bare.stdout.starts_with("Compare Excel workbooks"));
    assert!(bare.stderr.is_empty());
}

#[test]
fn version_is_plain_text() {
    let v = xldif(["--version"]);
    assert_eq!(v.code, 0);
    assert_eq!(v.stdout, format!("xldif {}\n", env!("CARGO_PKG_VERSION")));
    assert!(v.stderr.is_empty());
}

#[test]
fn invalid_invocation_is_a_json_failure_with_exit_2() {
    let run = xldif(["no-such-command"]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.is_empty());
    let env = envelope(&run);
    assert_eq!(env["ok"], false);
    assert_eq!(env["command"], "no-such-command");
    assert_eq!(env["result"], Value::Null);
    assert_eq!(env["error"]["code"], "invalid_invocation");
    assert!(env["error"]["hint"].as_str().unwrap().contains("xldif --help"));
    assert_eq!(env["metadata"]["tool"], "xldif");
    assert_eq!(env["metadata"]["contract"], "1");
    assert_eq!(env["metadata"]["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn pretty_indents_failures() {
    let run = xldif(["--pretty", "--no-such-flag"]);
    assert_eq!(run.code, 2);
    assert!(run.stdout.starts_with("{\n"));
    let env: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(env["command"], Value::Null);
    assert_eq!(env["error"]["code"], "invalid_invocation");
}

// ---------------------------------------------------------------------------
// inspect and cells

fn ok(run: &Run, command: &str) -> Value {
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(run.stderr.is_empty());
    let env = envelope(run);
    assert_eq!((env["ok"].clone(), env["command"].clone()), (Value::Bool(true), Value::from(command)));
    assert_eq!(env["error"], Value::Null);
    env
}

#[test]
fn inspect_reports_structure_without_contents() {
    let work = Work::new();
    let book = work.basic();
    let env = ok(&xldif(["inspect", s(&book)]), "inspect");
    let r = &env["result"];
    assert_eq!(r["file"], "basic.xlsx");
    assert_eq!(r["format"], "xlsx");
    assert_eq!(r["date1904"], false);
    assert_eq!(r["calculation"]["mode"], "auto");
    let bs = &r["sheets"][0];
    assert_eq!((bs["name"].clone(), bs["usedRange"].clone()), (Value::from("BS"), Value::from("A1:B4")));
    assert_eq!((bs["cells"].as_u64(), bs["formulas"].as_u64()), (Some(8), Some(1)));
    assert_eq!(bs["visibility"], "visible");
    assert!(!env.to_string().contains("Receivables"), "inspect must not print cell contents");
    let codes: Vec<&str> = env["warnings"].as_array().unwrap().iter().map(|w| w["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["full_calc_on_load"]);
}

#[test]
fn inspect_text_lists_sheets() {
    let work = Work::new();
    let book = work.basic();
    let run = xldif(["inspect", s(&book), "--format", "text"]);
    assert_eq!(run.code, 0);
    assert!(run.stdout.starts_with("basic.xlsx ("));
    assert!(run.stdout.contains("1\tBS\tWorksheet\tVisible\tA1:B4\tcells 8\tformulas 1\n"), "{}", run.stdout);
}

/// A workbook with one formula through an external link, and the files it
/// links to in the order formulas number them.
fn linked_book(links: &[&str], formula: &str) -> Vec<u8> {
    let rows = format!(
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Opening balance</t></is></c><c r="B1"><f>{formula}</f><v>10</v></c></row>"#
    );
    let mut book = xldif_fixtures::raw::RawBook::new().sheet("Summary", &rows);
    for target in links {
        book = book.external_link(target);
    }
    book.build()
}

#[test]
fn external_links_are_listed_and_compared_by_the_file_they_name() {
    let work = Work::new();
    let base = work.file("base.xlsx", &linked_book(&["FY2024.xlsx"], "[1]Sheet1!A1"));
    let renumbered = work.file("renumbered.xlsx", &linked_book(&["Budget.xlsx", "FY2024.xlsx"], "[2]Sheet1!A1"));
    let repointed = work.file("repointed.xlsx", &linked_book(&["FY2025.xlsx"], "[1]Sheet1!A1"));

    let env = ok(&xldif(["inspect", s(&renumbered)]), "inspect");
    assert_eq!(env["result"]["externalLinks"], serde_json::json!(["Budget.xlsx", "FY2024.xlsx"]));
    assert_eq!(env["warnings"], serde_json::json!([]));
    let text = xldif(["inspect", s(&renumbered), "--format", "text"]);
    assert!(text.stdout.contains("link\t[1]\tBudget.xlsx\nlink\t[2]\tFY2024.xlsx\n"), "{}", text.stdout);
    let env = ok(&xldif(["inspect", s(&work.basic())]), "inspect");
    assert_eq!(env["result"]["externalLinks"], serde_json::json!([]));

    // Another number for the same file is not a change; the new link is.
    let env = ok(&xldif(["compare", s(&base), s(&renumbered)]), "compare");
    let r = &env["result"];
    assert_eq!((r["summary"]["formula"].as_u64(), r["summary"]["objects"].as_u64()), (Some(0), Some(1)));
    let link = &r["objects"][0];
    assert_eq!(link["id"], "externalLink:-:1");
    assert_eq!((link["kind"].clone(), link["status"].clone()), ("externalLink".into(), "onlyOther".into()));
    assert_eq!(link["other"]["text"], "Budget.xlsx");

    // The same number for another file is one change, the link's; the
    // formula through it, whose text did not change, is not listed.
    let env = ok(&xldif(["compare", s(&base), s(&repointed)]), "compare");
    let r = &env["result"];
    assert_eq!(r["objects"][0]["id"], "externalLink:1:1");
    assert_eq!(r["objects"][0]["status"], "changed");
    assert_eq!(
        r["objects"][0]["properties"][0],
        serde_json::json!({"name": "target", "base": "FY2024.xlsx", "other": "FY2025.xlsx"})
    );
    assert_eq!(r["summary"]["formula"], 0);
    let text = xldif(["compare", s(&base), s(&repointed), "--format", "text"]);
    assert!(
        text.stdout.contains("externalLink:1:1\texternalLink\tchanged\t-→-\ttarget: FY2024.xlsx → FY2025.xlsx\n"),
        "{}",
        text.stdout
    );

    // The kind filters like any other.
    let env = ok(&xldif(["compare", s(&base), s(&repointed), "--kind", "external-link"]), "compare");
    assert_eq!(env["result"]["objects"].as_array().unwrap().len(), 1);
    let env = ok(&xldif(["compare", s(&base), s(&repointed), "--kind", "formula"]), "compare");
    assert_eq!(env["result"]["objects"].as_array().unwrap().len(), 0);

    // The report lists the link among the workbook's changes.
    let report = work.0.join("links.html");
    ok(&xldif(["compare", s(&base), s(&repointed), "--format", "html", "-o", s(&report)]), "compare");
    let html = std::fs::read_to_string(&report).unwrap();
    assert!(html.contains("externalLink:1:1") && html.contains("FY2025.xlsx"), "the report names the link");
}

#[test]
fn cells_lists_values_and_formulas() {
    let work = Work::new();
    let book = work.basic();
    let env = ok(&xldif(["cells", s(&book), "--sheet", "BS", "--range", "B3:B4"]), "cells");
    let cells = env["result"]["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0]["cell"], "B3");
    assert_eq!(cells[0]["type"], "number");
    assert_eq!(cells[0]["value"], 200.0);
    assert!(cells[0].get("formula").is_none());
    assert_eq!(cells[1]["formula"], "SUM(B2:B3)");
    assert_eq!(cells[1]["formulaKind"], "normal");
    assert_eq!(env["result"]["omitted"], 0);
}

#[test]
fn cells_limit_counts_what_it_leaves_out() {
    let work = Work::new();
    let book = work.basic();
    let env = ok(&xldif(["cells", s(&book), "--sheet", "BS", "--limit", "3"]), "cells");
    assert_eq!(env["result"]["cells"].as_array().unwrap().len(), 3);
    assert_eq!(env["result"]["omitted"], 5);
    let all = ok(&xldif(["cells", s(&book), "--sheet", "BS", "--limit", "0"]), "cells");
    assert_eq!(all["result"]["cells"].as_array().unwrap().len(), 8);
    let text = xldif(["cells", s(&book), "--sheet", "BS", "--format", "text", "--limit", "1"]);
    assert_eq!(text.stdout, "A1\ttext\tAccount\n... 7 more cells (raise --limit, or 0 for all)\n");
}

fn failure(run: &Run, code: &str) -> Value {
    assert_eq!(run.code, 1, "{}", run.stdout);
    let env = envelope(run);
    assert_eq!(env["ok"], false);
    assert_eq!(env["error"]["code"], code, "{env}");
    env
}

#[test]
fn reading_failures_have_stable_codes() {
    let work = Work::new();
    let book = work.basic();
    let env = failure(&xldif(["cells", s(&book), "--sheet", "Nope"]), "invalid_argument");
    assert!(env["error"]["hint"].as_str().unwrap().contains(r#""BS", "Notes""#));
    failure(&xldif(["cells", s(&book), "--sheet", "BS", "--range", "B0"]), "invalid_argument");
    failure(&xldif(["inspect", s(&work.0.join("missing.xlsx"))]), "file_not_found");
    failure(&xldif(["inspect", s(&work.0)]), "file_not_found");
    let text = work.file("notes.xlsx", b"just text");
    failure(&xldif(["inspect", s(&text)]), "invalid_archive");
    let ole = work.file("old.xls", &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]);
    let env = failure(&xldif(["inspect", s(&ole)]), "unsupported_format");
    assert!(env["error"]["hint"].is_string());
}

#[test]
fn cells_text_escapes_and_always_prints_a_document() {
    let work = Work::new();
    let rows = r#"<row r="1"><c r="A1" t="str"><f>LET(x,1,
x)</f><v>a\b</v></c><c r="B1" t="s"><v>9</v></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>_x001B_[31mred</t></is></c></row>"#;
    let book = work.file("odd.xlsx", &xldif_fixtures::raw::RawBook::new().sheet("S", rows).sheet("E", "").build());
    let run = xldif(["cells", s(&book), "--sheet", "S", "--format", "text"]);
    assert_eq!(run.code, 0);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines[0], r"A1	text	a\\b	=LET(x,1,\nx)");
    // ESC would start a terminal escape sequence; it is shown, not sent.
    assert_eq!(lines[2], r"A2	text	\u{1b}[31mred");
    assert!(lines.iter().any(|l| l.starts_with("warning\tunreadable_value")), "{}", run.stdout);
    let empty = xldif(["cells", s(&book), "--sheet", "E", "--format", "text"]);
    // Workbook warnings follow, whichever sheet is listed.
    assert!(empty.stdout.starts_with("(no cells)\nwarning\tunreadable_value"), "{}", empty.stdout);
}

#[test]
fn missing_arguments_hint_at_the_command_help() {
    let run = xldif(["cells", "book.xlsx"]);
    assert_eq!(run.code, 2);
    let env = envelope(&run);
    assert_eq!(env["command"], "cells");
    assert!(env["error"]["hint"].as_str().unwrap().contains("xldif cells --help"));
}

// ---------------------------------------------------------------------------
// compare

#[test]
fn compare_reports_aligned_changes() {
    let work = Work::new();
    let base = work.file(
        "v1.xlsx",
        &xldif_fixtures::grid_book(&[("BS", &[&["Account", "2025"], &["Cash", "100"], &["Total", "100"]])]),
    );
    let other = work.file(
        "v2.xlsx",
        &xldif_fixtures::grid_book(&[(
            "BS",
            &[&["Account", "2025"], &["Cash", "100"], &["Prepaid", "5"], &["Total", "105"]],
        )]),
    );
    let env = ok(&xldif(["compare", s(&base), s(&other)]), "compare");
    let r = &env["result"];
    assert_eq!(
        (r["base"]["file"].clone(), r["other"]["file"].clone()),
        (Value::from("v1.xlsx"), Value::from("v2.xlsx"))
    );
    assert_eq!(r["summary"]["rowsOnlyOther"], 1);
    assert_eq!(r["summary"]["value"], 1);
    let changes = r["sheets"][0]["changes"].as_array().unwrap();
    assert_eq!(changes[0]["id"], "s0:row:-:3");
    assert_eq!(changes[1]["id"], "s0:value:B3:B4");
    assert_eq!(changes[1]["delta"], 5.0);
    assert_eq!(changes[1]["labels"]["row"], "Total");
    assert_eq!(r["listed"], 2);

    let text = xldif(["compare", s(&base), s(&other), "--format", "text", "--kind", "value"]);
    assert_eq!(text.code, 0);
    assert!(text.stdout.contains("value\tchanged\tB3→B4\t100 → 105 (+5)\tTotal | 2025"), "{}", text.stdout);
    assert!(text.stdout.contains("1 more change(s) left out"), "{}", text.stdout);

    let same = ok(&xldif(["compare", s(&base), s(&base)]), "compare");
    assert_eq!(same["result"]["sheets"].as_array().unwrap().len(), 0);
    assert_eq!(
        ok(&xldif(["compare", s(&base), s(&base), "--all"]), "compare")["result"]["sheets"][0]["status"],
        "same"
    );
}

#[test]
fn compare_rejects_bad_options() {
    let work = Work::new();
    let book = work.basic();
    failure(&xldif(["compare", s(&book), s(&book), "--sheet", "Nope"]), "invalid_argument");
    failure(&xldif(["compare", s(&book), s(&book), "--tolerance=-1"]), "invalid_argument");
    let run = xldif(["compare", s(&book), s(&book), "--kind", "colour"]);
    assert_eq!(run.code, 2);
}

#[test]
fn compare_reports_formula_changes_in_json_and_text() {
    let work = Work::new();
    let grid = |total: &str| -> Vec<u8> {
        xldif_fixtures::grid_book(&[(
            "BS",
            &[&["Account", "2025"], &["Cash", "100"], &["Debt", "40"], &["Net", total]],
        )])
    };
    let base = work.file("v1.xlsx", &grid("=B2-B3|60"));
    let other = work.file("v2.xlsx", &grid("=B2+B3|140"));
    let env = ok(&xldif(["compare", s(&base), s(&other), "--kind", "formula"]), "compare");
    let changes = env["result"]["sheets"][0]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["id"], "s0:formula:B4:B4");
    assert_eq!(changes[0]["base"]["formula"], "B2-B3");
    assert_eq!(changes[0]["other"]["formula"], "B2+B3");
    assert_eq!(env["result"]["summary"]["formula"], 1);
    let text = xldif(["compare", s(&base), s(&other), "--kind", "formula", "--format", "text"]);
    assert!(text.stdout.contains("formula	changed	B4→B4	=B2-B3 → =B2+B3	Net | 2025"), "{}", text.stdout);
}

#[test]
fn compare_writes_a_self_contained_html_report() {
    let work = Work::new();
    let grid =
        |v: &str| xldif_fixtures::grid_book(&[("BS", &[&["Account", "2025"], &["<script>alert(1)</script>", v]])]);
    let base = work.file("v1.xlsx", &grid("1"));
    let other = work.file("v2.xlsx", &grid("2"));
    let report = work.0.join("report.html");

    let run = xldif(["compare", s(&base), s(&other), "--format", "html"]);
    assert_eq!(envelope(&failure_run(run))["error"]["code"], "invalid_argument");

    let env = ok(&xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&report)]), "compare");
    assert_eq!(env["result"]["summary"]["value"], 1);
    let html = std::fs::read_to_string(&report).unwrap();
    assert!(html.starts_with("<!doctype html>"));
    // Self-contained: no external scripts, styles, images, or fonts.
    for needle in ["<script src", "<link", "<img", "url(http", "@import"] {
        assert!(!html.contains(needle), "{needle}");
    }
    // Workbook text never becomes markup, even inside the embedded data.
    assert!(!html.contains("<script>alert(1)"));

    let again = xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&report)]);
    assert_eq!(envelope(&failure_run(again))["error"]["code"], "output_exists");
    ok(&xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&report), "--force"]), "compare");
    let over_input = xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&base), "--force"]);
    assert_eq!(envelope(&failure_run(over_input))["error"]["code"], "output_exists");
    assert!(std::fs::read(&base).unwrap().starts_with(b"PK"), "the input is untouched");
}

fn failure_run(run: Run) -> Run {
    assert_eq!(run.code, 1, "{}", run.stdout);
    run
}

#[test]
fn html_grids_follow_the_alignment_whatever_is_listed() {
    let work = Work::new();
    let base =
        work.file("a.xlsx", &xldif_fixtures::grid_book(&[("S", &[&["k", "v"], &["Cash", "1"], &["Debt", "2"]])]));
    let other = work.file(
        "b.xlsx",
        &xldif_fixtures::grid_book(&[(
            "S",
            &[&["k", "v", "new"], &["Cash", "1", "x"], &["Loans", "9", ""], &["Debt", "3", "y"]],
        )]),
    );
    let report = work.0.join("r.html");
    ok(&xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&report), "--kind", "value"]), "compare");
    let html = std::fs::read_to_string(&report).unwrap();
    // The inserted row is drawn (as a one-sided row) even though only values are listed.
    assert!(html.contains("class=\"r ro"), "inserted row missing");
    assert!(html.contains("Loans"));

    ok(&xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&report), "--force"]), "compare");
    let html = std::fs::read_to_string(&report).unwrap();
    // The inserted column's header carries its change for navigation.
    assert!(html.contains("<th class=\"ch co\" data-c="), "column change not marked");

    let dir = work.0.join("a-dir");
    std::fs::create_dir(&dir).unwrap();
    let env = envelope(&failure_run(xldif(["compare", s(&base), s(&other), "-o", s(&dir), "--force"])));
    assert_eq!(env["error"]["code"], "invalid_argument");
    let leftovers: Vec<_> = std::fs::read_dir(&work.0)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left behind");
}

/// A sheet with a formatted amount, a date, and a plain number in row 2,
/// under labels in row 1.
fn formatted_book(amount: f64, date: f64) -> Vec<u8> {
    use xldif_fixtures::rust_xlsxwriter::{Format, Workbook};
    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name("S").unwrap();
    for (col, label) in ["Amount", "Date", "Plain"].into_iter().enumerate() {
        sheet.write_string(0, u16::try_from(col).unwrap(), label).unwrap();
    }
    sheet.write_number_with_format(1, 0, amount, &Format::new().set_num_format("#,##0.00")).unwrap();
    sheet.write_number_with_format(1, 1, date, &Format::new().set_num_format("yyyy-mm-dd")).unwrap();
    // A format that does not change how 7 reads.
    sheet.write_number_with_format(1, 2, 7.0, &Format::new().set_num_format("0")).unwrap();
    book.save_to_buffer().unwrap()
}

#[test]
fn cells_and_changes_carry_display_text_and_iso_dates() {
    let work = Work::new();
    let base = work.file("base.xlsx", &formatted_book(1234.5, 45_292.0));
    let other = work.file("other.xlsx", &formatted_book(1300.0, 45_293.0));
    let env = ok(&xldif(["cells", s(&base), "--sheet", "S", "--range", "A2:C2"]), "cells");
    let cells = env["result"]["cells"].as_array().unwrap();
    assert_eq!((cells[0]["value"].clone(), cells[0]["display"].clone()), (1234.5.into(), "1,234.50".into()));
    assert!(cells[0].get("date").is_none());
    assert_eq!((cells[1]["display"].clone(), cells[1]["date"].clone()), ("2024-01-01".into(), "2024-01-01".into()));
    assert!(cells[2].get("display").is_none() && cells[2].get("date").is_none(), "7 reads as 7");
    let text = xldif(["cells", s(&base), "--sheet", "S", "--range", "A2", "--format", "text"]);
    assert_eq!(text.stdout, "A2\tnumber\t1234.5 [1,234.50]\n");

    let env = ok(&xldif(["compare", s(&base), s(&other)]), "compare");
    let change = &env["result"]["sheets"][0]["changes"][1];
    assert_eq!(change["kind"], "value");
    assert_eq!(
        (change["base"]["date"].clone(), change["other"]["date"].clone()),
        ("2024-01-01".into(), "2024-01-02".into())
    );
    let text = xldif(["compare", s(&base), s(&other), "--format", "text"]);
    assert!(text.stdout.contains("45292 [2024-01-01] → 45293 [2024-01-02] (+1)"), "{}", text.stdout);
    assert!(text.stdout.contains("1234.5 [1,234.50] → 1300 [1,300.00] (+65.5)"), "{}", text.stdout);

    let report = work.0.join("report.html");
    ok(&xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&report)]), "compare");
    let html = std::fs::read_to_string(&report).unwrap();
    assert!(html.contains(">2024-01-02<") && html.contains(">1,300.00<"), "the grid shows display text");
}

#[test]
fn compare_warns_about_content_it_does_not_read() {
    let work = Work::new();
    let main = xldif_fixtures::raw::MAIN_NS;
    let protected = xldif_fixtures::raw::RawBook::new()
        .sheet_part(
            "S",
            &format!(
                r#"<worksheet xmlns="{main}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><sheetProtection sheet="1"/></worksheet>"#
            ),
        )
        .build();
    let plain = xldif_fixtures::raw::RawBook::new().sheet("S", r#"<row r="1"><c r="A1"><v>1</v></c></row>"#).build();
    let (base, other) = (work.file("plain.xlsx", &plain), work.file("protected.xlsx", &protected));
    let env = ok(&xldif(["compare", s(&base), s(&other)]), "compare");
    // Nothing the model holds changed, and the warning says what it leaves out.
    assert_eq!(env["result"]["summary"]["sheetsChanged"], 0);
    let warnings: Vec<(&str, &str)> = env["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["code"].as_str().unwrap(), w["message"].as_str().unwrap()))
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].0, "not_compared");
    assert!(warnings[0].1.starts_with("other: ") && warnings[0].1.ends_with("sheet protection (1)"), "{warnings:?}");
    let inspected = ok(&xldif(["inspect", s(&other)]), "inspect");
    assert_eq!(inspected["warnings"][0]["code"], "not_compared");
    // The report a person reads carries it too.
    let report = work.0.join("report.html");
    ok(&xldif(["compare", s(&base), s(&other), "--format", "html", "-o", s(&report)]), "compare");
    let html = std::fs::read_to_string(&report).unwrap();
    assert!(html.contains("<b>not_compared</b> other: ") && html.contains("sheet protection (1)"));
}

#[test]
fn a_malformed_formula_does_not_break_the_contract() {
    let work = Work::new();
    let main = xldif_fixtures::raw::MAIN_NS;
    let book = |formula: &str| {
        xldif_fixtures::raw::RawBook::new()
            .sheet_part(
                "S",
                &format!(
                    r#"<worksheet xmlns="{main}"><sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1"><f>{formula}</f><v>2</v></c></row></sheetData></worksheet>"#
                ),
            )
            .build()
    };
    let (base, other) = (work.file("a.xlsx", &book("A1+1")), work.file("b.xlsx", &book("A1+[")));
    let run = xldif(["compare", s(&base), s(&other)]);
    assert!(run.stderr.is_empty(), "{}", run.stderr);
    let env = ok(&run, "compare");
    assert_eq!(env["result"]["summary"]["formula"], 1);
}

/// A one-sheet book: a rate and the revenue it drives, plus a note.
fn version_book(rate: f64, note: &str) -> Vec<u8> {
    let revenue = format!("{}", 1000.0 * (1.0 + rate));
    let grid: &[&[&str]] = &[
        &["Growth", &rate.to_string()],
        &["Base revenue", "1000"],
        &["Revenue", &format!("=B2*(1+B1)|{revenue}")],
        &["Note", note],
    ];
    xldif_fixtures::grid_book(&[("DCF", grid)])
}

#[test]
fn series_keeps_versions_with_enough_edits_since_the_last_kept() {
    let work = Work::new();
    work.file("DCF_261120_1637.xlsx", &version_book(0.1, "draft"));
    work.file("DCF_261120_1649.xlsx", &version_book(0.1, "draft 2"));
    work.file("DCF_261121_0900_review.xlsx", &version_book(0.2, "reviewed"));
    work.file("DCF_261122_1000.xlsx", &version_book(0.2, "final"));
    work.file("~$DCF_261122_1000.xlsx", b"owner");
    work.file("template.xlsx", &version_book(0.1, "x"));
    work.file("broken_261121_0800.xlsx", b"not a workbook");
    work.file("notes.txt", b"ignored");
    std::fs::create_dir(work.0.join("sub")).unwrap();
    work.file("sub/FS_261123_1112.xlsx", &version_book(0.1, "fs"));

    let r = &ok(&xldif(["series", s(&work.0), "--min-edits", "2"]), "series")["result"];
    let series = r["series"].as_array().unwrap();
    assert_eq!(series.len(), 2, "the subfolder is not read without --recursive");
    let dcf = &series[0];
    assert_eq!((dcf["name"].as_str(), dcf["folder"].as_str()), (Some("DCF"), Some("")));
    let verdicts: Vec<(&str, bool, Option<&str>, Option<u64>)> = dcf["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["reason"].as_str().unwrap(),
                v["keep"].as_bool().unwrap(),
                v["comparedWith"].as_str(),
                v["edits"].as_u64(),
            )
        })
        .collect();
    assert_eq!(
        verdicts,
        [
            ("first", true, None, None),
            ("fewEdits", false, Some("DCF_261120_1637.xlsx"), Some(1)),
            // The rate and the note: two edits; the revenue only recalculated.
            ("edited", true, Some("DCF_261120_1637.xlsx"), Some(2)),
            ("latest", true, Some("DCF_261121_0900_review.xlsx"), Some(1)),
        ]
    );
    let reviewed = &dcf["versions"][2]["summary"];
    assert_eq!((reviewed["valueInput"].as_u64(), reviewed["valueComputed"].as_u64()), (Some(2), Some(1)));
    let removed = dcf["versions"][1]["bytes"].as_u64().unwrap();
    assert_eq!((dcf["removable"].as_u64(), dcf["removableBytes"].as_u64()), (Some(1), Some(removed)));

    // A version that cannot be read is kept, with its error.
    let broken = &series[1]["versions"][0];
    assert_eq!((broken["reason"].as_str(), broken["keep"].as_bool()), (Some("unreadable"), Some(true)));
    assert_eq!(broken["error"]["code"], "invalid_archive");

    let skipped: Vec<(&str, &str)> = r["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["file"].as_str().unwrap(), s["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(skipped, [("template.xlsx", "noStamp"), ("~$DCF_261122_1000.xlsx", "lockFile")]);
    assert_eq!(r["totals"]["removable"], 1);

    let recursive = &ok(&xldif(["series", s(&work.0), "--recursive"]), "series")["result"];
    let folders: Vec<&str> =
        recursive["series"].as_array().unwrap().iter().map(|s| s["folder"].as_str().unwrap()).collect();
    assert_eq!(folders, ["", "", "sub"]);

    let text = xldif(["series", s(&work.0), "--min-edits", "2", "--format", "text"]);
    assert_eq!(text.code, 0);
    assert!(text.stdout.contains("keep 4, remove 1"), "{}", text.stdout);
    assert!(text.stdout.contains("261120_1649\tremove\tfewEdits\t1\t1\t0\t0\t0\t"), "{}", text.stdout);
    assert!(text.stdout.contains("skipped\tlockFile\t~$DCF_261122_1000.xlsx"), "{}", text.stdout);
}

#[test]
fn series_rejects_a_file_or_a_missing_folder() {
    let work = Work::new();
    let book = work.basic();
    failure(&xldif(["series", s(&book)]), "invalid_argument");
    failure(&xldif(["series", s(&work.0.join("missing"))]), "file_not_found");
    failure(&xldif(["series", s(&work.0), "--tolerance=-1"]), "invalid_argument");
}
