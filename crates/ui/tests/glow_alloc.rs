//! A theme's glow is drawn without allocating: the halo behind a meter or the playhead line is a
//! few fills of shapes that hold no heap, so a frame that has it costs no more allocations than
//! one that does not. A canvas that only counts the fills stands in for the real ones (what the
//! real canvases do with a fill is measured by the frame tests of the app).

use atomik_ui::kurbo::{Affine, Rect, Stroke};
use atomik_ui::peniko::{Brush, Color};
use atomik_ui::{Canvas, Geom, Glow, TextRun, Theme};

atomik_testkit::install_alloc_guard!();

#[derive(Default)]
struct Counting {
    fills: usize,
}

impl Canvas for Counting {
    fn fill(&mut self, _shape: &Geom, _brush: &Brush) {
        self.fills += 1;
    }
    fn stroke(&mut self, _shape: &Geom, _stroke: &Stroke, _brush: &Brush) {}
    fn push_clip(&mut self, _shape: &Geom) {}
    fn pop_clip(&mut self) {}
    fn push_transform(&mut self, _transform: Affine) {}
    fn pop_transform(&mut self) {}
    fn text(&mut self, _run: &TextRun) {}
}

#[test]
fn drawing_a_glow_allocates_nothing() {
    let on: Glow = Theme::synthwave().glow;
    assert!(on.active());
    let off = Theme::dark().glow;
    let mut canvas = Counting::default();
    let bar = Rect::new(10.0, 20.0, 21.0, 120.0);
    let (_, allocations) = atomik_testkit::no_alloc::allocations_of(|| {
        for _ in 0..200 {
            on.halo(&mut canvas, bar, Color::WHITE);
            on.vline(&mut canvas, 40.5, 0.0, 300.0, Color::WHITE);
            off.halo(&mut canvas, bar, Color::WHITE);
        }
    });
    assert_eq!(allocations, 0);
    assert!(canvas.fills >= 200 * 2, "the glow drew: {}", canvas.fills);
    // A theme with no glow draws nothing at all.
    let mut quiet = Counting::default();
    off.halo(&mut quiet, bar, Color::WHITE);
    assert_eq!(quiet.fills, 0);
}
