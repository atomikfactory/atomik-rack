//! The vello [`Scene`] backend.

use crate::canvas::{Canvas, Geom, TextRun};
use crate::display_list::DisplayList;
use crate::text::TextEngine;
use std::collections::BTreeMap;
use vello::Scene;
use vello::kurbo::{Affine, Shape, Stroke};
use vello::peniko::{Brush, Fill};

/// Encoded scenes for cached display lists, keyed by list id. Keep one per window
/// and call [`SceneCache::trim`] once per frame.
#[derive(Default)]
pub struct SceneCache {
    entries: BTreeMap<u64, CacheEntry>,
    frame: u64,
}

struct CacheEntry {
    revision: u64,
    last_used: u64,
    scene: Scene,
}

impl std::fmt::Debug for SceneCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneCache")
            .field("entries", &self.entries.len())
            .field("frame", &self.frame)
            .finish()
    }
}

impl SceneCache {
    /// Frames an unused entry is kept for.
    pub const KEEP_FRAMES: u64 = 600;

    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cached scenes.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Advances the frame counter and drops entries unused for [`Self::KEEP_FRAMES`].
    pub fn trim(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.entries
            .retain(|_, e| frame.saturating_sub(e.last_used) <= Self::KEEP_FRAMES);
    }
}

/// A [`Canvas`] that encodes into a vello [`Scene`].
pub struct SceneCanvas<'a> {
    scene: &'a mut Scene,
    text: &'a mut TextEngine,
    cache: Option<&'a mut SceneCache>,
    transforms: Vec<Affine>,
}

impl std::fmt::Debug for SceneCanvas<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneCanvas").finish_non_exhaustive()
    }
}

fn fill_s(scene: &mut Scene, t: Affine, brush: &Brush, s: &impl Shape) {
    scene.fill(Fill::NonZero, t, brush, None, s);
}
fn stroke_s(scene: &mut Scene, t: Affine, st: &Stroke, brush: &Brush, s: &impl Shape) {
    scene.stroke(st, t, brush, None, s);
}
fn clip_s(scene: &mut Scene, t: Affine, s: &impl Shape) {
    scene.push_clip_layer(Fill::NonZero, t, s);
}

macro_rules! dispatch {
    ($g:expr, $s:ident => $e:expr) => {
        match $g {
            Geom::Rect($s) => $e,
            Geom::RoundedRect($s) => $e,
            Geom::Circle($s) => $e,
            Geom::Ellipse($s) => $e,
            Geom::Line($s) => $e,
            Geom::Cubic($s) => $e,
            Geom::Path($s) => $e,
        }
    };
}

impl<'a> SceneCanvas<'a> {
    /// Encodes into `scene`, shaping text with `text`. No list caching.
    pub fn new(scene: &'a mut Scene, text: &'a mut TextEngine) -> Self {
        Self {
            scene,
            text,
            cache: None,
            transforms: vec![Affine::IDENTITY],
        }
    }

    /// Also caches encoded display lists in `cache`.
    pub fn with_cache(mut self, cache: &'a mut SceneCache) -> Self {
        self.cache = Some(cache);
        self
    }

    fn current(&self) -> Affine {
        self.transforms.last().copied().unwrap_or(Affine::IDENTITY)
    }
}

impl Canvas for SceneCanvas<'_> {
    fn fill(&mut self, shape: &Geom, brush: &Brush) {
        let t = self.current();
        dispatch!(shape, s => fill_s(self.scene, t, brush, s));
    }
    fn stroke(&mut self, shape: &Geom, stroke: &Stroke, brush: &Brush) {
        let t = self.current();
        dispatch!(shape, s => stroke_s(self.scene, t, stroke, brush, s));
    }
    fn push_clip(&mut self, shape: &Geom) {
        let t = self.current();
        dispatch!(shape, s => clip_s(self.scene, t, s));
    }
    fn pop_clip(&mut self) {
        self.scene.pop_layer();
    }
    fn push_transform(&mut self, transform: Affine) {
        let t = self.current() * transform;
        self.transforms.push(t);
    }
    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }
    fn text(&mut self, run: &TextRun) {
        let t = self.current();
        let shaped = self.text.shape_shared(run);
        for r in shaped.runs.iter() {
            self.scene
                .draw_glyphs(&r.font)
                .font_size(r.font_size)
                .transform(t)
                .normalized_coords(&r.coords)
                .brush(run.color)
                .draw(
                    Fill::NonZero,
                    r.glyphs.iter().map(|g| vello::Glyph {
                        id: g.id,
                        x: shaped.x + g.x,
                        y: shaped.y + g.y,
                    }),
                );
        }
    }
    fn draw_list(&mut self, list: &DisplayList) {
        let t = self.current();
        let Some(cache) = self.cache.as_deref_mut() else {
            list.replay(self);
            return;
        };
        if list.id() == 0 {
            list.replay(self);
            return;
        }
        let frame = cache.frame;
        let stale = cache
            .entries
            .get(&list.id())
            .is_none_or(|e| e.revision != list.revision());
        if stale {
            let mut scene = Scene::new();
            {
                let mut sub = SceneCanvas::new(&mut scene, self.text);
                list.replay(&mut sub);
            }
            cache.entries.insert(
                list.id(),
                CacheEntry {
                    revision: list.revision(),
                    last_used: frame,
                    scene,
                },
            );
        }
        if let Some(e) = cache.entries.get_mut(&list.id()) {
            e.last_used = frame;
            self.scene.append(&e.scene, Some(t));
        }
    }
}
