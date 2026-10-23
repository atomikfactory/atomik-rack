//! The minimal drawing API: filled and stroked kurbo shapes painted with
//! peniko brushes, clips, transforms and text runs.
//!
//! Backends:
//! - [`crate::SceneCanvas`]: encodes into a [`vello::Scene`] (GPU path, used by the app);
//! - [`crate::DisplayList`]: records ops as plain data (deterministic snapshot tests, caching);
//! - a `vello_cpu` backend for pixel previews and screenshots is `cpu_canvas::CpuCanvas`
//!   (feature `cpu-canvas`).

use crate::display_list::DisplayList;
use vello::kurbo::{
    Affine, BezPath, Circle, CubicBez, Ellipse, Line, Point, Rect, RoundedRect, Shape, Stroke,
};
use vello::peniko::{Brush, Color};

/// A shape the canvas can fill, stroke or clip to.
#[derive(Debug, Clone, PartialEq)]
pub enum Geom {
    /// Axis-aligned rectangle.
    Rect(Rect),
    /// Rounded rectangle.
    RoundedRect(RoundedRect),
    /// Circle.
    Circle(Circle),
    /// Ellipse.
    Ellipse(Ellipse),
    /// Line segment (stroke only).
    Line(Line),
    /// Cubic Bézier segment (stroke only).
    Cubic(CubicBez),
    /// Arbitrary path.
    Path(BezPath),
}

impl Geom {
    /// Bounding box.
    pub fn bounding_box(&self) -> Rect {
        match self {
            Geom::Rect(s) => s.bounding_box(),
            Geom::RoundedRect(s) => s.bounding_box(),
            Geom::Circle(s) => s.bounding_box(),
            Geom::Ellipse(s) => s.bounding_box(),
            Geom::Line(s) => s.bounding_box(),
            Geom::Cubic(s) => s.bounding_box(),
            Geom::Path(s) => s.bounding_box(),
        }
    }

    /// Flattens to a path (tolerance 0.1).
    pub fn to_path(&self) -> BezPath {
        match self {
            Geom::Rect(s) => s.to_path(0.1),
            Geom::RoundedRect(s) => s.to_path(0.1),
            Geom::Circle(s) => s.to_path(0.1),
            Geom::Ellipse(s) => s.to_path(0.1),
            Geom::Line(s) => s.to_path(0.1),
            Geom::Cubic(s) => s.to_path(0.1),
            Geom::Path(s) => s.clone(),
        }
    }
}

macro_rules! geom_from {
    ($($t:ident),*) => {$(
        impl From<$t> for Geom {
            fn from(s: $t) -> Self {
                Geom::$t(s)
            }
        }
    )*};
}
geom_from!(Rect, RoundedRect, Circle, Ellipse, Line);

impl From<CubicBez> for Geom {
    fn from(s: CubicBez) -> Self {
        Geom::Cubic(s)
    }
}

impl From<BezPath> for Geom {
    fn from(s: BezPath) -> Self {
        Geom::Path(s)
    }
}

/// Horizontal text alignment relative to [`TextRun::pos`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// `pos.x` is the left edge.
    #[default]
    Start,
    /// `pos.x` is the centre.
    Center,
    /// `pos.x` is the right edge.
    End,
}

/// A single line of text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// The text (one line).
    pub text: String,
    /// Anchor: `x` per [`Align`], `y` is the baseline.
    pub pos: Point,
    /// Font size in logical units.
    pub size: f32,
    /// Font weight (400 regular, 600 semi-bold, 700 bold).
    pub weight: u16,
    /// Colour.
    pub color: Color,
    /// Alignment.
    pub align: Align,
    /// Extra spacing between letters, in logical units.
    pub letter_spacing: f32,
}

impl TextRun {
    /// A regular-weight, start-aligned run.
    pub fn new(text: impl Into<String>, pos: Point, size: f32, color: Color) -> Self {
        Self {
            text: text.into(),
            pos,
            size,
            weight: 400,
            color,
            align: Align::Start,
            letter_spacing: 0.0,
        }
    }

    /// Sets the alignment.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Sets the weight.
    pub fn weight(mut self, weight: u16) -> Self {
        self.weight = weight;
        self
    }

    /// Sets the letter spacing.
    pub fn spacing(mut self, letter_spacing: f32) -> Self {
        self.letter_spacing = letter_spacing;
        self
    }
}

/// The drawing API implemented by every backend. Coordinates are transformed by the current
/// transform stack ([`Canvas::push_transform`]).
pub trait Canvas {
    /// Fills a shape (non-zero rule).
    fn fill(&mut self, shape: &Geom, brush: &Brush);
    /// Strokes a shape.
    fn stroke(&mut self, shape: &Geom, stroke: &Stroke, brush: &Brush);
    /// Clips subsequent drawing to `shape` until the matching [`Canvas::pop_clip`].
    fn push_clip(&mut self, shape: &Geom);
    /// Ends the innermost clip.
    fn pop_clip(&mut self);
    /// Multiplies the current transform by `transform` until [`Canvas::pop_transform`].
    fn push_transform(&mut self, transform: Affine);
    /// Restores the previous transform.
    fn pop_transform(&mut self);
    /// Draws a line of text.
    fn text(&mut self, run: &TextRun);
    /// Draws a recorded list under the current transform. Backends may cache it by
    /// `(list.id(), list.revision())`; the default replays every op.
    fn draw_list(&mut self, list: &DisplayList) {
        list.replay(self);
    }
}

/// Convenience helpers available on every [`Canvas`].
pub trait CanvasExt: Canvas {
    /// Fills any shape with a solid colour or brush.
    fn fill_with(&mut self, shape: impl Into<Geom>, brush: impl Into<Brush>) {
        self.fill(&shape.into(), &brush.into());
    }

    /// Strokes any shape with the given width.
    fn stroke_with(&mut self, shape: impl Into<Geom>, width: f64, brush: impl Into<Brush>) {
        self.stroke(&shape.into(), &Stroke::new(width), &brush.into());
    }
}

impl<C: Canvas + ?Sized> CanvasExt for C {}

/// A point helper used throughout painting code.
pub(crate) fn pt(x: f64, y: f64) -> Point {
    Point::new(x, y)
}
