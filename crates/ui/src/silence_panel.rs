//! The strip silence panel: threshold, minimum silence, padding and fades.
//!
//! ```text
//! STRIP SILENCE                                                          [ CLOSE ]
//! Applies to: 2 audio clips
//! Threshold        [-]  -40 dB        [+]
//! Minimum silence  [-]  100 ms        [+]
//! Pad before       [-]  10 ms         [+]
//! Pad after        [-]  10 ms         [+]
//! Fade             [-]  5 ms          [+]
//! [ APPLY ]
//! ```
//!
//! A modal view the app places over the window like the quantize panel. It has no document: the
//! app tells it what a click on APPLY would work on ([`SilenceView::set_target`]) and reads the
//! settings back ([`SilenceView::values`]) when [`UiAction::StripSilence`] arrives:
//!
//! * `APPLY` emits [`SilenceAction::Apply`] (not while there is no audio clip to work on).
//! * `CLOSE` emits [`SilenceAction::Close`].
//!
//! Every value has a `-` and a `+` button and answers the wheel. The settings live in
//! [`SilenceValues`], which the app keeps between openings. Times are in milliseconds; the app
//! turns them into frames with each clip's own sample rate.

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
const APPLY_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 420.0;
const BTN_H: f64 = 20.0;
const STEP_BTN_W: f64 = 24.0;
const VALUE_W: f64 = 130.0;

/// Lowest threshold, in dB.
pub const THRESHOLD_MIN_DB: i32 = -96;
/// Highest threshold, in dB.
pub const THRESHOLD_MAX_DB: i32 = -6;
/// The minimum silences the row steps through, in ms.
pub const MIN_SILENCE_MS: [u32; 8] = [10, 20, 50, 100, 200, 500, 1000, 2000];
/// The pads and fades the rows step through, in ms.
pub const PAD_MS: [u32; 10] = [0, 1, 2, 5, 10, 20, 50, 100, 200, 500];

/// The settings of the panel; the app keeps them between openings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SilenceValues {
    /// Below this level (dBFS) a frame is silent.
    pub threshold_db: i32,
    /// A silent run is removed only when at least this long (ms).
    pub min_silence_ms: u32,
    /// Sound kept before each piece (ms).
    pub pad_before_ms: u32,
    /// Sound kept after each piece (ms).
    pub pad_after_ms: u32,
    /// Fade at the cut edges of each piece (ms).
    pub fade_ms: u32,
}

impl Default for SilenceValues {
    fn default() -> Self {
        Self {
            threshold_db: -40,
            min_silence_ms: 100,
            pad_before_ms: 10,
            pad_after_ms: 10,
            fade_ms: 5,
        }
    }
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SilenceAction {
    /// Strip the silence of the target clips with the panel's settings.
    Apply,
    /// Close the panel.
    Close,
}

/// A row of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceRow {
    /// The silence threshold.
    Threshold,
    /// The shortest silence that goes.
    MinSilence,
    /// Padding kept before a piece.
    PadBefore,
    /// Padding kept after a piece.
    PadAfter,
    /// The fade at the cut edges.
    Fade,
}

impl SilenceRow {
    /// Every row, top down.
    pub const ALL: [SilenceRow; 5] = [
        SilenceRow::Threshold,
        SilenceRow::MinSilence,
        SilenceRow::PadBefore,
        SilenceRow::PadAfter,
        SilenceRow::Fade,
    ];

    fn title(self) -> &'static str {
        match self {
            SilenceRow::Threshold => "Threshold",
            SilenceRow::MinSilence => "Minimum silence",
            SilenceRow::PadBefore => "Pad before",
            SilenceRow::PadAfter => "Pad after",
            SilenceRow::Fade => "Fade",
        }
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

/// The strip silence panel. See the module documentation.
#[derive(Debug)]
pub struct SilenceView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    values: SilenceValues,
    target: String,
    can_apply: bool,
    hover: Option<Point>,
    /// A drag on a value: the row and the vertical travel (up steps the value up).
    drag: Option<(SilenceRow, StepDrag)>,
    repaint: bool,
}

impl SilenceView {
    /// A panel with these settings; call [`Self::set_bounds`] and [`Self::set_target`].
    pub fn new(theme: Theme, values: SilenceValues) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            values,
            target: String::new(),
            can_apply: false,
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

    /// Says what APPLY would work on (`text`, shown under the title) and whether it can.
    pub fn set_target(&mut self, text: String, can_apply: bool) {
        if (&text, can_apply) != (&self.target, self.can_apply) {
            self.target = text;
            self.can_apply = can_apply;
            self.repaint = true;
        }
    }

    /// The settings.
    pub fn values(&self) -> &SilenceValues {
        &self.values
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The text of a row's value.
    pub fn value_text(&self, row: SilenceRow) -> String {
        let v = &self.values;
        match row {
            SilenceRow::Threshold => format!("{} dB", v.threshold_db),
            SilenceRow::MinSilence => format!("{} ms", v.min_silence_ms),
            SilenceRow::PadBefore => format!("{} ms", v.pad_before_ms),
            SilenceRow::PadAfter => format!("{} ms", v.pad_after_ms),
            SilenceRow::Fade => format!("{} ms", v.fade_ms),
        }
    }

    /// Steps a row by `dir` (+1 or -1): a click on `+` or `-`, or a wheel notch.
    pub fn step(&mut self, row: SilenceRow, dir: i32) {
        let v = &mut self.values;
        match row {
            SilenceRow::Threshold => {
                v.threshold_db = (v.threshold_db + dir).clamp(THRESHOLD_MIN_DB, THRESHOLD_MAX_DB);
            }
            SilenceRow::MinSilence => {
                v.min_silence_ms = step_list(&MIN_SILENCE_MS, v.min_silence_ms, dir);
            }
            SilenceRow::PadBefore => v.pad_before_ms = step_list(&PAD_MS, v.pad_before_ms, dir),
            SilenceRow::PadAfter => v.pad_after_ms = step_list(&PAD_MS, v.pad_after_ms, dir),
            SilenceRow::Fade => v.fade_ms = step_list(&PAD_MS, v.fade_ms, dir),
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
        HEADER_H + TARGET_H + SilenceRow::ALL.len() as f64 * ROW_H + APPLY_H + PAD
    }

    fn rows_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + TARGET_H
    }

    /// The row of a value.
    pub fn row_rect(&self, row: SilenceRow) -> Rect {
        let p = self.panel();
        let i = SilenceRow::ALL.iter().position(|r| *r == row).unwrap_or(0);
        let y = self.rows_top() + i as f64 * ROW_H;
        Rect::new(p.x0, y, p.x1, y + ROW_H)
    }

    fn cluster_x(&self) -> f64 {
        self.panel().x0 + PAD + 130.0
    }

    /// The `-` button of a row.
    pub fn minus_rect(&self, row: SilenceRow) -> Rect {
        let r = self.row_rect(row);
        let x = self.cluster_x();
        let y = r.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + STEP_BTN_W, y + BTN_H)
    }

    /// The value window of a row.
    pub fn value_rect(&self, row: SilenceRow) -> Rect {
        let m = self.minus_rect(row);
        Rect::new(m.x1 + 3.0, m.y0, m.x1 + 3.0 + VALUE_W, m.y1)
    }

    /// The `+` button of a row.
    pub fn plus_rect(&self, row: SilenceRow) -> Rect {
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

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let top = self.rows_top() + SilenceRow::ALL.len() as f64 * ROW_H;
        let y = top + (APPLY_H - BTN_H - 4.0) / 2.0;
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
            return help(self.close_rect(), "Close the panel; the settings are kept");
        }
        if self.apply_rect().contains(pos) {
            return help(
                self.apply_rect(),
                "Cut the silences out of the selected clips (one undo step)",
            );
        }
        SilenceRow::ALL.into_iter().find_map(|row| {
            let rects = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            stepper_help(pos, row.title(), rects, true)
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
                    out.push(UiAction::StripSilence(action));
                } else if let Some(row) = SilenceRow::ALL
                    .into_iter()
                    .find(|r| self.value_rect(*r).contains(*pos))
                {
                    self.drag = Some((row, StepDrag::new(pos.y)));
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                self.repaint = true;
                let dir = wheel_steps(delta.y);
                if let Some(row) = SilenceRow::ALL
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

    fn click(&mut self, pos: Point) -> Option<SilenceAction> {
        if self.close_rect().contains(pos) {
            return Some(SilenceAction::Close);
        }
        if self.apply_rect().contains(pos) {
            return self.can_apply.then_some(SilenceAction::Apply);
        }
        for row in SilenceRow::ALL {
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
            "STRIP SILENCE",
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
            ("no audio clip selected", t.text_muted)
        } else {
            (self.target.as_str(), t.text)
        };
        sp::label(
            c,
            &fit_label(&format!("Applies to: {target_text}"), width, 10.0),
            p.x0 + PAD,
            p.y0 + HEADER_H + 15.0,
            10.0,
            target_color,
            Align::Start,
        );
        for row in SilenceRow::ALL {
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
        let apply = self.apply_rect();
        sp::button(
            c,
            st,
            apply,
            "APPLY",
            self.can_apply.then_some(st.loop_on),
            self.can_apply && hovered(apply),
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
        assert_eq!(step_list(&PAD_MS, 10, 1), 20);
        assert_eq!(step_list(&PAD_MS, 10, -1), 5);
        assert_eq!(step_list(&PAD_MS, 0, -1), 0);
        assert_eq!(step_list(&PAD_MS, 500, 1), 500);
        // A value between entries goes to the nearer neighbour in the direction of the step.
        assert_eq!(step_list(&PAD_MS, 30, 1), 50);
        assert_eq!(step_list(&PAD_MS, 30, -1), 20);
    }
}
