//! Text shaping with parley (system fonts and fallback through fontique).
//!
//! Layout logic never measures text, so layouts stay platform-independent; only the
//! rendering backends shape text, through [`TextEngine::shape`].

use crate::canvas::{Align, TextRun};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontData, FontFamily, FontFamilyName, FontWeight,
    GenericFamily, Layout, LayoutContext, PositionedLayoutItem, StyleProperty,
};

/// One glyph, positioned in the coordinate space of the [`TextRun`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    /// Glyph id in the font.
    pub id: u32,
    /// X position.
    pub x: f32,
    /// Y position (baseline).
    pub y: f32,
}

/// A run of glyphs sharing one font.
#[derive(Debug, Clone)]
pub struct ShapedRun {
    /// The font.
    pub font: FontData,
    /// Font size.
    pub font_size: f32,
    /// Variable-font normalized coordinates (F2Dot14 bits).
    pub coords: Vec<i16>,
    /// Positioned glyphs.
    pub glyphs: Vec<ShapedGlyph>,
}

/// Shapes text with parley. Create one per app (font discovery is expensive).
pub struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<()>,
}

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine").finish_non_exhaustive()
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    /// A text engine using the system font collection.
    pub fn new() -> Self {
        Self {
            fonts: FontContext::new(),
            layouts: LayoutContext::new(),
        }
    }

    fn layout(&mut self, run: &TextRun) -> Layout<()> {
        let mut b = self
            .layouts
            .ranged_builder(&mut self.fonts, &run.text, 1.0, false);
        b.push_default(StyleProperty::FontFamily(FontFamily::Single(
            FontFamilyName::Generic(GenericFamily::SansSerif),
        )));
        b.push_default(StyleProperty::FontSize(run.size));
        b.push_default(StyleProperty::FontWeight(FontWeight::new(f32::from(
            run.weight,
        ))));
        if run.letter_spacing.abs() > f32::EPSILON {
            b.push_default(StyleProperty::LetterSpacing(run.letter_spacing));
        }
        let mut layout: Layout<()> = b.build(&run.text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }

    /// Advance width of a run.
    pub fn measure(&mut self, run: &TextRun) -> f32 {
        self.layout(run).width()
    }

    /// Shapes a run: glyphs are aligned per [`TextRun::align`] with the first line's baseline
    /// at `run.pos.y`.
    pub fn shape(&mut self, run: &TextRun) -> Vec<ShapedRun> {
        let layout = self.layout(run);
        let width = layout.width();
        let dx = match run.align {
            Align::Start => 0.0,
            Align::Center => -width / 2.0,
            Align::End => -width,
        };
        let mut out = Vec::new();
        let mut first_baseline = None;
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else {
                    continue;
                };
                let base = *first_baseline.get_or_insert(gr.baseline());
                let r = gr.run();
                out.push(ShapedRun {
                    font: r.font().clone(),
                    font_size: r.font_size(),
                    coords: r.normalized_coords().iter().map(|c| c.to_bits()).collect(),
                    glyphs: gr
                        .positioned_glyphs()
                        .map(|g| ShapedGlyph {
                            id: g.id,
                            x: run.pos.x as f32 + dx + g.x,
                            y: run.pos.y as f32 + (g.y - base),
                        })
                        .collect(),
                });
            }
        }
        out
    }
}
