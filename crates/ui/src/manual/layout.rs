//! Laying a manual page out: blocks to positioned text, fills and lines for one width.
//!
//! The layout is a flat list of drawing operations in page coordinates (x from the left edge of
//! the text column, y from the top of the page) plus the boxes of the links and the y of every
//! heading, so the view can paint any scroll position without measuring again. Text is measured
//! on the canvas it will be painted on ([`crate::paint::measure`]), so wrapping follows the glyphs
//! actually drawn.

use super::markdown::{Block, Run};
use crate::canvas::Canvas;
use crate::paint::{fit_label_weight, measure};
use std::collections::BTreeMap;
use vello::kurbo::Rect;

/// Body text size.
pub(super) const BODY: f32 = 12.0;
/// Height of one body line.
const LINE: f64 = 17.0;
const CODE_SIZE: f32 = 10.5;
const CODE_LINE: f64 = 15.0;
const CELL_PAD: f64 = 6.0;
const CELL_VPAD: f64 = 4.0;
/// The most a code span's box is padded on each side.
const SPAN_PAD: f64 = 3.0;
const INDENT: f64 = 18.0;

/// Which theme token a drawing operation uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Ink,
    Muted,
    Heading,
    Link,
    Code,
    Italic,
    Marker,
    CodeBack,
    HeadBack,
    Rule,
}

/// One drawing operation in page coordinates.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Draw {
    /// Text with its left edge at `x` and its baseline at `y`.
    Text {
        x: f64,
        y: f64,
        text: String,
        size: f32,
        weight: u16,
        tone: Tone,
    },
    /// A filled box.
    Fill { rect: Rect, tone: Tone },
    /// A hairline.
    Line {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        tone: Tone,
    },
}

impl Draw {
    /// The top and bottom y of the operation (for culling).
    pub(super) fn span(&self) -> (f64, f64) {
        match self {
            Self::Text { y, size, .. } => (*y - f64::from(*size), *y + 4.0),
            Self::Fill { rect, .. } => (rect.y0, rect.y1),
            Self::Line { y0, y1, .. } => (y0.min(*y1), y0.max(*y1)),
        }
    }

    fn moved(&self, dx: f64, dy: f64) -> Self {
        match self {
            Self::Text {
                x,
                y,
                text,
                size,
                weight,
                tone,
            } => Self::Text {
                x: x + dx,
                y: y + dy,
                text: text.clone(),
                size: *size,
                weight: *weight,
                tone: *tone,
            },
            Self::Fill { rect, tone } => Self::Fill {
                rect: Rect::new(rect.x0 + dx, rect.y0 + dy, rect.x1 + dx, rect.y1 + dy),
                tone: *tone,
            },
            Self::Line {
                x0,
                y0,
                x1,
                y1,
                tone,
            } => Self::Line {
                x0: x0 + dx,
                y0: y0 + dy,
                x1: x1 + dx,
                y1: y1 + dy,
                tone: *tone,
            },
        }
    }
}

/// The box of a link in page coordinates and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LinkBox {
    pub rect: Rect,
    pub target: String,
}

/// A laid-out page.
#[derive(Debug, Clone, Default)]
pub(super) struct PageLayout {
    /// The page this is the layout of.
    pub page: usize,
    /// The text column width it was laid out for.
    pub width: f64,
    /// Total height.
    pub height: f64,
    pub draws: Vec<Draw>,
    pub links: Vec<LinkBox>,
    /// The y of the top of each heading, by the heading's block index.
    pub headings: BTreeMap<usize, f64>,
}

#[derive(Default)]
struct Out {
    draws: Vec<Draw>,
    links: Vec<LinkBox>,
}

impl Out {
    fn append(&mut self, other: Self, dx: f64, dy: f64) {
        self.draws
            .extend(other.draws.iter().map(|d| d.moved(dx, dy)));
        self.links.extend(other.links.into_iter().map(|l| LinkBox {
            rect: Rect::new(
                l.rect.x0 + dx,
                l.rect.y0 + dy,
                l.rect.x1 + dx,
                l.rect.y1 + dy,
            ),
            target: l.target,
        }));
    }
}

/// Lays `blocks` out for a text column `width` wide.
pub(super) fn layout_page(
    c: &mut dyn Canvas,
    page: usize,
    blocks: &[Block],
    width: f64,
) -> PageLayout {
    let width = width.max(80.0);
    let mut out = Out::default();
    let mut headings = BTreeMap::new();
    let mut y = 4.0;
    for (i, block) in blocks.iter().enumerate() {
        let next_is_item = matches!(blocks.get(i + 1), Some(Block::Item { .. }));
        match block {
            Block::Heading { level, runs, .. } => {
                let size = heading_size(*level);
                if i > 0 {
                    y += if *level <= 1 { 4.0 } else { 12.0 };
                }
                headings.insert(i, y);
                let line = f64::from(size) * 1.35;
                let h = flow(
                    c,
                    runs,
                    (0.0, y),
                    width,
                    size,
                    line,
                    700,
                    Tone::Heading,
                    &mut out,
                );
                y += h;
                if *level <= 2 {
                    out.draws.push(Draw::Line {
                        x0: 0.0,
                        y0: y + 2.5,
                        x1: width,
                        y1: y + 2.5,
                        tone: Tone::Rule,
                    });
                    y += 4.0;
                }
                y += 6.0;
            }
            Block::Paragraph(runs) => {
                y += flow(
                    c,
                    runs,
                    (0.0, y),
                    width,
                    BODY,
                    LINE,
                    400,
                    Tone::Ink,
                    &mut out,
                ) + 9.0;
            }
            Block::Item {
                depth,
                number,
                runs,
            } => {
                let at = f64::from(*depth) * INDENT;
                let marker = match number {
                    Some(n) => format!("{n}."),
                    None => "\u{2022}".to_owned(),
                };
                out.draws.push(Draw::Text {
                    x: at + 4.0,
                    y: y + LINE / 2.0 + f64::from(BODY) * 0.36,
                    text: marker,
                    size: BODY,
                    weight: 400,
                    tone: Tone::Marker,
                });
                let h = flow(
                    c,
                    runs,
                    (at + 24.0, y),
                    width - at - 24.0,
                    BODY,
                    LINE,
                    400,
                    Tone::Ink,
                    &mut out,
                );
                y += h + if next_is_item { 3.0 } else { 9.0 };
            }
            Block::Code { lines, .. } => {
                let h = lines.len() as f64 * CODE_LINE + 12.0;
                out.draws.push(Draw::Fill {
                    rect: Rect::new(0.0, y, width, y + h),
                    tone: Tone::CodeBack,
                });
                for (k, line) in lines.iter().enumerate() {
                    let shown = fit_label_weight(c, line, width - 16.0, CODE_SIZE, 400);
                    out.draws.push(Draw::Text {
                        x: 8.0,
                        y: y + 6.0
                            + k as f64 * CODE_LINE
                            + CODE_LINE / 2.0
                            + f64::from(CODE_SIZE) * 0.36,
                        text: shown,
                        size: CODE_SIZE,
                        weight: 400,
                        tone: Tone::Code,
                    });
                }
                y += h + 10.0;
            }
            Block::Table { head, rows } => {
                y += table(c, head, rows, y, width, &mut out) + 10.0;
            }
            Block::Quote(runs) => {
                let h = flow(
                    c,
                    runs,
                    (14.0, y),
                    width - 14.0,
                    BODY,
                    LINE,
                    400,
                    Tone::Muted,
                    &mut out,
                );
                out.draws.push(Draw::Fill {
                    rect: Rect::new(2.0, y, 5.0, y + h),
                    tone: Tone::Marker,
                });
                y += h + 9.0;
            }
            Block::Rule => {
                out.draws.push(Draw::Line {
                    x0: 0.0,
                    y0: y + 4.5,
                    x1: width,
                    y1: y + 4.5,
                    tone: Tone::Rule,
                });
                y += 12.0;
            }
        }
    }
    PageLayout {
        page,
        width,
        height: y + 12.0,
        draws: out.draws,
        links: out.links,
        headings,
    }
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 20.0,
        2 => 16.0,
        3 => 13.5,
        _ => 12.5,
    }
}

// ---------------------------------------------------------------------------- tables

fn table(
    c: &mut dyn Canvas,
    head: &[Vec<Run>],
    rows: &[Vec<Vec<Run>>],
    y: f64,
    width: f64,
    out: &mut Out,
) -> f64 {
    let n = head.len().max(1);
    // The natural (one line) width of every column.
    let mut natural = vec![0.0f64; n];
    for (k, cell) in head.iter().enumerate() {
        natural[k] = natural[k].max(line_width(c, cell, 700) + 2.0 * CELL_PAD);
    }
    for row in rows {
        for (k, cell) in row.iter().enumerate().take(n) {
            natural[k] = natural[k].max(line_width(c, cell, 400) + 2.0 * CELL_PAD);
        }
    }
    let widths = column_widths(&natural, width);
    let mut total = 0.0;
    let place = |c: &mut dyn Canvas, cells: &[Vec<Run>], header: bool, top: f64, out: &mut Out| {
        let mut parts: Vec<(f64, f64, Out)> = Vec::new();
        let mut x = 0.0;
        let mut height = LINE;
        for (k, w) in widths.iter().enumerate() {
            let runs: &[Run] = cells.get(k).map_or(&[], Vec::as_slice);
            let mut cell = Out::default();
            let h = flow(
                c,
                runs,
                (0.0, 0.0),
                (w - 2.0 * CELL_PAD).max(20.0),
                BODY,
                LINE,
                if header { 700 } else { 400 },
                Tone::Ink,
                &mut cell,
            );
            height = height.max(h);
            parts.push((x + CELL_PAD, h, cell));
            x += w;
        }
        let row_h = height + 2.0 * CELL_VPAD;
        if header {
            out.draws.push(Draw::Fill {
                rect: Rect::new(0.0, top, widths.iter().sum::<f64>(), top + row_h),
                tone: Tone::HeadBack,
            });
        }
        for (x, _h, cell) in parts {
            out.append(cell, x, top + CELL_VPAD);
        }
        out.draws.push(Draw::Line {
            x0: 0.0,
            y0: top + row_h,
            x1: widths.iter().sum(),
            y1: top + row_h,
            tone: Tone::Rule,
        });
        row_h
    };
    total += place(c, head, true, y, out);
    for row in rows {
        total += place(c, row, false, y + total, out);
    }
    total
}

/// Column widths for `natural` widths in `avail`: a column that fits an equal share keeps its
/// natural width, the others share what is left in proportion to theirs.
fn column_widths(natural: &[f64], avail: f64) -> Vec<f64> {
    let n = natural.len();
    if natural.iter().sum::<f64>() <= avail {
        return natural.to_vec();
    }
    let mut width = vec![0.0; n];
    let mut open: Vec<usize> = (0..n).collect();
    let mut left = avail;
    loop {
        let share = left / open.len() as f64;
        let fits: Vec<usize> = open
            .iter()
            .copied()
            .filter(|&k| natural[k] <= share)
            .collect();
        if fits.is_empty() {
            break;
        }
        for k in &fits {
            width[*k] = natural[*k];
            left -= natural[*k];
        }
        open.retain(|k| !fits.contains(k));
        if open.is_empty() {
            return width;
        }
    }
    let sum: f64 = open.iter().map(|&k| natural[k]).sum();
    for &k in &open {
        width[k] = (left * natural[k] / sum).max(40.0);
    }
    width
}

fn line_width(c: &mut dyn Canvas, runs: &[Run], weight: u16) -> f64 {
    runs.iter()
        .map(|r| {
            let w = if r.bold { 700 } else { weight };
            measure(c, &r.text, BODY, w) + if r.code { 2.0 * SPAN_PAD } else { 0.0 }
        })
        .sum()
}

// ---------------------------------------------------------------------------- text flow

struct Word {
    text: String,
    run: usize,
    space_before: bool,
}

struct Seg {
    text: String,
    run: usize,
    x: f64,
    w: f64,
}

/// Wraps `runs` into lines of `width` starting at `at` (left edge, top). Returns the height.
#[allow(clippy::too_many_arguments)]
fn flow(
    c: &mut dyn Canvas,
    runs: &[Run],
    at: (f64, f64),
    width: f64,
    size: f32,
    line_h: f64,
    base_weight: u16,
    base_tone: Tone,
    out: &mut Out,
) -> f64 {
    let (x0, y0) = at;
    let words = words(runs);
    if words.is_empty() {
        return 0.0;
    }
    let space = measure(c, " ", size, 400);
    let weight_of = |run: usize| if runs[run].bold { 700 } else { base_weight };
    let mut lines = 0usize;
    let mut segs: Vec<Seg> = Vec::new();
    let mut cursor = 0.0f64;
    let finish = |segs: &mut Vec<Seg>, lines: &mut usize, out: &mut Out| {
        let top = y0 + *lines as f64 * line_h;
        for seg in segs.drain(..) {
            let run = &runs[seg.run];
            let baseline = top + line_h / 2.0 + f64::from(size) * 0.36;
            let tone = if run.link.is_some() {
                Tone::Link
            } else if run.code {
                Tone::Code
            } else if run.italic {
                Tone::Italic
            } else {
                base_tone
            };
            if run.code {
                out.draws.push(Draw::Fill {
                    rect: Rect::new(
                        x0 + seg.x - SPAN_PAD,
                        top + 1.0,
                        x0 + seg.x + seg.w + SPAN_PAD,
                        top + line_h - 1.0,
                    ),
                    tone: Tone::CodeBack,
                });
            }
            if let Some(target) = &run.link {
                out.draws.push(Draw::Line {
                    x0: x0 + seg.x,
                    y0: baseline + 2.0,
                    x1: x0 + seg.x + seg.w,
                    y1: baseline + 2.0,
                    tone: Tone::Link,
                });
                out.links.push(LinkBox {
                    rect: Rect::new(x0 + seg.x, top, x0 + seg.x + seg.w, top + line_h),
                    target: target.clone(),
                });
            }
            out.draws.push(Draw::Text {
                x: x0 + seg.x,
                y: baseline,
                text: seg.text,
                size,
                weight: if run.bold { 700 } else { base_weight },
                tone,
            });
        }
        *lines += 1;
    };
    for word in words {
        let run = &runs[word.run];
        let weight = weight_of(word.run);
        let pad = if run.code { SPAN_PAD } else { 0.0 };
        let gap = if word.space_before && !segs.is_empty() {
            space
        } else {
            0.0
        };
        // Joins the segment before it when that has the same style and a space between.
        let joins = gap > 0.0 && segs.last().is_some_and(|s| s.run == word.run);
        let (text, x, w) = if joins {
            let last = segs.last().map_or(0.0, |s| s.x);
            let joined = format!(
                "{} {}",
                segs.last().map_or("", |s| s.text.as_str()),
                word.text
            );
            let w = measure(c, &joined, size, weight);
            (joined, last, w)
        } else {
            let w = measure(c, &word.text, size, weight);
            (word.text.clone(), cursor + gap + pad, w)
        };
        let end = x + w + pad;
        if end > width && !segs.is_empty() {
            finish(&mut segs, &mut lines, out);
            let w = measure(c, &word.text, size, weight);
            let (text, w) = clip_word(c, word.text, w, width - 2.0 * pad, size, weight);
            segs.push(Seg {
                text,
                run: word.run,
                x: pad,
                w,
            });
            cursor = pad + w + pad;
            continue;
        }
        if joins {
            segs.pop();
        }
        let (text, w) = if segs.is_empty() && end > width {
            clip_word(c, text, w, width - 2.0 * pad, size, weight)
        } else {
            (text, w)
        };
        segs.push(Seg {
            text,
            run: word.run,
            x,
            w,
        });
        cursor = x + w + pad;
    }
    finish(&mut segs, &mut lines, out);
    lines as f64 * line_h
}

/// A word wider than a whole line is cut to it.
fn clip_word(
    c: &mut dyn Canvas,
    text: String,
    w: f64,
    width: f64,
    size: f32,
    weight: u16,
) -> (String, f64) {
    if w <= width {
        return (text, w);
    }
    let cut = fit_label_weight(c, &text, width, size, weight);
    let w = measure(c, &cut, size, weight);
    (cut, w)
}

/// The words of `runs`, each with its run and whether a space comes before it.
fn words(runs: &[Run]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut space = false;
    for (k, run) in runs.iter().enumerate() {
        let mut word = String::new();
        let mut pending = space;
        for ch in run.text.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    out.push(Word {
                        text: std::mem::take(&mut word),
                        run: k,
                        space_before: pending,
                    });
                }
                pending = true;
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            out.push(Word {
                text: word,
                run: k,
                space_before: pending,
            });
            space = false;
        } else {
            space = pending;
        }
    }
    out
}
