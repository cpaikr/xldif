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
