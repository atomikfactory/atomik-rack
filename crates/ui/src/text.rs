//! Text shaping with parley (system fonts and fallback through fontique).
//!
//! Layout logic never measures text, so layouts stay platform-independent; only the
//! rendering backends shape text, through [`TextEngine::shape`].

use crate::canvas::{Align, TextRun};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontData, FontFamily, FontFamilyName, FontWeight,
    GenericFamily, Layout, LayoutContext, PositionedLayoutItem, StyleProperty,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

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

/// Entries kept at most (a screenful of a busy song holds a few thousand distinct labels).
const CACHE_MAX_ENTRIES: usize = 4096;
/// Approximate heap kept at most by the cache.
const CACHE_MAX_BYTES: usize = 8 << 20;

/// What identifies a shaped string: everything the layout depends on, except the position and
/// alignment (applied when the glyphs are drawn).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
    text: String,
    size_bits: u32,
    weight: u16,
    spacing_bits: u32,
}

struct CacheEntry {
    key: ShapeKey,
    runs: Arc<[ShapedRun]>,
    width: f32,
    bytes: usize,
    used: u64,
}

/// A shaped string with glyph positions relative to its own origin (start-aligned, first
/// baseline at y = 0). Draw the glyphs at `origin + glyph position`.
#[derive(Debug, Clone)]
pub struct SharedText {
    /// The runs, shared with the cache.
    pub runs: Arc<[ShapedRun]>,
    /// X of the origin (the run's position plus its alignment shift).
    pub x: f32,
    /// Y of the origin (the first line's baseline).
    pub y: f32,
}

/// Shapes text with parley. Create one per app (font discovery is expensive).
///
/// Shaped strings are kept in a bounded least-recently-used cache keyed by text, size, weight
/// and letter spacing, so a label drawn every frame is shaped once.
pub struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    cache: HashMap<u64, CacheEntry>,
    cache_bytes: usize,
    clock: u64,
    hits: u64,
    misses: u64,
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

fn run_bytes(runs: &[ShapedRun]) -> usize {
    runs.iter()
        .map(|r| {
            std::mem::size_of::<ShapedRun>()
                + r.coords.len() * 2
                + r.glyphs.len() * std::mem::size_of::<ShapedGlyph>()
        })
        .sum()
}

impl TextEngine {
    /// A text engine using the system font collection.
    pub fn new() -> Self {
        Self {
            fonts: FontContext::new(),
            layouts: LayoutContext::new(),
            cache: HashMap::new(),
            cache_bytes: 0,
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Forgets every shaped string (call when the fonts change).
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.cache_bytes = 0;
    }

    /// `(entries, approximate bytes, hits, misses)` of the shaping cache.
    pub fn cache_stats(&self) -> (usize, usize, u64, u64) {
        (self.cache.len(), self.cache_bytes, self.hits, self.misses)
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
        self.entry(run).width
    }

    /// Shapes a run: glyphs are aligned per [`TextRun::align`] with the first line's baseline
    /// at `run.pos.y`.
    pub fn shape(&mut self, run: &TextRun) -> Vec<ShapedRun> {
        let s = self.shape_shared(run);
        s.runs
            .iter()
            .map(|r| ShapedRun {
                glyphs: r
                    .glyphs
                    .iter()
                    .map(|g| ShapedGlyph {
                        id: g.id,
                        x: s.x + g.x,
                        y: s.y + g.y,
                    })
                    .collect(),
                ..r.clone()
            })
            .collect()
    }

    /// Shapes a run through the cache: the glyphs are shared and relative to the returned
    /// origin, which holds the run's position and alignment.
    pub fn shape_shared(&mut self, run: &TextRun) -> SharedText {
        let e = self.entry(run);
        let dx = match run.align {
            Align::Start => 0.0,
            Align::Center => -e.width / 2.0,
            Align::End => -e.width,
        };
        SharedText {
            runs: e.runs,
            x: run.pos.x as f32 + dx,
            y: run.pos.y as f32,
        }
    }

    /// The cached shaping of `run` (start-aligned, baseline at 0), shaping it on a miss.
    fn entry(&mut self, run: &TextRun) -> Shaped {
        self.clock += 1;
        // Hashed from the borrowed text so a hit allocates nothing; the stored key settles a
        // (64-bit) collision.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        run.text.hash(&mut h);
        run.size.to_bits().hash(&mut h);
        run.weight.hash(&mut h);
        run.letter_spacing.to_bits().hash(&mut h);
        let hash = h.finish();
        let same = |k: &ShapeKey| {
            *k.text == *run.text
                && k.size_bits == run.size.to_bits()
                && k.weight == run.weight
                && k.spacing_bits == run.letter_spacing.to_bits()
        };
        if let Some(e) = self.cache.get_mut(&hash).filter(|e| same(&e.key)) {
            e.used = self.clock;
            self.hits += 1;
            return Shaped {
                runs: e.runs.clone(),
                width: e.width,
            };
        }
        self.misses += 1;
        let key = ShapeKey {
            text: run.text.to_string(),
            size_bits: run.size.to_bits(),
            weight: run.weight,
            spacing_bits: run.letter_spacing.to_bits(),
        };
        let (runs, width) = self.shape_cold(run);
        let bytes = run_bytes(&runs) + key.text.len();
        let runs: Arc<[ShapedRun]> = runs.into();
        if let Some(old) = self.cache.remove(&hash) {
            self.cache_bytes -= old.bytes;
        }
        self.cache_bytes += bytes;
        self.cache.insert(
            hash,
            CacheEntry {
                key,
                runs: runs.clone(),
                width,
                bytes,
                used: self.clock,
            },
        );
        self.evict();
        Shaped { runs, width }
    }

    /// Drops the least recently used entries once over the bounds (down to 3/4 of them, so the
    /// scan is amortised).
    fn evict(&mut self) {
        if self.cache.len() <= CACHE_MAX_ENTRIES && self.cache_bytes <= CACHE_MAX_BYTES {
            return;
        }
        let mut order: Vec<(u64, usize)> = self
            .cache
            .values()
            .map(|e| (e.used, e.bytes))
            .collect::<Vec<_>>();
        order.sort_unstable_by_key(|&(used, _)| used);
        let mut count = self.cache.len();
        let mut bytes = self.cache_bytes;
        let mut cutoff = 0;
        for (used, b) in order {
            if count <= CACHE_MAX_ENTRIES * 3 / 4 && bytes <= CACHE_MAX_BYTES * 3 / 4 {
                break;
            }
            cutoff = used;
            count -= 1;
            bytes -= b;
        }
        self.cache.retain(|_, e| e.used > cutoff);
        self.cache_bytes = self.cache.values().map(|e| e.bytes).sum();
    }

    /// Shapes a string from scratch: start-aligned, positions relative to the first baseline.
    fn shape_cold(&mut self, run: &TextRun) -> (Vec<ShapedRun>, f32) {
        let layout = self.layout(run);
        let width = layout.width();
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
                            x: g.x,
                            y: g.y - base,
                        })
                        .collect(),
                });
            }
        }
        (out, width)
    }
}

struct Shaped {
    runs: Arc<[ShapedRun]>,
    width: f32,
}
