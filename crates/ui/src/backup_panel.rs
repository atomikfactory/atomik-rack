//! The restore list of File > Restore from Backup: the backup copies of the open project.
//!
//! ```text
//! RESTORE FROM BACKUP                                          CLOSE
//! 6 copies, newest first
//!   2026-11-10 09:31:12          12.4 KB
//!   2026-11-09 18:02:40          12.1 KB
//!   ...
//! [RESTORE]   Up and Down choose; Enter opens the copy as a new song
//! ```
//!
//! The panel owns no files: the host gives it the rows ([`BackupView::set_rows`]; until then it
//! says it is looking), and a row picked with RESTORE, Enter or a double click asks the host to
//! open that copy as a new unsaved song ([`BackupAction::Restore`], the index of the row).

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const STATUS_H: f64 = 24.0;
const ROW_H: f64 = 22.0;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 480.0;
const BTN_H: f64 = 22.0;
const FONT: f32 = 10.5;
/// The most rows shown at once; more scroll.
pub const MAX_ROWS: usize = 12;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupAction {
    /// Open the copy at this place of the list as a new song.
    Restore(usize),
    /// Close the panel.
    Close,
}

/// One copy of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupItem {
    /// When it was made.
    pub when: String,
    /// Its size for a person.
    pub size: String,
}

/// The restore list panel. See the module documentation.
#[derive(Debug)]
pub struct BackupView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    rows: Vec<BackupItem>,
    /// What the status line says when there are no rows (looking, none, or why not).
    note: String,
    selected: usize,
    top: usize,
    hover: Option<Point>,
    repaint: bool,
}

impl BackupView {
    /// A panel that is looking for the copies; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            rows: Vec::new(),
            note: "Looking for backup copies...".to_owned(),
            selected: 0,
            top: 0,
            hover: None,
            repaint: true,
        }
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Shows the copies, newest first. The newest is chosen.
    pub fn set_rows(&mut self, rows: Vec<BackupItem>) {
        self.note = if rows.is_empty() {
            "There are no backup copies of this project yet".to_owned()
        } else {
            format!(
                "{} cop{}, newest first",
                rows.len(),
                if rows.len() == 1 { "y" } else { "ies" }
            )
        };
        self.rows = rows;
        self.selected = 0;
        self.top = 0;
        self.repaint = true;
    }

    /// Shows `why` instead of a list (the folder could not be read).
    pub fn set_failed(&mut self, why: String) {
        self.rows.clear();
        self.note = why;
        self.repaint = true;
    }

    /// The copies shown.
    pub fn rows(&self) -> &[BackupItem] {
        &self.rows
    }

    /// The chosen copy.
    pub fn selected(&self) -> Option<usize> {
        (self.selected < self.rows.len()).then_some(self.selected)
    }

    /// The line under the title.
    pub fn note(&self) -> &str {
        &self.note
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (it does not: its keys are commands of its own).
    pub fn is_text_owner(&self) -> bool {
        false
    }

    // ------------------------------------------------------------------ geometry

    fn shown(&self) -> usize {
        self.rows.len().clamp(1, MAX_ROWS)
    }

    fn height(&self) -> f64 {
        HEADER_H + STATUS_H + ROW_H * self.shown() as f64 + FOOT_H
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + self.height().min(room))
    }

    fn list_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + STATUS_H
    }

    /// The row at place `i` of the list, when it is shown.
    pub fn row_rect(&self, i: usize) -> Option<Rect> {
        let shown = self.rows.len().saturating_sub(self.top).min(MAX_ROWS);
        (i >= self.top && i < self.top + shown).then(|| {
            let p = self.panel();
            let y = self.list_top() + (i - self.top) as f64 * ROW_H;
            Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + ROW_H)
        })
    }

    fn row_at(&self, pos: Point) -> Option<usize> {
        (self.top..self.rows.len()).find(|i| self.row_rect(*i).is_some_and(|r| r.contains(pos)))
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    /// The RESTORE button.
    pub fn restore_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.list_top() + ROW_H * self.shown() as f64 + (FOOT_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 100.0, y + BTN_H)
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the list without opening a copy");
        }
        if self.restore_rect().contains(pos) {
            return help(
                self.restore_rect(),
                "Open the chosen copy as a new song; the copy itself is never changed",
            );
        }
        None
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + MAX_ROWS {
            self.top = self.selected + 1 - MAX_ROWS;
        }
        self.repaint = true;
    }

    fn restore(&self) -> Vec<UiAction> {
        self.selected()
            .map(|i| UiAction::Backup(BackupAction::Restore(i)))
            .into_iter()
            .collect()
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::Wheel { delta, .. } => {
                let max_top = self.rows.len().saturating_sub(MAX_ROWS);
                if delta.y > 0.0 {
                    self.top = self.top.saturating_sub(1);
                } else if delta.y < 0.0 {
                    self.top = (self.top + 1).min(max_top);
                }
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
                ..
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(UiAction::Backup(BackupAction::Close));
                } else if self.restore_rect().contains(*pos) {
                    out = self.restore();
                } else if let Some(i) = self.row_at(*pos) {
                    self.selected = i;
                    if *clicks >= 2 {
                        out = self.restore();
                    }
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => {
                match key {
                    Key::Escape => out.push(UiAction::Backup(BackupAction::Close)),
                    Key::Enter => out = self.restore(),
                    Key::Up => self.move_selection(-1),
                    Key::Down => self.move_selection(1),
                    Key::Home => self.move_selection(-(self.rows.len() as isize)),
                    Key::End => self.move_selection(self.rows.len() as isize),
                    _ => {}
                }
                self.repaint = true;
            }
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            "RESTORE FROM BACKUP",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        sp::label(
            c,
            &fit_label(&self.note, p.width() - 2.0 * PAD, FONT),
            p.x0 + PAD,
            p.y0 + HEADER_H + 16.0,
            FONT,
            t.text_muted,
            Align::Start,
        );
        for i in self.top..self.rows.len() {
            let Some(r) = self.row_rect(i) else { break };
            if i == self.selected {
                sp::rect(c, r, mix(t.focus, st.background, 0.65));
            } else if hovered(r) {
                sp::rect(c, r, st.lane_alt);
            }
            let item = &self.rows[i];
            sp::label(
                c,
                &item.when,
                r.x0 + 6.0,
                r.y0 + 15.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &item.size,
                r.x1 - 6.0,
                r.y0 + 15.0,
                FONT,
                t.text_muted,
                Align::End,
            );
        }
        let restore = self.restore_rect();
        sp::button(
            c,
            st,
            restore,
            "RESTORE",
            self.selected().map(|_| st.loop_on),
            hovered(restore),
        );
        sp::label(
            c,
            &fit_label(
                "Up and Down choose; Enter opens the copy as a new song",
                p.x1 - PAD - restore.x1 - 12.0,
                9.5,
            ),
            restore.x1 + 12.0,
            restore.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}
