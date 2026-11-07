//! The reverb send wizard: a small modal with the choices of Track > Add Reverb Send.
//!
//! ```text
//! ADD REVERB SEND                                                        [ CLOSE ]
//! Track: Lead
//! De-esser (vocal)  [-]  no           [+]
//! Duck from track   [-]  yes          [+]
//! Send              [-]  post-fader   [+]
//! Decay             [-]  auto: 1.0 s  [+]
//! High-pass         [-]  300 Hz       [+]
//! Low-pass          [-]  8000 Hz      [+]
//! Send level        [-]  -12 dB       [+]
//! [ ADD ]
//! ```
//!
//! A modal view the app places over the window like the strip silence panel. It has no document:
//! the app tells it the track and the decay the track suggests ([`ReverbSendView::set_target`])
//! and reads the choices back ([`ReverbSendView::values`]) when [`UiAction::ReverbSend`] arrives:
//!
//! * `ADD` emits [`ReverbSendAction::Add`] (not while no track is selected).
//! * `CLOSE` emits [`ReverbSendAction::Close`].
//!
//! Every row has a `-` and a `+` button and answers the wheel and a drag; the three yes/no rows
//! switch with either button. The choices live in [`ReverbSendValues`], which the app keeps
//! between openings; the defaults are the sensible ones: no de-esser, a duck from the track, a
//! post-fader send at -12 dB, the decay suggested from the track, a high-pass at 300 Hz and a
//! low-pass at 8 kHz.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::step_drag::{PX_PER_STEP, StepDrag, wheel_steps};
use crate::theme::Theme;
use crate::tooltip::{Help, stepper_help};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const TARGET_H: f64 = 24.0;
const ROW_H: f64 = 26.0;
const ADD_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 440.0;
const BTN_H: f64 = 20.0;
const STEP_BTN_W: f64 = 24.0;
const VALUE_W: f64 = 130.0;

/// The decays the row steps through, in tenths of a second.
pub const DECAYS: [u32; 14] = [4, 6, 8, 10, 12, 15, 20, 25, 30, 40, 50, 60, 80, 100];
/// The high-pass frequencies the row steps through, in Hz.
pub const HIGH_PASSES: [u32; 10] = [100, 150, 200, 250, 300, 350, 400, 500, 600, 800];
/// The low-pass frequencies the row steps through, in Hz.
pub const LOW_PASSES: [u32; 9] = [
    4_000, 5_000, 6_000, 7_000, 8_000, 9_000, 10_000, 12_000, 16_000,
];
/// The lowest send level, in dB.
pub const SEND_MIN_DB: i32 = -30;
/// The highest send level, in dB.
pub const SEND_MAX_DB: i32 = 0;

/// The choices of the wizard; the app keeps them between openings.
#[derive(Debug, Clone, PartialEq)]
pub struct ReverbSendValues {
    /// A de-esser before the EQ.
    pub vocal: bool,
    /// The reverb is keyed by the track and ducks.
    pub duck: bool,
    /// The send taps the signal before the fader.
    pub pre_fader: bool,
    /// The decay in seconds; `None` takes the one the track suggests.
    pub decay: Option<f32>,
    /// The EQ's high-pass in Hz.
    pub high_pass: u32,
    /// The EQ's low-pass in Hz.
    pub low_pass: u32,
    /// The send level in dB.
    pub send_db: i32,
}

impl Default for ReverbSendValues {
    fn default() -> Self {
        Self {
            vocal: false,
            duck: true,
            pre_fader: false,
            decay: None,
            high_pass: 300,
            low_pass: 8_000,
            send_db: -12,
        }
    }
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReverbSendAction {
    /// Add the reverb send of the target track with the panel's choices.
    Add,
    /// Close the panel.
    Close,
}

/// A row of choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverbSendRow {
    /// A de-esser before the EQ.
    Vocal,
    /// The duck from the track.
    Duck,
    /// Pre- or post-fader.
    Position,
    /// The decay.
    Decay,
    /// The EQ's high-pass.
    HighPass,
    /// The EQ's low-pass.
    LowPass,
    /// The send level.
    Level,
}

impl ReverbSendRow {
    /// Every row, top down.
    pub const ALL: [ReverbSendRow; 7] = [
        ReverbSendRow::Vocal,
        ReverbSendRow::Duck,
        ReverbSendRow::Position,
        ReverbSendRow::Decay,
        ReverbSendRow::HighPass,
        ReverbSendRow::LowPass,
        ReverbSendRow::Level,
    ];

    fn title(self) -> &'static str {
        match self {
            ReverbSendRow::Vocal => "De-esser (vocal)",
            ReverbSendRow::Duck => "Duck from track",
            ReverbSendRow::Position => "Send",
            ReverbSendRow::Decay => "Decay",
            ReverbSendRow::HighPass => "High-pass",
            ReverbSendRow::LowPass => "Low-pass",
            ReverbSendRow::Level => "Send level",
        }
    }

    fn is_switch(self) -> bool {
        matches!(
            self,
            ReverbSendRow::Vocal | ReverbSendRow::Duck | ReverbSendRow::Position
        )
    }
}

/// The entry of `list` next to `value` in direction `dir` (the nearest entry first when `value`
/// is not on the list).
fn step_list(list: &[u32], value: u32, dir: i32) -> u32 {
    let last = list.len() - 1;
    match list.iter().position(|v| *v >= value) {
        None => list[last],
        Some(at) if list[at] == value => list[(at as i32 + dir).clamp(0, last as i32) as usize],
        Some(at) => list[if dir > 0 { at } else { at.saturating_sub(1) }],
    }
}

/// The reverb send wizard panel. See the module documentation.
#[derive(Debug)]
pub struct ReverbSendView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    values: ReverbSendValues,
    target: String,
    suggested: f32,
    can_add: bool,
    hover: Option<Point>,
    /// A drag on a value: the row and the vertical travel (up steps the value up).
    drag: Option<(ReverbSendRow, StepDrag)>,
    repaint: bool,
}

impl ReverbSendView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A panel with these choices; call [`Self::set_bounds`] and [`Self::set_target`].
    pub fn new(theme: Theme, values: ReverbSendValues) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            values,
            target: String::new(),
            suggested: 2.0,
            can_add: false,
            hover: None,
            drag: None,
            repaint: true,
        }
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

    /// Says which track ADD would work on (`name`, empty for none) and the decay it suggests.
    pub fn set_target(&mut self, name: String, suggested_decay: f32) {
        let can_add = !name.is_empty();
        if (&name, suggested_decay.to_bits(), can_add)
            != (&self.target, self.suggested.to_bits(), self.can_add)
        {
            self.target = name;
            self.suggested = suggested_decay;
            self.can_add = can_add;
            self.repaint = true;
        }
    }

    /// The choices.
    pub fn values(&self) -> &ReverbSendValues {
        &self.values
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The text of a row's value.
    pub fn value_text(&self, row: ReverbSendRow) -> String {
        let v = &self.values;
        let yes = |on: bool| if on { "yes" } else { "no" }.to_owned();
        match row {
            ReverbSendRow::Vocal => yes(v.vocal),
            ReverbSendRow::Duck => yes(v.duck),
            ReverbSendRow::Position => if v.pre_fader {
                "pre-fader"
            } else {
                "post-fader"
            }
            .to_owned(),
            ReverbSendRow::Decay => match v.decay {
                Some(d) => format!("{d:.1} s"),
                None => format!("auto: {:.1} s", self.suggested),
            },
            ReverbSendRow::HighPass => format!("{} Hz", v.high_pass),
            ReverbSendRow::LowPass => format!("{} Hz", v.low_pass),
            ReverbSendRow::Level => format!("{} dB", v.send_db),
        }
    }

    /// Steps a row by `dir` (+1 or -1): a click on `+` or `-`, a drag or a wheel notch. A yes/no
    /// row goes to yes (or pre-fader) with `+` and to no (or post-fader) with `-`.
    pub fn step(&mut self, row: ReverbSendRow, dir: i32) {
        let on = dir > 0;
        let suggested = self.suggested;
        let v = &mut self.values;
        match row {
            ReverbSendRow::Vocal => v.vocal = on,
            ReverbSendRow::Duck => v.duck = on,
            ReverbSendRow::Position => v.pre_fader = on,
            ReverbSendRow::Decay => {
                let tenths = |s: f32| (s * 10.0).round().max(0.0) as u32;
                v.decay = match v.decay {
                    // From the suggestion to the list entry nearest to it; the lowest entry's `-`
                    // goes back to the suggestion.
                    None => {
                        let near = DECAYS
                            .into_iter()
                            .min_by_key(|e| e.abs_diff(tenths(suggested)))
                            .unwrap_or(DECAYS[0]);
                        Some(near as f32 / 10.0)
                    }
                    Some(d) if dir < 0 && tenths(d) <= DECAYS[0] => None,
                    Some(d) => Some(step_list(&DECAYS, tenths(d), dir) as f32 / 10.0),
                };
            }
            ReverbSendRow::HighPass => v.high_pass = step_list(&HIGH_PASSES, v.high_pass, dir),
            ReverbSendRow::LowPass => v.low_pass = step_list(&LOW_PASSES, v.low_pass, dir),
            ReverbSendRow::Level => {
                v.send_db = (v.send_db + dir).clamp(SEND_MIN_DB, SEND_MAX_DB);
            }
        }
        self.repaint = true;
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
        HEADER_H + TARGET_H + ReverbSendRow::ALL.len() as f64 * ROW_H + ADD_H + PAD
    }

    fn rows_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + TARGET_H
    }

    /// The row of a value.
    pub fn row_rect(&self, row: ReverbSendRow) -> Rect {
        let p = self.panel();
        let i = ReverbSendRow::ALL
            .iter()
            .position(|r| *r == row)
            .unwrap_or(0);
        let y = self.rows_top() + i as f64 * ROW_H;
        Rect::new(p.x0, y, p.x1, y + ROW_H)
    }

    fn cluster_x(&self) -> f64 {
        self.panel().x0 + PAD + 140.0
    }

    /// The `-` button of a row.
    pub fn minus_rect(&self, row: ReverbSendRow) -> Rect {
        let r = self.row_rect(row);
        let x = self.cluster_x();
        let y = r.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + STEP_BTN_W, y + BTN_H)
    }

    /// The value window of a row.
    pub fn value_rect(&self, row: ReverbSendRow) -> Rect {
        let m = self.minus_rect(row);
        Rect::new(m.x1 + 3.0, m.y0, m.x1 + 3.0 + VALUE_W, m.y1)
    }

    /// The `+` button of a row.
    pub fn plus_rect(&self, row: ReverbSendRow) -> Rect {
        let v = self.value_rect(row);
        Rect::new(v.x1 + 3.0, v.y0, v.x1 + 3.0 + STEP_BTN_W, v.y1)
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H + 2.0,
        )
    }

    /// The ADD button.
    pub fn add_rect(&self) -> Rect {
        let p = self.panel();
        let top = self.rows_top() + ReverbSendRow::ALL.len() as f64 * ROW_H;
        let y = top + (ADD_H - BTN_H - 4.0) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 120.0, y + BTN_H + 4.0)
    }

    /// True while a value is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    // ------------------------------------------------------------------ events

    /// What the button or value under `pos` does, for its tooltip (`None` during a drag and
    /// away from a button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.drag.is_some() {
            return None;
        }
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the wizard; the choices are kept");
        }
        if self.add_rect().contains(pos) {
            return help(
                self.add_rect(),
                "Add the send, the return, the EQ and the reverb (one undo step)",
            );
        }
        ReverbSendRow::ALL.into_iter().find_map(|row| {
            let rects = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            stepper_help(pos, row.title(), rects, !row.is_switch())
        })
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                self.repaint = true;
                if let Some((row, steps)) = &mut self.drag {
                    let (row, n) = (*row, steps.update(pos.y, PX_PER_STEP));
                    for _ in 0..n.abs() {
                        self.step(row, n.signum());
                    }
                }
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => {
                self.drag = None;
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
                if let Some(action) = self.click(*pos) {
                    out.push(UiAction::ReverbSend(action));
                } else if let Some(row) = ReverbSendRow::ALL
                    .into_iter()
                    .find(|r| !r.is_switch() && self.value_rect(*r).contains(*pos))
                {
                    self.drag = Some((row, StepDrag::new(pos.y)));
                } else if let Some(row) = ReverbSendRow::ALL
                    .into_iter()
                    .find(|r| r.is_switch() && self.value_rect(*r).contains(*pos))
                {
                    // A click on the value of a yes/no row switches it.
                    let on = self.is_on(row);
                    self.step(row, if on { -1 } else { 1 });
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                self.repaint = true;
                let dir = wheel_steps(delta.y);
                if let Some(row) = ReverbSendRow::ALL
                    .into_iter()
                    .find(|r| self.row_rect(*r).contains(*pos))
                {
                    self.step(row, dir);
                }
            }
            _ => {}
        }
        out
    }

    fn is_on(&self, row: ReverbSendRow) -> bool {
        match row {
            ReverbSendRow::Vocal => self.values.vocal,
            ReverbSendRow::Duck => self.values.duck,
            ReverbSendRow::Position => self.values.pre_fader,
            _ => false,
        }
    }

    fn click(&mut self, pos: Point) -> Option<ReverbSendAction> {
        if self.close_rect().contains(pos) {
            return Some(ReverbSendAction::Close);
        }
        if self.add_rect().contains(pos) {
            return self.can_add.then_some(ReverbSendAction::Add);
        }
        for row in ReverbSendRow::ALL {
            if self.minus_rect(row).contains(pos) {
                self.step(row, -1);
                return None;
            }
            if self.plus_rect(row).contains(pos) {
                self.step(row, 1);
                return None;
            }
        }
        None
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
            "ADD REVERB SEND",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let width = p.width() - 2.0 * PAD;
        let (target_text, target_color) = if self.target.is_empty() {
            ("no track selected", t.text_muted)
        } else {
            (self.target.as_str(), t.text)
        };
        sp::label(
            c,
            &fit_label(&format!("Track: {target_text}"), width, 10.0),
            p.x0 + PAD,
            p.y0 + HEADER_H + 15.0,
            10.0,
            target_color,
            Align::Start,
        );
        for row in ReverbSendRow::ALL {
            let r = self.row_rect(row);
            sp::label(
                c,
                row.title(),
                r.x0 + PAD,
                r.y0 + 17.0,
                10.5,
                t.text,
                Align::Start,
            );
            let (minus, value, plus) = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            sp::button(c, st, minus, "-", None, hovered(minus));
            sp::lcd(c, st, value, None);
            sp::label(
                c,
                &fit_label(&self.value_text(row), VALUE_W - 12.0, 10.5),
                value.x0 + 8.0,
                value.y0 + 14.0,
                10.5,
                t.text,
                Align::Start,
            );
            sp::button(c, st, plus, "+", None, hovered(plus));
        }
        let add = self.add_rect();
        sp::button(
            c,
            st,
            add,
            "ADD",
            self.can_add.then_some(st.loop_on),
            self.can_add && hovered(add),
        );
        c.pop_clip();
        c.pop_transform();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_step_to_the_neighbouring_entry() {
        assert_eq!(step_list(&HIGH_PASSES, 300, 1), 350);
        assert_eq!(step_list(&HIGH_PASSES, 300, -1), 250);
        assert_eq!(step_list(&HIGH_PASSES, 100, -1), 100);
        assert_eq!(step_list(&HIGH_PASSES, 800, 1), 800);
        assert_eq!(step_list(&HIGH_PASSES, 320, 1), 350);
        assert_eq!(step_list(&HIGH_PASSES, 320, -1), 300);
        assert_eq!(step_list(&DECAYS, 10, 1), 12);
    }
}
