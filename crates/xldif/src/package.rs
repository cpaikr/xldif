//! The OPC package: a bounded ZIP archive, member lookup, and relationships.
//!
//! Every read is capped so a hostile or corrupt file fails with
//! `invalid_archive` instead of exhausting memory: archive size, member
//! count, each member's decompressed size (measured while reading, since
//! declared sizes can lie), and the total decompressed across one workbook.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use zip::ZipArchive;

use crate::xml::{Event, Parser};
use crate::{Error, ErrorCode};

/// Bounds on what one workbook may make the reader do. The defaults sit far
/// above real workbooks; tests lower them to exercise each bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_file_bytes: u64,
    pub max_members: usize,
    /// One member's decompressed size, measured while reading.
    pub max_member_bytes: u64,
    /// All members read from one workbook, decompressed.
    pub max_total_bytes: u64,
    /// Cells, rows, shared strings, sheets, and defined names together
    /// (about 100 bytes of memory each).
    pub max_records: usize,
    /// Formula text written out for the cells of shared formulas, which the
    /// file stores once: a long anchor shared by many cells would otherwise
    /// cost its length once per cell.
    pub max_formula_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1 << 30,
            max_members: 20_000,
            max_member_bytes: 1 << 30,
            max_total_bytes: 4 << 30,
            max_records: 20_000_000,
            max_formula_bytes: 1 << 29,
        }
    }
}

const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
/// OLE compound files: legacy `.xls`, and password-protected OOXML workbooks.
const OLE_MAGIC: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidArchive, message)
}

/// One relationship from a `.rels` part, its target resolved to a member name.
#[derive(Debug, Clone)]
pub struct Relationship {
    pub id: String,
    /// The relationship type's last path segment, such as `worksheet`. The
    /// transitional and strict namespaces differ only before it.
    pub kind: String,
    pub target: String,
}

pub struct Package<R> {
    archive: ZipArchive<R>,
    /// Lower-cased member name → actual name; some generators vary case.
    names: HashMap<String, String>,
    read_total: u64,
    limits: Limits,
}

/// Classifies the first bytes of a file before treating it as a ZIP.
fn check_magic(head: &[u8]) -> Result<(), Error> {
    if head.starts_with(ZIP_MAGIC) {
        Ok(())
    } else if head.starts_with(OLE_MAGIC) {
        Err(Error::new(
            ErrorCode::UnsupportedFormat,
            "this is a legacy .xls workbook or a password-protected workbook, which xldif cannot read",
        )
        .with_hint("save it from Excel as .xlsx without a password to open, then compare that copy"))
    } else {
        Err(invalid("the file is not a ZIP-based Excel workbook"))
    }
}

impl Package<File> {
    pub fn open(path: &Path, limits: &Limits) -> Result<Self, Error> {
        let meta = std::fs::metadata(path).map_err(|_| {
            Error::new(ErrorCode::FileNotFound, format!("{} does not exist or is not readable", path.display()))
        })?;
        if !meta.is_file() {
            return Err(Error::new(ErrorCode::FileNotFound, format!("{} is not a file", path.display())));
        }
        if meta.len() > limits.max_file_bytes {
            return Err(invalid(format!("the file is larger than {} bytes", limits.max_file_bytes)));
        }
        let io = |e: std::io::Error| Error::new(ErrorCode::IoError, format!("reading {}: {e}", path.display()));
        let mut file = File::open(path).map_err(io)?;
        let mut head = [0u8; 8];
        let n = file.read(&mut head).map_err(io)?;
        check_magic(&head[..n])?;
        file.rewind().map_err(io)?;
        Self::new(file, limits)
    }
}

impl Package<std::io::Cursor<Vec<u8>>> {
    /// A workbook held in memory, checked to start like a ZIP.
    pub fn from_bytes(bytes: Vec<u8>, limits: &Limits) -> Result<Self, Error> {
        if bytes.len() as u64 > limits.max_file_bytes {
            return Err(invalid(format!("the file is larger than {} bytes", limits.max_file_bytes)));
        }
        check_magic(&bytes[..bytes.len().min(8)])?;
        Self::new(std::io::Cursor::new(bytes), limits)
    }
}

impl<R: Read + Seek> Package<R> {
    pub fn new(reader: R, limits: &Limits) -> Result<Self, Error> {
        let archive = ZipArchive::new(reader).map_err(|e| invalid(format!("the ZIP container is unreadable: {e}")))?;
        if archive.len() > limits.max_members {
            return Err(invalid(format!("the archive has more than {} members", limits.max_members)));
        }
        let mut names = HashMap::new();
        for name in archive.file_names() {
            let name = name.map_err(|e| invalid(format!("a member name is unreadable: {e}")))?;
            names.insert(name.to_ascii_lowercase(), name.into_owned());
        }
        Ok(Self { archive, names, read_total: 0, limits: *limits })
    }

    /// The member as UTF-8 text (a leading BOM dropped), or `None` when absent.
    pub fn read_text(&mut self, name: &str) -> Result<Option<String>, Error> {
        let Some(actual) = self.names.get(&name.to_ascii_lowercase()).cloned() else {
            return Ok(None);
        };
        let Limits { max_member_bytes, max_total_bytes, .. } = self.limits;
        let room = max_member_bytes.min(max_total_bytes.saturating_sub(self.read_total));
        let mut member = self.archive.by_name(&actual).map_err(|e| invalid(format!("{actual}: {e}")))?;
        let mut bytes = Vec::new();
        (&mut member)
            .take(room + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| invalid(format!("{actual} cannot be decompressed: {e}")))?;
        drop(member);
        let len = bytes.len() as u64;
        if len > max_member_bytes {
            return Err(invalid(format!("{actual} decompresses to more than {max_member_bytes} bytes")));
        }
        if len > room {
            return Err(invalid(format!("the workbook decompresses to more than {max_total_bytes} bytes")));
        }
        self.read_total += len;
        let mut text = String::from_utf8(bytes)
            .map_err(|_| Error::new(ErrorCode::InvalidXml, format!("{actual} is not UTF-8 text")))?;
        if text.starts_with('\u{feff}') {
            text.drain(..'\u{feff}'.len_utf8());
        }
        Ok(Some(text))
    }

    /// The relationships of `part` (from `<dir>/_rels/<file>.rels`), with
    /// internal targets resolved against the part's directory. External
    /// targets (hyperlinks, linked workbooks) are left out.
    pub fn relationships(&mut self, part: &str) -> Result<Vec<Relationship>, Error> {
        let (dir, file) = part.rsplit_once('/').map_or(("", part), |(d, f)| (d, f));
        let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        let Some(text) = self.read_text(&rels_name)? else {
            return Ok(Vec::new());
        };
        let mut parser = Parser::new(&text, &rels_name);
        let mut out = Vec::new();
        loop {
            match parser.next()? {
                Event::Start(e) if e.name == "Relationship" => {
                    if e.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External")) {
                        continue;
                    }
                    let (Some(id), Some(kind), Some(target)) = (e.attr("Id"), e.attr("Type"), e.attr("Target")) else {
                        continue;
                    };
                    out.push(Relationship {
                        id: id.to_owned(),
                        kind: kind.rsplit('/').next().unwrap_or(kind).to_owned(),
                        target: resolve(dir, target),
                    });
                }
                Event::Eof => return Ok(out),
                _ => {}
            }
        }
    }

    /// The content type declared for `part` in `[Content_Types].xml`.
    pub fn content_type(&mut self, part: &str) -> Result<Option<String>, Error> {
        let Some(text) = self.read_text("[Content_Types].xml")? else {
            return Ok(None);
        };
        let wanted = format!("/{part}");
        let ext = part.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        let mut parser = Parser::new(&text, "[Content_Types].xml");
        let mut by_default = None;
        loop {
            match parser.next()? {
                Event::Start(e) if e.name == "Override" => {
                    if e.attr("PartName").is_some_and(|p| p.eq_ignore_ascii_case(&wanted)) {
                        return Ok(e.attr("ContentType").map(str::to_owned));
                    }
                }
                Event::Start(e) if e.name == "Default" => {
                    if e.attr("Extension").map(str::to_ascii_lowercase) == ext {
                        by_default = e.attr("ContentType").map(str::to_owned);
                    }
                }
                Event::Eof => return Ok(by_default),
                _ => {}
            }
        }
    }
}

/// Resolves a relationship target against the source part's directory.
fn resolve(dir: &str, target: &str) -> String {
    let joined = if let Some(abs) = target.strip_prefix('/') {
        abs.to_owned()
    } else if dir.is_empty() {
        target.to_owned()
    } else {
        format!("{dir}/{target}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::{Limits, Package, resolve};
    use crate::ErrorCode;

    #[test]
    fn targets_resolve_relative_and_absolute() {
        assert_eq!(resolve("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("xl/worksheets", "../comments1.xml"), "xl/comments1.xml");
        assert_eq!(resolve("xl", "/xl/styles.xml"), "xl/styles.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
    }

    #[test]
    fn non_zip_input_is_classified() {
        let ole = [0xD0u8, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0];
        let err = Package::from_bytes(ole.to_vec(), &Limits::default()).err().unwrap();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
        let err = Package::from_bytes(b"hello".to_vec(), &Limits::default()).err().unwrap();
        assert_eq!(err.code, ErrorCode::InvalidArchive);
    }
}
