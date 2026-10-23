//! The relink panel: the list of the audio files a project lost, with a way to find each.
//!
//! ```text
//! RELINK MISSING AUDIO                                                   [ CLOSE ]
//! 2 audio files are missing; their clips play silence. Pick any file for each one,
//! or search a folder: files are matched by name.
//! [ SEARCH FOLDER... ]
//!   kick.wav                    3f2a9c01                              [ LOCATE... ]
//!   snare.wav                   b71e0d44                              [ LOCATE... ]
//! ```
//!
//! A modal view the app places over the window like the plugin settings. It has no document:
//! the app hands it the missing assets with [`RelinkView::set_missing`] (every frame is fine)
//! and what the user does arrives as [`UiAction::Relink`]:
//!
//! * `LOCATE...` on a row emits [`RelinkAction::Locate`] with that asset's id; the app asks for
//!   a file and relinks the asset to it.
//! * `SEARCH FOLDER...` emits [`RelinkAction::SearchFolder`]; the app asks for a folder and
//!   matches the missing assets by file name.
//! * `CLOSE` emits [`RelinkAction::Close`].
//!
//! The list scrolls with the wheel; only the rows in view are painted.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const HINT_H: f64 = 38.0;
const TOOLBAR_H: f64 = 36.0;
const ROW_H: f64 = 28.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 640.0;
const BTN_H: f64 = 22.0;

/// One missing audio file, as the panel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelinkEntry {
    /// Asset id (content hash).
    pub hash: String,
    /// The original file name.
    pub name: String,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelinkAction {
    /// Pick a replacement file for the asset `hash`.
    Locate {
        /// The missing asset.
        hash: String,
    },
    /// Pick a folder to search for the missing files.
    SearchFolder,
    /// Close the panel.
    Close,
}

/// The relink panel. See the module documentation.
#[derive(Debug)]
pub struct RelinkView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    entries: Vec<RelinkEntry>,
    scroll: f64,
    hover: Option<Point>,
    repaint: bool,
}

impl RelinkView {
    /// An empty panel; call [`Self::set_bounds`] and [`Self::set_missing`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            entries: Vec::new(),
            scroll: 0.0,
            hover: None,
            repaint: true,
        }
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it, at
    /// most 640 wide, and as tall as its list needs.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Replaces the list. Cheap when nothing changed.
    pub fn set_missing(&mut self, entries: Vec<RelinkEntry>) {
        if entries == self.entries {
            return;
        }
        self.entries = entries;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The listed assets.
    pub fn entries(&self) -> &[RelinkEntry] {
        &self.entries
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The list's scroll offset in pixels.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let wanted = HEADER_H + HINT_H + TOOLBAR_H + self.rows().max(1) as f64 * ROW_H + PAD;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + wanted.min(room))
    }

    fn rows(&self) -> usize {
        self.entries.len()
    }

    fn list_area(&self) -> Rect {
        let p = self.panel();
        Rect::new(p.x0, p.y0 + HEADER_H + HINT_H + TOOLBAR_H, p.x1, p.y1 - PAD)
    }

    fn clamp_scroll(&mut self) {
        let content = self.rows() as f64 * ROW_H;
        let max = (content - self.list_area().height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
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

    /// The SEARCH FOLDER button.
    pub fn search_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + HINT_H + (TOOLBAR_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 160.0, y + BTN_H)
    }

    /// The row of entry `i`, `None` when it is scrolled out of the list.
    pub fn row_rect(&self, i: usize) -> Option<Rect> {
        if i >= self.rows() {
            return None;
        }
        let area = self.list_area();
        let y = area.y0 + i as f64 * ROW_H - self.scroll;
        let row = Rect::new(area.x0, y, area.x1, y + ROW_H);
        (row.y0 >= area.y0 - 0.5 && row.y1 <= area.y1 + 0.5).then_some(row)
    }

    /// The LOCATE button of entry `i`.
    pub fn locate_rect(&self, i: usize) -> Option<Rect> {
        let row = self.row_rect(i)?;
        Some(Rect::new(
            row.x1 - PAD - 110.0,
            row.y0 + (ROW_H - BTN_H) / 2.0,
            row.x1 - PAD,
            row.y0 + (ROW_H - BTN_H) / 2.0 + BTN_H,
        ))
    }

    // ------------------------------------------------------------------ events

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
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.repaint = true;
                if let Some(action) = self.hit(*pos) {
                    out.push(UiAction::Relink(action));
                }
            }
            UiEvent::Wheel { delta, .. } => {
                self.scroll -= delta.y * ROW_H;
                self.clamp_scroll();
                self.repaint = true;
            }
            _ => {}
        }
        out
    }

    fn hit(&self, pos: Point) -> Option<RelinkAction> {
        if self.close_rect().contains(pos) {
            return Some(RelinkAction::Close);
        }
        if self.entries.is_empty() {
            return None;
        }
        if self.search_rect().contains(pos) {
            return Some(RelinkAction::SearchFolder);
        }
        (0..self.rows())
            .find(|i| self.locate_rect(*i).is_some_and(|r| r.contains(pos)))
            .map(|i| RelinkAction::Locate {
                hash: self.entries[i].hash.clone(),
            })
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
            "RELINK MISSING AUDIO",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let count = self.entries.len();
        let width = p.width() - 2.0 * PAD;
        if count == 0 {
            sp::label(
                c,
                "Nothing is missing.",
                p.x0 + PAD,
                p.y0 + HEADER_H + 24.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
            c.pop_clip();
            c.pop_transform();
            return;
        }
        let plural = if count == 1 { "" } else { "s" };
        let hint = [
            format!(
                "{count} audio file{plural} missing: their clips play silence. Pick any file for each one,"
            ),
            "or search a folder: files are matched by their name.".to_owned(),
        ];
        for (n, line) in hint.iter().enumerate() {
            sp::label(
                c,
                &fit_label(line, width, 10.0),
                p.x0 + PAD,
                p.y0 + HEADER_H + 14.0 + n as f64 * 13.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        let search = self.search_rect();
        sp::button(c, st, search, "SEARCH FOLDER...", None, hovered(search));
        sp::line(
            c,
            p.x0,
            search.y1 + 7.5,
            p.x1,
            search.y1 + 7.5,
            st.separator,
        );
        let area = self.list_area();
        c.push_clip(&area.into());
        for (i, entry) in self.entries.iter().enumerate() {
            let y = area.y0 + i as f64 * ROW_H - self.scroll;
            if y + ROW_H < area.y0 || y > area.y1 {
                continue;
            }
            let row = Rect::new(area.x0, y, area.x1, y + ROW_H);
            if i % 2 == 1 {
                sp::rect(c, row, st.lane_alt);
            }
            let name_w = (row.width() - 2.0 * PAD - 110.0 - 100.0).max(60.0);
            sp::label(
                c,
                &fit_label(&entry.name, name_w, 11.0),
                row.x0 + PAD,
                row.y0 + 18.0,
                11.0,
                t.text,
                Align::Start,
            );
            let short: String = entry.hash.chars().take(8).collect();
            sp::label(
                c,
                &short,
                row.x1 - PAD - 110.0 - 12.0,
                row.y0 + 18.0,
                10.0,
                t.text_muted,
                Align::End,
            );
            if let Some(button) = self.locate_rect(i) {
                sp::button(c, st, button, "LOCATE...", None, hovered(button));
            }
        }
        c.pop_clip();
        c.pop_clip();
        c.pop_transform();
    }
}
