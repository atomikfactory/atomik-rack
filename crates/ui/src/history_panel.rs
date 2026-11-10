//! The Undo History panel: the edits made to the song, oldest at the top, with a click to go
//! back (or forward) to any of them.
//!
//! The first row is the starting state ("Opened" or "New song"); each row after it is one undo
//! step, with the time it was made as a relative time ("2 min ago"). The row the song stands on
//! is highlighted and the steps after it (the redo history) are dimmed. A click on a row asks
//! the app to undo or redo as many steps as it takes to reach the state right after that step
//! ([`UiAction::HistoryGoto`]); Up and Down step one undo or redo, Home and End go to the
//! first and the last row. The list scrolls (the shared [`PickList`] geometry) and keeps the
//! current row in view.
//!
//! The panel owns no history: the app hands it the rows and the position with
//! [`HistoryView::sync`].

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::pick_list::{PAD, PickList, ROW_HEIGHT};
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Point, Rect, RoundedRect};

/// Width of the panel in logical pixels.
pub const HISTORY_PANEL_WIDTH: f64 = 236.0;
/// The height of the title above the list.
const HEADER_H: f64 = 26.0;
/// The room the time at the right end of a row takes.
const TIME_W: f64 = 62.0;

/// One undo step as the panel shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    /// The name of the step ("Move Device").
    pub label: String,
    /// Seconds since the step was made (`None`: not known).
    pub age_secs: Option<u64>,
}

/// A time span as a short relative time: "just now", "2 min ago", "3 h ago", "2 days ago".
pub fn ago(secs: u64) -> String {
    match secs {
        0..=9 => "just now".to_owned(),
        10..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => {
            let days = secs / 86_400;
            format!("{days} day{} ago", if days == 1 { "" } else { "s" })
        }
    }
}

/// The Undo History pane.
pub struct HistoryView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    /// The name of the starting row.
    start: String,
    start_age: Option<u64>,
    /// The steps, oldest first.
    rows: Vec<HistoryRow>,
    /// How many steps are applied: the row of this index (0 = the starting row) is current.
    position: usize,
    /// The first row shown.
    top: usize,
    hover: Option<usize>,
    repaint: std::cell::Cell<bool>,
}

impl HistoryView {
    /// A panel with an empty history.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, HISTORY_PANEL_WIDTH, 400.0),
            scale: 1.0,
            start: "New song".to_owned(),
            start_age: None,
            rows: Vec::new(),
            position: 0,
            top: 0,
            hover: None,
            repaint: std::cell::Cell::new(true),
        }
    }

    /// Switches to `theme` (the next paint uses it).
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint.set(true);
    }

    /// Whether something the panel shows changed since the last paint.
    pub fn needs_repaint(&self) -> bool {
        self.repaint.get()
    }

    /// Where the panel is, in window logical pixels.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.reveal();
            self.repaint.set(true);
        }
    }

    /// Where the panel is.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Display scale factor (the panel paints in logical units).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
    }

    /// The steps shown, oldest first (not the starting row).
    pub fn rows(&self) -> &[HistoryRow] {
        &self.rows
    }

    /// How many steps are applied; the row of this index (0 is the starting row) is current.
    pub fn position(&self) -> usize {
        self.position
    }

    /// The name of the starting row.
    pub fn start_label(&self) -> &str {
        &self.start
    }

    /// The first row shown.
    pub fn top(&self) -> usize {
        self.top
    }

    /// Hands the panel the history: the name of the starting row (and its age), the steps
    /// oldest first and how many of them are applied. The current row is scrolled into view
    /// when the position or the length changed.
    pub fn sync(
        &mut self,
        start: &str,
        start_age: Option<u64>,
        rows: Vec<HistoryRow>,
        position: usize,
    ) {
        let moved = position != self.position || rows.len() != self.rows.len();
        let changed = start != self.start
            || start_age != self.start_age
            || rows != self.rows
            || position != self.position;
        if !changed {
            return;
        }
        start.clone_into(&mut self.start);
        self.start_age = start_age;
        self.position = position.min(rows.len());
        self.rows = rows;
        if moved {
            self.reveal();
        }
        self.top = self
            .top
            .min(self.list().len.saturating_sub(self.max_rows()));
        self.repaint.set(true);
    }

    // ------------------------------------------------------------------ geometry

    /// Rows in the list: the starting row and one per step.
    fn len(&self) -> usize {
        self.rows.len() + 1
    }

    fn max_rows(&self) -> usize {
        let room = self.bounds.height() - HEADER_H - 2.0 * PAD;
        ((room / ROW_HEIGHT).floor().max(1.0)) as usize
    }

    /// The shared list geometry: rows inside the panel under the title.
    fn list(&self) -> PickList {
        let mut list = PickList::new(
            Point::new(self.bounds.x0, self.bounds.y0 + HEADER_H),
            self.bounds.width(),
            self.max_rows(),
            self.len(),
            self.top,
        );
        list.hover = self.hover;
        list
    }

    /// Scrolls so that the current row shows.
    fn reveal(&mut self) {
        let max = self.max_rows();
        if self.position < self.top {
            self.top = self.position;
        } else if self.position >= self.top + max {
            self.top = self.position + 1 - max;
        }
        self.top = self.top.min(self.len().saturating_sub(max));
    }

    /// The rectangle of row `i` (`None` when it is scrolled out).
    pub fn row_rect(&self, i: usize) -> Option<Rect> {
        self.list().row_rect(i)
    }

    /// The row under `pos`.
    pub fn row_at(&self, pos: Point) -> Option<usize> {
        if !self.bounds.contains(pos) {
            return None;
        }
        self.list().row_at(pos)
    }

    // ------------------------------------------------------------------ events

    /// The row under `pos`, for the hover help.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let i = self.row_at(pos)?;
        let text = if i == self.position {
            "Where the song is now"
        } else if i < self.position {
            "Go back to the song as it was after this step"
        } else {
            "Go forward to the song as it was after this step"
        };
        Some(Help::new(self.row_rect(i)?, text))
    }

    /// Handles an event; the actions ask the app to move through the history.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let hover = self.row_at(*pos);
                if hover != self.hover {
                    self.hover = hover;
                    self.repaint.set(true);
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if let Some(row) = self.row_at(*pos)
                    && row != self.position
                {
                    out.push(UiAction::HistoryGoto(row));
                }
            }
            UiEvent::Wheel { delta, pos, .. } if self.bounds.contains(*pos) => {
                let mut list = self.list();
                if list.wheel(delta.y) {
                    self.top = list.top;
                    self.repaint.set(true);
                }
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    self.repaint.set(true);
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } if !modifiers.command() && !modifiers.alt => {
                let last = self.rows.len();
                let target = match key {
                    Key::Up => self.position.checked_sub(1),
                    Key::Down => (self.position < last).then_some(self.position + 1),
                    Key::Home => Some(0),
                    Key::End => Some(last),
                    _ => None,
                };
                if let Some(row) = target.filter(|row| *row != self.position) {
                    out.push(UiAction::HistoryGoto(row));
                }
            }
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ paint

    /// Draws the panel inside its bounds.
    pub fn paint(&self, c: &mut dyn Canvas) {
        self.repaint.set(false);
        c.push_transform(vello::kurbo::Affine::scale(self.scale));
        self.paint_logical(c);
        c.pop_transform();
    }

    fn paint_logical(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let b = self.bounds;
        c.fill_with(b, self.st.header);
        c.fill_with(Rect::new(b.x0, b.y0, b.x0 + 1.0, b.y1), self.st.separator);
        c.text(
            &TextRun::new(
                "UNDO HISTORY",
                pt(b.x0 + PAD * 2.0, b.y0 + 17.0),
                10.5,
                t.text,
            )
            .weight(700),
        );
        let list = self.list();
        for i in list.visible() {
            if let Some(row) = list.row_rect(i) {
                self.paint_row(c, i, row);
            }
        }
        list.paint_scrollbar(c, &self.st);
    }

    fn paint_row(&self, c: &mut dyn Canvas, i: usize, row: Rect) {
        let t = &self.st.base;
        let current = i == self.position;
        let undone = i > self.position;
        if current {
            c.fill_with(
                RoundedRect::from_rect(row, 3.0),
                t.selection.with_alpha(0.35),
            );
        } else if self.hover == Some(i) {
            c.fill_with(RoundedRect::from_rect(row, 3.0), t.browser_row);
        }
        let (label, age) = if i == 0 {
            (self.start.as_str(), self.start_age)
        } else {
            let step = &self.rows[i - 1];
            (step.label.as_str(), step.age_secs)
        };
        let color = if undone { t.text_muted } else { t.text };
        let base = row.center().y + 3.5;
        let room = row.width() - TIME_W - PAD * 3.0;
        let max = (room / 5.6).max(4.0) as usize;
        let shown = if label.chars().count() > max {
            let cut: String = label.chars().take(max.saturating_sub(1)).collect();
            format!("{cut}...")
        } else {
            label.to_owned()
        };
        c.text(
            &TextRun::new(shown, pt(row.x0 + PAD, base), 10.0, color).weight(if current {
                700
            } else {
                400
            }),
        );
        if let Some(secs) = age {
            c.text(
                &TextRun::new(ago(secs), pt(row.x1 - PAD, base), 9.0, t.text_muted)
                    .align(Align::End),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_short_relative_phrases() {
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(9), "just now");
        assert_eq!(ago(10), "10 s ago");
        assert_eq!(ago(59), "59 s ago");
        assert_eq!(ago(60), "1 min ago");
        assert_eq!(ago(120), "2 min ago");
        assert_eq!(ago(3599), "59 min ago");
        assert_eq!(ago(3600), "1 h ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }
}
