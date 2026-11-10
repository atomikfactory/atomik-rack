//! Searching the manual: ranked hits, one per section, with a snippet.

use super::markdown::{Block, plain};
use super::{Location, Manual};

/// The most hits one search gives.
pub const MAX_HITS: usize = 60;
/// The most words of a query that count.
const MAX_TERMS: usize = 6;
/// How many characters a snippet shows.
const WINDOW: usize = 110;
/// How many characters of context a snippet keeps before the first match.
const LEAD: usize = 28;

/// One search result: a section of the manual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The heading of the section (or the top of a page that starts without one).
    pub at: Location,
    /// The heading text.
    pub title: String,
    /// The character ranges of the heading text that matched.
    pub title_hits: Vec<(usize, usize)>,
    /// The name of the document.
    pub doc: String,
    /// A line of the section, around the match when the body matched.
    pub snippet: String,
    /// The character ranges of the snippet that matched.
    pub snippet_hits: Vec<(usize, usize)>,
    /// The heading matched (otherwise the body did).
    pub in_heading: bool,
    /// The rank (higher is better).
    pub score: u32,
}

/// Lower case that keeps the number of characters.
fn fold(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Every place `term` occurs in `hay`, as character ranges.
fn find_all(hay: &[char], term: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if term.is_empty() || term.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + term.len() <= hay.len() {
        if hay[i..i + term.len()] == *term {
            out.push((i, i + term.len()));
            i += term.len();
        } else {
            i += 1;
        }
    }
    out
}

/// The ranges of every term in `text`, sorted and merged.
fn marks(text: &str, terms: &[Vec<char>]) -> Vec<(usize, usize)> {
    let hay = fold(text);
    let mut all: Vec<(usize, usize)> = terms.iter().flat_map(|t| find_all(&hay, t)).collect();
    all.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in all {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

fn has_all(hay: &[char], terms: &[Vec<char>]) -> bool {
    terms.iter().all(|t| !find_all(hay, t).is_empty())
}

/// A window of `text` around its first match, cut at word edges and marked with `...` when it
/// does not start or end the text, with the matched ranges of the result.
fn snippet(text: &str, terms: &[Vec<char>]) -> (String, Vec<(usize, usize)>) {
    let flat: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let chars: Vec<char> = flat.trim().chars().collect();
    let first = marks(&flat, terms).first().map_or(0, |m| m.0);
    let mut start = first.saturating_sub(LEAD);
    // Start on a word.
    if start > 0
        && let Some(sp) = chars[start..first].iter().position(|c| *c == ' ')
    {
        start += sp + 1;
    }
    let mut end = (start + WINDOW).min(chars.len());
    // End on a word (when the cut falls inside one).
    if end < chars.len()
        && let Some(sp) = chars[start..end].iter().rposition(|c| *c == ' ')
        && start + sp > first
    {
        end = start + sp;
    }
    let mut text: String = chars[start..end].iter().collect();
    if start > 0 {
        text.insert_str(0, "...");
    }
    if end < chars.len() {
        text.push_str("...");
    }
    let hits = marks(&text, terms);
    (text, hits)
}

impl Manual {
    /// Searches every section for the words of `query` (all of them must occur in the heading
    /// or in one block of the body, ignoring case). Best hits first: a heading that equals the
    /// query, then one that starts with it, then any heading match (higher levels first), then
    /// body matches by how often the words occur. At most [`MAX_HITS`] hits.
    pub fn search(&self, query: &str) -> Vec<Hit> {
        let terms: Vec<Vec<char>> = query.split_whitespace().take(MAX_TERMS).map(fold).collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let whole: Vec<char> = terms.join(&' ');
        let mut hits: Vec<Hit> = Vec::new();
        for (p, page) in self.pages.iter().enumerate() {
            // The section being read: its place, title, level and first plain block.
            let mut section: Option<Section> = None;
            let finish = |s: Option<Section>, hits: &mut Vec<Hit>| {
                let Some(s) = s else { return };
                if let Some(hit) = s.into_hit(&terms, &whole, &self.docs[page.doc].name) {
                    hits.push(hit);
                }
            };
            for (b, block) in page.blocks.iter().enumerate() {
                let at = Location { page: p, block: b };
                if let Block::Heading { level, runs, .. } = block {
                    finish(section.take(), &mut hits);
                    section = Some(Section::new(at, plain(runs), *level));
                    continue;
                }
                if section.is_none() {
                    section = Some(Section::new(at, page.title.clone(), 1));
                }
                if let Some(s) = &mut section {
                    s.add(block);
                }
            }
            finish(section.take(), &mut hits);
        }
        // Stable: equal scores keep reading order.
        hits.sort_by_key(|h| std::cmp::Reverse(h.score));
        hits.truncate(MAX_HITS);
        hits
    }
}

struct Section {
    at: Location,
    title: String,
    level: u8,
    /// The first block with prose (the snippet when only the heading matched).
    lead: Option<String>,
    /// The body blocks as plain text.
    body: Vec<String>,
}

impl Section {
    fn new(at: Location, title: String, level: u8) -> Self {
        Self {
            at,
            title,
            level,
            lead: None,
            body: Vec::new(),
        }
    }

    fn add(&mut self, block: &Block) {
        let text = block.plain();
        if text.trim().is_empty() {
            return;
        }
        if self.lead.is_none() && matches!(block, Block::Paragraph(_) | Block::Item { .. }) {
            self.lead = Some(text.clone());
        }
        self.body.push(text);
    }

    fn into_hit(self, terms: &[Vec<char>], whole: &[char], doc: &str) -> Option<Hit> {
        let title_fold = fold(&self.title);
        let in_heading = has_all(&title_fold, terms);
        // The best body block: the one where the words occur most.
        let best = self
            .body
            .iter()
            .filter_map(|text| {
                let hay = fold(text);
                has_all(&hay, terms).then(|| {
                    let n: usize = terms.iter().map(|t| find_all(&hay, t).len()).sum();
                    (n, text)
                })
            })
            .max_by_key(|(n, _)| *n);
        if !in_heading && best.is_none() {
            return None;
        }
        let score = if in_heading {
            let mut s = 1000 + u32::from(4 - self.level.min(4)) * 10;
            if title_fold == whole {
                s += 200;
            } else if title_fold.starts_with(whole) {
                s += 100;
            }
            s
        } else {
            100 + best.map_or(0, |(n, _)| n.min(20) as u32)
        };
        let (snippet, snippet_hits) = match best {
            Some((_, text)) => snippet(text, terms),
            None => self
                .lead
                .as_deref()
                .map_or((String::new(), Vec::new()), |t| snippet(t, &[])),
        };
        Some(Hit {
            at: self.at,
            title_hits: marks(&self.title, terms),
            title: self.title,
            doc: doc.to_owned(),
            snippet,
            snippet_hits,
            in_heading,
            score,
        })
    }
}
