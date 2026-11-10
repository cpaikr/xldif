//! A small pull parser over `quick-xml` for SpreadsheetML parts.
//!
//! It yields namespace-agnostic local names (generators differ in prefixes:
//! `<c>` and `<x:c>` are the same element), resolves entity and character
//! references in text and attributes, and caps nesting depth so hostile
//! input fails instead of exhausting the stack of a recursive consumer.

use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesStart, Event as QEvent};
use quick_xml::{Reader, XmlVersion};

use crate::{Error, ErrorCode};

/// Deeper nesting than any SpreadsheetML part needs.
pub const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// Local name, without any namespace prefix.
    pub name: String,
    /// Attributes by local name, values unescaped, in document order.
    pub attrs: Vec<(String, String)>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// An opening tag. A self-closing tag yields `Start` then `End`.
    Start(Element),
    End(String),
    /// Character data with references resolved. Adjacent pieces may arrive
    /// as several events.
    Text(String),
    Eof,
}

pub struct Parser<'a> {
    reader: Reader<&'a [u8]>,
    part: &'a str,
    depth: usize,
    /// The `End` owed for a self-closing tag.
    pending_end: Option<String>,
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_owned()
}

impl<'a> Parser<'a> {
    /// `part` names the archive member in error messages.
    pub fn new(text: &'a str, part: &'a str) -> Self {
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(false);
        Self { reader, part, depth: 0, pending_end: None }
    }

    fn error(&self, detail: impl std::fmt::Display) -> Error {
        Error::new(ErrorCode::InvalidXml, format!("{} is not well-formed XML: {detail}", self.part))
    }

    fn element(&self, start: &BytesStart<'_>) -> Result<Element, Error> {
        let mut attrs = Vec::new();
        for attr in start.attributes() {
            let attr = attr.map_err(|e| self.error(e))?;
            let value = attr.normalized_value(XmlVersion::Implicit1_0).map_err(|e| self.error(e))?;
            attrs.push((local(attr.key.as_ref()), value.into_owned()));
        }
        Ok(Element { name: local(start.name().as_ref()), attrs })
    }

    pub fn next(&mut self) -> Result<Event, Error> {
        if let Some(name) = self.pending_end.take() {
            self.depth -= 1;
            return Ok(Event::End(name));
        }
        loop {
            let event = self.reader.read_event().map_err(|e| self.error(e))?;
            return Ok(match event {
                QEvent::Start(start) => {
                    self.depth += 1;
                    if self.depth > MAX_DEPTH {
                        return Err(Error::new(
                            ErrorCode::InvalidArchive,
                            format!("{} nests deeper than {MAX_DEPTH} elements", self.part),
                        ));
                    }
                    Event::Start(self.element(&start)?)
                }
                QEvent::Empty(start) => {
                    let element = self.element(&start)?;
                    self.depth += 1;
                    self.pending_end = Some(element.name.clone());
                    Event::Start(element)
                }
                QEvent::End(end) => {
                    self.depth = self.depth.saturating_sub(1);
                    Event::End(local(end.name().as_ref()))
                }
                QEvent::Text(text) => Event::Text(text.xml10_content().into_owned()),
                QEvent::CData(data) => Event::Text(data.xml10_content().into_owned()),
                QEvent::GeneralRef(reference) => {
                    if let Some(ch) = reference.resolve_char_ref().map_err(|e| self.error(e))? {
                        Event::Text(ch.to_string())
                    } else {
                        let name: &str = &reference;
                        let resolved =
                            resolve_xml_entity(name).ok_or_else(|| self.error(format!("unknown entity &{name};")))?;
                        Event::Text(resolved.to_owned())
                    }
                }
                QEvent::Eof => Event::Eof,
                // Declarations, comments, processing instructions, doctypes.
                _ => continue,
            });
        }
    }

    /// Text up to the end of the element just started, skipping any child
    /// elements whose local name is in `skip` (with everything inside them)
    /// and collecting text from all others.
    pub fn text_until_end(&mut self, skip: &[&str]) -> Result<String, Error> {
        let mut out = String::new();
        let mut depth = 1usize;
        let mut skipping: Option<usize> = None;
        loop {
            match self.next()? {
                Event::Start(e) => {
                    depth += 1;
                    if skipping.is_none() && skip.contains(&e.name.as_str()) {
                        skipping = Some(depth);
                    }
                }
                Event::End(_) => {
                    if skipping == Some(depth) {
                        skipping = None;
                    }
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                }
                Event::Text(t) => {
                    if skipping.is_none() {
                        out.push_str(&t);
                    }
                }
                Event::Eof => return Err(self.error("unexpected end of part")),
            }
        }
    }

    /// The text inside `wanted` elements up to the end of the element just
    /// started, skipping any element named in `skip` with everything inside
    /// it. Text outside `wanted` (indentation between elements) is dropped.
    pub fn text_of(&mut self, wanted: &str, skip: &[&str]) -> Result<String, Error> {
        let mut out = String::new();
        let mut depth = 1usize;
        let mut skipping: Option<usize> = None;
        let mut inside: Option<usize> = None;
        loop {
            match self.next()? {
                Event::Start(e) => {
                    depth += 1;
                    if skipping.is_none() && skip.contains(&e.name.as_str()) {
                        skipping = Some(depth);
                    } else if skipping.is_none() && inside.is_none() && e.name == wanted {
                        inside = Some(depth);
                    }
                }
                Event::End(_) => {
                    if skipping == Some(depth) {
                        skipping = None;
                    }
                    if inside == Some(depth) {
                        inside = None;
                    }
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                }
                Event::Text(t) => {
                    if skipping.is_none() && inside.is_some() {
                        out.push_str(&t);
                    }
                }
                Event::Eof => return Err(self.error("unexpected end of part")),
            }
        }
    }

    /// Skips the rest of the element just started.
    pub fn skip_element(&mut self) -> Result<(), Error> {
        let mut depth = 1usize;
        loop {
            match self.next()? {
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Event::Text(_) => {}
                Event::Eof => return Err(self.error("unexpected end of part")),
            }
        }
    }
}

/// Decodes the `_xHHHH_` escapes OOXML uses for characters XML cannot carry
/// (ECMA-376 Part 1, `ST_Xstring`). `_x005F_` escapes a literal underscore, so
/// `_x005F_x000D_` stays the text `_x000D_`.
pub fn decode_xstring(text: &str) -> String {
    if !text.contains("_x") {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if bytes[i] == b'_'
            && i + 7 <= text.len()
            && bytes[i + 1] == b'x'
            && bytes[i + 6] == b'_'
            && bytes[i + 2..i + 6].iter().all(u8::is_ascii_hexdigit)
        {
            let code = u32::from_str_radix(&text[i + 2..i + 6], 16).expect("four hex digits");
            if let Some(ch) = char::from_u32(code) {
                out.push(ch);
                i += 7;
                continue;
            }
        }
        let ch = text[i..].chars().next().expect("index on a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Event, Parser, decode_xstring};

    fn events(xml: &str) -> Vec<Event> {
        let mut p = Parser::new(xml, "test.xml");
        let mut out = Vec::new();
        loop {
            let e = p.next().unwrap();
            if e == Event::Eof {
                return out;
            }
            out.push(e);
        }
    }

    #[test]
    fn prefixes_are_dropped_and_empty_tags_close() {
        let ev = events(r#"<x:c r="A1" x:t="s"/>"#);
        let Event::Start(e) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(e.name, "c");
        assert_eq!(e.attr("t"), Some("s"));
        assert_eq!(ev[1], Event::End("c".into()));
    }

    #[test]
    fn references_resolve_in_text_and_attributes() {
        let mut p = Parser::new(r#"<t a="&lt;&#x41;">x &amp; y&#66;</t>"#, "test.xml");
        let Event::Start(e) = p.next().unwrap() else { panic!() };
        assert_eq!(e.attr("a"), Some("<A"));
        assert_eq!(p.text_until_end(&[]).unwrap(), "x & yB");
    }

    #[test]
    fn skipped_children_drop_their_text() {
        let mut p = Parser::new("<si><r><t>a</t></r><rPh><t>b</t></rPh><t>c</t></si>", "test.xml");
        p.next().unwrap();
        assert_eq!(p.text_until_end(&["rPh"]).unwrap(), "ac");
    }

    #[test]
    fn text_of_keeps_only_wanted_elements() {
        let xml = "<si>\n  <r>\n    <t>rich </t>\n  </r>\n  <r><t>text</t></r>\n  <rPh><t>yomi</t></rPh>\n</si>";
        let mut p = Parser::new(xml, "test.xml");
        p.next().unwrap();
        assert_eq!(p.text_of("t", &["rPh"]).unwrap(), "rich text");
    }

    #[test]
    fn depth_is_capped() {
        let xml = "<a>".repeat(super::MAX_DEPTH + 1);
        let mut p = Parser::new(&xml, "deep.xml");
        let err = loop {
            match p.next() {
                Ok(Event::Eof) => panic!("no error"),
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert_eq!(err.code, crate::ErrorCode::InvalidArchive);
    }

    #[test]
    fn xstring_escapes_decode() {
        assert_eq!(decode_xstring("a_x000D_b"), "a\rb");
        assert_eq!(decode_xstring("_x005F_x000D_"), "_x000D_");
        assert_eq!(decode_xstring("plain_xyz_"), "plain_xyz_");
        assert_eq!(decode_xstring("한글_x0041_"), "한글A");
    }
}
