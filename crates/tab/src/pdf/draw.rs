//! The drawing list the layouts produce and the writer that turns it into a PDF.
//!
//! A layout is pure: it fills [`Page`]s with [`Op`]s in points, with `y` counted **down** from the
//! top edge of the page (the PDF counts up; the writer flips). Tests read the list, the writer
//! turns it into content streams. The file has no timestamp and no random id, and the numbers are
//! rounded to a hundredth of a point, so the same pages always give the same bytes.

use super::font::{Font, encode, text_width};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use pdf_writer::Filter;
use pdf_writer::types::LineCapStyle;
use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str, TextStr};
use std::io::Write;

/// The grey levels of the printout (it is made for paper: no colour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    /// Black.
    Black,
    /// Dark grey (secondary text).
    Dark,
    /// Mid grey (bar numbers, footers).
    Mid,
    /// Light grey (thin helper lines).
    Light,
    /// White (the patch behind a fret number that cuts the line).
    White,
}

impl Ink {
    fn gray(self) -> f32 {
        match self {
            Ink::Black => 0.0,
            Ink::Dark => 0.25,
            Ink::Mid => 0.45,
            Ink::Light => 0.75,
            Ink::White => 1.0,
        }
    }
}

/// Where a text sits relative to its x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The text starts at x.
    Start,
    /// The text is centred on x.
    Middle,
    /// The text ends at x.
    End,
}

/// One drawing operation.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A straight line.
    Line {
        /// Start.
        a: (f32, f32),
        /// End.
        b: (f32, f32),
        /// Width in points.
        width: f32,
        /// Colour.
        ink: Ink,
        /// Dash length (0 = solid).
        dash: f32,
        /// Round ends (a barre).
        round: bool,
    },
    /// A rectangle, filled and/or stroked.
    Rect {
        /// Left.
        x: f32,
        /// Top.
        y: f32,
        /// Width.
        w: f32,
        /// Height.
        h: f32,
        /// Fill colour.
        fill: Option<Ink>,
        /// Outline width and colour.
        stroke: Option<(f32, Ink)>,
    },
    /// A circle.
    Circle {
        /// Centre.
        c: (f32, f32),
        /// Radius.
        r: f32,
        /// Filled instead of stroked.
        fill: bool,
        /// Stroke width.
        width: f32,
        /// Colour.
        ink: Ink,
    },
    /// A quadratic curve (slurs, bend arrows).
    Curve {
        /// Start.
        a: (f32, f32),
        /// Control point.
        ctrl: (f32, f32),
        /// End.
        b: (f32, f32),
        /// Stroke width.
        width: f32,
        /// Colour.
        ink: Ink,
    },
    /// A filled polygon (arrow heads).
    Poly {
        /// The corners.
        pts: Vec<(f32, f32)>,
        /// Colour.
        ink: Ink,
    },
    /// A text on a baseline.
    Text {
        /// x of the anchor.
        x: f32,
        /// y of the baseline.
        y: f32,
        /// Size in points.
        size: f32,
        /// The font.
        font: Font,
        /// Colour.
        ink: Ink,
        /// The anchor.
        anchor: Anchor,
        /// The text.
        text: String,
    },
}

/// One page of drawing operations.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    /// The operations, in painting order.
    pub ops: Vec<Op>,
}

impl Page {
    /// A solid line.
    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, ink: Ink) {
        self.ops.push(Op::Line {
            a,
            b,
            width,
            ink,
            dash: 0.0,
            round: false,
        });
    }

    /// A dashed line.
    pub fn dashed(&mut self, a: (f32, f32), b: (f32, f32), width: f32, ink: Ink, dash: f32) {
        self.ops.push(Op::Line {
            a,
            b,
            width,
            ink,
            dash,
            round: false,
        });
    }

    /// A line with round ends.
    pub fn round_line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, ink: Ink) {
        self.ops.push(Op::Line {
            a,
            b,
            width,
            ink,
            dash: 0.0,
            round: true,
        });
    }

    /// A filled rectangle.
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, ink: Ink) {
        self.ops.push(Op::Rect {
            x,
            y,
            w,
            h,
            fill: Some(ink),
            stroke: None,
        });
    }

    /// A filled circle.
    pub fn dot(&mut self, c: (f32, f32), r: f32, ink: Ink) {
        self.ops.push(Op::Circle {
            c,
            r,
            fill: true,
            width: 0.0,
            ink,
        });
    }

    /// A stroked circle.
    pub fn ring(&mut self, c: (f32, f32), r: f32, width: f32, ink: Ink) {
        self.ops.push(Op::Circle {
            c,
            r,
            fill: false,
            width,
            ink,
        });
    }

    /// A curve through one control point.
    pub fn curve(&mut self, a: (f32, f32), ctrl: (f32, f32), b: (f32, f32), width: f32, ink: Ink) {
        self.ops.push(Op::Curve {
            a,
            ctrl,
            b,
            width,
            ink,
        });
    }

    /// A filled triangle.
    pub fn triangle(&mut self, p: [(f32, f32); 3], ink: Ink) {
        self.ops.push(Op::Poly {
            pts: p.to_vec(),
            ink,
        });
    }

    /// A text on a baseline.
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
        if text.is_empty() {
            return;
        }
        self.ops.push(Op::Text {
            x,
            y,
            size,
            font,
            ink,
            anchor,
            text: text.to_owned(),
        });
    }

    /// The texts of the page in painting order.
    pub fn texts(&self) -> Vec<&str> {
        self.ops
            .iter()
            .filter_map(|o| match o {
                Op::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// The facts of the document information dictionary.
#[derive(Debug, Clone, Default)]
pub struct DocInfo {
    /// The title.
    pub title: String,
    /// The author.
    pub author: String,
}

fn r2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn paint(content: &mut Content, ink: Ink, stroke: bool) {
    if stroke {
        content.set_stroke_gray(ink.gray());
    } else {
        content.set_fill_gray(ink.gray());
    }
}

/// Writes `pages` (each `size` points wide and high) as a PDF, with the content streams deflated
/// when `compress` is set (a plain file is for tests and for reading by eye).
pub fn render(size: (f32, f32), pages: &[Page], info: &DocInfo, compress: bool) -> Vec<u8> {
    let catalog = Ref::new(1);
    let tree = Ref::new(2);
    let regular = Ref::new(3);
    let bold = Ref::new(4);
    let mono = Ref::new(5);
    let info_ref = Ref::new(6);
    let page_ref = |i: usize| Ref::new(7 + 2 * i as i32);
    let content_ref = |i: usize| Ref::new(8 + 2 * i as i32);

    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids((0..pages.len()).map(page_ref))
        .count(pages.len() as i32);
    for (r, font) in [
        (regular, Font::Regular),
        (bold, Font::Bold),
        (mono, Font::Mono),
    ] {
        pdf.type1_font(r)
            .base_font(Name(font.base_name()))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
    }
    {
        let mut d = pdf.document_info(info_ref);
        d.title(TextStr(&info.title));
        d.author(TextStr(&info.author));
        d.creator(TextStr("Atomik Rack"));
    }
    let (pw, ph) = size;
    for (i, page) in pages.iter().enumerate() {
        {
            let mut p = pdf.page(page_ref(i));
            p.parent(tree)
                .media_box(Rect::new(0.0, 0.0, pw, ph))
                .contents(content_ref(i));
            let mut res = p.resources();
            let mut fonts = res.fonts();
            fonts.pair(Name(b"F1"), regular);
            fonts.pair(Name(b"F2"), bold);
            fonts.pair(Name(b"F3"), mono);
        }
        let stream = page_stream(ph, page);
        if compress {
            let packed = deflate(&stream);
            pdf.stream(content_ref(i), &packed)
                .filter(Filter::FlateDecode);
        } else {
            pdf.stream(content_ref(i), &stream);
        }
    }
    pdf.finish()
}

fn page_stream(ph: f32, page: &Page) -> Vec<u8> {
    let mut c = Content::new();
    let y = |v: f32| r2(ph - v);
    for op in &page.ops {
        match op {
            Op::Line {
                a,
                b,
                width,
                ink,
                dash,
                round,
            } => {
                c.save_state();
                paint(&mut c, *ink, true);
                c.set_line_width(r2(*width));
                if *round {
                    c.set_line_cap(LineCapStyle::RoundCap);
                }
                if *dash > 0.0 {
                    c.set_dash_pattern([r2(*dash)], 0.0);
                }
                c.move_to(r2(a.0), y(a.1)).line_to(r2(b.0), y(b.1)).stroke();
                c.restore_state();
            }
            Op::Rect {
                x,
                y: top,
                w,
                h,
                fill,
                stroke,
            } => {
                c.save_state();
                if let Some(f) = fill {
                    paint(&mut c, *f, false);
                }
                if let Some((sw, s)) = stroke {
                    paint(&mut c, *s, true);
                    c.set_line_width(r2(*sw));
                }
                c.rect(r2(*x), y(*top + *h), r2(*w), r2(*h));
                match (fill.is_some(), stroke.is_some()) {
                    (true, true) => c.fill_nonzero_and_stroke(),
                    (true, false) => c.fill_nonzero(),
                    (false, true) => c.stroke(),
                    (false, false) => c.end_path(),
                };
                c.restore_state();
            }
            Op::Circle {
                c: ctr,
                r,
                fill,
                width,
                ink,
            } => {
                c.save_state();
                paint(&mut c, *ink, !*fill);
                if *fill {
                    paint(&mut c, *ink, false);
                } else {
                    c.set_line_width(r2(*width));
                }
                // Four cubic arcs.
                let k = 0.552_284_8 * r;
                let (cx, cy) = (ctr.0, ctr.1);
                c.move_to(r2(cx + r), y(cy));
                c.cubic_to(
                    r2(cx + r),
                    y(cy - k),
                    r2(cx + k),
                    y(cy - r),
                    r2(cx),
                    y(cy - r),
                );
                c.cubic_to(
                    r2(cx - k),
                    y(cy - r),
                    r2(cx - r),
                    y(cy - k),
                    r2(cx - r),
                    y(cy),
                );
                c.cubic_to(
                    r2(cx - r),
                    y(cy + k),
                    r2(cx - k),
                    y(cy + r),
                    r2(cx),
                    y(cy + r),
                );
                c.cubic_to(
                    r2(cx + k),
                    y(cy + r),
                    r2(cx + r),
                    y(cy + k),
                    r2(cx + r),
                    y(cy),
                );
                c.close_path();
                if *fill {
                    c.fill_nonzero();
                } else {
                    c.stroke();
                }
                c.restore_state();
            }
            Op::Curve {
                a,
                ctrl,
                b,
                width,
                ink,
            } => {
                c.save_state();
                paint(&mut c, *ink, true);
                c.set_line_width(r2(*width));
                // A quadratic curve as the equal cubic.
                let c1 = (
                    a.0 + 2.0 / 3.0 * (ctrl.0 - a.0),
                    a.1 + 2.0 / 3.0 * (ctrl.1 - a.1),
                );
                let c2 = (
                    b.0 + 2.0 / 3.0 * (ctrl.0 - b.0),
                    b.1 + 2.0 / 3.0 * (ctrl.1 - b.1),
                );
                c.move_to(r2(a.0), y(a.1));
                c.cubic_to(r2(c1.0), y(c1.1), r2(c2.0), y(c2.1), r2(b.0), y(b.1));
                c.stroke();
                c.restore_state();
            }
            Op::Poly { pts, ink } => {
                let Some(first) = pts.first() else { continue };
                c.save_state();
                paint(&mut c, *ink, false);
                c.move_to(r2(first.0), y(first.1));
                for p in &pts[1..] {
                    c.line_to(r2(p.0), y(p.1));
                }
                c.close_path().fill_nonzero();
                c.restore_state();
            }
            Op::Text {
                x,
                y: base,
                size,
                font,
                ink,
                anchor,
                text,
            } => {
                let w = text_width(*font, *size, text);
                let left = match anchor {
                    Anchor::Start => *x,
                    Anchor::Middle => *x - w / 2.0,
                    Anchor::End => *x - w,
                };
                c.begin_text();
                paint(&mut c, *ink, false);
                let name = match font {
                    Font::Regular => b"F1",
                    Font::Bold => b"F2",
                    Font::Mono => b"F3",
                };
                c.set_font(Name(name), r2(*size));
                c.set_text_matrix([1.0, 0.0, 0.0, 1.0, r2(left), y(*base)]);
                c.show(Str(&encode(text)));
                c.end_text();
            }
        }
    }
    c.finish().to_vec()
}

/// Deflates a content stream (the pure-Rust backend: the same bytes on every platform).
fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(6));
    // Writing to a Vec cannot fail.
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}
