//! The one pick-list widget of the rack-style views.
//!
//! The arrangement and the console both open a plain list of rows (an input source, an
//! automation parameter, a bus, a take command) that scrolls with the wheel and is dismissed by
//! a press. They share this widget for the geometry (panel rectangle, row rectangles, the row
//! under a point, scrolling), the hover row and the panel frame with its scroll bar; each view
//! keeps its own rows (what a row does) and draws the row contents, since a row can be marked as
//! the current choice or with a check.
//!
//! The registry-driven [`crate::context_menu::ContextMenu`] stays the menu for commands. This
//! list is for rows that are not commands. Both take the pointer in the coordinates they were
//! opened in and own no document state.

use crate::canvas::{Canvas, CanvasExt};
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use vello::kurbo::{Point, Rect, RoundedRect, Vec2};

/// Height of one row, logical pixels.
pub const ROW_HEIGHT: f64 = 20.0;
/// Padding between the panel edge and the first row.
pub const PAD: f64 = 4.0;
/// Default panel width.
pub const WIDTH: f64 = 200.0;

/// The geometry and interaction state of an open pick list.
#[derive(Debug, Clone, PartialEq)]
pub struct PickList {
    /// Top-left of the panel.
    pub origin: Point,
    /// Panel width.
    pub width: f64,
    /// Rows shown at once; more rows scroll.
    pub max_rows: usize,
    /// Rows in the list.
    pub len: usize,
    /// The first row shown.
    pub top: usize,
    /// The row under the pointer.
    pub hover: Option<usize>,
}

impl PickList {
    /// A list of `len` rows at `origin`, scrolled so that row `top` is first (clamped).
    pub fn new(origin: Point, width: f64, max_rows: usize, len: usize, top: usize) -> Self {
        let mut list = Self {
            origin,
            width,
            max_rows,
            len,
            top,
            hover: None,
        };
        list.top = list.top.min(list.max_top());
        list
    }

    fn max_top(&self) -> usize {
        self.len.saturating_sub(self.max_rows)
    }

    /// Rows shown now.
    pub fn rows(&self) -> usize {
        self.len.saturating_sub(self.top).min(self.max_rows)
    }

    /// The rows shown, as indices into the list.
    pub fn visible(&self) -> std::ops::Range<usize> {
        self.top..self.top + self.rows()
    }

    /// Whether the list has more rows than it shows.
    pub fn scrolls(&self) -> bool {
        self.len > self.max_rows
    }

    /// The panel rectangle.
    pub fn rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + self.width,
            self.origin.y + self.rows() as f64 * ROW_HEIGHT + 2.0 * PAD,
        )
    }

    /// The rectangle of row `i` (an index into the list), `None` when it is scrolled out.
    pub fn row_rect(&self, i: usize) -> Option<Rect> {
        self.visible().contains(&i).then(|| {
            let y = self.origin.y + PAD + (i - self.top) as f64 * ROW_HEIGHT;
            Rect::new(
                self.origin.x + PAD,
                y,
                self.origin.x + self.width - PAD,
                y + ROW_HEIGHT,
            )
        })
    }

    /// The row under `p`.
    pub fn row_at(&self, p: Point) -> Option<usize> {
        self.visible()
            .find(|i| self.row_rect(*i).is_some_and(|r| r.contains(p)))
    }

    /// Scrolls one row for a wheel turn of `dy` (positive scrolls up). Returns whether the
    /// list scrolls at all.
    pub fn wheel(&mut self, dy: f64) -> bool {
        if !self.scrolls() {
            return false;
        }
        if dy > 0.0 {
            self.top = self.top.saturating_sub(1);
        } else if dy < 0.0 {
            self.top = (self.top + 1).min(self.max_top());
        }
        true
    }

    /// Moves the panel up when it would run below `bottom` (not above `floor`).
    pub fn keep_above(&mut self, bottom: f64, floor: f64) {
        let h = self.rect().height();
        if self.origin.y + h > bottom {
            self.origin.y = (bottom - h).max(floor);
        }
    }

    /// Draws the panel: the shadow, the fill, the border and, for a long list, the scroll bar.
    /// The caller draws the rows on top, inside [`Self::row_rect`].
    pub fn paint_frame(&self, c: &mut dyn Canvas, st: &SeqTheme) {
        let t = &st.base;
        let r = self.rect();
        c.fill_with(
            RoundedRect::from_rect(r.inset(2.0) + Vec2::new(0.0, 3.0), 5.0),
            t.cable_shadow,
        );
        c.fill_with(RoundedRect::from_rect(r, 4.0), t.browser_bg);
        c.stroke_with(
            RoundedRect::from_rect(r.inset(-0.5), 4.5),
            1.0,
            t.bevel_dark,
        );
    }

    /// Draws the scroll bar of a long list (after the rows).
    pub fn paint_scrollbar(&self, c: &mut dyn Canvas, st: &SeqTheme) {
        if !self.scrolls() {
            return;
        }
        let r = self.rect();
        let frac = self.max_rows as f64 / self.len as f64;
        let h = (r.height() - 2.0 * PAD) * frac;
        let max = self.max_top() as f64;
        let y = r.y0 + PAD + (r.height() - 2.0 * PAD - h) * (self.top as f64 / max);
        c.fill_with(
            RoundedRect::new(r.x1 - 6.0, y, r.x1 - 3.0, y + h, 1.5),
            st.base.text_muted.with_alpha(0.6),
        );
    }
}

/// A small popup list drawn over a panel (the macro and CV menus, a choice param's entries):
/// a fixed-height row each, no scrolling, the tooltip colours.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactList {
    /// The panel rectangle.
    pub rect: Rect,
    /// Height of one row.
    pub row_height: f64,
    /// The row under the pointer.
    pub hover: Option<usize>,
}

impl CompactList {
    /// Padding between the panel edge and the rows.
    pub const PAD: f64 = 2.0;

    /// The panel height of `rows` rows of `row_height`.
    pub fn height(rows: usize, row_height: f64) -> f64 {
        rows as f64 * row_height + 2.0 * Self::PAD
    }

    /// The top of a panel `height` tall that wants to start at `below`, or at `above` when it
    /// does not fit under it (`bottom` is the lowest edge it may reach): kept between `top` and
    /// `bottom`, so every row shows whichever way it opens.
    pub fn fit_y(below: f64, above: f64, height: f64, top: f64, bottom: f64) -> f64 {
        let y = if below + height > bottom {
            above
        } else {
            below
        };
        y.min(bottom - height).max(top)
    }

    /// A list whose panel is `rect`.
    pub fn new(rect: Rect, row_height: f64) -> Self {
        Self {
            rect,
            row_height,
            hover: None,
        }
    }

    /// The rectangle of row `i`.
    pub fn row_rect(&self, i: usize) -> Rect {
        let y = self.rect.y0 + Self::PAD + i as f64 * self.row_height;
        Rect::new(
            self.rect.x0 + Self::PAD,
            y,
            self.rect.x1 - Self::PAD,
            y + self.row_height,
        )
    }

    /// The row of `rows` under `p`.
    pub fn row_at(&self, rows: usize, p: Point) -> Option<usize> {
        (0..rows).find(|&i| self.row_rect(i).contains(p))
    }

    /// Draws the panel.
    pub fn paint_frame(&self, c: &mut dyn Canvas, theme: &Theme) {
        c.fill_with(RoundedRect::from_rect(self.rect, 4.0), theme.tooltip_bg);
        c.stroke_with(
            RoundedRect::from_rect(self.rect, 4.0),
            1.0,
            theme.ink.with_alpha(0.14),
        );
    }

    /// Draws the highlight of row `i` when the pointer is on it.
    pub fn paint_hover(&self, c: &mut dyn Canvas, theme: &Theme, i: usize) {
        if self.hover == Some(i) {
            c.fill_with(
                RoundedRect::from_rect(self.row_rect(i).inset(-1.0), 3.0),
                theme.browser_row,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(len: usize, top: usize) -> PickList {
        PickList::new(Point::new(10.0, 10.0), WIDTH, 5, len, top)
    }

    #[test]
    fn a_short_list_shows_every_row_and_does_not_scroll() {
        let mut l = list(3, 0);
        assert_eq!(l.rows(), 3);
        assert!(!l.wheel(-1.0));
        assert_eq!(l.rect().height(), 3.0 * ROW_HEIGHT + 8.0);
        assert_eq!(l.visible(), 0..3);
    }

    #[test]
    fn a_long_list_scrolls_within_its_bounds() {
        let mut l = list(12, 99);
        assert_eq!(l.top, 7, "the start row is clamped");
        for _ in 0..20 {
            l.wheel(-1.0);
        }
        assert_eq!(l.top, 7);
        for _ in 0..20 {
            l.wheel(1.0);
        }
        assert_eq!(l.top, 0);
        assert_eq!(l.rows(), 5);
    }

    #[test]
    fn rows_are_found_by_their_list_index_not_their_slot() {
        let l = list(12, 4);
        assert!(l.row_rect(3).is_none(), "scrolled out above");
        assert!(l.row_rect(9).is_none(), "scrolled out below");
        let r = l.row_rect(4).unwrap();
        assert_eq!(l.row_at(r.center()), Some(4));
        assert_eq!(l.row_at(Point::new(0.0, 0.0)), None);
    }

    #[test]
    fn a_compact_list_opens_below_then_above_and_is_always_kept_inside() {
        // Fits below: starts at the pointer.
        assert_eq!(CompactList::fit_y(100.0, 20.0, 80.0, 0.0, 400.0), 100.0);
        // Does not fit below: opens above the pointer.
        assert_eq!(CompactList::fit_y(350.0, 270.0, 80.0, 0.0, 400.0), 270.0);
        // Fits neither below nor above (a short window): shifted down from the top edge, so
        // the first row is not cut off.
        assert_eq!(CompactList::fit_y(150.0, -130.0, 280.0, 20.0, 300.0), 20.0);
        // Above the top by a little: moved down to it.
        assert_eq!(CompactList::fit_y(340.0, -10.0, 100.0, 0.0, 380.0), 0.0);
        // Never below the bottom edge.
        assert_eq!(CompactList::fit_y(380.0, 100.0, 100.0, 0.0, 400.0), 100.0);
    }

    #[test]
    fn compact_rows_are_found_inside_the_padding() {
        let l = CompactList::new(
            Rect::new(0.0, 0.0, 100.0, CompactList::height(3, 18.0)),
            18.0,
        );
        assert_eq!(l.row_at(3, Point::new(50.0, 2.0 + 18.0 + 1.0)), Some(1));
        assert_eq!(l.row_at(3, Point::new(50.0, 1.0)), None);
        assert_eq!(l.row_at(2, Point::new(50.0, 2.0 + 36.0 + 1.0)), None);
    }

    #[test]
    fn the_panel_moves_up_to_fit_but_not_past_the_floor() {
        let mut l = list(3, 0);
        l.origin.y = 50.0;
        l.keep_above(90.0, 0.0);
        assert_eq!(l.rect().y1, 90.0);
        l.keep_above(20.0, 5.0);
        assert_eq!(l.origin.y, 5.0);
    }
}
