//! The warp panel: warp on or off, mode, source tempo and transposition of the selected audio
//! clips.
//!
//! ```text
//! WARP                                                                   [ CLOSE ]
//! Applies to: 2 audio clips
//! Warp             [-]  On            [+]
//! Mode             [-]  Beats         [+]
//! Source tempo     [-]  96.0 BPM      [+]  [/2] [x2]
//! Scale            [-]  100 %         [+]
//! Transpose        [-]  +3 st         [+]
//! Fine tune        [-]  +20 cents     [+]
//! [ APPLY ]  [ DETECT TEMPO ]  [ RESET ]
//! ```
//!
//! A modal view the app places over the window like the strip silence panel. It has no
//! document: the app fills it from the selected clip ([`WarpView::set_values`]), tells it what
//! APPLY would work on ([`WarpView::set_target`]) and reads the settings back
//! ([`WarpView::values`]) when [`UiAction::Warp`] arrives:
//!
//! * `APPLY` emits [`WarpAction::Apply`] (not while there is no audio clip to work on).
//! * `DETECT TEMPO` emits [`WarpAction::Detect`]; the app listens to the first selected clip and
//!   answers with [`WarpView::set_values`].
//! * `RESET` emits [`WarpAction::Reset`] (Reset to Unstretched Tempo: warp off, markers cleared; not
//!   while there is no audio clip to work on).
//! * `CLOSE` emits [`WarpAction::Close`].
//!
//! Every value has a `-` and a `+` button and answers the wheel. The tempo steps by 1 BPM, `/2`
//! and `x2` halve and double it (the usual fix for a detector that found half or double time).
//! Scale is the playback speed in percent of the clip as it is (the source tempo divided by it
//! when APPLY runs): 200 plays the audio twice as fast. It reads 100 whenever the panel is filled
//! from a clip.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::step_drag::{PX_PER_STEP, StepDrag, wheel_steps};
use crate::theme::Theme;
use crate::tooltip::{Help, stepper_help};
use atomik_model::{WARP_BPM_MAX, WARP_BPM_MIN, WARP_TRANSPOSE_MAX_CENTS, WarpMode, WarpSettings};

/// The lowest scale in percent.
pub const WARP_SCALE_MIN: u32 = 25;
/// The highest scale in percent.
pub const WARP_SCALE_MAX: u32 = 400;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const TARGET_H: f64 = 24.0;
const ROW_H: f64 = 26.0;
const APPLY_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 460.0;
const BTN_H: f64 = 20.0;
const STEP_BTN_W: f64 = 24.0;
const VALUE_W: f64 = 120.0;
const FACTOR_W: f64 = 30.0;

/// The settings of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarpValues {
    /// Follow the song tempo.
    pub enabled: bool,
    /// The stretch engine.
    pub mode: WarpMode,
    /// The source tempo in tenths of a BPM.
    pub bpm_tenths: u32,
    /// Whole semitones of the transposition.
    pub semitones: i32,
    /// Cents on top of the semitones, `-99..=99`.
    pub cents: i32,
    /// Playback speed in percent of the clip as it is, [`WARP_SCALE_MIN`]..=[`WARP_SCALE_MAX`];
    /// APPLY divides the source tempo by it. A clip always reads 100.
    pub scale_pct: u32,
}

impl Default for WarpValues {
    fn default() -> Self {
        Self::from_settings(&WarpSettings::default())
    }
}

impl WarpValues {
    /// The panel's view of a clip's settings.
    pub fn from_settings(w: &WarpSettings) -> Self {
        Self {
            enabled: w.enabled,
            mode: w.mode,
            bpm_tenths: (w.source_bpm * 10.0).round() as u32,
            semitones: w.semitones(),
            cents: w.cents(),
            scale_pct: 100,
        }
    }

    /// `base` with these values (its markers stay).
    pub fn apply_to(&self, base: &WarpSettings) -> WarpSettings {
        WarpSettings {
            enabled: self.enabled,
            mode: self.mode,
            source_bpm: (f64::from(self.bpm_tenths) / 10.0 * 100.0
                / f64::from(self.scale_pct.clamp(WARP_SCALE_MIN, WARP_SCALE_MAX)))
            .clamp(WARP_BPM_MIN, WARP_BPM_MAX),
            transpose_cents: (self.semitones * 100 + self.cents)
                .clamp(-WARP_TRANSPOSE_MAX_CENTS, WARP_TRANSPOSE_MAX_CENTS),
            markers: base.markers.clone(),
        }
    }
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarpAction {
    /// Set the panel's values on the target clips.
    Apply,
    /// Estimate the source tempo of the first target clip.
    Detect,
    /// Turn warp off and clear the markers of the target clips (Reset to Unstretched Tempo).
    Reset,
    /// Close the panel.
    Close,
}

/// A row of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarpRow {
    /// Warp on or off.
    Warp,
    /// The mode.
    Mode,
    /// The source tempo.
    Tempo,
    /// The scale in percent.
    Scale,
    /// Semitones.
    Transpose,
    /// Cents.
    Fine,
}

impl WarpRow {
    /// Every row, top down.
    pub const ALL: [WarpRow; 6] = [
        WarpRow::Warp,
        WarpRow::Mode,
        WarpRow::Tempo,
        WarpRow::Scale,
        WarpRow::Transpose,
        WarpRow::Fine,
    ];

    fn title(self) -> &'static str {
        match self {
            WarpRow::Warp => "Warp",
            WarpRow::Mode => "Mode",
            WarpRow::Tempo => "Source tempo",
            WarpRow::Scale => "Scale",
            WarpRow::Transpose => "Transpose",
            WarpRow::Fine => "Fine tune",
        }
    }
}

/// The warp panel. See the module documentation.
#[derive(Debug)]
pub struct WarpView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    values: WarpValues,
    target: String,
    can_apply: bool,
    hover: Option<Point>,
    /// A drag on a value: the row and the vertical travel (up steps the value up).
    drag: Option<(WarpRow, StepDrag)>,
    repaint: bool,
}

impl WarpView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A panel with these values; call [`Self::set_bounds`] and [`Self::set_target`].
    pub fn new(theme: Theme, values: WarpValues) -> Self {
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

    /// Replaces the values (the app does it when the panel opens and after DETECT).
    pub fn set_values(&mut self, values: WarpValues) {
        if values != self.values {
            self.values = values;
            self.repaint = true;
        }
    }

    /// The values.
    pub fn values(&self) -> &WarpValues {
        &self.values
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The text of a row's value.
    pub fn value_text(&self, row: WarpRow) -> String {
        let v = &self.values;
        match row {
            WarpRow::Warp => if v.enabled { "On" } else { "Off" }.to_owned(),
            WarpRow::Mode => v.mode.label().to_owned(),
            WarpRow::Tempo => format!("{:.1} BPM", f64::from(v.bpm_tenths) / 10.0),
            WarpRow::Scale => format!("{} %", v.scale_pct),
            WarpRow::Transpose => format!("{:+} st", v.semitones),
            WarpRow::Fine => format!("{:+} cents", v.cents),
        }
    }

    /// Steps a row by `dir` (+1 or -1): a click on `+` or `-`, or a wheel notch.
    pub fn step(&mut self, row: WarpRow, dir: i32) {
        let v = &mut self.values;
        match row {
            WarpRow::Warp => v.enabled = !v.enabled,
            WarpRow::Mode => {
                let all = WarpMode::ALL;
                let at = all.iter().position(|m| *m == v.mode).unwrap_or(0) as i32;
                v.mode = all[(at + dir).rem_euclid(all.len() as i32) as usize];
            }
            WarpRow::Tempo => {
                let (lo, hi) = ((WARP_BPM_MIN * 10.0) as i32, (WARP_BPM_MAX * 10.0) as i32);
                v.bpm_tenths = (v.bpm_tenths as i32 + dir * 10).clamp(lo, hi) as u32;
            }
            WarpRow::Scale => {
                v.scale_pct = (v.scale_pct as i32 + dir)
                    .clamp(WARP_SCALE_MIN as i32, WARP_SCALE_MAX as i32)
                    as u32;
            }
            WarpRow::Transpose => {
                let max = WARP_TRANSPOSE_MAX_CENTS / 100;
                v.semitones = (v.semitones + dir).clamp(-max, max);
            }
            WarpRow::Fine => v.cents = (v.cents + dir * 10).clamp(-90, 90),
        }
        // The total stays within two octaves.
        let total = v.semitones * 100 + v.cents;
        if total.abs() > WARP_TRANSPOSE_MAX_CENTS {
            v.cents = 0;
        }
        self.repaint = true;
    }

    /// Halves (`up == false`) or doubles the source tempo, within the allowed range.
    pub fn scale_tempo(&mut self, up: bool) {
        let t = f64::from(self.values.bpm_tenths) * if up { 2.0 } else { 0.5 };
        let (lo, hi) = (WARP_BPM_MIN * 10.0, WARP_BPM_MAX * 10.0);
        self.values.bpm_tenths = t.round().clamp(lo, hi) as u32;
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
        HEADER_H + TARGET_H + WarpRow::ALL.len() as f64 * ROW_H + APPLY_H + PAD
    }

    fn rows_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + TARGET_H
    }

    /// The row of a value.
    pub fn row_rect(&self, row: WarpRow) -> Rect {
        let p = self.panel();
        let i = WarpRow::ALL.iter().position(|r| *r == row).unwrap_or(0);
        let y = self.rows_top() + i as f64 * ROW_H;
        Rect::new(p.x0, y, p.x1, y + ROW_H)
    }

    fn cluster_x(&self) -> f64 {
        self.panel().x0 + PAD + 110.0
    }

    /// The `-` button of a row.
    pub fn minus_rect(&self, row: WarpRow) -> Rect {
        let r = self.row_rect(row);
        let x = self.cluster_x();
        let y = r.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + STEP_BTN_W, y + BTN_H)
    }

    /// The value window of a row.
    pub fn value_rect(&self, row: WarpRow) -> Rect {
        let m = self.minus_rect(row);
        Rect::new(m.x1 + 3.0, m.y0, m.x1 + 3.0 + VALUE_W, m.y1)
    }

    /// The `+` button of a row.
    pub fn plus_rect(&self, row: WarpRow) -> Rect {
        let v = self.value_rect(row);
        Rect::new(v.x1 + 3.0, v.y0, v.x1 + 3.0 + STEP_BTN_W, v.y1)
    }

    /// The `/2` (`false`) or `x2` (`true`) button of the tempo row.
    pub fn factor_rect(&self, up: bool) -> Rect {
        let p = self.plus_rect(WarpRow::Tempo);
        let x = p.x1 + 6.0 + if up { FACTOR_W + 3.0 } else { 0.0 };
        Rect::new(x, p.y0, x + FACTOR_W, p.y1)
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

    fn buttons_y(&self) -> f64 {
        let top = self.rows_top() + WarpRow::ALL.len() as f64 * ROW_H;
        top + (APPLY_H - BTN_H - 4.0) / 2.0
    }

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.buttons_y();
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 100.0, y + BTN_H + 4.0)
    }

    /// The DETECT TEMPO button.
    pub fn detect_rect(&self) -> Rect {
        let a = self.apply_rect();
        Rect::new(a.x1 + 8.0, a.y0, a.x1 + 8.0 + 150.0, a.y1)
    }

    /// The RESET button.
    pub fn reset_rect(&self) -> Rect {
        let d = self.detect_rect();
        Rect::new(d.x1 + 8.0, d.y0, d.x1 + 8.0 + 70.0, d.y1)
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
                "Apply these warp settings to the selected clips (one undo step)",
            );
        }
        if self.detect_rect().contains(pos) {
            return help(
                self.detect_rect(),
                "Estimate the source tempo from the transients of the clip",
            );
        }
        if self.reset_rect().contains(pos) {
            return help(
                self.reset_rect(),
                "Reset to Unstretched Tempo: warp off and markers cleared, the clip plays its audio as it is (one undo step)",
            );
        }
        for (up, text) in [
            (false, "Halve the source tempo"),
            (true, "Double the source tempo"),
        ] {
            if self.factor_rect(up).contains(pos) {
                return help(self.factor_rect(up), text);
            }
        }
        WarpRow::ALL.into_iter().find_map(|row| {
            let rects = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            stepper_help(
                pos,
                row.title(),
                rects,
                !matches!(row, WarpRow::Warp | WarpRow::Mode),
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
                    out.push(UiAction::Warp(action));
                } else if let Some(row) = WarpRow::ALL
                    .into_iter()
                    .find(|r| self.value_rect(*r).contains(*pos))
                {
                    self.drag = Some((row, StepDrag::new(pos.y)));
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                self.repaint = true;
                let dir = wheel_steps(delta.y);
                if let Some(row) = WarpRow::ALL
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

    fn click(&mut self, pos: Point) -> Option<WarpAction> {
        if self.close_rect().contains(pos) {
            return Some(WarpAction::Close);
        }
        if self.apply_rect().contains(pos) {
            return self.can_apply.then_some(WarpAction::Apply);
        }
        if self.detect_rect().contains(pos) {
            return self.can_apply.then_some(WarpAction::Detect);
        }
        if self.reset_rect().contains(pos) {
            return self.can_apply.then_some(WarpAction::Reset);
        }
        for up in [false, true] {
            if self.factor_rect(up).contains(pos) {
                self.scale_tempo(up);
                return None;
            }
        }
        for row in WarpRow::ALL {
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
            "WARP",
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
        for row in WarpRow::ALL {
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
        for (up, text) in [(false, "/2"), (true, "x2")] {
            let r = self.factor_rect(up);
            sp::button(c, st, r, text, None, hovered(r));
        }
        let (apply, detect) = (self.apply_rect(), self.detect_rect());
        sp::button(
            c,
            st,
            apply,
            "APPLY",
            self.can_apply.then_some(st.loop_on),
            self.can_apply && hovered(apply),
        );
        sp::button(
            c,
            st,
            detect,
            "DETECT TEMPO",
            None,
            self.can_apply && hovered(detect),
        );
        let reset = self.reset_rect();
        sp::button(
            c,
            st,
            reset,
            "RESET",
            None,
            self.can_apply && hovered(reset),
        );
        c.pop_clip();
        c.pop_transform();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_through_settings_and_keep_the_markers() {
        let w = WarpSettings {
            enabled: true,
            mode: WarpMode::Tonal,
            source_bpm: 93.5,
            transpose_cents: -250,
            markers: vec![atomik_model::WarpMarker { tick: 5, source: 9 }],
        };
        let v = WarpValues::from_settings(&w);
        assert_eq!((v.bpm_tenths, v.semitones, v.cents), (935, -2, -50));
        assert_eq!(v.apply_to(&w), w);
        let other = WarpSettings::default();
        assert_eq!(v.apply_to(&other).markers, Vec::new());
    }
}
