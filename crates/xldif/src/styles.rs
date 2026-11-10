//! Cell formatting from `styles.xml`, resolved into comparable properties.
//!
//! Each `cellXfs` entry becomes one [`CellFormat`] holding what a person
//! sees: number format code, font, fill, borders, alignment, protection,
//! and the named cell style. Colors stay symbolic ([`Color`]): a theme
//! index with tint, an indexed palette entry, an RGB value, or automatic.
//! Comparing symbols means a theme change is one change, not a different
//! RGB in every themed cell.
//!
//! Precedence (ECMA-376 Part 1, 18.3.1.4 and 18.8.45): a written cell uses
//! its own `s` index, 0 when absent. Excel writes the full set of a cell
//! format's properties on the `cellXfs` entry itself, so the parent named
//! style (`cellStyleXfs`, by `xfId`) contributes only its name; the
//! `applyX` flags are not consulted (unverified against files that rely on
//! inheriting from the named style).

use std::fmt;

use crate::Error;
use crate::xml::{Element, Event, Parser};

/// A color as the file names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// No color given (the application default).
    #[default]
    None,
    Auto,
    /// `AARRGGBB`, upper-cased.
    Rgb(String),
    /// A theme color index with tint, the tint kept as written.
    Theme {
        index: u32,
        tint: String,
    },
    /// An entry of the indexed palette (possibly overridden by `indexedColors`).
    Indexed(u32),
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Auto => f.write_str("auto"),
            Self::Rgb(rgb) => write!(f, "rgb:{rgb}"),
            Self::Theme { index, tint } if tint.is_empty() => write!(f, "theme:{index}"),
            Self::Theme { index, tint } => write!(f, "theme:{index} tint {tint}"),
            Self::Indexed(i) => write!(f, "indexed:{i}"),
        }
    }
}

fn color(e: &Element) -> Color {
    if e.attr("auto").is_some_and(|v| v == "1" || v == "true") {
        Color::Auto
    } else if let Some(rgb) = e.attr("rgb") {
        Color::Rgb(rgb.to_ascii_uppercase())
    } else if let Some(theme) = e.attr("theme").and_then(|t| t.parse().ok()) {
        let tint = e.attr("tint").filter(|t| t.parse::<f64>().is_ok_and(|v| v != 0.0)).unwrap_or_default();
        Color::Theme { index: theme, tint: tint.to_owned() }
    } else if let Some(i) = e.attr("indexed").and_then(|t| t.parse().ok()) {
        Color::Indexed(i)
    } else {
        Color::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Font {
    pub name: String,
    /// Points, as written.
    pub size: String,
    pub bold: bool,
    pub italic: bool,
    /// `single`, `double`, `singleAccounting`, …; empty for none.
    pub underline: String,
    pub strike: bool,
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Fill {
    /// `solid`, `gray125`, …; empty for none.
    pub pattern: String,
    pub fg: Color,
    pub bg: Color,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Edge {
    /// `thin`, `medium`, `double`, …; empty for none.
    pub style: String,
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Border {
    pub left: Edge,
    pub right: Edge,
    pub top: Edge,
    pub bottom: Edge,
    pub diagonal: Edge,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Alignment {
    pub horizontal: String,
    pub vertical: String,
    pub wrap: bool,
    pub indent: u32,
    pub rotation: u32,
    pub shrink: bool,
}

/// Everything a person sees of one cell's format, plus its named style.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CellFormat {
    /// The number format code, such as `#,##0` (built-in ids resolved).
    pub number_format: String,
    pub font: Font,
    pub fill: Fill,
    pub border: Border,
    pub alignment: Alignment,
    pub locked: bool,
    pub hidden: bool,
    /// The named cell style, such as `Normal` or `Comma [0]`.
    pub style: String,
}

impl CellFormat {
    /// `(property, value)` for every property, in a fixed order; two formats
    /// differ exactly where these differ.
    pub fn properties(&self) -> Vec<(&'static str, String)> {
        let edge = |e: &Edge| if e.style.is_empty() { "none".to_owned() } else { format!("{} {}", e.style, e.color) };
        let flag = |b: bool| if b { "yes" } else { "no" }.to_owned();
        let text = |s: &str| if s.is_empty() { "none".to_owned() } else { s.to_owned() };
        let f = &self.font;
        vec![
            ("numberFormat", self.number_format.clone()),
            ("font.name", f.name.clone()),
            ("font.size", f.size.clone()),
            ("font.bold", flag(f.bold)),
            ("font.italic", flag(f.italic)),
            ("font.underline", text(&f.underline)),
            ("font.strike", flag(f.strike)),
            ("font.color", f.color.to_string()),
            ("fill.pattern", text(&self.fill.pattern)),
            ("fill.fg", self.fill.fg.to_string()),
            ("fill.bg", self.fill.bg.to_string()),
            ("border.left", edge(&self.border.left)),
            ("border.right", edge(&self.border.right)),
            ("border.top", edge(&self.border.top)),
            ("border.bottom", edge(&self.border.bottom)),
            ("border.diagonal", edge(&self.border.diagonal)),
            ("alignment.horizontal", text(&self.alignment.horizontal)),
            ("alignment.vertical", text(&self.alignment.vertical)),
            ("alignment.wrap", flag(self.alignment.wrap)),
            ("alignment.indent", self.alignment.indent.to_string()),
            ("alignment.rotation", self.alignment.rotation.to_string()),
            ("alignment.shrink", flag(self.alignment.shrink)),
            ("protection.locked", flag(self.locked)),
            ("protection.hidden", flag(self.hidden)),
            ("style", self.style.clone()),
        ]
    }

    /// `(property, base, other)` where `self` (base) and `other` differ.
    pub fn differences(&self, other: &Self) -> Vec<(&'static str, String, String)> {
        self.properties()
            .into_iter()
            .zip(other.properties())
            .filter(|(a, b)| a.1 != b.1)
            .map(|((name, a), (_, b))| (name, a, b))
            .collect()
    }
}

/// The resolved cell formats of a workbook, by `cellXfs` index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Styles {
    pub formats: Vec<CellFormat>,
}

impl Styles {
    /// The format of style index `s`; index 0 (or an index past the table)
    /// falls back to the first format, then to defaults.
    pub fn format(&self, s: u32) -> CellFormat {
        self.formats.get(s as usize).or_else(|| self.formats.first()).cloned().unwrap_or_default()
    }
}

/// Built-in number formats (ECMA-376 Part 1, 18.8.30) that are the same in
/// every locale. Ids 5–8, 23–36, and 41–44 vary by locale and are named
/// `builtin:<id>` rather than guessed.
fn builtin_number_format(id: u32) -> String {
    let code = match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return format!("builtin:{id}"),
    };
    code.to_owned()
}

/// One `<xf>` as written: indexes into the other tables.
#[derive(Default)]
struct Xf {
    num_fmt: u32,
    font: usize,
    fill: usize,
    border: usize,
    parent: Option<usize>,
    alignment: Alignment,
    locked: bool,
    hidden: bool,
}

fn truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1" | "true"))
}

fn index(e: &Element, name: &str) -> usize {
    e.attr(name).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Reads `<font>`'s children up to its end.
fn read_font(p: &mut Parser<'_>) -> Result<Font, Error> {
    let mut font = Font::default();
    let mut depth = 1;
    while depth > 0 {
        match p.next()? {
            Event::Start(e) => {
                depth += 1;
                let val = e.attr("val");
                match e.name.as_str() {
                    "name" | "rFont" => val.unwrap_or_default().clone_into(&mut font.name),
                    "sz" => val.unwrap_or_default().clone_into(&mut font.size),
                    "b" => font.bold = val.is_none_or(|v| v != "0" && v != "false"),
                    "i" => font.italic = val.is_none_or(|v| v != "0" && v != "false"),
                    "strike" => font.strike = val.is_none_or(|v| v != "0" && v != "false"),
                    "u" => val.unwrap_or("single").clone_into(&mut font.underline),
                    "color" => font.color = color(&e),
                    _ => {}
                }
            }
            Event::End(_) => depth -= 1,
            Event::Text(_) => {}
            Event::Eof => break,
        }
    }
    if font.underline == "none" {
        font.underline.clear();
    }
    Ok(font)
}

fn read_fill(p: &mut Parser<'_>) -> Result<Fill, Error> {
    let mut fill = Fill::default();
    let mut depth = 1;
    while depth > 0 {
        match p.next()? {
            Event::Start(e) => {
                depth += 1;
                match e.name.as_str() {
                    "patternFill" => e.attr("patternType").unwrap_or_default().clone_into(&mut fill.pattern),
                    "fgColor" => fill.fg = color(&e),
                    "bgColor" => fill.bg = color(&e),
                    // Gradient fills are rare; they compare by their type only.
                    "gradientFill" => fill.pattern = format!("gradient:{}", e.attr("type").unwrap_or("linear")),
                    _ => {}
                }
            }
            Event::End(_) => depth -= 1,
            Event::Text(_) => {}
            Event::Eof => break,
        }
    }
    if fill.pattern == "none" {
        fill.pattern.clear();
    }
    Ok(fill)
}

fn read_border(p: &mut Parser<'_>) -> Result<Border, Error> {
    let mut border = Border::default();
    let mut depth = 1;
    let mut edge: Option<String> = None;
    while depth > 0 {
        match p.next()? {
            Event::Start(e) => {
                depth += 1;
                match e.name.as_str() {
                    "left" | "start" | "right" | "end" | "top" | "bottom" | "diagonal" if depth == 2 => {
                        let style = e.attr("style").unwrap_or_default().to_owned();
                        let slot = edge_slot(&mut border, &e.name);
                        slot.style = if style == "none" { String::new() } else { style };
                        edge = Some(e.name.clone());
                    }
                    "color" => {
                        if let Some(name) = &edge {
                            edge_slot(&mut border, name).color = color(&e);
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 1 {
                    edge = None;
                }
            }
            Event::Text(_) => {}
            Event::Eof => break,
        }
    }
    // A color on an edge without a style draws nothing.
    for e in [&mut border.left, &mut border.right, &mut border.top, &mut border.bottom, &mut border.diagonal] {
        if e.style.is_empty() {
            e.color = Color::None;
        }
    }
    Ok(border)
}

fn edge_slot<'a>(b: &'a mut Border, name: &str) -> &'a mut Edge {
    match name {
        "left" | "start" => &mut b.left,
        "right" | "end" => &mut b.right,
        "top" => &mut b.top,
        "bottom" => &mut b.bottom,
        _ => &mut b.diagonal,
    }
}

fn read_xf(e: &Element, p: &mut Parser<'_>, nested: bool) -> Result<Xf, Error> {
    let mut xf = Xf {
        num_fmt: e.attr("numFmtId").and_then(|v| v.parse().ok()).unwrap_or(0),
        font: index(e, "fontId"),
        fill: index(e, "fillId"),
        border: index(e, "borderId"),
        parent: e.attr("xfId").and_then(|v| v.parse().ok()),
        // Cells are locked unless the format says otherwise.
        locked: true,
        ..Xf::default()
    };
    if !nested {
        return Ok(xf);
    }
    let mut depth = 1;
    while depth > 0 {
        match p.next()? {
            Event::Start(c) => {
                depth += 1;
                match c.name.as_str() {
                    "alignment" => {
                        xf.alignment = Alignment {
                            horizontal: c.attr("horizontal").unwrap_or_default().to_owned(),
                            vertical: c.attr("vertical").unwrap_or_default().to_owned(),
                            wrap: truthy(c.attr("wrapText")),
                            indent: c.attr("indent").and_then(|v| v.parse().ok()).unwrap_or(0),
                            rotation: c.attr("textRotation").and_then(|v| v.parse().ok()).unwrap_or(0),
                            shrink: truthy(c.attr("shrinkToFit")),
                        };
                    }
                    "protection" => {
                        xf.locked = c.attr("locked").is_none_or(|v| v != "0" && v != "false");
                        xf.hidden = truthy(c.attr("hidden"));
                    }
                    _ => {}
                }
            }
            Event::End(_) => depth -= 1,
            Event::Text(_) => {}
            Event::Eof => break,
        }
    }
    Ok(xf)
}

/// Parses `styles.xml` into resolved cell formats.
pub fn parse_styles(text: &str, part: &str) -> Result<Styles, Error> {
    let mut p = Parser::new(text, part);
    let mut num_fmts: Vec<(u32, String)> = Vec::new();
    let mut fonts = Vec::new();
    let mut fills = Vec::new();
    let mut borders = Vec::new();
    let mut style_xfs: Vec<Xf> = Vec::new();
    let mut cell_xfs: Vec<Xf> = Vec::new();
    let mut style_names: Vec<(usize, String)> = Vec::new();
    // Which list an `<xf>` belongs to.
    let mut section = "";
    loop {
        match p.next()? {
            Event::Start(e) => match e.name.as_str() {
                "numFmt" => {
                    if let Some(id) = e.attr("numFmtId").and_then(|v| v.parse().ok()) {
                        num_fmts.push((id, e.attr("formatCode").unwrap_or_default().to_owned()));
                    }
                }
                "font" if section == "fonts" => fonts.push(read_font(&mut p)?),
                "fill" if section == "fills" => fills.push(read_fill(&mut p)?),
                "border" if section == "borders" => borders.push(read_border(&mut p)?),
                "xf" => {
                    // Self-closing <xf/> ends at once; read_xf consumes children otherwise.
                    let xf = read_xf(&e, &mut p, true)?;
                    match section {
                        "cellStyleXfs" => style_xfs.push(xf),
                        "cellXfs" => cell_xfs.push(xf),
                        _ => {}
                    }
                }
                "cellStyle" => {
                    if let Some(xf) = e.attr("xfId").and_then(|v| v.parse().ok()) {
                        style_names.push((xf, e.attr("name").unwrap_or_default().to_owned()));
                    }
                }
                "fonts" | "fills" | "borders" | "cellStyleXfs" | "cellXfs" => {
                    section = match e.name.as_str() {
                        "fonts" => "fonts",
                        "fills" => "fills",
                        "borders" => "borders",
                        "cellStyleXfs" => "cellStyleXfs",
                        _ => "cellXfs",
                    };
                }
                // Differential formats belong to conditional formatting (M3 objects).
                "dxfs" | "extLst" | "tableStyles" | "colors" => p.skip_element()?,
                _ => {}
            },
            Event::End(name) if matches!(name.as_str(), "fonts" | "fills" | "borders" | "cellStyleXfs" | "cellXfs") => {
                section = "";
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let number_format = |id: u32| {
        num_fmts.iter().find(|(n, _)| *n == id).map_or_else(|| builtin_number_format(id), |(_, code)| code.clone())
    };
    let formats = cell_xfs
        .iter()
        .map(|xf| CellFormat {
            number_format: number_format(xf.num_fmt),
            font: fonts.get(xf.font).cloned().unwrap_or_default(),
            fill: fills.get(xf.fill).cloned().unwrap_or_default(),
            border: borders.get(xf.border).cloned().unwrap_or_default(),
            alignment: xf.alignment.clone(),
            locked: xf.locked,
            hidden: xf.hidden,
            style: xf
                .parent
                .and_then(|parent| style_names.iter().find(|(x, _)| *x == parent))
                .map(|(_, n)| n.clone())
                .unwrap_or_default(),
        })
        .collect();
    Ok(Styles { formats })
}

#[cfg(test)]
mod tests {
    use super::{Color, parse_styles};

    const STYLES: &str = r##"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
      <numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0;[Red]\(#,##0\)"/></numFmts>
      <fonts count="2">
        <font><sz val="11"/><color theme="1"/><name val="Calibri"/></font>
        <font><b/><i val="0"/><u/><sz val="10"/><color rgb="ffff0000"/><name val="Arial"/></font>
      </fonts>
      <fills count="3">
        <fill><patternFill patternType="none"/></fill>
        <fill><patternFill patternType="gray125"/></fill>
        <fill><patternFill patternType="solid"><fgColor theme="4" tint="0.39997558519241921"/><bgColor indexed="64"/></patternFill></fill>
      </fills>
      <borders count="2">
        <border><left/><right/><top/><bottom/><diagonal/></border>
        <border><left style="thin"><color indexed="64"/></left><right/><top/><bottom style="double"><color auto="1"/></bottom><diagonal/></border>
      </borders>
      <cellStyleXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="3" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
      <cellXfs count="3">
        <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
        <xf numFmtId="164" fontId="1" fillId="2" borderId="1" xfId="1" applyNumberFormat="1"><alignment horizontal="center" wrapText="1"/><protection locked="0"/></xf>
        <xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0"/>
      </cellXfs>
      <cellStyles count="2"><cellStyle name="Normal" xfId="0" builtinId="0"/><cellStyle name="Comma [0]" xfId="1" builtinId="6"/></cellStyles>
      <dxfs count="1"><dxf><font><b/></font></dxf></dxfs>
    </styleSheet>"##;

    #[test]
    fn cell_formats_resolve_every_property() {
        let styles = parse_styles(STYLES, "styles.xml").unwrap();
        assert_eq!(styles.formats.len(), 3);
        let plain = styles.format(0);
        assert_eq!((plain.number_format.as_str(), plain.font.name.as_str()), ("General", "Calibri"));
        assert_eq!(plain.font.color, Color::Theme { index: 1, tint: String::new() });
        assert!(plain.locked && plain.style == "Normal");
        let fancy = styles.format(1);
        assert_eq!(fancy.number_format, r"#,##0;[Red]\(#,##0\)");
        assert!(fancy.font.bold && !fancy.font.italic);
        assert_eq!(fancy.font.underline, "single");
        assert_eq!(fancy.font.color, Color::Rgb("FFFF0000".into()));
        assert_eq!(fancy.fill.pattern, "solid");
        assert_eq!(fancy.fill.fg.to_string(), "theme:4 tint 0.39997558519241921");
        assert_eq!(fancy.border.left.style, "thin");
        assert_eq!(fancy.border.bottom.color, Color::Auto);
        assert!(fancy.border.top.style.is_empty());
        assert_eq!((fancy.alignment.horizontal.as_str(), fancy.alignment.wrap), ("center", true));
        assert!(!fancy.locked);
        assert_eq!(fancy.style, "Comma [0]");
        assert_eq!(styles.format(2).number_format, "mm-dd-yy");
        // An index past the table falls back to the first format.
        assert_eq!(styles.format(99), plain);
    }

    #[test]
    fn differences_name_each_property() {
        let styles = parse_styles(STYLES, "styles.xml").unwrap();
        let diff = styles.format(0).differences(&styles.format(2));
        assert_eq!(diff, [("numberFormat", "General".to_owned(), "mm-dd-yy".to_owned())]);
        assert!(styles.format(1).differences(&styles.format(1)).is_empty());
    }
}
