//! The geometry of the in-window menus: where a dropdown or a flyout goes so that it stays on
//! screen, how far it scrolls when the window is shorter than the list, and what scrolling does
//! to the rows. Pure arithmetic in logical pixels, shared by the in-window menu bar and the
//! context menus.

use vello::kurbo::Rect;

/// Height of one menu row, logical pixels.
pub const ITEM_HEIGHT: f64 = 24.0;
/// Height of a separator.
pub const SEPARATOR_HEIGHT: f64 = 9.0;

/// Space kept between a menu and the edge of the window.
pub const EDGE_MARGIN: f64 = 6.0;
/// Height of the strip at the top and at the bottom of a list that scrolls: hovering it scrolls.
pub const ARROW_HEIGHT: f64 = 14.0;
/// Padding above the first and below the last row of a list that fits.
const PAD: f64 = 4.0;
/// How far one notch of the mouse wheel scrolls.
pub const WHEEL_STEP: f64 = ITEM_HEIGHT * 3.0;
/// How fast a pointer resting on a scroll strip scrolls, pixels per second.
pub const ARROW_SPEED: f64 = 420.0;
/// The least a list that scrolls shows: two strips and two rows.
const MIN_HEIGHT: f64 = 2.0 * ARROW_HEIGHT + 2.0 * ITEM_HEIGHT;
/// How far the pointer must be from a row to the side before it leaves a flyout, pixels.
const FLYOUT_OVERLAP: f64 = 2.0;

/// The height of an entry: a row, or a separator.
pub fn row_height(separator: bool) -> f64 {
    if separator {
        SEPARATOR_HEIGHT
    } else {
        ITEM_HEIGHT
    }
}

/// Where one list of entries is and what it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ListLayout {
    /// The whole panel, strips included.
    pub panel: Rect,
    /// Where the rows show: the panel inside its padding, or between the two strips.
    pub view: Rect,
    /// Each entry's place for choosing: its row cut to `view`, empty when scrolled out.
    pub rows: Vec<Rect>,
    /// Each entry at its scrolled place, not cut: for drawing under a clip.
    pub drawn: Vec<Rect>,
    /// How far the list is scrolled.
    pub offset: f64,
    /// The furthest it can scroll: 0 when it fits.
    pub max_offset: f64,
}

impl ListLayout {
    /// Whether the list is taller than its panel.
    pub fn scrolls(&self) -> bool {
        self.max_offset > 0.0
    }

    /// The strip that scrolls up (only when the list scrolls).
    pub fn up_strip(&self) -> Option<Rect> {
        self.scrolls().then(|| {
            Rect::new(
                self.panel.x0,
                self.panel.y0,
                self.panel.x1,
                self.panel.y0 + ARROW_HEIGHT,
            )
        })
    }

    /// The strip that scrolls down (only when the list scrolls).
    pub fn down_strip(&self) -> Option<Rect> {
        self.scrolls().then(|| {
            Rect::new(
                self.panel.x0,
                self.panel.y1 - ARROW_HEIGHT,
                self.panel.x1,
                self.panel.y1,
            )
        })
    }

    /// The position of entry `at` from the start of the list, not scrolled.
    pub fn content_top(&self, at: usize) -> f64 {
        self.drawn[at].y0 + self.offset - self.view.y0
    }
}

/// The height a list of rows `heights` needs when it fits: the rows and the padding.
pub fn natural_height(heights: &[f64]) -> f64 {
    heights.iter().sum::<f64>() + 2.0 * PAD
}

/// The top and height of a panel that wants `natural` height from `top`, kept between `min_top`
/// and `max_bottom`. With `may_shift` it moves up (a flyout) before it is made shorter.
pub fn fit_vertical(
    top: f64,
    natural: f64,
    min_top: f64,
    max_bottom: f64,
    may_shift: bool,
) -> (f64, f64) {
    let top = if may_shift {
        top.min(max_bottom - natural).max(min_top)
    } else {
        top
    };
    let room = max_bottom - top;
    let height = if natural <= room {
        natural
    } else {
        room.max(MIN_HEIGHT).min(natural)
    };
    (top, height)
}

/// The left edge of a dropdown `width` wide that wants to start at `x0`: pulled left so that it
/// ends inside the window.
pub fn place_dropdown_x(x0: f64, width: f64, window_width: f64) -> f64 {
    x0.min(window_width - EDGE_MARGIN - width).max(0.0)
}

/// The left edge of a flyout `width` wide beside `parent`: to its right, or to its left when the
/// right side would leave the window, or against the right edge when neither side has room.
pub fn place_flyout_x(parent: Rect, width: f64, window_width: f64) -> f64 {
    let right = parent.x1 - FLYOUT_OVERLAP;
    if right + width <= window_width - EDGE_MARGIN {
        return right;
    }
    let left = parent.x0 + FLYOUT_OVERLAP - width;
    if left >= EDGE_MARGIN {
        return left;
    }
    (window_width - EDGE_MARGIN - width).max(0.0)
}

/// Lays out rows of `heights` in a panel `x0..x1` wide, at `top`, `height` tall, scrolled by
/// `offset` (kept between 0 and what the list allows).
pub fn layout(heights: &[f64], x0: f64, x1: f64, top: f64, height: f64, offset: f64) -> ListLayout {
    let content: f64 = heights.iter().sum();
    let natural = content + 2.0 * PAD;
    let fits = height >= natural - 1e-9;
    let (panel_h, view, max_offset) = if fits {
        let view = Rect::new(x0 + PAD, top + PAD, x1 - PAD, top + PAD + content);
        (natural, view, 0.0)
    } else {
        let view = Rect::new(
            x0 + PAD,
            top + ARROW_HEIGHT,
            x1 - PAD,
            top + height - ARROW_HEIGHT,
        );
        (height, view, (content - view.height()).max(0.0))
    };
    let offset = offset.clamp(0.0, max_offset);
    let mut y = view.y0 - offset;
    let mut rows = Vec::with_capacity(heights.len());
    let mut drawn = Vec::with_capacity(heights.len());
    for h in heights {
        let r = Rect::new(view.x0, y, view.x1, y + h);
        let cut = r.intersect(view);
        rows.push(if cut.width() > 0.0 && cut.height() > 0.0 {
            cut
        } else {
            Rect::ZERO
        });
        drawn.push(r);
        y += h;
    }
    ListLayout {
        panel: Rect::new(x0, top, x1, top + panel_h),
        view,
        rows,
        drawn,
        offset,
        max_offset,
    }
}

/// The offset that shows a row from `row_top` to `row_bottom` (measured from the start of the
/// list) in a view `view_height` tall, moving as little as possible from `offset`.
pub fn offset_to_show(
    offset: f64,
    view_height: f64,
    max_offset: f64,
    row_top: f64,
    row_bottom: f64,
) -> f64 {
    let moved = if row_top < offset {
        row_top
    } else if row_bottom > offset + view_height {
        row_bottom - view_height
    } else {
        offset
    };
    moved.clamp(0.0, max_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize) -> Vec<f64> {
        vec![ITEM_HEIGHT; n]
    }

    #[test]
    fn a_list_that_fits_is_not_cut_and_does_not_scroll() {
        let h = rows(5);
        let (top, height) = fit_vertical(26.0, natural_height(&h), 26.0, 700.0, false);
        let l = layout(&h, 10.0, 230.0, top, height, 50.0);
        assert!(!l.scrolls());
        assert_eq!(l.offset, 0.0, "nothing to scroll");
        assert_eq!(
            l.panel,
            Rect::new(10.0, 26.0, 230.0, 26.0 + 5.0 * 24.0 + 8.0)
        );
        assert_eq!(l.rows, l.drawn);
        assert_eq!(l.up_strip(), None);
        assert_eq!(l.down_strip(), None);
    }

    #[test]
    fn a_list_taller_than_the_window_is_clamped_to_it_and_scrolls() {
        let h = rows(62);
        let window = 720.0;
        let (top, height) =
            fit_vertical(26.0, natural_height(&h), 26.0, window - EDGE_MARGIN, false);
        assert_eq!(top, 26.0);
        assert!((top + height - (window - EDGE_MARGIN)).abs() < 1e-9);
        let l = layout(&h, 0.0, 220.0, top, height, 0.0);
        assert!(l.scrolls());
        assert!(l.panel.y1 <= window - EDGE_MARGIN + 1e-9);
        // The rows show between the two strips, and the last ones are cut away.
        assert_eq!(l.view.y0, 26.0 + ARROW_HEIGHT);
        assert_eq!(l.view.y1, l.panel.y1 - ARROW_HEIGHT);
        assert!(l.rows[0].height() > 0.0);
        assert_eq!(l.rows[61], Rect::ZERO);
        assert!(l.rows.iter().all(|r| {
            *r == Rect::ZERO
                || (r.x0 >= l.view.x0
                    && r.x1 <= l.view.x1
                    && r.y0 >= l.view.y0 - 1e-9
                    && r.y1 <= l.view.y1 + 1e-9)
        }));
        assert!(l.up_strip().is_some() && l.down_strip().is_some());
    }

    #[test]
    fn the_offset_moves_the_rows_and_stops_at_both_ends() {
        let h = rows(40);
        let (top, height) = fit_vertical(26.0, natural_height(&h), 26.0, 500.0, false);
        let l0 = layout(&h, 0.0, 200.0, top, height, 0.0);
        let l1 = layout(&h, 0.0, 200.0, top, height, 48.0);
        assert_eq!(l1.drawn[2].y0, l0.drawn[2].y0 - 48.0);
        let past = layout(&h, 0.0, 200.0, top, height, 1.0e9);
        assert_eq!(past.offset, l0.max_offset);
        assert!(
            (past.drawn[39].y1 - past.view.y1).abs() < 1e-9,
            "the last row ends at the bottom of the view"
        );
        let before = layout(&h, 0.0, 200.0, top, height, -30.0);
        assert_eq!(before.offset, 0.0);
    }

    #[test]
    fn hit_boxes_are_cut_to_the_view_and_hidden_rows_have_none() {
        let h = rows(40);
        let (top, height) = fit_vertical(26.0, natural_height(&h), 26.0, 400.0, false);
        let l = layout(&h, 0.0, 200.0, top, height, 30.0);
        let strip = l.up_strip().unwrap();
        for r in &l.rows {
            assert!(*r == Rect::ZERO || r.y0 >= strip.y1 - 1e-9);
        }
        // A row half under the top strip keeps only its visible part.
        let cut = l
            .rows
            .iter()
            .zip(&l.drawn)
            .find(|(r, d)| **r != Rect::ZERO && r.height() < d.height())
            .unwrap();
        assert!(cut.0.y0 >= l.view.y0 - 1e-9);
    }

    #[test]
    fn a_row_moved_to_by_the_keyboard_is_made_visible_with_the_least_scrolling() {
        // A view 100 tall, rows 24 high.
        let max = 500.0;
        assert_eq!(
            offset_to_show(0.0, 100.0, max, 48.0, 72.0),
            0.0,
            "already shown"
        );
        assert_eq!(
            offset_to_show(0.0, 100.0, max, 120.0, 144.0),
            44.0,
            "below: its bottom"
        );
        assert_eq!(
            offset_to_show(200.0, 100.0, max, 120.0, 144.0),
            120.0,
            "above: its top"
        );
        assert_eq!(
            offset_to_show(0.0, 100.0, 30.0, 400.0, 424.0),
            30.0,
            "never past the end"
        );
        assert_eq!(
            offset_to_show(90.0, 100.0, max, 0.0, 24.0),
            0.0,
            "Home returns to the start"
        );
    }

    #[test]
    fn every_row_can_be_shown_by_walking_down_the_list() {
        let h = rows(62);
        let (top, height) = fit_vertical(26.0, natural_height(&h), 26.0, 400.0, false);
        let mut offset = 0.0;
        for i in 0..62 {
            let l = layout(&h, 0.0, 200.0, top, height, offset);
            let row_top = l.content_top(i);
            offset = offset_to_show(
                l.offset,
                l.view.height(),
                l.max_offset,
                row_top,
                row_top + ITEM_HEIGHT,
            );
            let l = layout(&h, 0.0, 200.0, top, height, offset);
            assert!(
                l.rows[i].height() >= ITEM_HEIGHT - 1e-9,
                "row {i} shows whole"
            );
        }
    }

    #[test]
    fn a_flyout_near_the_bottom_moves_up_before_it_scrolls() {
        let h = rows(10);
        let natural = natural_height(&h);
        // Wants to start at 600 in a window ending at 700: moves up to fit.
        let (top, height) = fit_vertical(600.0, natural, 26.0, 694.0, true);
        assert!((top + height - 694.0).abs() < 1e-9);
        assert_eq!(height, natural, "all of it shows");
        // Too tall even from the top: clamped and scrolls.
        let h = rows(60);
        let (top, height) = fit_vertical(600.0, natural_height(&h), 26.0, 694.0, true);
        assert_eq!(top, 26.0);
        assert!((top + height - 694.0).abs() < 1e-9);
        assert!(layout(&h, 0.0, 200.0, top, height, 0.0).scrolls());
    }

    #[test]
    fn a_dropdown_near_the_right_edge_is_pulled_in() {
        assert_eq!(place_dropdown_x(100.0, 220.0, 1280.0), 100.0);
        let x = place_dropdown_x(1200.0, 220.0, 1280.0);
        assert_eq!(x, 1280.0 - EDGE_MARGIN - 220.0);
        assert_eq!(
            place_dropdown_x(5.0, 400.0, 300.0),
            0.0,
            "never past the left edge"
        );
    }

    #[test]
    fn a_flyout_goes_right_then_flips_left_then_clamps() {
        let parent = Rect::new(100.0, 26.0, 320.0, 500.0);
        assert_eq!(place_flyout_x(parent, 200.0, 1280.0), 318.0);
        // No room to the right: it opens on the left of the parent, overlapping its edge.
        let x = place_flyout_x(Rect::new(1000.0, 26.0, 1220.0, 500.0), 200.0, 1280.0);
        assert_eq!(x, 1000.0 + 2.0 - 200.0);
        // No room on either side (a narrow window): against the right edge.
        let x = place_flyout_x(Rect::new(20.0, 26.0, 240.0, 500.0), 250.0, 300.0);
        assert_eq!(x, 300.0 - EDGE_MARGIN - 250.0);
    }

    #[test]
    fn at_twice_the_scale_the_window_is_half_the_logical_size_and_the_lists_still_fit_it() {
        // The same physical screen: 1x is 1440 x 900 logical, 2x is 720 x 450.
        for (w, hgt) in [(1440.0, 900.0), (720.0, 450.0)] {
            let rows_h = rows(62);
            let (top, height) = fit_vertical(
                26.0,
                natural_height(&rows_h),
                26.0,
                hgt - EDGE_MARGIN,
                false,
            );
            let l = layout(&rows_h, 0.0, 220.0, top, height, 0.0);
            assert!(l.panel.y1 <= hgt - EDGE_MARGIN + 1e-9, "{w}x{hgt}");
            let x = place_dropdown_x(w - 100.0, 220.0, w);
            assert!(x + 220.0 <= w - EDGE_MARGIN + 1e-9);
            // However small the window, a list never shrinks below two rows and the strips.
            assert!(l.panel.height() >= MIN_HEIGHT - 1e-9);
        }
        let (_, tiny) = fit_vertical(26.0, natural_height(&rows(62)), 26.0, 60.0, false);
        assert_eq!(tiny, MIN_HEIGHT);
    }
}
