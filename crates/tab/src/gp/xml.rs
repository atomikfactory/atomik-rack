//! A small, capped XML tree for the GPIF score.
//!
//! The score is read once into a flat arena of nodes (names interned, children linked by index),
//! so a hostile file is bounded by [`limits::MAX_XML_NODES`] and [`limits::MAX_XML_DEPTH`] and the
//! document can be queried by name without a deserialiser. Namespaces, comments, processing
//! instructions and the document type are skipped (entities other than the five predefined ones and
//! character references are ignored, never expanded).

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{GpError, limits};

const NONE: u32 = u32::MAX;
/// Text kept per element.
const MAX_TEXT_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct Node {
    name: u16,
    first: u32,
    last: u32,
    next: u32,
    attrs: Vec<(u16, String)>,
    text: String,
}

/// A parsed document.
#[derive(Debug)]
pub(crate) struct Xml {
    names: Vec<String>,
    nodes: Vec<Node>,
}

/// A reference to one element of an [`Xml`] document.
#[derive(Clone, Copy)]
pub(crate) struct El<'a> {
    xml: &'a Xml,
    id: u32,
}

impl Xml {
    fn intern(
        names: &mut Vec<String>,
        index: &mut BTreeMap<String, u16>,
        s: &str,
    ) -> Result<u16, GpError> {
        if let Some(i) = index.get(s) {
            return Ok(*i);
        }
        let i = u16::try_from(names.len()).map_err(|_| GpError::TooMany("distinct XML names"))?;
        names.push(s.to_owned());
        index.insert(s.to_owned(), i);
        Ok(i)
    }

    /// Parses a document.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Xml, GpError> {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        let mut reader = Reader::from_reader(bytes);
        let mut buf = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut index: BTreeMap<String, u16> = BTreeMap::new();
        let doc_name = Self::intern(&mut names, &mut index, "")?;
        let mut nodes = vec![Node {
            name: doc_name,
            first: NONE,
            last: NONE,
            next: NONE,
            attrs: Vec::new(),
            text: String::new(),
        }];
        let mut stack: Vec<u32> = vec![0];
        let xml_err = |e: &dyn std::fmt::Display| GpError::Xml(e.to_string());

        loop {
            buf.clear();
            let ev = reader.read_event_into(&mut buf).map_err(|e| xml_err(&e))?;
            match ev {
                Event::Start(e) => {
                    open(&mut nodes, &mut stack, &mut names, &mut index, &e)?;
                }
                Event::Empty(e) => {
                    open(&mut nodes, &mut stack, &mut names, &mut index, &e)?;
                    stack.pop();
                }
                Event::End(_) => {
                    if stack.len() <= 1 {
                        return Err(GpError::Xml("an end tag without a start".into()));
                    }
                    if let Some(id) = stack.pop() {
                        let t = &mut nodes[id as usize].text;
                        let trimmed = t.trim();
                        if trimmed.len() != t.len() {
                            *t = trimmed.to_owned();
                        }
                    }
                }
                Event::Text(t) => {
                    add_text(&mut nodes, &stack, &t);
                }
                Event::CData(c) => {
                    add_text(&mut nodes, &stack, &c);
                }
                Event::GeneralRef(r) => {
                    let name: &str = &r;
                    let piece = match name {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => r.resolve_char_ref().ok().flatten(),
                    };
                    if let Some(c) = piece {
                        let mut tmp = [0u8; 4];
                        add_text(&mut nodes, &stack, c.encode_utf8(&mut tmp));
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if stack.len() != 1 {
            return Err(GpError::Xml("the document ends inside an element".into()));
        }
        if nodes[0].first == NONE {
            return Err(GpError::Xml("the document is empty".into()));
        }
        Ok(Xml { names, nodes })
    }

    /// The root element.
    pub(crate) fn root(&self) -> El<'_> {
        let id = self.nodes[0].first;
        El { xml: self, id }
    }

    fn name_id(&self, name: &str) -> Option<u16> {
        self.names
            .iter()
            .position(|n| n == name)
            .and_then(|i| u16::try_from(i).ok())
    }
}

fn add_text(nodes: &mut [Node], stack: &[u32], s: &str) {
    let Some(&id) = stack.last() else {
        return;
    };
    let node = &mut nodes[id as usize];
    if s.trim().is_empty() && node.text.is_empty() {
        return;
    }
    if node.text.len() + s.len() <= MAX_TEXT_BYTES {
        node.text.push_str(s);
    }
}

fn open(
    nodes: &mut Vec<Node>,
    stack: &mut Vec<u32>,
    names: &mut Vec<String>,
    index: &mut BTreeMap<String, u16>,
    e: &BytesStart<'_>,
) -> Result<(), GpError> {
    if nodes.len() >= limits::MAX_XML_NODES {
        return Err(GpError::TooMany("XML elements"));
    }
    if stack.len() > limits::MAX_XML_DEPTH {
        return Err(GpError::Xml("the elements are nested too deeply".into()));
    }
    let bad = |e: &dyn std::fmt::Display| GpError::Xml(e.to_string());
    let local = e.local_name().into_inner().to_owned();
    let name = Xml::intern(names, index, &local)?;
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|e| bad(&e))?;
        if attrs.len() >= 32 {
            break;
        }
        let key = a.key.local_name().into_inner().to_owned();
        let k = Xml::intern(names, index, &key)?;
        let v = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| bad(&e))?;
        attrs.push((k, v.chars().take(4096).collect::<String>()));
    }
    let id = u32::try_from(nodes.len()).map_err(|_| GpError::TooMany("XML elements"))?;
    nodes.push(Node {
        name,
        first: NONE,
        last: NONE,
        next: NONE,
        attrs,
        text: String::new(),
    });
    let parent = stack.last().copied().unwrap_or(0) as usize;
    let last = nodes[parent].last;
    if last == NONE {
        nodes[parent].first = id;
    } else {
        nodes[last as usize].next = id;
    }
    nodes[parent].last = id;
    stack.push(id);
    Ok(())
}

impl<'a> El<'a> {
    fn node(&self) -> &'a Node {
        &self.xml.nodes[self.id as usize]
    }

    /// The element name without a namespace prefix.
    pub(crate) fn name(&self) -> &'a str {
        &self.xml.names[usize::from(self.node().name)]
    }

    /// The text of the element (trimmed), `""` when it has none.
    pub(crate) fn text(&self) -> &'a str {
        &self.node().text
    }

    /// An attribute value.
    pub(crate) fn attr(&self, name: &str) -> Option<&'a str> {
        let id = self.xml.name_id(name)?;
        self.node()
            .attrs
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| v.as_str())
    }

    /// The child elements in document order.
    pub(crate) fn children(&self) -> Children<'a> {
        Children {
            xml: self.xml,
            next: self.node().first,
        }
    }

    /// The first child with this name.
    pub(crate) fn child(&self, name: &str) -> Option<El<'a>> {
        self.children().find(|c| c.name() == name)
    }

    /// The children with this name.
    pub(crate) fn all<'n>(&self, name: &'n str) -> impl Iterator<Item = El<'a>> + use<'a, 'n> {
        self.children().filter(move |c| c.name() == name)
    }

    /// The text of the first child with this name.
    pub(crate) fn child_text(&self, name: &str) -> Option<&'a str> {
        self.child(name).map(|c| c.text())
    }

    /// Whether a child with this name exists.
    pub(crate) fn has(&self, name: &str) -> bool {
        self.child(name).is_some()
    }
}

/// Iterates the children of an element.
pub(crate) struct Children<'a> {
    xml: &'a Xml,
    next: u32,
}

impl<'a> Iterator for Children<'a> {
    type Item = El<'a>;

    fn next(&mut self) -> Option<El<'a>> {
        if self.next == NONE {
            return None;
        }
        let id = self.next;
        self.next = self.xml.nodes[id as usize].next;
        Some(El { xml: self.xml, id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_tree() {
        let x = Xml::parse(
            b"\xEF\xBB\xBF<?xml version=\"1.0\"?><A k=\"1&amp;2\"><B>x &amp; y</B><C/><B><![CDATA[z<]]></B></A>",
        )
        .unwrap();
        let r = x.root();
        assert_eq!(r.name(), "A");
        assert_eq!(r.attr("k"), Some("1&2"));
        let bs: Vec<_> = r.all("B").map(|b| b.text().to_owned()).collect();
        assert_eq!(bs, ["x & y", "z<"]);
        assert!(r.has("C"));
        assert!(!r.has("D"));
        assert_eq!(r.child_text("B"), Some("x & y"));
    }

    #[test]
    fn namespaces_are_dropped() {
        let x = Xml::parse(b"<a:A xmlns:a=\"u\"><a:B a:k=\"v\"/></a:A>").unwrap();
        assert_eq!(x.root().name(), "A");
        assert_eq!(x.root().child("B").and_then(|b| b.attr("k")), Some("v"));
    }

    #[test]
    fn bad_documents_are_errors() {
        assert!(Xml::parse(b"").is_err());
        assert!(Xml::parse(b"<a><b></a>").is_err());
        assert!(Xml::parse(b"<a>").is_err());
        assert!(Xml::parse(b"</a>").is_err());
        assert!(Xml::parse(b"just text").is_err());
    }

    #[test]
    fn deep_nesting_is_refused() {
        let mut s = String::new();
        for _ in 0..(limits::MAX_XML_DEPTH + 10) {
            s.push_str("<a>");
        }
        assert!(matches!(Xml::parse(s.as_bytes()), Err(GpError::Xml(_))));
    }

    #[test]
    fn many_elements_are_refused_not_stored() {
        let mut s = String::from("<a>");
        s.push_str(&"<b/>".repeat(1000));
        s.push_str("</a>");
        assert_eq!(Xml::parse(s.as_bytes()).unwrap().nodes.len(), 1002);
    }
}
