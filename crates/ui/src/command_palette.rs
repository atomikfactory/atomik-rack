//! The command palette: one window to find and run any command of the app by name.
//!
//! ```text
//! > quan|                                                          3 of 520
//!   Quantize                       Edit                              Ctrl+K
//!   Quantize Options               Edit
//!   Quantize Audio                 Edit > Audio
//! Up and Down choose, Enter runs, Esc closes
//! ```
//!
//! A floating panel the app places over the window, centred, like the other panels, but it never
//! blocks: it takes the keyboard while it is open (every key is text for its field or a key of
//! the list, so a typed letter never reaches a shortcut), and a click outside it closes it.
//!
//! The app gives it the commands ([`PaletteEntry`]: name, menu path or group, id, the shortcut
//! the live keymap has now, whether it can run) and the recent commands. The view ranks them
//! itself as the field changes (see [`crate::palette_rank`]), highlights the matched letters and
//! reports two things: [`PaletteAction::Run`] with the place of the command in the entries given
//! (Enter, or a click on a row; a command that cannot run now is listed dimmed and still asked
//! to run, so it can say why not) and [`PaletteAction::Close`] (Esc, a click outside).
//!
//! The field is a [`TextField`]: caret and selection, Ctrl+A, copy and paste, input methods.
//! Up and Down move the highlight, Page Up and Page Down move it a page, the wheel scrolls.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::{fit_label_in, measure};
use crate::palette_rank::{PaletteEntry, Ranked, rank};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const FIELD_H: f64 = 38.0;
const ROW_H: f64 = 26.0;
const FOOT_H: f64 = 24.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 700.0;
const MIN_ROWS: usize = 3;
const MAX_ROWS: usize = 10;
const FONT: f32 = 11.0;
const SMALL: f32 = 9.5;
/// Most letters of a query.
const MAX_QUERY_CHARS: usize = 80;

/// What the palette asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    /// Close the palette and run the command at this place of the entries given.
    Run(usize),
    /// Close the palette.
    Close,
}

/// The command palette. See the module documentation.
#[derive(Debug)]
pub struct CommandPaletteView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    entries: Vec<PaletteEntry>,
    recent: Vec<String>,
    field: TextField,
    ranked: Vec<Ranked>,
    selected: usize,
    /// First row shown.
    scroll: usize,
    dragging: bool,
    hover: Option<Point>,
    repaint: bool,
}

impl CommandPaletteView {
    /// A palette over `entries` with the field empty; `recent` lists command keys, newest first.
    /// Call [`Self::set_bounds`].
    pub fn new(theme: Theme, entries: Vec<PaletteEntry>, recent: Vec<String>) -> Self {
        let mut view = Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 1000.0, 700.0),
            scale: 1.0,
            entries,
            recent,
            field: TextField::new("", MAX_QUERY_CHARS, false),
            ranked: Vec::new(),
            selected: 0,
            scroll: 0,
            dragging: false,
            hover: None,
            repaint: true,
        };
        view.requery();
        view
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the palette: the area it may use (logical pixels). It is centred in it.
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

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the palette takes typed text (always: every key goes to its field or its list).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    /// The text typed.
    pub fn query(&self) -> &str {
        self.field.text()
    }

    /// Types `text` in place of the query (the app's tests and the headless driver).
    pub fn set_query(&mut self, text: &str) {
        self.field.set_text(text);
        self.requery();
    }

    /// The commands that match, best first.
    pub fn ranked(&self) -> &[Ranked] {
        &self.ranked
    }

    /// The commands given.
    pub fn entries(&self) -> &[PaletteEntry] {
        &self.entries
    }

    /// The entry of the row at `row` of the list.
    pub fn entry_at(&self, row: usize) -> Option<&PaletteEntry> {
        self.ranked.get(row).map(|r| &self.entries[r.entry])
    }

    /// The highlighted row.
    pub fn selected(&self) -> Option<usize> {
        (!self.ranked.is_empty()).then_some(self.selected)
    }

    fn requery(&mut self) {
        self.ranked = rank(&self.entries, self.field.text(), &self.recent);
        self.selected = 0;
        self.scroll = 0;
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry

    fn shown_rows(&self) -> usize {
        self.ranked.len().clamp(MIN_ROWS, MAX_ROWS)
    }

    fn height(rows: usize) -> f64 {
        FIELD_H + rows as f64 * ROW_H + FOOT_H
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let full = Self::height(MAX_ROWS).min(room);
        let y0 = self.bounds.y0 + MARGIN + ((room - full) / 2.0).max(0.0);
        let h = Self::height(self.shown_rows()).min(room);
        Rect::new(x0, y0, x0 + width, y0 + h)
    }

    /// The box of the field.
    pub fn field_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(p.x0 + PAD, p.y0 + 6.0, p.x1 - PAD, p.y0 + FIELD_H - 6.0)
    }

    fn list_area(&self) -> Rect {
        let p = self.panel();
        Rect::new(p.x0, p.y0 + FIELD_H, p.x1, p.y1 - FOOT_H)
    }

    fn visible_rows(&self) -> usize {
        ((self.list_area().height() / ROW_H).floor() as usize).max(1)
    }

    fn clamp_scroll(&mut self) {
        let max = self.ranked.len().saturating_sub(self.visible_rows());
        self.scroll = self.scroll.min(max);
    }

    fn show_selected(&mut self) {
        let visible = self.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        self.clamp_scroll();
    }

    /// The box of the list row `row`, `None` when it is scrolled out of view.
    pub fn row_rect(&self, row: usize) -> Option<Rect> {
        if row < self.scroll || row >= self.ranked.len() {
            return None;
        }
        let area = self.list_area();
        let y = area.y0 + (row - self.scroll) as f64 * ROW_H;
        (y + ROW_H <= area.y1 + 0.5).then(|| Rect::new(area.x0, y, area.x1, y + ROW_H))
    }

    /// The centre of the list row `row`, for tests and the headless driver.
    pub fn row_center(&self, row: usize) -> Option<Point> {
        self.row_rect(row).map(|r| r.center())
    }

    fn row_at(&self, pos: Point) -> Option<usize> {
        if !self.list_area().contains(pos) {
            return None;
        }
        (self.scroll..self.ranked.len())
            .find(|i| self.row_rect(*i).is_some_and(|r| r.contains(pos)))
    }

    // ------------------------------------------------------------------ events

    /// What is under `pos`, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let row = self.row_at(pos)?;
        let entry = self.entry_at(row)?;
        (!entry.enabled).then(|| {
            Help::new(
                self.row_rect(row).unwrap_or(self.panel()),
                "Not available right now: running it says why",
            )
        })
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                if self.dragging {
                    self.field.drag(*pos);
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.dragging = false,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if !self.panel().contains(*pos) {
                    out.push(act(PaletteAction::Close));
                } else if self.field_rect().contains(*pos) {
                    self.dragging = true;
                    self.field.press(*pos, *clicks, modifiers.shift);
                } else if let Some(row) = self.row_at(*pos) {
                    self.selected = row;
                    out.push(act(PaletteAction::Run(self.ranked[row].entry)));
                }
            }
            UiEvent::PointerDown { pos, .. } => {
                // Any other button outside the palette also gives way.
                if !self.panel().contains(*pos) {
                    out.push(act(PaletteAction::Close));
                }
            }
            UiEvent::Wheel { delta, pos, .. } => {
                if self.list_area().contains(*pos) && delta.y != 0.0 {
                    let rows = (delta.y.abs().ceil() as usize).max(1);
                    self.scroll = if delta.y > 0.0 {
                        self.scroll.saturating_sub(rows)
                    } else {
                        self.scroll + rows
                    };
                    self.clamp_scroll();
                    self.repaint = true;
                }
            }
            UiEvent::Ime(ime) => {
                if self.field.handle_ime(ime) {
                    self.requery();
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => out.extend(self.key(key, *modifiers)),
            _ => {}
        }
        out
    }

    fn key(&mut self, key: &Key, m: crate::event::Modifiers) -> Vec<UiAction> {
        self.repaint = true;
        let page = self.visible_rows();
        let last = self.ranked.len().saturating_sub(1);
        match key {
            Key::Escape => return vec![act(PaletteAction::Close)],
            Key::Enter => {
                return self
                    .ranked
                    .get(self.selected)
                    .map(|r| act(PaletteAction::Run(r.entry)))
                    .into_iter()
                    .collect();
            }
            Key::Up if !m.command() => self.selected = self.selected.saturating_sub(1),
            Key::Down if !m.command() => self.selected = (self.selected + 1).min(last),
            Key::PageUp => self.selected = self.selected.saturating_sub(page),
            Key::PageDown => self.selected = (self.selected + page).min(last),
            _ => {
                let before = self.field.text().to_owned();
                if self.field.handle_key(key, m).handled() && self.field.text() != before {
                    self.requery();
                }
                return Vec::new();
            }
        }
        self.show_selected();
        Vec::new()
    }

    // ------------------------------------------------------------------ painting

    /// Paints the palette (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        // The field.
        let field = self.field_rect();
        sp::lcd(c, st, field, Some(t.focus));
        if self.field.is_empty() && self.field.preedit().is_none() {
            sp::label(
                c,
                "Type a command: quantize, export, edit undo",
                field.x0 + 8.0,
                field.y0 + 17.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
        }
        let colors = FieldColors {
            text: t.text,
            caret: t.text,
            selection: mix(t.lcd, t.focus, 0.5),
        };
        self.field
            .paint(c, field, FONT, field.height() - 6.0, colors, true);
        let count = if self.field.is_empty() {
            format!("{}", self.entries.len())
        } else {
            format!("{} of {}", self.ranked.len(), self.entries.len())
        };
        sp::label(
            c,
            &count,
            field.x1 - 8.0,
            field.y0 + 17.0,
            SMALL,
            t.text_muted,
            Align::End,
        );
        sp::line(
            c,
            p.x0,
            p.y0 + FIELD_H - 0.5,
            p.x1,
            p.y0 + FIELD_H - 0.5,
            st.separator,
        );
        // The rows.
        let area = self.list_area();
        c.push_clip(&area.into());
        if self.ranked.is_empty() {
            sp::label(
                c,
                "No command matches.",
                area.x0 + PAD,
                area.y0 + 18.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
        }
        for row in self.scroll..self.ranked.len() {
            let Some(r) = self.row_rect(row) else { break };
            self.paint_row(c, row, r);
        }
        c.pop_clip();
        // The foot.
        sp::line(
            c,
            p.x0,
            p.y1 - FOOT_H + 0.5,
            p.x1,
            p.y1 - FOOT_H + 0.5,
            st.separator,
        );
        let hint = if self.field.is_empty() && !self.recent.is_empty() {
            "Recent commands first. Up and Down choose, Enter runs, Esc closes"
        } else {
            "Up and Down choose, Enter runs, Esc closes"
        };
        sp::label(
            c,
            hint,
            p.x0 + PAD,
            p.y1 - FOOT_H + 16.0,
            SMALL,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_row(&self, c: &mut dyn Canvas, row: usize, r: Rect) {
        let st = &self.st;
        let t = &st.base;
        let ranked = &self.ranked[row];
        let entry = &self.entries[ranked.entry];
        if row == self.selected {
            sp::rect(c, r, t.selection.with_alpha(0.35));
        } else if self.hover.is_some_and(|h| r.contains(h)) {
            sp::rect(c, r, t.selection.with_alpha(0.15));
        }
        let ink = if entry.enabled { t.text } else { t.text_muted };
        let hit = if entry.enabled {
            t.highlight
        } else {
            t.highlight.with_alpha(0.6)
        };
        let base = r.y0 + 17.0;
        // The shortcut, flush right.
        let mut right = r.x1 - PAD;
        if let Some(short) = &entry.shortcut {
            let w = measure(c, short, SMALL, 400);
            sp::label(c, short, right, base, SMALL, t.text_muted, Align::End);
            right -= w + 12.0;
        }
        let inner = r.x1 - r.x0 - 2.0 * PAD;
        let name_w = (inner * 0.48).min((right - r.x0 - PAD) * 0.7).max(20.0);
        highlighted(
            c,
            &entry.name,
            &ranked.name_hits,
            (r.x0 + PAD, base),
            name_w,
            (ink, hit),
        );
        let path_x = r.x0 + PAD + name_w + 12.0;
        if right - path_x > 24.0 {
            highlighted(
                c,
                &entry.path,
                &ranked.path_hits,
                (path_x, base),
                right - path_x,
                (t.text_muted, hit),
            );
        }
    }
}

fn act(a: PaletteAction) -> UiAction {
    UiAction::CommandPalette(a)
}

/// Draws `text` at `at` (left edge and baseline) cut to `width`, with the characters at `hits`
/// bold in the second colour.
fn highlighted(
    c: &mut dyn Canvas,
    text: &str,
    hits: &[usize],
    at: (f64, f64),
    width: f64,
    colours: (vello::peniko::Color, vello::peniko::Color),
) {
    let shown = fit_label_in(c, text, width, FONT);
    let mut x = at.0;
    let mut run = String::new();
    let mut run_hit = false;
    let flush = |c: &mut dyn Canvas, run: &mut String, hit: bool, x: &mut f64| {
        if run.is_empty() {
            return;
        }
        let (colour, weight) = if hit {
            (colours.1, 700)
        } else {
            (colours.0, 400)
        };
        c.text(
            &crate::canvas::TextRun::new(run.as_str(), Point::new(*x, at.1), FONT, colour)
                .weight(weight),
        );
        *x += measure(c, run, FONT, weight);
        run.clear();
    };
    for (i, ch) in shown.chars().enumerate() {
        let hit = hits.contains(&i);
        if hit != run_hit {
            flush(c, &mut run, run_hit, &mut x);
            run_hit = hit;
        }
        run.push(ch);
    }
    flush(c, &mut run, run_hit, &mut x);
}
