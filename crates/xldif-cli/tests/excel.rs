//! The two things the CLI uses Excel for: converting a workbook the reader
//! rejects, and reading a workbook as a running Excel holds it.
//!
//! These tests start hidden Excel instances and take tens of seconds, so
//! they run only when `XLDIF_TEST_EXCEL=1` (`scripts/check.py` sets it where
//! Excel is installed) and pass without doing anything otherwise.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn excel_tests_enabled() -> bool {
    let enabled = cfg!(windows) && std::env::var_os("XLDIF_TEST_EXCEL").is_some_and(|v| v == "1");
    if !enabled {
        eprintln!("skipped: set XLDIF_TEST_EXCEL=1 on Windows with Excel to run");
    }
    enabled
}

/// A scratch directory under the system temp dir, removed on drop.
struct Work(PathBuf);

impl Work {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("xldif-excel-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn basic(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, xldif_fixtures::basic().unwrap().save_to_buffer().unwrap()).unwrap();
        path
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

/// Runs the CLI and returns its envelope, which must be a success.
fn ok(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_xldif")).args(args).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success() && out.stderr.is_empty(), "{stdout}");
    serde_json::from_str(&stdout).unwrap()
}

fn warning_codes(env: &Value) -> Vec<&str> {
    env["warnings"].as_array().unwrap().iter().map(|w| w["code"].as_str().unwrap()).collect()
}

fn powershell(script: &Path) -> Command {
    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(script);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command
}

#[test]
fn legacy_and_binary_workbooks_are_read_through_excel() {
    if !excel_tests_enabled() {
        return;
    }
    let work = Work::new("convert");
    let book = work.basic("basic.xlsx");
    let (xls, xlsb) = (work.0.join("basic.xls"), work.0.join("basic.xlsb"));
    // Excel itself writes the two formats the reader rejects (56 and 50).
    let script = work.0.join("save.ps1");
    std::fs::write(
        &script,
        "$xl = New-Object -ComObject Excel.Application\n\
         try { $xl.DisplayAlerts = $false; $wb = $xl.Workbooks.Open($env:XLDIF_T_IN)\n\
         $wb.SaveAs($env:XLDIF_T_XLS, 56); $wb.SaveAs($env:XLDIF_T_XLSB, 50); $wb.Close($false) }\n\
         finally { $xl.Quit() }\n",
    )
    .unwrap();
    let saved = powershell(&script)
        .env("XLDIF_T_IN", &book)
        .env("XLDIF_T_XLS", &xls)
        .env("XLDIF_T_XLSB", &xlsb)
        .status()
        .unwrap();
    assert!(saved.success() && xls.is_file() && xlsb.is_file(), "Excel wrote the two test workbooks");

    let env = ok(&["inspect", s(&xls)]);
    assert_eq!(warning_codes(&env)[0], "converted_by_excel");
    let names: Vec<&str> =
        env["result"]["sheets"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["BS", "Notes"]);

    // The same figures and formulas come back from both formats.
    let env = ok(&["compare", s(&xls), s(&xlsb)]);
    let summary = &env["result"]["summary"];
    assert_eq!((summary["value"].clone(), summary["formula"].clone()), (0.into(), 0.into()), "{summary}");
    let messages: Vec<&str> =
        env["warnings"].as_array().unwrap().iter().map(|w| w["message"].as_str().unwrap()).collect();
    assert!(messages.iter().any(|m| m.starts_with("base: read through a copy Excel saved")), "{messages:?}");
    assert!(messages.iter().any(|m| m.starts_with("other: read through a copy Excel saved")), "{messages:?}");
}

/// A hidden Excel holding a workbook open with one unsaved edit (B2 on the
/// first sheet) until dropped; it never saves.
struct Held {
    child: Child,
    stop: PathBuf,
}

impl Held {
    fn open(work: &Work, book: &Path) -> Self {
        let (script, ready, stop) = (work.0.join("hold.ps1"), work.0.join("ready"), work.0.join("stop"));
        std::fs::write(
            &script,
            "$xl = New-Object -ComObject Excel.Application\n\
             try { $xl.DisplayAlerts = $false; $wb = $xl.Workbooks.Open($env:XLDIF_T_IN)\n\
             $wb.Worksheets.Item(1).Range('B2').Value2 = 424242\n\
             Set-Content -Path $env:XLDIF_T_READY -Value 'ready'\n\
             $until = (Get-Date).AddSeconds(240)\n\
             while (-not (Test-Path $env:XLDIF_T_STOP) -and (Get-Date) -lt $until) { Start-Sleep -Milliseconds 100 }\n\
             $wb.Close($false) }\n\
             finally { $xl.Quit() }\n",
        )
        .unwrap();
        let child = powershell(&script)
            .env("XLDIF_T_IN", book)
            .env("XLDIF_T_READY", &ready)
            .env("XLDIF_T_STOP", &stop)
            .spawn()
            .unwrap();
        let held = Self { child, stop };
        let started = Instant::now();
        while !ready.is_file() {
            assert!(started.elapsed() < Duration::from_secs(120), "Excel opened the test workbook");
            std::thread::sleep(Duration::from_millis(100));
        }
        held
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.stop, "stop");
        let _ = self.child.wait();
    }
}

#[test]
fn a_workbook_open_in_excel_is_read_as_saved_unless_unsaved_is_asked() {
    if !excel_tests_enabled() {
        return;
    }
    let work = Work::new("open");
    let (base, open) = (work.basic("base.xlsx"), work.basic("open.xlsx"));
    let on_disk = std::fs::read(&open).unwrap();
    let held = Held::open(&work, &open);

    // As saved: the edit Excel holds is not on disk, and a warning says the
    // file is open.
    let env = ok(&["compare", s(&base), s(&open)]);
    assert_eq!(env["result"]["summary"]["value"], 0);
    let messages: Vec<&str> =
        env["warnings"].as_array().unwrap().iter().map(|w| w["message"].as_str().unwrap()).collect();
    assert!(warning_codes(&env).contains(&"open_in_another_program"), "{messages:?}");
    assert!(messages.iter().any(|m| m.starts_with("other: the file is open in another program")), "{messages:?}");
    assert!(!messages.iter().any(|m| m.starts_with("base: the file is open")), "{messages:?}");

    // As Excel holds it: the unsaved edit and the total it feeds.
    let env = ok(&["compare", s(&base), s(&open), "--unsaved", "--kind", "value"]);
    let codes = warning_codes(&env);
    assert!(codes.contains(&"unsaved_state") && !codes.contains(&"open_in_another_program"), "{codes:?}");
    let changed: Vec<(&str, f64)> = env["result"]["sheets"][0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["other"]["cell"].as_str().unwrap(), c["other"]["value"].as_f64().unwrap()))
        .collect();
    assert_eq!(changed, [("B2", 424_242.0), ("B4", 424_442.0)]);

    // A workbook nobody has open reads as usual, with or without the flag.
    let codes_of = |env: Value| warning_codes(&env).iter().map(|c| (*c).to_owned()).collect::<Vec<_>>();
    assert_eq!(codes_of(ok(&["inspect", s(&base), "--unsaved"])), codes_of(ok(&["inspect", s(&base)])));

    // Excel was never made to save or close the workbook.
    drop(held);
    assert_eq!(std::fs::read(&open).unwrap(), on_disk, "the open workbook is unchanged on disk");
}
