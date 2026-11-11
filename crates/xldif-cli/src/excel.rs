//! Reading a workbook for a command: straight from disk, or through Excel
//! when that is the only way to get at it.
//!
//! The reader takes `.xlsx` and `.xlsm` as saved. Two cases need Excel, on
//! Windows with Excel installed:
//!
//! - **A format the reader rejects** (`.xls`, `.xlsb`, a password to open).
//!   A new, hidden Excel opens the file read-only and saves a copy as
//!   `.xlsx`, which is read instead.
//! - **A workbook open in Excel**, when `--unsaved` asks for what Excel
//!   holds now rather than the last saved state. The running Excel saves a
//!   copy (`SaveCopyAs`), which leaves the open workbook unsaved and
//!   untouched.
//!
//! Excel is a converter here, never the reader: the copy goes through the
//! same reader as any other file. Copies live in a scratch directory that is
//! removed before the command returns. A warning on the workbook says which
//! route it took.

use std::path::{Path, PathBuf};

use xldif::model::{Warning, Workbook};
use xldif::{Error, ErrorCode};

use crate::envelope::Failure;

/// Why Excel did not produce a copy.
#[derive(Debug)]
struct Refusal {
    /// No running Excel has the workbook open.
    not_open: bool,
    message: String,
}

/// A directory for the copies Excel writes, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, Failure> {
        let mut attempt = 0u32;
        loop {
            let dir = std::env::temp_dir().join(format!("xldif-{}-{attempt}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => attempt += 1,
                Err(e) => {
                    return Err(Error::new(ErrorCode::IoError, format!("creating {}: {e}", dir.display())).into());
                }
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether another program holds the file open and refuses writers, as
/// Excel does with a workbook it has open for editing.
#[cfg(windows)]
fn in_use(path: &Path) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    // Opening for writing writes nothing.
    matches!(
        std::fs::OpenOptions::new().write(true).open(path),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION)
    )
}

#[cfg(not(windows))]
fn in_use(_: &Path) -> bool {
    false
}

/// Whether the reader rejected the file for a reason Excel can remove: a
/// compound file (`.xls`, or an encrypted workbook) or a binary workbook.
fn convertible(path: &Path) -> bool {
    use std::io::Read as _;
    const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    let mut head = [0u8; 8];
    let compound = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == OLE_MAGIC;
    compound || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xlsb"))
}

#[cfg(windows)]
fn excel(mode: &str, input: &Path, output: &Path, scratch: &Scratch) -> Result<(), Refusal> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Starting Excel and converting a workbook takes ten to twenty seconds.
    const PATIENCE: Duration = Duration::from_secs(180);
    let failed = |message: String| Refusal { not_open: false, message };

    let script = scratch.0.join("excel.ps1");
    std::fs::write(&script, include_str!("excel.ps1")).map_err(|e| failed(format!("writing the Excel script: {e}")))?;
    // Windows PowerShell by its full path: it is part of Windows, and a
    // `powershell` found on PATH need not be.
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let mut child = Command::new(root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .env("XLDIF_EXCEL_MODE", mode)
        .env("XLDIF_EXCEL_IN", input)
        .env("XLDIF_EXCEL_OUT", output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| failed(format!("starting PowerShell: {e}")))?;
    // The script prints one short line, so the pipe never fills while this waits.
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PATIENCE => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(format!(
                    "Excel did not answer within {} seconds; a hidden Excel may still be running",
                    PATIENCE.as_secs()
                )));
            }
            Err(e) => return Err(failed(format!("waiting for PowerShell: {e}"))),
        }
    }
    let out = child.wait_with_output().map_err(|e| failed(format!("reading PowerShell's answer: {e}")))?;
    let answer = String::from_utf8_lossy(&out.stdout);
    let answer = answer.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default();
    match answer.split_once('\t') {
        _ if answer == "ok" && output.is_file() => Ok(()),
        Some((reason, message)) => Err(Refusal { not_open: reason == "not_open", message: message.to_owned() }),
        None => Err(failed("PowerShell could not run the Excel script".to_owned())),
    }
}

#[cfg(not(windows))]
fn excel(_: &str, _: &Path, _: &Path, _: &Scratch) -> Result<(), Refusal> {
    Err(Refusal { not_open: false, message: "this needs Windows with Excel installed".to_owned() })
}

fn note(code: &'static str, message: &str) -> Warning {
    Warning { code, message: message.to_owned() }
}

/// Reads the workbook at `path`. With `unsaved`, a workbook that is open in
/// Excel is read as Excel holds it now.
pub fn read(path: &Path, unsaved: bool) -> Result<Workbook, Failure> {
    let mut notes = Vec::new();
    let mut scratch = None;
    let mut source = path.to_path_buf();
    // Another program holding the file open and refusing writers is the
    // sign of a workbook open in Excel; it is not proof either way (a file
    // that is read-only on disk never shows it), so `--unsaved` asks Excel
    // whatever it says.
    let held = in_use(path);
    if unsaved && cfg!(windows) {
        let dir = Scratch::new()?;
        // The copy keeps the workbook's format, so its extension too.
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("xlsx");
        let copy = dir.0.join(format!("open.{extension}"));
        let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        match excel("snapshot", &full, &copy, &dir) {
            Ok(()) => {
                source = copy;
                scratch = Some(dir);
                notes.push(note("unsaved_state", "read as Excel has it open now, unsaved edits included"));
            }
            // Nobody has it open: the file on disk is all there is.
            Err(refusal) if refusal.not_open && !held => {}
            Err(refusal) if refusal.not_open => {
                return Err(Failure::excel(
                    format!(
                        "{} is open in another program, but no running Excel has it open under this path",
                        path.display()
                    ),
                    "a workbook opened from OneDrive or SharePoint is known to Excel by its web address: save a                      local copy and compare that, or drop --unsaved to read the last saved state",
                ));
            }
            Err(refusal) => {
                return Err(Failure::excel(
                    format!("Excel could not hand over {} as it is open now: {}", path.display(), refusal.message),
                    "drop --unsaved to read the last saved state",
                ));
            }
        }
    } else if held {
        notes.push(note(
            "open_in_another_program",
            "the file is open in another program, probably Excel; this is its last saved state (pass --unsaved to              read what Excel has now)",
        ));
    }
    let mut book = match xldif::read_path(&source) {
        Ok(book) => book,
        Err(e) if cfg!(windows) && e.code == ErrorCode::UnsupportedFormat && convertible(&source) => {
            let dir = match scratch.take() {
                Some(dir) => dir,
                None => Scratch::new()?,
            };
            let converted = dir.0.join("converted.xlsx");
            let full = std::path::absolute(&source).unwrap_or_else(|_| source.clone());
            if let Err(refusal) = excel("convert", &full, &converted, &dir) {
                return Err(Error { hint: None, ..e }
                    .with_hint(format!(
                        "Excel could not convert it either: {} (for a password to open, set XLDIF_PASSWORD)",
                        refusal.message
                    ))
                    .into());
            }
            notes.push(note(
                "converted_by_excel",
                "read through a copy Excel saved as .xlsx: values are as Excel calculated them on opening the \
                 file, and fonts and colours as it rewrote them",
            ));
            xldif::read_path(&converted)?
        }
        Err(e) => return Err(e.into()),
    };
    notes.append(&mut book.warnings);
    book.warnings = notes;
    Ok(book)
}
