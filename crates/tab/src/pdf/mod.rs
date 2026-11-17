//! Tab and chord sheets as PDF.
//!
//! [`write_tab_pdf`] prints a [`TabScore`]: a header (title, artist, credits), then for every
//! fretted track its tuning and capo, a legend of the chords it uses, and systems of bars that fill
//! the page width. Each system is a tab staff with one line per string (a banjo's drone and a bass's
//! four, a seven- or eight-string guitar's extra lines included), fret numbers, the playing
//! techniques the editor shows (bends with their amount, slides, hammer-ons and pull-offs, harmonics,
//! palm mute and let ring spans, dead and ghost notes, vibrato, tapping and the rest), rhythm stems
//! and beams under the staff, bar numbers, time signature, tempo, section markers, chord names and
//! diagrams above and lyrics below. Pages break between systems and carry a page number.
//!
//! [`write_sheet_pdf`] prints a [`ChordSheet`]: chords over lyrics (each chord anchored over the
//! character it belongs to and pushed along when two would touch, like the editor's pane), section
//! headings, comments and tab blocks, with long lines wrapped at a word and the chord kept over the
//! same word, and optionally a strip of chord diagrams for an instrument at the top.
//!
//! Both are two steps. The layout is pure and fills [`draw::Page`]s with drawing operations (see
//! [`layout_tab`] and [`layout_sheet`]), which tests read without a PDF parser; [`draw::render`]
//! writes them with the `pdf-writer` crate. The output is deterministic: no date, no random id, the
//! numbers rounded, so the same input gives the same bytes.
//!
//! The text is set in the standard Helvetica and Helvetica-Bold fonts every viewer has (nothing is
//! embedded, see [`font`]): characters outside Latin-1 and the usual typographic punctuation print as
//! `?` and the result says how many did.

pub mod diagram;
pub mod draw;
pub mod font;
mod sheet;
mod tab;

use crate::score::TabScore;
use crate::sheet::ChordSheet;
use draw::{Anchor, DocInfo, Ink, Page};
use font::{Font, encode_char, text_width};

pub use sheet::{SheetDiagrams, SheetPdfOptions, layout_sheet};
pub use tab::{TabPdfOptions, layout_tab};

/// The most pages one file gets.
pub const MAX_PAGES: usize = 400;

/// The paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PageSize {
    /// 210 x 297 mm.
    #[default]
    A4,
    /// 8.5 x 11 inches.
    Letter,
}

impl PageSize {
    /// Width and height in points (1/72 inch).
    pub fn points(self) -> (f32, f32) {
        match self {
            PageSize::A4 => (595.28, 841.89),
            PageSize::Letter => (612.0, 792.0),
        }
    }

    /// The paper a language is usually printed on: Letter for the United States, Canada and a few
    /// others, A4 elsewhere. `region` is the country part of a locale (`US` in `en-US`).
    pub fn for_region(region: &str) -> PageSize {
        match region.to_ascii_uppercase().as_str() {
            "US" | "CA" | "MX" | "CL" | "CO" | "VE" | "PH" => PageSize::Letter,
            _ => PageSize::A4,
        }
    }

    /// The paper for a locale such as `en-US`, `fr_CA.UTF-8` or `fr` (A4 when it has no region).
    pub fn for_locale(locale: &str) -> PageSize {
        let region: String = locale
            .split(['-', '_'])
            .nth(1)
            .unwrap_or("")
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        PageSize::for_region(&region)
    }
}

/// Why a printout could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    /// There is nothing to print.
    #[error("there is nothing to print: {0}")]
    Empty(&'static str),
}

/// The finished layout: the pages and what had to be changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The page size in points.
    pub size: (f32, f32),
    /// The pages.
    pub pages: Vec<Page>,
    /// What the printout could not keep, for the user.
    pub warnings: Vec<String>,
    /// The document title.
    pub title: String,
    /// The author.
    pub author: String,
}

impl Layout {
    /// Every text of every page in painting order.
    pub fn texts(&self) -> Vec<&str> {
        self.pages.iter().flat_map(Page::texts).collect()
    }

    /// Writes the layout as a PDF file (content streams deflated).
    pub fn to_pdf(&self) -> Vec<u8> {
        self.render(true)
    }

    /// Writes the layout as a PDF file with plain content streams: larger, readable, and the form the
    /// golden files keep.
    pub fn to_pdf_plain(&self) -> Vec<u8> {
        self.render(false)
    }

    fn render(&self, compress: bool) -> Vec<u8> {
        draw::render(
            self.size,
            &self.pages,
            &DocInfo {
                title: self.title.clone(),
                author: self.author.clone(),
            },
            compress,
        )
    }
}

/// Prints a tab score as PDF: the file and the warnings.
pub fn write_tab_pdf(
    score: &TabScore,
    opts: &TabPdfOptions,
) -> Result<(Vec<u8>, Vec<String>), PdfError> {
    let layout = layout_tab(score, opts)?;
    Ok((layout.to_pdf(), layout.warnings))
}

/// Prints a chord sheet as PDF: the file and the warnings.
pub fn write_sheet_pdf(
    sheet: &ChordSheet,
    opts: &SheetPdfOptions,
) -> Result<(Vec<u8>, Vec<String>), PdfError> {
    let layout = layout_sheet(sheet, opts)?;
    Ok((layout.to_pdf(), layout.warnings))
}

/// Space kept at the foot of every page for the footer line.
const FOOTER: f32 = 18.0;

/// Fills pages from the top down and breaks between blocks.
pub(crate) struct Pager {
    pub size: (f32, f32),
    pub margin: f32,
    pub pages: Vec<Page>,
    /// The next free y on the current page.
    pub y: f32,
    /// Characters the font cannot print.
    pub lost: usize,
    /// The page limit was reached: nothing more is drawn.
    pub full: bool,
}

impl Pager {
    pub fn new(size: (f32, f32), margin: f32) -> Self {
        Self {
            size,
            margin,
            pages: vec![Page::default()],
            y: margin,
            lost: 0,
            full: false,
        }
    }

    pub fn left(&self) -> f32 {
        self.margin
    }

    pub fn right(&self) -> f32 {
        self.size.0 - self.margin
    }

    pub fn width(&self) -> f32 {
        self.size.0 - 2.0 * self.margin
    }

    pub fn bottom(&self) -> f32 {
        self.size.1 - self.margin - FOOTER
    }

    /// The current page.
    pub fn page(&mut self) -> &mut Page {
        if self.pages.is_empty() {
            self.pages.push(Page::default());
        }
        let last = self.pages.len() - 1;
        &mut self.pages[last]
    }

    /// Starts a new page (at the page limit, sets `full` instead).
    pub fn break_page(&mut self) {
        if self.pages.len() >= MAX_PAGES {
            self.full = true;
            return;
        }
        self.pages.push(Page::default());
        self.y = self.margin;
    }

    /// Makes room for a block `h` points high: breaks the page when it does not fit (and the page
    /// is not empty). Returns true when it broke.
    pub fn ensure(&mut self, h: f32) -> bool {
        if self.y + h > self.bottom() && self.y > self.margin + 0.5 {
            self.break_page();
            return true;
        }
        false
    }

    /// Draws a text, counting the characters the font cannot print.
    #[allow(clippy::too_many_arguments, reason = "a text run has this many facts")]
    pub fn text(
        &mut self,
        x: f32,
        y: f32,
        size: f32,
        font: Font,
        ink: Ink,
        anchor: Anchor,
        text: &str,
    ) {
        self.lost += lost_chars(text);
        self.page().text(x, y, size, font, ink, anchor, text);
    }

    /// Adds the footers and returns the layout.
    pub fn finish(mut self, title: &str, author: &str, mut warnings: Vec<String>) -> Layout {
        if self.lost > 0 {
            warnings.push(format!(
                "{} character{} the PDF fonts cannot print {} replaced by ?",
                self.lost,
                if self.lost == 1 { "" } else { "s" },
                if self.lost == 1 { "was" } else { "were" }
            ));
        }
        if self.full {
            warnings.push(format!("only the first {MAX_PAGES} pages were written"));
        }
        let total = self.pages.len();
        let (w, h) = self.size;
        let (left, right) = (self.margin, w - self.margin);
        let base = h - self.margin + 2.0;
        for (i, p) in self.pages.iter_mut().enumerate() {
            p.line((left, base - 10.0), (right, base - 10.0), 0.4, Ink::Light);
            let mut name: String = title.chars().take(70).collect();
            if name.is_empty() {
                name.push_str("Atomik Rack");
            }
            p.text(
                left,
                base,
                7.5,
                Font::Regular,
                Ink::Mid,
                Anchor::Start,
                &name,
            );
            p.text(
                right,
                base,
                7.5,
                Font::Regular,
                Ink::Mid,
                Anchor::End,
                &format!("{} / {}", i + 1, total),
            );
        }
        Layout {
            size: self.size,
            pages: self.pages,
            warnings,
            title: title.to_owned(),
            author: author.to_owned(),
        }
    }
}

/// The characters of `text` the font prints as `?` although they are not one.
pub(crate) fn lost_chars(text: &str) -> usize {
    text.chars()
        .filter(|c| *c != '?' && encode_char(*c) == b'?')
        .count()
}

/// Breaks `text` into lines no wider than `max` at spaces (a single word wider than the line stays
/// whole).
pub(crate) fn wrap(font: Font, size: f32, text: &str, max: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if line.is_empty() || text_width(font, size, &candidate) <= max {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_owned();
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_spaces_and_keeps_long_words() {
        let lines = wrap(Font::Regular, 10.0, "aaa bbb ccc ddd", 40.0);
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|l| !l.is_empty()));
        let long = wrap(Font::Regular, 10.0, "supercalifragilistic x", 20.0);
        assert_eq!(long[0], "supercalifragilistic");
    }

    #[test]
    fn paper_follows_the_region() {
        assert_eq!(PageSize::for_region("us"), PageSize::Letter);
        assert_eq!(PageSize::for_region("FR"), PageSize::A4);
        assert_eq!(PageSize::for_locale("en-US"), PageSize::Letter);
        assert_eq!(PageSize::for_locale("fr_CA.UTF-8"), PageSize::Letter);
        assert_eq!(PageSize::for_locale("fr-FR"), PageSize::A4);
        assert_eq!(PageSize::for_locale("fr"), PageSize::A4);
        assert_eq!(PageSize::A4.points().0.round(), 595.0);
    }

    #[test]
    fn pager_breaks_when_a_block_does_not_fit() {
        let mut p = Pager::new((200.0, 200.0), 20.0);
        assert!(!p.ensure(50.0));
        p.y = 120.0;
        assert!(p.ensure(50.0));
        assert_eq!(p.pages.len(), 2);
        assert_eq!(p.y, 20.0);
    }
}
