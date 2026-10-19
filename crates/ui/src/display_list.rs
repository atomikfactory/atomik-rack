//! The display-list recorder: a [`Canvas`] that records drawing ops as plain data.
//!
//! Used for deterministic snapshot tests (text is stored as strings and anchor positions, not
//! glyphs, so snapshots are platform-independent) and as the per-device panel cache:
//! a cached list carries an `(id, revision)` stamp that GPU backends use to keep
//! an encoded scene per list.

use crate::canvas::{Align, Canvas, Geom, TextRun};
use std::fmt::Write as _;
use vello::kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Stroke};
use vello::peniko::color::{AlphaColor, Srgb};
use vello::peniko::{Brush, Color, GradientKind};

/// One recorded drawing op.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Fill.
    Fill {
        /// Shape.
        shape: Geom,
        /// Brush.
        brush: Brush,
    },
    /// Stroke.
    Stroke {
        /// Shape.
        shape: Geom,
        /// Stroke style.
        stroke: Stroke,
        /// Brush.
        brush: Brush,
    },
    /// Push a clip.
    PushClip(Geom),
    /// Pop a clip.
    PopClip,
    /// Push a transform.
    PushTransform(Affine),
    /// Pop a transform.
    PopTransform,
    /// A text run.
    Text(TextRun),
}

/// A recorded list of ops (also a [`Canvas`] that records).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayList {
    ops: Vec<Op>,
    id: u64,
    revision: u64,
}

impl DisplayList {
    /// An empty, uncached list (id 0).
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty list stamped for caching: backends may reuse an encoding of a list with the
    /// same `id` and `revision`. `id` must be non-zero.
    pub fn cached(id: u64, revision: u64) -> Self {
        Self {
            ops: Vec::new(),
            id,
            revision,
        }
    }

    /// Cache id (0 = never cached).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Cache revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The recorded ops.
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// Removes every op.
    pub fn clear(&mut self) {
        self.ops.clear();
    }

    /// Number of ops.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// True when nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Replays every op into `canvas`.
    pub fn replay<C: Canvas + ?Sized>(&self, canvas: &mut C) {
        for op in &self.ops {
            match op {
                Op::Fill { shape, brush } => canvas.fill(shape, brush),
                Op::Stroke {
                    shape,
                    stroke,
                    brush,
                } => canvas.stroke(shape, stroke, brush),
                Op::PushClip(s) => canvas.push_clip(s),
                Op::PopClip => canvas.pop_clip(),
                Op::PushTransform(t) => canvas.push_transform(*t),
                Op::PopTransform => canvas.pop_transform(),
                Op::Text(run) => canvas.text(run),
            }
        }
    }

    /// All text strings, in order (handy for tests).
    pub fn texts(&self) -> Vec<&str> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                Op::Text(r) => Some(r.text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Deterministic JSON: one op per line, numbers rounded to 2 decimals, colours as
    /// `#rrggbbaa`. Used for golden snapshots.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\"ops\":[\n");
        for (i, op) in self.ops.iter().enumerate() {
            if i > 0 {
                s.push_str(",\n");
            }
            write_op(&mut s, op);
        }
        s.push_str("\n]}\n");
        s
    }
}

impl Canvas for DisplayList {
    fn fill(&mut self, shape: &Geom, brush: &Brush) {
        self.ops.push(Op::Fill {
            shape: shape.clone(),
            brush: brush.clone(),
        });
    }
    fn stroke(&mut self, shape: &Geom, stroke: &Stroke, brush: &Brush) {
        self.ops.push(Op::Stroke {
            shape: shape.clone(),
            stroke: stroke.clone(),
            brush: brush.clone(),
        });
    }
    fn push_clip(&mut self, shape: &Geom) {
        self.ops.push(Op::PushClip(shape.clone()));
    }
    fn pop_clip(&mut self) {
        self.ops.push(Op::PopClip);
    }
    fn push_transform(&mut self, transform: Affine) {
        self.ops.push(Op::PushTransform(transform));
    }
    fn pop_transform(&mut self) {
        self.ops.push(Op::PopTransform);
    }
    fn text(&mut self, run: &TextRun) {
        self.ops.push(Op::Text(run.clone()));
    }
    fn draw_list(&mut self, list: &DisplayList) {
        // Flatten so the recording is self-contained.
        self.ops.extend_from_slice(&list.ops);
    }
}

fn num(s: &mut String, v: f64) {
    let r = (v * 100.0).round() / 100.0;
    let r = r + 0.0; // normalise -0 to +0
    let _ = write!(s, "{r:.2}");
}

fn nums(s: &mut String, vs: &[f64]) {
    s.push('[');
    for (i, v) in vs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        num(s, *v);
    }
    s.push(']');
}

fn color_hex(s: &mut String, c: AlphaColor<Srgb>) {
    let c = c.to_rgba8();
    let _ = write!(s, "\"#{:02x}{:02x}{:02x}{:02x}\"", c.r, c.g, c.b, c.a);
}

fn json_str(s: &mut String, text: &str) {
    s.push('"');
    for ch in text.chars() {
        match ch {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

fn write_path(s: &mut String, p: &BezPath) {
    s.push('"');
    for el in p.elements() {
        let mut pts = |tag: &str, ps: &[Point]| {
            s.push_str(tag);
            for q in ps {
                num(s, q.x);
                s.push(' ');
                num(s, q.y);
                s.push(' ');
            }
        };
        match *el {
            PathEl::MoveTo(a) => pts("M", &[a]),
            PathEl::LineTo(a) => pts("L", &[a]),
            PathEl::QuadTo(a, b) => pts("Q", &[a, b]),
            PathEl::CurveTo(a, b, c) => pts("C", &[a, b, c]),
            PathEl::ClosePath => s.push('Z'),
        }
    }
    s.push('"');
}

fn write_geom(s: &mut String, g: &Geom) {
    s.push('{');
    match g {
        Geom::Rect(r) => {
            s.push_str("\"rect\":");
            nums(s, &[r.x0, r.y0, r.x1, r.y1]);
        }
        Geom::RoundedRect(r) => {
            let rr = r.rect();
            let rad = r.radii();
            s.push_str("\"rrect\":");
            nums(s, &[rr.x0, rr.y0, rr.x1, rr.y1, rad.top_left]);
        }
        Geom::Circle(c) => {
            s.push_str("\"circle\":");
            nums(s, &[c.center.x, c.center.y, c.radius]);
        }
        Geom::Ellipse(e) => {
            let r = e.radii();
            let c = e.center();
            s.push_str("\"ellipse\":");
            nums(s, &[c.x, c.y, r.x, r.y, e.rotation()]);
        }
        Geom::Line(l) => {
            s.push_str("\"line\":");
            nums(s, &[l.p0.x, l.p0.y, l.p1.x, l.p1.y]);
        }
        Geom::Cubic(c) => {
            s.push_str("\"cubic\":");
            nums(
                s,
                &[
                    c.p0.x, c.p0.y, c.p1.x, c.p1.y, c.p2.x, c.p2.y, c.p3.x, c.p3.y,
                ],
            );
        }
        Geom::Path(p) => {
            s.push_str("\"path\":");
            write_path(s, p);
        }
    }
    s.push('}');
}

fn write_brush(s: &mut String, b: &Brush) {
    match b {
        Brush::Solid(c) => color_hex(s, *c),
        Brush::Gradient(g) => {
            s.push_str("{\"gradient\":");
            match &g.kind {
                GradientKind::Linear(p) => {
                    s.push_str("{\"linear\":");
                    nums(s, &[p.start.x, p.start.y, p.end.x, p.end.y]);
                }
                GradientKind::Radial(p) => {
                    s.push_str("{\"radial\":");
                    nums(
                        s,
                        &[
                            p.start_center.x,
                            p.start_center.y,
                            f64::from(p.start_radius),
                            p.end_center.x,
                            p.end_center.y,
                            f64::from(p.end_radius),
                        ],
                    );
                }
                GradientKind::Sweep(p) => {
                    s.push_str("{\"sweep\":");
                    nums(
                        s,
                        &[
                            p.center.x,
                            p.center.y,
                            f64::from(p.start_angle),
                            f64::from(p.end_angle),
                        ],
                    );
                }
            }
            s.push_str(",\"stops\":[");
            for (i, st) in g.stops.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push('[');
                num(s, f64::from(st.offset));
                s.push(',');
                color_hex(s, st.color.to_alpha_color::<Srgb>());
                s.push(']');
            }
            s.push_str("]}}");
        }
        Brush::Image(_) => s.push_str("\"image\""),
    }
}

fn write_op(s: &mut String, op: &Op) {
    match op {
        Op::Fill { shape, brush } => {
            s.push_str("{\"fill\":");
            write_geom(s, shape);
            s.push_str(",\"brush\":");
            write_brush(s, brush);
            s.push('}');
        }
        Op::Stroke {
            shape,
            stroke,
            brush,
        } => {
            s.push_str("{\"stroke\":");
            write_geom(s, shape);
            s.push_str(",\"width\":");
            num(s, stroke.width);
            let cap = match stroke.start_cap {
                Cap::Butt => "butt",
                Cap::Round => "round",
                Cap::Square => "square",
            };
            let join = match stroke.join {
                Join::Bevel => "bevel",
                Join::Miter => "miter",
                Join::Round => "round",
            };
            let _ = write!(s, ",\"cap\":\"{cap}\",\"join\":\"{join}\",\"brush\":");
            write_brush(s, brush);
            s.push('}');
        }
        Op::PushClip(g) => {
            s.push_str("{\"clip\":");
            write_geom(s, g);
            s.push('}');
        }
        Op::PopClip => s.push_str("{\"pop_clip\":true}"),
        Op::PushTransform(t) => {
            s.push_str("{\"transform\":");
            nums(s, &t.as_coeffs());
            s.push('}');
        }
        Op::PopTransform => s.push_str("{\"pop_transform\":true}"),
        Op::Text(r) => {
            s.push_str("{\"text\":");
            json_str(s, &r.text);
            s.push_str(",\"at\":");
            nums(s, &[r.pos.x, r.pos.y]);
            s.push_str(",\"size\":");
            num(s, f64::from(r.size));
            let align = match r.align {
                Align::Start => "start",
                Align::Center => "center",
                Align::End => "end",
            };
            let _ = write!(
                s,
                ",\"weight\":{},\"align\":\"{align}\",\"color\":",
                r.weight
            );
            color_hex(s, r.color);
            if r.letter_spacing.abs() > f32::EPSILON {
                s.push_str(",\"spacing\":");
                num(s, f64::from(r.letter_spacing));
            }
            s.push('}');
        }
    }
}

/// A colour from 8-bit sRGB components.
pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(r, g, b, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::CanvasExt;
    use vello::kurbo::{Circle, Rect};

    #[test]
    fn recorder_records_and_serialises_deterministically() {
        let mut dl = DisplayList::new();
        dl.push_transform(Affine::scale(2.0));
        dl.fill_with(Rect::new(0.0, 0.0, 10.0, 5.0), rgba(255, 0, 0, 255));
        dl.stroke_with(Circle::new((1.0, 2.0), 3.0), 1.5, rgba(0, 0, 255, 128));
        dl.text(&TextRun::new(
            "Hi \"x\"",
            Point::new(1.0, 2.0),
            10.0,
            rgba(1, 2, 3, 4),
        ));
        dl.pop_transform();
        let json = dl.to_json();
        assert_eq!(json, dl.clone().to_json());
        assert!(
            json.contains("{\"fill\":{\"rect\":[0.00,0.00,10.00,5.00]},\"brush\":\"#ff0000ff\"}")
        );
        assert!(json.contains("\"text\":\"Hi \\\"x\\\"\""));
        assert_eq!(dl.texts(), vec!["Hi \"x\""]);
        let mut copy = DisplayList::new();
        copy.draw_list(&dl);
        assert_eq!(copy.ops(), dl.ops());
    }
}
