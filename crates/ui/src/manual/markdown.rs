//! A small Markdown subset parser for the built-in manual.
//!
//! It reads what the project's own documents use and nothing else:
//!
//! * headings (`#` to `######`) with a GitHub-style anchor each (lower case, letters, digits,
//!   `-` and `_` kept, spaces turned into `-`, a repeated heading gets `-1`, `-2`...);
//! * paragraphs, block quotes and rules;
//! * bullet and numbered lists (nested by indent; a wrapped item line joins the item);
//! * fenced code blocks (three backticks or tildes) and HTML comments (skipped);
//! * tables with a header row and a `---` row;
//! * inline `**bold**`, `*italic*` and `_italic_`, `` `code` ``, `[text](target)` links, bare
//!   `http(s)://` addresses, `![alt](image)` (shown as its alt text) and backslash escapes.
//!
//! Anything else is plain text. The parser never fails and never panics: odd input is text.

use std::collections::BTreeMap;

/// A piece of inline text with one style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    /// The text.
    pub text: String,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Inline code.
    pub code: bool,
    /// The link target, for a link.
    pub link: Option<String>,
}

impl Run {
    /// A plain run.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    fn same_style(&self, other: &Self) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.code == other.code
            && self.link == other.link
    }
}

/// One block of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A heading.
    Heading {
        /// 1 to 6.
        level: u8,
        /// The text.
        runs: Vec<Run>,
        /// The anchor links use (`#anchor`).
        anchor: String,
    },
    /// A paragraph.
    Paragraph(Vec<Run>),
    /// One list item (a list is a run of items).
    Item {
        /// Nesting, 0 for the outermost list.
        depth: u8,
        /// The number of a numbered item.
        number: Option<u32>,
        /// The text.
        runs: Vec<Run>,
    },
    /// A fenced code block.
    Code {
        /// The text after the opening fence.
        lang: String,
        /// The lines.
        lines: Vec<String>,
    },
    /// A table.
    Table {
        /// The header cells.
        head: Vec<Vec<Run>>,
        /// The body rows, each as many cells as the header.
        rows: Vec<Vec<Vec<Run>>>,
    },
    /// A block quote.
    Quote(Vec<Run>),
    /// A horizontal rule.
    Rule,
}

impl Block {
    /// The text of the block without styles (cells and code lines separated by spaces).
    pub fn plain(&self) -> String {
        match self {
            Self::Heading { runs, .. } | Self::Paragraph(runs) | Self::Quote(runs) => plain(runs),
            Self::Item { runs, .. } => plain(runs),
            Self::Code { lines, .. } => lines.join("\n"),
            Self::Table { head, rows } => {
                let mut cells: Vec<String> = head.iter().map(|c| plain(c)).collect();
                for row in rows {
                    cells.extend(row.iter().map(|c| plain(c)));
                }
                cells.join(" ")
            }
            Self::Rule => String::new(),
        }
    }
}

/// The text of `runs` without styles.
pub fn plain(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// The anchor of a heading text: lower case, letters, digits, `-` and `_` kept, spaces turned
/// into `-`, everything else dropped.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            out.extend(ch.to_lowercase());
        } else if ch == ' ' {
            out.push('-');
        }
    }
    out
}

/// Parses a document.
pub fn parse(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut para: Vec<String> = Vec::new();
    let mut quote: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        // A fenced code block.
        if let Some(fence) = fence_of(trimmed) {
            flush(&mut blocks, &mut para, &mut quote);
            let lang = trimmed[fence.len()..].trim().to_owned();
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(fence) {
                body.push(lines[i].trim_end().to_owned());
                i += 1;
            }
            i += 1;
            blocks.push(Block::Code { lang, lines: body });
            continue;
        }
        // An HTML comment, on one line or several.
        if trimmed.starts_with("<!--") {
            flush(&mut blocks, &mut para, &mut quote);
            while i < lines.len() && !lines[i].contains("-->") {
                i += 1;
            }
            i += 1;
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut blocks, &mut para, &mut quote);
            i += 1;
            continue;
        }
        if let Some((level, text)) = heading(trimmed) {
            flush(&mut blocks, &mut para, &mut quote);
            let runs = inline(text);
            let base = slug(&plain(&runs));
            let n = seen.entry(base.clone()).or_insert(0);
            let anchor = if *n == 0 { base } else { format!("{base}-{n}") };
            *n += 1;
            blocks.push(Block::Heading {
                level,
                runs,
                anchor,
            });
            i += 1;
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut blocks, &mut para, &mut quote);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if trimmed.starts_with('|') && i + 1 < lines.len() && is_separator(lines[i + 1].trim()) {
            flush(&mut blocks, &mut para, &mut quote);
            let head = cells(trimmed);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                let mut row = cells(lines[i].trim());
                row.resize(head.len(), Vec::new());
                rows.push(row);
                i += 1;
            }
            blocks.push(Block::Table { head, rows });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            if !para.is_empty() {
                flush(&mut blocks, &mut para, &mut quote);
            }
            quote.push(rest.trim().to_owned());
            i += 1;
            continue;
        }
        if let Some((depth, number, text)) = list_item(line) {
            flush(&mut blocks, &mut para, &mut quote);
            let mut body = text.to_owned();
            i += 1;
            // Wrapped lines of the item (indented, or lazy) until a blank line or a new block.
            while i < lines.len() {
                let next = lines[i].trim_end();
                let t = next.trim_start();
                if t.is_empty()
                    || list_item(next).is_some()
                    || heading(t).is_some()
                    || fence_of(t).is_some()
                    || t.starts_with('|')
                    || t.starts_with('>')
                    || is_rule(t)
                {
                    break;
                }
                body.push(' ');
                body.push_str(t);
                i += 1;
            }
            blocks.push(Block::Item {
                depth,
                number,
                runs: inline(&body),
            });
            continue;
        }
        if !quote.is_empty() {
            flush(&mut blocks, &mut para, &mut quote);
        }
        para.push(trimmed.to_owned());
        i += 1;
    }
    flush(&mut blocks, &mut para, &mut quote);
    blocks
}

fn flush(blocks: &mut Vec<Block>, para: &mut Vec<String>, quote: &mut Vec<String>) {
    if !para.is_empty() {
        blocks.push(Block::Paragraph(inline(&para.join(" "))));
        para.clear();
    }
    if !quote.is_empty() {
        blocks.push(Block::Quote(inline(&quote.join(" "))));
        quote.clear();
    }
}

fn fence_of(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn heading(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.starts_with(' ') && !rest.is_empty() {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim_end();
    Some((hashes as u8, text))
}

fn is_rule(trimmed: &str) -> bool {
    let mut chars = trimmed.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_')
        && trimmed.chars().filter(|c| !c.is_whitespace()).count() >= 3
        && trimmed.chars().all(|c| c == first || c.is_whitespace())
}

fn is_separator(trimmed: &str) -> bool {
    trimmed.starts_with('|')
        && trimmed.contains('-')
        && trimmed
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// A list item line: its depth, number (numbered lists) and text.
fn list_item(line: &str) -> Option<(u8, Option<u32>, &str)> {
    let indent: usize = line
        .chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    let t = line.trim_start();
    let depth = (indent / 2).min(6) as u8;
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(bullet) {
            // `* * *` is a rule, not an item.
            if is_rule(t) {
                return None;
            }
            return Some((depth, None, rest.trim()));
        }
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let (num, rest) = t.split_at(digits);
        if let Some(text) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((depth, num.parse().ok(), text.trim()));
        }
    }
    None
}

/// The cells of a table row, split at `|` that is not escaped.
fn cells(row: &str) -> Vec<Vec<Run>> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = if row.ends_with(r"\|") {
        row
    } else {
        row.strip_suffix('|').unwrap_or(row)
    };
    let mut out = Vec::new();
    let mut cell = String::new();
    let mut chars = row.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'|') {
            cell.push('|');
            chars.next();
        } else if ch == '|' {
            out.push(inline(cell.trim()));
            cell.clear();
        } else {
            cell.push(ch);
        }
    }
    out.push(inline(cell.trim()));
    out
}

// ---------------------------------------------------------------------------- inline

#[derive(Clone, Default)]
struct Style {
    bold: bool,
    italic: bool,
    link: Option<String>,
}

/// Parses inline Markdown into styled runs.
pub fn inline(text: &str) -> Vec<Run> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    inline_into(&chars, &Style::default(), &mut out, 0);
    // Merge neighbours of the same style.
    let mut merged: Vec<Run> = Vec::new();
    for run in out {
        if run.text.is_empty() {
            continue;
        }
        match merged.last_mut() {
            Some(last) if last.same_style(&run) => last.text.push_str(&run.text),
            _ => merged.push(run),
        }
    }
    merged
}

/// How deep emphasis and links may nest (a guard, not a feature).
const MAX_DEPTH: usize = 6;

fn push(out: &mut Vec<Run>, text: &str, style: &Style, code: bool) {
    out.push(Run {
        text: text.to_owned(),
        bold: style.bold,
        italic: style.italic,
        code,
        link: style.link.clone(),
    });
}

fn inline_into(chars: &[char], style: &Style, out: &mut Vec<Run>, depth: usize) {
    let mut plain = String::new();
    let mut i = 0;
    macro_rules! flush_plain {
        () => {
            if !plain.is_empty() {
                push(out, &plain, style, false);
                plain.clear();
            }
        };
    }
    while i < chars.len() {
        let ch = chars[i];
        let prev = if i == 0 { ' ' } else { chars[i - 1] };
        match ch {
            '\\' if i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() => {
                plain.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                let n = chars[i..].iter().take_while(|c| **c == '`').count();
                if let Some(end) = find_run(chars, i + n, '`', n) {
                    flush_plain!();
                    let inner: String = chars[i + n..end].iter().collect();
                    let inner = if inner.len() > 2 && inner.starts_with(' ') && inner.ends_with(' ')
                    {
                        inner[1..inner.len() - 1].to_owned()
                    } else {
                        inner
                    };
                    push(out, &inner, style, true);
                    i = end + n;
                } else {
                    plain.extend(std::iter::repeat_n('`', n));
                    i += n;
                }
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                if let Some((label, _target, next)) = link_at(chars, i + 1) {
                    flush_plain!();
                    let alt: String = label.iter().collect();
                    let shown = if alt.trim().is_empty() {
                        "[image]".to_owned()
                    } else {
                        format!("[image: {}]", alt.trim())
                    };
                    let mut s = style.clone();
                    s.italic = true;
                    push(out, &shown, &s, false);
                    i = next;
                } else {
                    plain.push(ch);
                    i += 1;
                }
            }
            '[' if depth < MAX_DEPTH => {
                if let Some((label, target, next)) = link_at(chars, i) {
                    flush_plain!();
                    let mut s = style.clone();
                    s.link = Some(target);
                    inline_into(label, &s, out, depth + 1);
                    i = next;
                } else {
                    plain.push(ch);
                    i += 1;
                }
            }
            '*' | '_' if depth < MAX_DEPTH => {
                let n = chars[i..].iter().take_while(|c| **c == ch).count();
                let marks = n.min(2);
                // `_` only opens a word: not inside one (snake_case stays text).
                let opens = ch == '*' || !prev.is_alphanumeric();
                let next_ok = chars.get(i + marks).is_some_and(|c| !c.is_whitespace());
                if opens
                    && next_ok
                    && let Some(end) = find_close(chars, i + marks, ch, marks)
                {
                    flush_plain!();
                    let mut s = style.clone();
                    if marks == 2 {
                        s.bold = true;
                    } else {
                        s.italic = true;
                    }
                    inline_into(&chars[i + marks..end], &s, out, depth + 1);
                    i = end + marks;
                } else {
                    plain.extend(std::iter::repeat_n(ch, n));
                    i += n;
                }
            }
            'h' if style.link.is_none()
                && (prev.is_whitespace() || "([".contains(prev))
                && (starts_with(chars, i, "http://") || starts_with(chars, i, "https://")) =>
            {
                let mut end = i;
                while end < chars.len() && !chars[end].is_whitespace() && chars[end] != ')' {
                    end += 1;
                }
                while end > i && ".,;:!?".contains(chars[end - 1]) {
                    end -= 1;
                }
                flush_plain!();
                let url: String = chars[i..end].iter().collect();
                let mut s = style.clone();
                s.link = Some(url.clone());
                push(out, &url, &s, false);
                i = end;
            }
            _ => {
                plain.push(ch);
                i += 1;
            }
        }
    }
    flush_plain!();
}

fn starts_with(chars: &[char], at: usize, word: &str) -> bool {
    word.chars()
        .enumerate()
        .all(|(k, w)| chars.get(at + k) == Some(&w))
}

/// The start of the next run of exactly `n` of `ch` at or after `from`.
fn find_run(chars: &[char], from: usize, ch: char, n: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == ch {
            let len = chars[i..].iter().take_while(|c| **c == ch).count();
            if len == n {
                return Some(i);
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}

/// The closing mark of emphasis: `marks` of `ch`, after a non-space, skipping code spans.
fn find_close(chars: &[char], from: usize, ch: char, marks: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            '`' => {
                let n = chars[i..].iter().take_while(|c| **c == '`').count();
                match find_run(chars, i + n, '`', n) {
                    Some(end) => i = end + n,
                    None => i += n,
                }
            }
            '\\' => i += 2,
            c if c == ch => {
                let len = chars[i..].iter().take_while(|k| **k == ch).count();
                let after = chars.get(i + len).copied().unwrap_or(' ');
                let closes = len >= marks
                    && !chars[i - 1].is_whitespace()
                    && (ch == '*' || !after.is_alphanumeric());
                if closes && i > from {
                    return Some(i + len - marks);
                }
                i += len;
            }
            _ => i += 1,
        }
    }
    None
}

/// A link `[label](target)` starting at `at` (the `[`): the label, the target and the index
/// after it.
fn link_at(chars: &[char], at: usize) -> Option<(&[char], String, usize)> {
    let mut depth = 0usize;
    let mut i = at;
    let close = loop {
        match chars.get(i)? {
            '\\' => i += 1,
            '`' => {
                let n = chars[i..].iter().take_while(|c| **c == '`').count();
                if let Some(end) = find_run(chars, i + n, '`', n) {
                    i = end + n - 1;
                } else {
                    i += n - 1;
                }
            }
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break i;
                }
            }
            _ => {}
        }
        i += 1;
    };
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut parens = 1usize;
    let mut j = close + 2;
    while j < chars.len() {
        match chars[j] {
            '(' => parens += 1,
            ')' => {
                parens -= 1;
                if parens == 0 {
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    if j >= chars.len() {
        return None;
    }
    let raw: String = chars[close + 2..j].iter().collect();
    // An optional title after the address is dropped.
    let target = raw.split_whitespace().next().unwrap_or("").to_owned();
    if target.is_empty() {
        return None;
    }
    Some((&chars[at + 1..close], target, j + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Vec<Run> {
        inline(text)
    }

    #[test]
    fn emphasis_code_and_links_become_styled_runs() {
        let runs = p("a **bold _and italic_** b *it* `co*de` [go](#x) end");
        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "a ",
                "bold ",
                "and italic",
                " b ",
                "it",
                " ",
                "co*de",
                " ",
                "go",
                " end"
            ]
        );
        assert!(runs[1].bold && !runs[1].italic);
        assert!(runs[2].bold && runs[2].italic);
        assert!(runs[4].italic);
        assert!(runs[6].code);
        assert_eq!(runs[8].link.as_deref(), Some("#x"));
    }

    #[test]
    fn snake_case_stars_and_unclosed_marks_stay_text() {
        assert_eq!(
            plain(&p("a snake_case_name and 2 * 3 * 4")),
            "a snake_case_name and 2 * 3 * 4"
        );
        assert_eq!(plain(&p("**open and `tick")), "**open and `tick");
        assert_eq!(plain(&p(r"\*not\* \[x\](y)")), "*not* [x](y)");
    }

    #[test]
    fn a_link_label_may_hold_code_and_an_address_may_have_parentheses() {
        let runs = p("see [`fn x`](a_(b).md#c) now");
        assert_eq!(runs[1].text, "fn x");
        assert!(runs[1].code);
        assert_eq!(runs[1].link.as_deref(), Some("a_(b).md#c"));
        let runs = p("go to https://example.org/docs, or not");
        assert_eq!(runs[1].link.as_deref(), Some("https://example.org/docs"));
        assert_eq!(runs[2].text, ", or not");
    }

    #[test]
    fn an_image_is_its_alt_text() {
        assert_eq!(
            plain(&p("![the rack](img/rack.png) here")),
            "[image: the rack] here"
        );
        assert_eq!(plain(&p("![](a.png)")), "[image]");
    }

    #[test]
    fn slugs_follow_the_github_rule() {
        assert_eq!(slug("The main window"), "the-main-window");
        assert_eq!(slug("Key and chords"), "key-and-chords");
        assert_eq!(
            slug("Poly Synth: `atomik.poly-synth` (Instrument)"),
            "poly-synth-atomikpoly-synth-instrument"
        );
        assert_eq!(slug("A_b - c"), "a_b---c");
    }

    #[test]
    fn nothing_panics_on_odd_input() {
        for text in [
            "", "[", "[]", "[](", "![", "**", "*", "_", "`", "``", "[a](b", "[[[[", "\\", "|",
            "| a |",
        ] {
            let _ = parse(text);
            let _ = inline(text);
        }
    }
}
