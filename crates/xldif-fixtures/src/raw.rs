//! Workbooks assembled from hand-written XML parts, for shapes that writer
//! libraries never produce: Excel's shared formulas and phonetic runs,
//! prefixed namespaces, omitted cell references, and malformed parts.

use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use zip::write::SimpleFileOptions;

const WORKBOOK_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const SHEET_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const SST_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

/// A workbook built part by part. Sheets are numbered in order from 1.
pub struct RawBook {
    /// Name, `state`, part XML, and relationship kind.
    sheets: Vec<(String, Option<&'static str>, String, &'static str)>,
    shared_strings: Option<String>,
    workbook_before: String,
    workbook_after: String,
    workbook_type: &'static str,
    workbook_rels: String,
    /// `<externalReference>` entries, in the order formulas number them.
    external_references: String,
    extra_parts: Vec<(String, Vec<u8>)>,
}

impl Default for RawBook {
    fn default() -> Self {
        Self::new()
    }
}

impl RawBook {
    pub fn new() -> Self {
        Self {
            sheets: Vec::new(),
            shared_strings: None,
            workbook_before: String::new(),
            workbook_after: String::new(),
            workbook_type: WORKBOOK_TYPE,
            workbook_rels: String::new(),
            external_references: String::new(),
            extra_parts: Vec::new(),
        }
    }

    /// Adds a sheet whose `<sheetData>` children are `rows_xml`.
    #[must_use]
    pub fn sheet(self, name: &str, rows_xml: &str) -> Self {
        self.sheet_with_state(name, None, rows_xml)
    }

    #[must_use]
    pub fn sheet_with_state(mut self, name: &str, state: Option<&'static str>, rows_xml: &str) -> Self {
        let xml = format!(r#"<worksheet xmlns="{MAIN_NS}"><sheetData>{rows_xml}</sheetData></worksheet>"#);
        self.sheets.push((name.to_owned(), state, xml, "worksheet"));
        self
    }

    /// Adds a relationship from the workbook part, such as `externalLink`;
    /// the target part itself is added with [`Self::part`] when it matters.
    #[must_use]
    pub fn workbook_rel(mut self, kind: &str, target: &str) -> Self {
        let n = self.workbook_rels.matches("<Relationship ").count() + 1;
        let _ = write!(self.workbook_rels, r#"<Relationship Id="rIdX{n}" Type="{REL}/{kind}" Target="{target}"/>"#);
        self
    }

    /// Adds a link to another workbook, as Excel writes one: formulas name
    /// it by position (`[1]` is the first link added), and the link part's
    /// `externalBook` relationship holds `target`.
    #[must_use]
    pub fn external_link(mut self, target: &str) -> Self {
        let n = self.external_references.matches("<externalReference ").count() + 1;
        let rel = self.workbook_rels.matches("<Relationship ").count() + 1;
        let _ = write!(self.external_references, r#"<externalReference r:id="rIdX{rel}"/>"#);
        let part = format!(
            r#"<externalLink xmlns="{MAIN_NS}"><externalBook xmlns:r="{REL}" r:id="rId1"><sheetNames><sheetName val="Sheet1"/></sheetNames></externalBook></externalLink>"#
        );
        let rels = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/externalLinkPath" Target="{target}" TargetMode="External"/></Relationships>"#
        );
        self.workbook_rel("externalLink", &format!("externalLinks/externalLink{n}.xml"))
            .part(&format!("xl/externalLinks/externalLink{n}.xml"), part.as_bytes())
            .part(&format!("xl/externalLinks/_rels/externalLink{n}.xml.rels"), rels.as_bytes())
    }

    /// Adds a sheet whose whole part is `xml`, as written.
    #[must_use]
    pub fn sheet_part(mut self, name: &str, xml: &str) -> Self {
        self.sheets.push((name.to_owned(), None, xml.to_owned(), "worksheet"));
        self
    }

    /// Adds a chart sheet: a sheet entry whose part holds a chart, no cells.
    #[must_use]
    pub fn chartsheet(mut self, name: &str) -> Self {
        let xml = format!(
            r#"<chartsheet xmlns="{MAIN_NS}"><sheetViews><sheetView workbookViewId="0"/></sheetViews></chartsheet>"#
        );
        self.sheets.push((name.to_owned(), None, xml, "chartsheet"));
        self
    }

    /// The shared string table's `<si>` children.
    #[must_use]
    pub fn shared_strings(mut self, items_xml: &str) -> Self {
        self.shared_strings = Some(format!(r#"<sst xmlns="{MAIN_NS}">{items_xml}</sst>"#));
        self
    }

    /// Elements placed in `<workbook>` before `<sheets>`, such as `workbookPr`.
    #[must_use]
    pub fn workbook_before(mut self, xml: &str) -> Self {
        self.workbook_before.push_str(xml);
        self
    }

    /// Elements placed in `<workbook>` after `<sheets>`, such as
    /// `definedNames` and `calcPr`.
    #[must_use]
    pub fn workbook_after(mut self, xml: &str) -> Self {
        self.workbook_after.push_str(xml);
        self
    }

    #[must_use]
    pub fn macro_enabled(mut self) -> Self {
        self.workbook_type = "application/vnd.ms-excel.sheet.macroEnabled.main+xml";
        self
    }

    #[must_use]
    pub fn part(mut self, name: &str, bytes: &[u8]) -> Self {
        self.extra_parts.push((name.to_owned(), bytes.to_vec()));
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut types = format!(
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="{}"/>"#,
            self.workbook_type
        );
        let mut rels =
            String::from(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
        let mut sheets = String::new();
        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        for (i, (name, state, xml, kind)) in self.sheets.iter().enumerate() {
            let n = i + 1;
            let state = state.map(|s| format!(r#" state="{s}""#)).unwrap_or_default();
            let _ = write!(sheets, r#"<sheet name="{name}" sheetId="{n}"{state} r:id="rId{n}"/>"#);
            let _ = write!(rels, r#"<Relationship Id="rId{n}" Type="{REL}/{kind}" Target="worksheets/sheet{n}.xml"/>"#);
            let _ = write!(types, r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="{SHEET_TYPE}"/>"#);
            parts.push((format!("xl/worksheets/sheet{n}.xml"), xml.as_bytes().to_vec()));
        }
        if let Some(sst) = &self.shared_strings {
            let _ = write!(rels, r#"<Relationship Id="rIdS" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/>"#);
            let _ = write!(types, r#"<Override PartName="/xl/sharedStrings.xml" ContentType="{SST_TYPE}"/>"#);
            parts.push(("xl/sharedStrings.xml".to_owned(), sst.as_bytes().to_vec()));
        }
        rels.push_str(&self.workbook_rels);
        rels.push_str("</Relationships>");
        types.push_str("</Types>");
        let (before, after) = (&self.workbook_before, &self.workbook_after);
        let external = match self.external_references.as_str() {
            "" => String::new(),
            references => format!("<externalReferences>{references}</externalReferences>"),
        };
        let workbook = format!(
            r#"<workbook xmlns="{MAIN_NS}" xmlns:r="{REL}">{before}<sheets>{sheets}</sheets>{external}{after}</workbook>"#
        );
        let root_rels = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
        );
        let mut all = vec![
            ("[Content_Types].xml".to_owned(), types.into_bytes()),
            ("_rels/.rels".to_owned(), root_rels.into_bytes()),
            ("xl/workbook.xml".to_owned(), workbook.into_bytes()),
            ("xl/_rels/workbook.xml.rels".to_owned(), rels.into_bytes()),
        ];
        all.extend(parts);
        all.extend(self.extra_parts.iter().cloned());
        zip_parts(&all)
    }
}

/// Zips named parts with deflate, in order.
pub fn zip_parts(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in parts {
        out.start_file(name.as_str(), options).expect("start member");
        out.write_all(bytes).expect("write member");
    }
    out.finish().expect("finish archive").into_inner()
}
