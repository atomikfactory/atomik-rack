//! The repetition panel: the ideas the song repeats three times or more unchanged, each with where
//! it is and three suggested variations, a simple one a button away.
//!
//! ```text
//! FIND REPETITION                                        [ FIND ]  [ CLOSE ]
//! Drums   a 1-bar pattern, 4 times (bars 5-8)       [DROP A HIT] [FILTER ...] [CUT A BEAT]
//! Mute the last hit of the last bar
//! ...
//! note line
//! ```
//!
//! A modal view the app places over the window like the mix check. The app fills it with a
//! [`RepetitionData`] when it has searched the song ([`RepetitionView::set_data`]); the view only
//! draws and reports what was pressed as a [`UiAction::Repetition`]:
//!
//! * `FIND` ([`RepetitionAction::Find`]) searches the song again (the list is made on opening, and
//!   after an edit the ideas can change).
//! * A row chooses the idea (also Up and Down; the wheel scrolls) and emits
//!   [`RepetitionAction::Locate`]: the app puts the playhead at the start of the run.
//! * A suggestion button that is an edit ([`RepetitionChoice::can_apply`]) emits
//!   [`RepetitionAction::Apply`], once per suggestion (it shows `DONE` after); a suggestion that is
//!   advice only shows its text in the line under the idea and in the foot of the panel.
//! * `CLOSE` emits [`RepetitionAction::Close`]; the app also closes the panel on Esc.

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
const ROW_H: f64 = 62.0;
const FOOT_H: f64 = 30.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 900.0;
const BTN_H: f64 = 22.0;
const BTN_W: f64 = 118.0;
const BTN_GAP: f64 = 4.0;
const FONT: f32 = 10.5;
/// The ideas the list shows at once.
pub const ROWS: usize = 6;
/// The suggestions of an idea (the buttons of a row).
pub const CHOICES: usize = 3;

/// One suggestion of an idea.
#[derive(Debug, Clone, PartialEq)]
pub struct RepetitionChoice {
    /// The label of its button.
    pub label: String,
    /// What it does, or what to do by hand when it is advice only.
    pub text: String,
    /// It is an edit the app can make in one undo step.
    pub can_apply: bool,
    /// The edit was made.
    pub done: bool,
}

/// One idea, as the panel draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct RepetitionRow {
    /// The track (or the track's device when it has no name).
    pub track: String,
    /// What repeats, as a few words.
    pub what: String,
    /// How many times it plays.
    pub repeats: u32,
    /// Where: the bars the run covers (`bars 5-8`).
    pub bars: String,
    /// The suggestions, in the order of the buttons.
    pub choices: Vec<RepetitionChoice>,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepetitionData {
    /// How many tracks were searched.
    pub tracks: usize,
    /// The ideas, in time order.
    pub rows: Vec<RepetitionRow>,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepetitionAction {
    /// Search the song again.
    Find,
    /// Put the playhead at the start of idea `n`.
    Locate(usize),
    /// Make suggestion `k` of idea `n` (one undo step).
    Apply(usize, usize),
    /// Close the panel.
    Close,
}

/// The repetition panel. See the module documentation.
#[derive(Debug)]
pub struct RepetitionView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: Option<RepetitionData>,
    note: String,
    chosen: usize,
    top: usize,
    hover: Option<Point>,
    repaint: bool,
}

impl RepetitionView {
    /// A panel with nothing searched yet; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 960.0, 600.0),
            scale: 1.0,
            data: None,
            note: String::new(),
            chosen: 0,
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

    /// Shows the ideas of a search. The chosen idea and the scroll stay when the list has the same
    /// length (an edit changes a row, not the list).
    pub fn set_data(&mut self, data: Option<RepetitionData>) {
        if self.data != data {
            let same =
                matches!((&self.data, &data), (Some(a), Some(b)) if a.rows.len() == b.rows.len());
            if !same {
                self.chosen = 0;
                self.top = 0;
            }
            self.data = data;
            self.repaint = true;
        }
    }

    /// The line at the foot (what the last action did).
    pub fn set_note(&mut self, note: &str) {
        if self.note != note {
            note.clone_into(&mut self.note);
            self.repaint = true;
        }
    }

    /// The ideas that are shown.
    pub fn data(&self) -> Option<&RepetitionData> {
        self.data.as_ref()
    }

    /// The idea that is chosen.
    pub fn chosen(&self) -> usize {
        self.chosen
    }

    /// The first idea the list shows.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The line at the foot, as set.
    pub fn note(&self) -> &str {
        &self.note
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    fn count(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.rows.len())
    }

    fn row(&self, n: usize) -> Option<&RepetitionRow> {
        self.data.as_ref()?.rows.get(n)
    }

    fn choose(&mut self, n: usize) {
        let count = self.count();
        if count > 0 {
            self.chosen = n.min(count - 1);
            if self.chosen < self.top {
                self.top = self.chosen;
            } else if self.chosen >= self.top + ROWS {
                self.top = self.chosen + 1 - ROWS;
            }
        }
        self.repaint = true;
    }

    fn scroll(&mut self, by: isize) {
        let most = self.count().saturating_sub(ROWS);
        let top = (self.top as isize + by).clamp(0, most as isize) as usize;
        if top != self.top {
            self.top = top;
            self.repaint = true;
        }
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        HEADER_H + ROWS as f64 * ROW_H + FOOT_H + 3.0 * PAD
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

    /// The FIND button.
    pub fn find_rect(&self) -> Rect {
        let close = self.close_rect();
        Rect::new(close.x0 - 10.0 - 90.0, close.y0, close.x0 - 10.0, close.y1)
    }

    /// The list of ideas.
    pub fn list_rect(&self) -> Rect {
        let p = self.panel();
        let y0 = p.y0 + HEADER_H + PAD;
        Rect::new(p.x0 + PAD, y0, p.x1 - PAD, y0 + ROWS as f64 * ROW_H)
    }

    /// The row in slot `i` of the list (the idea `top() + i`).
    pub fn row_rect(&self, i: usize) -> Rect {
        let l = self.list_rect();
        Rect::new(
            l.x0,
            l.y0 + i as f64 * ROW_H,
            l.x1,
            l.y0 + (i + 1) as f64 * ROW_H,
        )
    }

    /// The button of suggestion `k` in the row of slot `i`.
    pub fn choice_rect(&self, i: usize, k: usize) -> Rect {
        let r = self.row_rect(i);
        let from_right = (CHOICES - 1 - k.min(CHOICES - 1)) as f64;
        let x1 = r.x1 - 6.0 - from_right * (BTN_W + BTN_GAP);
        Rect::new(x1 - BTN_W, r.y0 + 8.0, x1, r.y0 + 8.0 + BTN_H)
    }

    fn slots(&self) -> usize {
        self.count().saturating_sub(self.top).min(ROWS)
    }

    /// The suggestion button under `pos`: `(slot, suggestion)`.
    fn choice_at(&self, pos: Point) -> Option<(usize, usize)> {
        (0..self.slots()).find_map(|i| {
            let row = self.row(self.top + i)?;
            (0..row.choices.len().min(CHOICES))
                .find(|k| self.choice_rect(i, *k).contains(pos))
                .map(|k| (i, k))
        })
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        if self.find_rect().contains(pos) {
            return help(
                self.find_rect(),
                "Search the song again for ideas that repeat three times or more unchanged",
            );
        }
        if let Some((i, k)) = self.choice_at(pos) {
            let row = self.row(self.top + i)?;
            let choice = row.choices.get(k)?;
            let rect = self.choice_rect(i, k);
            return if choice.done {
                help(rect, "Already made")
            } else if choice.can_apply {
                help(rect, "Make this variation (one undo step)")
            } else {
                help(rect, "This one is advice: click to read it below")
            };
        }
        if self.list_rect().contains(pos) && self.count() > 0 {
            return help(
                self.list_rect(),
                "Click an idea to choose it and move the playhead to where it starts",
            );
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = UiAction::Repetition;
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
            UiEvent::Wheel { delta, pos, .. } => {
                if self.list_rect().contains(*pos) {
                    self.scroll(if delta.y > 0.0 { -1 } else { 1 });
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(act(RepetitionAction::Close));
                } else if self.find_rect().contains(*pos) {
                    out.push(act(RepetitionAction::Find));
                } else if let Some(i) = (0..self.slots()).find(|i| self.row_rect(*i).contains(*pos))
                {
                    let n = self.top + i;
                    self.choose(n);
                    match self.choice_at(*pos) {
                        Some((_, k)) => {
                            let choice = self.row(n).and_then(|r| r.choices.get(k)).cloned();
                            if let Some(choice) = choice {
                                if choice.can_apply && !choice.done {
                                    out.push(act(RepetitionAction::Apply(n, k)));
                                } else if !choice.can_apply {
                                    self.note = choice.text;
                                }
                            }
                        }
                        None => out.push(act(RepetitionAction::Locate(n))),
                    }
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Up => self.choose(self.chosen.saturating_sub(1)),
                Key::Down => self.choose(self.chosen + 1),
                _ => {}
            },
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
            "FIND REPETITION",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let find = self.find_rect();
        sp::button(c, st, find, "FIND", None, hovered(find));
        self.paint_list(c);
        let foot = if self.note.is_empty() {
            match &self.data {
                None => "Find lists what the song plays three times or more in a row, unchanged",
                Some(d) if d.rows.is_empty() => "Nothing repeats three times in a row unchanged",
                Some(_) => "A variation at the end of a run renews it with what is already there",
            }
        } else {
            self.note.as_str()
        };
        sp::label(
            c,
            &fit_label(foot, p.width() - 2.0 * PAD, 9.5),
            p.x0 + PAD,
            p.y1 - PAD,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_list(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let list = self.list_rect();
        sp::lcd(c, st, list.inflate(2.0, 2.0), None);
        let Some(data) = &self.data else {
            sp::label(
                c,
                "No search yet",
                list.center().x,
                list.center().y,
                FONT,
                t.text_muted,
                Align::Center,
            );
            return;
        };
        if data.rows.is_empty() {
            sp::label(
                c,
                &format!("No repeated ideas in {} tracks", data.tracks),
                list.center().x,
                list.center().y,
                FONT,
                t.text_muted,
                Align::Center,
            );
            return;
        }
        let text_w = list.width() - CHOICES as f64 * (BTN_W + BTN_GAP) - 24.0;
        for i in 0..self.slots() {
            let n = self.top + i;
            let Some(row) = data.rows.get(n) else {
                continue;
            };
            let r = self.row_rect(i);
            if n == self.chosen {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            if i > 0 {
                sp::line(c, r.x0, r.y0 + 0.5, r.x1, r.y0 + 0.5, st.separator);
            }
            let head = format!(
                "{}   {}, {} times ({})",
                row.track, row.what, row.repeats, row.bars
            );
            sp::label(
                c,
                &fit_label(&head, text_w, FONT),
                r.x0 + 8.0,
                r.y0 + 17.0,
                FONT,
                t.text,
                Align::Start,
            );
            // The line under the idea: the suggestion under the pointer, else the first.
            let shown = self
                .hover
                .and_then(|h| self.choice_at(h))
                .filter(|(slot, _)| *slot == i)
                .and_then(|(_, k)| row.choices.get(k))
                .or_else(|| row.choices.first());
            if let Some(choice) = shown {
                sp::label(
                    c,
                    &fit_label(&choice.text, r.width() - 16.0, 9.5),
                    r.x0 + 8.0,
                    r.y0 + 50.0,
                    9.5,
                    t.text_muted,
                    Align::Start,
                );
            }
            for (k, choice) in row.choices.iter().enumerate().take(CHOICES) {
                let b = self.choice_rect(i, k);
                let label = if choice.done {
                    "DONE".to_owned()
                } else {
                    choice.label.to_uppercase()
                };
                let fill = (choice.can_apply && !choice.done).then_some(st.loop_on);
                sp::button(
                    c,
                    st,
                    b,
                    &fit_label(&label, BTN_W - 8.0, 9.0),
                    fill,
                    self.hover.is_some_and(|h| b.contains(h)),
                );
            }
        }
        if data.rows.len() > ROWS {
            sp::label(
                c,
                &format!(
                    "{}-{} of {}",
                    self.top + 1,
                    self.top + self.slots(),
                    data.rows.len()
                ),
                list.x1 - 4.0,
                list.y1 + 12.0,
                9.0,
                t.text_muted,
                Align::End,
            );
        }
    }
}
