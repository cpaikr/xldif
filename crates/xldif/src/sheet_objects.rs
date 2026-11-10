//! Sheet objects outside the cell grid: row and column records, merged
//! ranges, data validation, conditional formatting, hyperlinks, and
//! comments. The reader calls these while it walks a sheet part (and its
//! related parts), so each takes a parser positioned on its element.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::cellref::Range;
use crate::model::{Comment, LineProps, RangeRule};
use crate::package::Relationship;
use crate::xml::{Element, Event, Parser, decode_xstring};
use crate::{Error, ErrorCode};

const PHONETIC: &[&str] = &["rPh", "phoneticPr"];

fn truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1" | "true"))
}

/// `A1:B2 C3` (space-separated, as `sqref` writes it) into ranges; parts
/// that are not ranges are skipped.
pub fn parse_sqref(text: &str) -> Vec<Range> {
    text.split_whitespace().filter_map(Range::parse).collect()
}

/// A `<row>` element's record, when it sets anything a person sees.
pub fn row_props(e: &Element, index: u32) -> Option<LineProps> {
    let size = truthy(e.attr("customHeight")).then(|| e.attr("ht").unwrap_or_default().to_owned());
    let hidden = truthy(e.attr("hidden"));
    let style = truthy(e.attr("customFormat")).then(|| e.attr("s").and_then(|s| s.parse().ok()).unwrap_or(0));
    (size.is_some() || hidden || style.is_some()).then_some(LineProps { index, last: index, size, hidden, style })
}

/// A `<col>` element's record (it may cover several columns), when it sets
/// anything a person sees.
pub fn col_props(e: &Element) -> Option<LineProps> {
    let index: u32 = e.attr("min")?.parse().ok().filter(|m| (1..=crate::cellref::MAX_COL).contains(m))?;
    let last: u32 = e.attr("max").and_then(|m| m.parse().ok()).unwrap_or(index).clamp(index, crate::cellref::MAX_COL);
    let size = truthy(e.attr("customWidth")).then(|| e.attr("width").unwrap_or_default().to_owned());
    let hidden = truthy(e.attr("hidden"));
    let style = e.attr("style").and_then(|s| s.parse().ok()).filter(|s| *s != 0);
    (size.is_some() || hidden || style.is_some()).then_some(LineProps { index, last, size, hidden, style })
}

/// Reads a `<dataValidation>` (just started) into a rule.
pub fn data_validation(e: &Element, p: &mut Parser<'_>) -> Result<RangeRule, Error> {
    let mut formulas: Vec<(String, String)> = Vec::new();
    let mut depth = 1;
    while depth > 0 {
        match p.next()? {
            Event::Start(c) => {
                if c.name == "formula1" || c.name == "formula2" {
                    let text = p.text_until_end(&[])?;
                    formulas.push((c.name.clone(), text));
                } else {
                    depth += 1;
                }
            }
            Event::End(_) => depth -= 1,
            Event::Text(_) => {}
            Event::Eof => return Err(Error::new(ErrorCode::InvalidXml, "a data validation is not closed")),
        }
    }
    // Every attribute but the ranges (type, operator, messages, drop-down…),
    // in a fixed order, then the formulas.
    let mut attrs: Vec<String> =
        e.attrs.iter().filter(|(k, _)| k != "sqref" && k != "uid").map(|(k, v)| format!("{k}={v}")).collect();
    attrs.sort();
    let mut rule = attrs.join("; ");
    for (name, text) in formulas {
        let _ = write!(rule, "; {name}={text}");
    }
    Ok(RangeRule { ranges: parse_sqref(e.attr("sqref").unwrap_or_default()).into(), rule })
}

/// Reads a `<conditionalFormatting>` (just started): one rule per
/// `<cfRule>`, described by its attributes (priority left out, since
/// reordering rules renumbers it), the style it applies (the resolved
/// differential format, not its index, which renumbers whenever a rule is
/// added elsewhere), and its children: formulas, color scales, data bars,
/// and icon sets with their thresholds and colors.
pub fn conditional_formatting(
    e: &Element,
    p: &mut Parser<'_>,
    styles: &crate::styles::Styles,
) -> Result<Vec<RangeRule>, Error> {
    let ranges: std::sync::Arc<[Range]> = parse_sqref(e.attr("sqref").unwrap_or_default()).into();
    let mut rules = Vec::new();
    let mut depth = 1;
    while depth > 0 {
        match p.next()? {
            Event::Start(c) if c.name == "cfRule" => {
                let mut attrs: Vec<String> = c
                    .attrs
                    .iter()
                    .filter(|(k, _)| k != "priority" && k != "dxfId")
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                if let Some(dxf) = c.attr("dxfId").and_then(|d| d.parse::<usize>().ok()) {
                    attrs.push(format!("style={}", styles.dxf(dxf)));
                }
                attrs.sort();
                let mut rule = attrs.join("; ");
                let children = p.describe_rest()?;
                if !children.is_empty() {
                    let _ = write!(rule, "; {children}");
                }
                rules.push(RangeRule { ranges: ranges.clone(), rule });
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            Event::Text(_) => {}
            Event::Eof => return Err(Error::new(ErrorCode::InvalidXml, "conditional formatting is not closed")),
        }
    }
    Ok(rules)
}

/// Legacy notes (`commentsN.xml`).
pub fn legacy_comments(text: &str, part: &str) -> Result<Vec<Comment>, Error> {
    let mut p = Parser::new(text, part);
    let mut authors = Vec::new();
    let mut out = Vec::new();
    loop {
        match p.next()? {
            Event::Start(e) if e.name == "author" => authors.push(decode_xstring(&p.text_until_end(&[])?)),
            Event::Start(e) if e.name == "comment" => {
                let Some((row, col)) = e.attr("ref").and_then(crate::cellref::parse_cell) else {
                    p.skip_element()?;
                    continue;
                };
                let author =
                    e.attr("authorId").and_then(|a| a.parse::<usize>().ok()).and_then(|a| authors.get(a)).cloned();
                let text = decode_xstring(&p.text_of("t", PHONETIC)?);
                out.push(Comment { row, col, author: author.unwrap_or_default(), text });
            }
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// People named by threaded comments (`persons.xml`): id → display name.
pub fn persons(text: &str, part: &str) -> Result<HashMap<String, String>, Error> {
    let mut p = Parser::new(text, part);
    let mut out = HashMap::new();
    loop {
        match p.next()? {
            Event::Start(e) if e.name == "person" => {
                if let (Some(id), Some(name)) = (e.attr("id"), e.attr("displayName")) {
                    out.insert(id.to_owned(), name.to_owned());
                }
            }
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// Threaded comments, one [`Comment`] per thread: the first author, and
/// every entry as `author: text` on its own line, in file order.
pub fn threaded_comments(text: &str, part: &str, people: &HashMap<String, String>) -> Result<Vec<Comment>, Error> {
    let mut p = Parser::new(text, part);
    let mut threads: Vec<Comment> = Vec::new();
    let mut at: HashMap<(u32, u32), usize> = HashMap::new();
    loop {
        match p.next()? {
            Event::Start(e) if e.name == "threadedComment" => {
                let Some((row, col)) = e.attr("ref").and_then(crate::cellref::parse_cell) else {
                    p.skip_element()?;
                    continue;
                };
                let author = e.attr("personId").and_then(|id| people.get(id)).cloned().unwrap_or_default();
                let body = decode_xstring(&p.text_of("text", &[])?);
                let line = format!("{author}: {body}");
                if let Some(&k) = at.get(&(row, col)) {
                    threads[k].text.push('\n');
                    threads[k].text.push_str(&line);
                } else {
                    at.insert((row, col), threads.len());
                    threads.push(Comment { row, col, author, text: line });
                }
            }
            Event::Eof => return Ok(threads),
            _ => {}
        }
    }
}

/// A hyperlink's target: its external address through the sheet's
/// relationships, or `#location` inside the workbook.
pub fn hyperlink_target(e: &Element, rels: &[Relationship]) -> String {
    let external = e.attr("id").and_then(|id| rels.iter().find(|r| r.id == id)).map(|r| r.target.clone());
    let location = e.attr("location").map(|l| format!("#{l}"));
    match (external, location) {
        (Some(t), Some(l)) => format!("{t}{l}"),
        (Some(t), None) => t,
        (None, Some(l)) => l,
        (None, None) => String::new(),
    }
}
