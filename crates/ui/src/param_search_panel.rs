//! The parameter search: a field and a list of every param of the project that matches it, each
//! as `track > device > param` with its current value, and the actions on the chosen row.
//!
//! ```text
//! FIND PARAMETER                                                        [ CLOSE ]
//! > lead cut|                                                           3 of 41
//!   Lead > Poly Synth > Cutoff                                M2  MIDI  ..  1.20 kHz
//!   Pad > Poly Synth 2 > Cutoff                                            4.00 kHz
//! [ REVEAL ] [ LANE ] [ MACRO ] [ LEARN ]   Ctrl+Left and Right adjust, Enter reveals
//! ```
//!
//! A modal view the app places over the window like the other panels. It has no document: the
//! app gives it the rows of the current query with [`ParamSearchView::set_rows`] (and the number
//! of matches the list was cut from) and every change of the query arrives as
//! [`ParamSearchAction::Query`], to which the app answers with fresh rows. The other actions name
//! a row by its position in the rows given last:
//!
//! * **Enter** or a double click reveals the param (selects its device in the rack, or its group
//!   in the console) and closes the panel; **Ctrl/Cmd+L** or LANE shows its automation lane;
//!   **Ctrl/Cmd+M** or MACRO assigns it to the next free macro of the current bank;
//!   **Ctrl/Cmd+E** or LEARN arms it for MIDI learn;
//! * **Ctrl/Cmd+Left** and **Right** (Shift: finer), or the wheel over the value, adjust the value of
//!   the chosen row in place ([`ParamSearchAction::Adjust`], one undo step per move);
//! * the arrows, Page Up and Down, Home and End move the choice; typing edits the query
//!   (Backspace deletes a letter, Delete clears it); Esc closes.
//!
//! A row can be greyed for an action the param does not allow (a VCA level has no lane, a driven
//! param cannot be adjusted or assigned again). Only the rows in view are painted.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const FIELD_H: f64 = 30.0;
const ROW_H: f64 = 24.0;
const FOOT_H: f64 = 34.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 900.0;
const MIN_WIDTH: f64 = 560.0;
const BTN_H: f64 = 20.0;
/// Most letters of a query.
const MAX_QUERY_CHARS: usize = 80;

/// One result row.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSearchRow {
    /// The track (`Lead`, `Master`, `Mixer`, `Rack`).
    pub track: String,
    /// The device's name.
    pub device: String,
    /// The param's name.
    pub param: String,
    /// The current value as text.
    pub value: String,
    /// The macro that drives the param (`M2`), if one does.
    pub macro_tag: Option<String>,
    /// A hardware control is mapped to it.
    pub midi: bool,
    /// The param has an automation lane.
    pub lane: bool,
    /// LANE can work (the device has a song track).
    pub can_lane: bool,
    /// MACRO can work (an assignable param that no macro drives yet).
    pub can_macro: bool,
    /// The value can be adjusted here (not a driven param).
    pub can_adjust: bool,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamSearchAction {
    /// The query changed: answer with the rows for it.
    Query(String),
    /// Select the param of row `n` in the rack or the console and close the panel.
    Reveal(usize),
    /// Show the automation lane of row `n` in the arrangement.
    Lane(usize),
    /// Assign row `n` to the next free macro of the current bank.
    Assign(usize),
    /// Arm row `n` for MIDI learn (the panel gives way).
    Learn(usize),
    /// Move the value of row `n` by `steps` steps of a fiftieth of its range (a step of a stepped
    /// param); `fine` is a tenth of that.
    Adjust {
        /// The row.
        row: usize,
        /// How many steps, negative for down.
        steps: i32,
        /// The fine step.
        fine: bool,
    },
    /// Close the panel.
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Close,
    Row(usize),
    Value(usize),
    Reveal,
    Lane,
    Macro,
    Learn,
}

/// The parameter search panel. See the module documentation.
#[derive(Debug)]
pub struct ParamSearchView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    query: String,
    rows: Vec<ParamSearchRow>,
    total: usize,
    selected: usize,
    scroll: f64,
    hover: Option<Point>,
    repaint: bool,
}

impl ParamSearchView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// An empty panel; call [`Self::set_bounds`] and [`Self::set_rows`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 1000.0, 600.0),
            scale: 1.0,
            query: String::new(),
            rows: Vec::new(),
            total: 0,
            selected: 0,
            scroll: 0.0,
            hover: None,
            repaint: true,
        }
    }

    /// Places the panel: the area it may use (logical pixels).
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

    /// The query text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Replaces the rows (the best matches, in order) and the number of matches they were cut
    /// from. The chosen row stays where it is when it still exists. Cheap when nothing changed.
    pub fn set_rows(&mut self, rows: Vec<ParamSearchRow>, total: usize) {
        if rows == self.rows && total == self.total {
            return;
        }
        self.rows = rows;
        self.total = total;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The rows shown.
    pub fn rows(&self) -> &[ParamSearchRow] {
        &self.rows
    }

    /// The chosen row.
    pub fn selected(&self) -> Option<usize> {
        (!self.rows.is_empty()).then_some(self.selected)
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN)
            .clamp(MIN_WIDTH.min(self.bounds.width()), MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let wanted = HEADER_H + FIELD_H + self.rows.len().max(3) as f64 * ROW_H + FOOT_H;
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + wanted.min(room))
    }

    fn list_area(&self) -> Rect {
        let p = self.panel();
        Rect::new(p.x0, p.y0 + HEADER_H + FIELD_H, p.x1, p.y1 - FOOT_H)
    }

    fn visible_rows(&self) -> usize {
        ((self.list_area().height() / ROW_H).floor() as usize).max(1)
    }

    fn clamp_scroll(&mut self) {
        let max = (self.rows.len() as f64 * ROW_H - self.list_area().height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// Scrolls so the chosen row is in view.
    fn show_selected(&mut self) {
        let top = self.selected as f64 * ROW_H;
        let h = self.list_area().height();
        if top < self.scroll {
            self.scroll = top;
        } else if top + ROW_H > self.scroll + h {
            self.scroll = top + ROW_H - h;
        }
        self.clamp_scroll();
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 7.0,
            p.x1 - PAD,
            p.y0 + 7.0 + BTN_H,
        )
    }

    /// The row `i`, `None` when it is scrolled out of the list.
    pub fn row_rect(&self, i: usize) -> Option<Rect> {
        if i >= self.rows.len() {
            return None;
        }
        let area = self.list_area();
        let y = area.y0 + i as f64 * ROW_H - self.scroll;
        let row = Rect::new(area.x0, y, area.x1, y + ROW_H);
        (row.y0 >= area.y0 - 0.5 && row.y1 <= area.y1 + 0.5).then_some(row)
    }

    /// The value cell of a row.
    fn value_cell(&self, row: Rect) -> Rect {
        Rect::new(
            row.x1 - PAD - 90.0,
            row.y0 + 2.0,
            row.x1 - PAD,
            row.y1 - 2.0,
        )
    }

    /// The action buttons of the footer: reveal, lane, macro, learn.
    fn foot_button(&self, n: usize) -> Rect {
        let p = self.panel();
        let x = p.x0 + PAD + n as f64 * 86.0;
        let y = p.y1 - FOOT_H + 7.0;
        Rect::new(x, y, x + 80.0, y + BTN_H)
    }

    fn hit(&self, pos: Point) -> Option<Hit> {
        if self.close_rect().contains(pos) {
            return Some(Hit::Close);
        }
        for (n, hit) in [Hit::Reveal, Hit::Lane, Hit::Macro, Hit::Learn]
            .into_iter()
            .enumerate()
        {
            if self.foot_button(n).contains(pos) {
                return Some(hit);
            }
        }
        if !self.list_area().contains(pos) {
            return None;
        }
        (0..self.rows.len()).find_map(|i| {
            let row = self.row_rect(i)?;
            row.contains(pos).then(|| {
                if self.value_cell(row).contains(pos) {
                    Hit::Value(i)
                } else {
                    Hit::Row(i)
                }
            })
        })
    }

    // ------------------------------------------------------------------ events

    /// What the button or value under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |r: Rect, text: &'static str| Some(Help::new(r, text));
        match self.hit(pos)? {
            Hit::Close => help(self.close_rect(), "Close the search"),
            Hit::Reveal => help(
                self.foot_button(0),
                "Show the selected parameter in the rack",
            ),
            Hit::Lane => help(
                self.foot_button(1),
                "Show the selected parameter's automation lane in the arrangement",
            ),
            Hit::Macro => help(
                self.foot_button(2),
                "Drive the selected parameter from a macro",
            ),
            Hit::Learn => help(
                self.foot_button(3),
                "Learn: move a knob or key to control the selected parameter",
            ),
            Hit::Value(i) => help(
                self.value_cell(self.row_rect(i)?),
                "Value: scroll to change (Shift: fine)",
            ),
            Hit::Row(_) => None,
        }
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
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
                ..
            } => {
                self.repaint = true;
                match self.hit(*pos) {
                    Some(Hit::Close) => out.push(act(ParamSearchAction::Close)),
                    Some(Hit::Row(i) | Hit::Value(i)) => {
                        self.selected = i;
                        if *clicks >= 2 {
                            out.push(act(ParamSearchAction::Reveal(i)));
                        }
                    }
                    Some(Hit::Reveal) => out.extend(self.on_selected(ParamSearchAction::Reveal)),
                    Some(Hit::Lane) => out.extend(self.on_selected(ParamSearchAction::Lane)),
                    Some(Hit::Macro) => out.extend(self.on_selected(ParamSearchAction::Assign)),
                    Some(Hit::Learn) => out.extend(self.on_selected(ParamSearchAction::Learn)),
                    None => {}
                }
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => {
                self.repaint = true;
                match self.hit(*pos) {
                    Some(Hit::Value(i)) if delta.y != 0.0 => {
                        self.selected = i;
                        out.push(act(ParamSearchAction::Adjust {
                            row: i,
                            steps: if delta.y > 0.0 { 1 } else { -1 },
                            fine: modifiers.shift,
                        }));
                    }
                    _ => {
                        self.scroll -= delta.y * ROW_H;
                        self.clamp_scroll();
                    }
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

    fn on_selected(&self, make: fn(usize) -> ParamSearchAction) -> Option<UiAction> {
        self.selected().map(|i| act(make(i)))
    }

    fn query_changed(&mut self) -> UiAction {
        self.selected = 0;
        self.scroll = 0.0;
        self.repaint = true;
        act(ParamSearchAction::Query(self.query.clone()))
    }

    fn key(&mut self, key: &Key, m: crate::event::Modifiers) -> Vec<UiAction> {
        self.repaint = true;
        let page = self.visible_rows();
        let last = self.rows.len().saturating_sub(1);
        match key {
            Key::Escape => return vec![act(ParamSearchAction::Close)],
            Key::Enter => {
                return self
                    .on_selected(ParamSearchAction::Reveal)
                    .into_iter()
                    .collect();
            }
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(last),
            Key::PageUp => self.selected = self.selected.saturating_sub(page),
            Key::PageDown => self.selected = (self.selected + page).min(last),
            Key::Home => self.selected = 0,
            Key::End => self.selected = last,
            Key::Left | Key::Right if m.command() => {
                let steps = if matches!(key, Key::Right) { 1 } else { -1 };
                return self
                    .selected()
                    .map(|row| {
                        act(ParamSearchAction::Adjust {
                            row,
                            steps,
                            fine: m.shift,
                        })
                    })
                    .into_iter()
                    .collect();
            }
            Key::Char(c) if m.command() => {
                let make: Option<fn(usize) -> ParamSearchAction> = match c.to_ascii_lowercase() {
                    'l' => Some(ParamSearchAction::Lane),
                    'm' => Some(ParamSearchAction::Assign),
                    'e' => Some(ParamSearchAction::Learn),
                    _ => None,
                };
                return make
                    .and_then(|make| self.on_selected(make))
                    .into_iter()
                    .collect();
            }
            Key::Char(c) if !m.ctrl && !m.meta && !c.is_control() => {
                if self.query.chars().count() < MAX_QUERY_CHARS {
                    self.query.push(*c);
                    return vec![self.query_changed()];
                }
            }
            Key::Space if self.query.chars().count() < MAX_QUERY_CHARS => {
                self.query.push(' ');
                return vec![self.query_changed()];
            }
            Key::Backspace => {
                if self.query.pop().is_some() {
                    return vec![self.query_changed()];
                }
            }
            Key::Delete if !self.query.is_empty() => {
                self.query.clear();
                return vec![self.query_changed()];
            }
            _ => {}
        }
        self.show_selected();
        Vec::new()
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
            "FIND PARAMETER",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        // The query field.
        let field = Rect::new(
            p.x0 + PAD,
            p.y0 + HEADER_H + 4.0,
            p.x1 - PAD,
            p.y0 + HEADER_H + FIELD_H - 4.0,
        );
        sp::rect(c, field, st.header);
        let shown = if self.query.is_empty() {
            "Type a track, device, param or unit: lead cutoff, drums > fader, hz".to_owned()
        } else {
            format!("{}|", self.query)
        };
        let colour = if self.query.is_empty() {
            t.text_muted
        } else {
            t.text
        };
        sp::label(
            c,
            &fit_label(&shown, field.width() - 130.0, 10.5),
            field.x0 + 8.0,
            field.y0 + 15.0,
            10.5,
            colour,
            Align::Start,
        );
        let count = if self.total > self.rows.len() {
            format!("{} of {}", self.rows.len(), self.total)
        } else {
            format!("{}", self.total)
        };
        sp::label(
            c,
            &count,
            field.x1 - 8.0,
            field.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::End,
        );
        // The rows.
        let area = self.list_area();
        c.push_clip(&area.into());
        if self.rows.is_empty() {
            sp::label(
                c,
                "No parameter matches.",
                area.x0 + PAD,
                area.y0 + 20.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        for (i, r) in self.rows.iter().enumerate() {
            let y = area.y0 + i as f64 * ROW_H - self.scroll;
            if y + ROW_H < area.y0 || y > area.y1 {
                continue;
            }
            let row = Rect::new(area.x0, y, area.x1, y + ROW_H);
            if i == self.selected {
                sp::rect(c, row, t.selection.with_alpha(0.35));
            } else if i % 2 == 1 {
                sp::rect(c, row, st.lane_alt);
            }
            let value = self.value_cell(row);
            let mut right = value.x0 - 8.0;
            for (on, text) in [
                (r.lane, "LANE"),
                (r.midi, "MIDI"),
                (r.macro_tag.is_some(), r.macro_tag.as_deref().unwrap_or("")),
            ] {
                if on {
                    sp::label(c, text, right, row.y0 + 16.0, 8.5, t.highlight, Align::End);
                    right -= 8.0 + text.chars().count() as f64 * 5.6;
                }
            }
            let path = format!("{} > {} > {}", r.track, r.device, r.param);
            sp::label(
                c,
                &fit_label(&path, right - (row.x0 + PAD), 10.5),
                row.x0 + PAD,
                row.y0 + 16.0,
                10.5,
                t.text,
                Align::Start,
            );
            let vcolour = if r.can_adjust { t.text } else { t.text_muted };
            sp::label(
                c,
                &fit_label(&r.value, value.width(), 10.0),
                value.x1,
                row.y0 + 16.0,
                10.0,
                vcolour,
                Align::End,
            );
        }
        c.pop_clip();
        // The footer.
        sp::line(
            c,
            p.x0,
            p.y1 - FOOT_H + 0.5,
            p.x1,
            p.y1 - FOOT_H + 0.5,
            st.separator,
        );
        let sel = self.rows.get(self.selected);
        for (n, (text, on)) in [
            ("REVEAL", sel.is_some()),
            ("LANE", sel.is_some_and(|r| r.can_lane)),
            ("MACRO", sel.is_some_and(|r| r.can_macro)),
            ("LEARN", sel.is_some()),
        ]
        .into_iter()
        .enumerate()
        {
            let r = self.foot_button(n);
            let lit = None;
            if on {
                sp::button(c, st, r, text, lit, hovered(r));
            } else {
                sp::button(c, st, r, text, None, false);
                sp::rect(c, r, st.background.with_alpha(0.55));
            }
        }
        sp::label(
            c,
            &fit_label(
                "Enter reveals, Ctrl+L lane, Ctrl+M macro, Ctrl+E MIDI learn, Ctrl+Left/Right adjust",
                p.width() - 4.0 * 86.0 - 2.0 * PAD,
                9.0,
            ),
            p.x0 + PAD + 4.0 * 86.0,
            p.y1 - FOOT_H + 21.0,
            9.0,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}

fn act(a: ParamSearchAction) -> UiAction {
    UiAction::ParamSearch(a)
}

impl ParamSearchView {
    /// The centre of row `i`'s name part and value cell, for tests and the headless driver.
    pub fn row_center(&self, i: usize, value: bool) -> Option<Point> {
        let row = self.row_rect(i)?;
        Some(if value {
            self.value_cell(row).center()
        } else {
            Point::new(row.x0 + 60.0, row.center().y)
        })
    }

    /// The centre of a footer button (`reveal`, `lane`, `macro`, `learn`).
    pub fn button_center(&self, what: &str) -> Option<Point> {
        let n = ["reveal", "lane", "macro", "learn"]
            .iter()
            .position(|w| *w == what)?;
        Some(self.foot_button(n).center())
    }
}
