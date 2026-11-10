//! The built-in manual: the project's documents parsed once, split into pages, with a table of
//! contents, anchors for links between sections and a ranked search.
//!
//! The app gives it the Markdown text of its documents ([`Source`]; it compiles the files of
//! `docs/` in, so the manual and the documentation are one text). [`Manual::new`] parses them
//! ([`markdown`]) and cuts each at its headings of level one and two: a [`Page`] is one such
//! section with everything under it. A [`Location`] names a heading (or the top of a page) by
//! page and block, and every heading has a GitHub-style anchor, so `#the-main-window` and
//! `KEYBOARD.md#global` links resolve ([`Manual::resolve`]).
//!
//! [`Manual::search`] looks in every section (a heading and the blocks under it until the next
//! heading): a heading match ranks above a body match, an exact or leading match above a plain
//! one, and each hit carries a snippet with the matched text marked.
//!
//! [`ManualView`] is the panel that shows it.

pub mod markdown;

mod layout;
mod search;
mod view;

pub use search::Hit;
pub use view::{ManualAction, ManualView};

use markdown::{Block, plain};
use std::collections::BTreeMap;

/// One document of the manual.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// The file name links use (`README.md`).
    pub file: &'a str,
    /// The name the manual shows for it.
    pub name: &'a str,
    /// The Markdown text.
    pub text: &'a str,
}

/// A place in the manual: a block of a page (a heading, or 0 for the top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Location {
    /// The page.
    pub page: usize,
    /// The block of the page.
    pub block: usize,
}

/// A page: a heading of level one or two with everything under it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The document it belongs to.
    pub doc: usize,
    /// Its title.
    pub title: String,
    /// Its blocks (the first is the heading, unless the document starts with text).
    pub blocks: Vec<Block>,
}

/// A document as the manual keeps it.
#[derive(Debug, Clone)]
pub struct Doc {
    /// The file name.
    pub file: String,
    /// The name shown.
    pub name: String,
    /// The first page of the document.
    pub first_page: usize,
    /// How many pages it has.
    pub pages: usize,
}

/// One row of the table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    /// The heading level (1 to 3).
    pub level: u8,
    /// The heading text.
    pub title: String,
    /// Where it is.
    pub at: Location,
}

/// Where a link goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// A place in the manual.
    Here(Location),
    /// Something outside it (another document, an address): the target as written.
    Outside(String),
}

/// The deepest heading level the table of contents lists.
pub const TOC_LEVELS: u8 = 3;

/// The parsed manual. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct Manual {
    docs: Vec<Doc>,
    pages: Vec<Page>,
    toc: Vec<TocEntry>,
    anchors: BTreeMap<(usize, String), Location>,
}

impl Manual {
    /// Parses `sources` into a manual.
    pub fn new(sources: &[Source<'_>]) -> Self {
        let mut manual = Self::default();
        for (d, source) in sources.iter().enumerate() {
            let blocks = markdown::parse(source.text);
            let first_page = manual.pages.len();
            let mut page: Option<Page> = None;
            let mut pages = 0;
            for block in blocks {
                let starts = matches!(&block, Block::Heading { level, .. } if *level <= 2);
                if starts || page.is_none() {
                    if let Some(done) = page.take() {
                        manual.pages.push(done);
                        pages += 1;
                    }
                    let title = match &block {
                        Block::Heading { runs, .. } => plain(runs),
                        _ => source.name.to_owned(),
                    };
                    page = Some(Page {
                        doc: d,
                        title,
                        blocks: Vec::new(),
                    });
                }
                if let Some(p) = &mut page {
                    p.blocks.push(block);
                }
            }
            if let Some(done) = page.take() {
                manual.pages.push(done);
                pages += 1;
            }
            manual.docs.push(Doc {
                file: source.file.to_owned(),
                name: source.name.to_owned(),
                first_page,
                pages,
            });
        }
        for (p, page) in manual.pages.iter().enumerate() {
            for (b, block) in page.blocks.iter().enumerate() {
                let Block::Heading {
                    level,
                    runs,
                    anchor,
                } = block
                else {
                    continue;
                };
                let at = Location { page: p, block: b };
                manual.anchors.insert((page.doc, anchor.clone()), at);
                if *level <= TOC_LEVELS {
                    manual.toc.push(TocEntry {
                        level: *level,
                        title: plain(runs),
                        at,
                    });
                }
            }
        }
        manual
    }

    /// The documents.
    pub fn docs(&self) -> &[Doc] {
        &self.docs
    }

    /// The pages, in reading order.
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// The table of contents, in reading order.
    pub fn toc(&self) -> &[TocEntry] {
        &self.toc
    }

    /// The top of the manual.
    pub fn start(&self) -> Location {
        Location::default()
    }

    /// The heading at `at` (its level, text and anchor), `None` when there is none.
    pub fn heading_at(&self, at: Location) -> Option<(u8, String, &str)> {
        match self.pages.get(at.page)?.blocks.get(at.block)? {
            Block::Heading {
                level,
                runs,
                anchor,
            } => Some((*level, plain(runs), anchor)),
            _ => None,
        }
    }

    /// The place of the heading `anchor` of document `doc`.
    pub fn anchor_location(&self, doc: usize, anchor: &str) -> Option<Location> {
        self.anchors.get(&(doc, anchor.to_owned())).copied()
    }

    /// The first heading whose text is `title` (ignoring case), in reading order.
    pub fn find_heading(&self, title: &str) -> Option<Location> {
        let want = title.trim().to_lowercase();
        for (p, page) in self.pages.iter().enumerate() {
            for (b, block) in page.blocks.iter().enumerate() {
                if let Block::Heading { runs, .. } = block
                    && plain(runs).to_lowercase() == want
                {
                    return Some(Location { page: p, block: b });
                }
            }
        }
        None
    }

    /// The headings down to `max_level`, as (text, place, document index), in reading order.
    pub fn headings(&self, max_level: u8) -> Vec<(String, Location, usize)> {
        let mut out = Vec::new();
        for (p, page) in self.pages.iter().enumerate() {
            for (b, block) in page.blocks.iter().enumerate() {
                if let Block::Heading { level, runs, .. } = block
                    && *level <= max_level
                {
                    out.push((plain(runs), Location { page: p, block: b }, page.doc));
                }
            }
        }
        out
    }

    /// Where the link `target` (as written in the text of page `from`) goes.
    pub fn resolve(&self, from: usize, target: &str) -> Link {
        let doc = self.pages.get(from).map_or(0, |p| p.doc);
        if let Some(anchor) = target.strip_prefix('#') {
            return match self.anchor_location(doc, anchor) {
                Some(at) => Link::Here(at),
                None => Link::Outside(target.to_owned()),
            };
        }
        let (file, anchor) = match target.split_once('#') {
            Some((f, a)) => (f, Some(a)),
            None => (target, None),
        };
        let file = file.trim_start_matches("./");
        match self
            .docs
            .iter()
            .position(|d| d.file.eq_ignore_ascii_case(file))
        {
            Some(d) => {
                let top = Location {
                    page: self.docs[d].first_page,
                    block: 0,
                };
                Link::Here(
                    anchor
                        .and_then(|a| self.anchor_location(d, a))
                        .unwrap_or(top),
                )
            }
            None => Link::Outside(target.to_owned()),
        }
    }

    /// Where the section of the table of contents that holds `at` starts: the entry shown as
    /// current.
    pub fn toc_index_of(&self, at: Location) -> Option<usize> {
        self.toc.iter().rposition(|e| e.at <= at)
    }
}
