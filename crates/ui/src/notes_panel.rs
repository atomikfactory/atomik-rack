//! The Audio to Notes panel: what to extract from the selected audio clip, and where to put it.
//!
//! ```text
//! AUDIO TO NOTES                                                         [ CLOSE ]
//! Applies to: Vocal.wav
//! Mode             [-]  Pitched (one voice)   [+]
//! Sensitivity      [-]  50 %                  [+]
//! Minimum note     [-]  60 ms                 [+]
//! Lowest note      [-]  C2                    [+]
//! Highest note     [-]  C7                    [+]
//! Pitch drift      [-]  70 cents              [+]
//! Open hats        [-]  n/a                   [+]
//! Beats timing     [-]  Free                  [+]
//! Put notes on     [-]  New track             [+]
//! [ EXTRACT ]
//! ```
//!
//! A modal view the app places over the window like the strip silence panel. It has no document:
//! the app tells it what a click on EXTRACT would work on ([`NotesView::set_target`]) and reads the
//! settings back ([`NotesView::values`]) when [`UiAction::AudioToNotes`] arrives:
//!
//! * `EXTRACT` emits [`NotesAction::Apply`] (not while there is no audio clip to work on).
//! * `CLOSE` emits [`NotesAction::Close`].
//!
//! Every value has a `-` and a `+` button and answers the wheel. The rows that do not apply to the
//! mode (the range and the pitch drift for drums, the open hats for a pitched voice) read `n/a` and
//! do not step. The settings live in [`NotesValues`], which the app keeps between openings.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::step_drag::{PX_PER_STEP, StepDrag, wheel_steps};
use crate::theme::Theme;
use crate::zone_edit::note_name;
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
const VALUE_W: f64 = 170.0;

/// The minimum note lengths the row steps through, in ms.
pub const MIN_NOTE_MS: [u32; 8] = [20, 30, 45, 60, 90, 120, 200, 400];
/// The pitch drifts the row steps through, in cents.
pub const DRIFT_CENTS: [u32; 7] = [30, 50, 70, 100, 150, 200, 300];
/// The grids of Beats timing, as the note value they divide a whole note into (0: no rounding).
pub const GRIDS: [u32; 5] = [0, 4, 8, 16, 32];
/// Sensitivity moves in steps of this many percent.
pub const SENSITIVITY_STEP: u32 = 5;

/// What the extraction looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesMode {
    /// One voice or instrument: a pitch tracker.
    Pitched,
    /// A drum loop: kick, snare and hats on the General MIDI keys.
    Drums,
}

impl NotesMode {
    /// The word in the panel and for scripts.
    pub fn label(self) -> &'static str {
        match self {
            NotesMode::Pitched => "Pitched (one voice)",
            NotesMode::Drums => "Drums",
        }
    }

    /// The id used by commands and scripts (`pitched`, `drums`).
    pub fn id(self) -> &'static str {
        match self {
            NotesMode::Pitched => "pitched",
            NotesMode::Drums => "drums",
        }
    }

    /// The mode with this id.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "pitched" => Some(NotesMode::Pitched),
            "drums" => Some(NotesMode::Drums),
            _ => None,
        }
    }
}

/// The settings of the panel; the app keeps them between openings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesValues {
    /// Pitched voice or drums.
    pub mode: NotesMode,
    /// `0..=100`: more finds quieter and less clear sound.
    pub sensitivity: u32,
    /// Notes shorter than this are left out (ms; pitched mode).
    pub min_note_ms: u32,
    /// Lowest key looked for (pitched mode).
    pub key_lo: u8,
    /// Highest key looked for (pitched mode).
    pub key_hi: u8,
    /// How far, in cents, a note's pitch may drift before it is another note (pitched mode).
    pub drift_cents: u32,
    /// Tell open hats from closed ones (drum mode).
    pub open_hats: bool,
    /// Round note starts to this note value (`0` free, else 4, 8, 16 or 32 per whole note).
    pub grid: u32,
    /// Put the notes on a new track (else on the selected note track).
    pub new_track: bool,
}

impl Default for NotesValues {
    fn default() -> Self {
        Self {
            mode: NotesMode::Pitched,
            sensitivity: 50,
            min_note_ms: 60,
            key_lo: 36,
            key_hi: 96,
            drift_cents: 70,
            open_hats: true,
            grid: 0,
            new_track: true,
        }
    }
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotesAction {
    /// Extract the notes of the target clip with the panel's settings.
    Apply,
    /// Close the panel.
    Close,
}

/// A row of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesRow {
    /// Pitched or drums.
    Mode,
    /// Sensitivity.
    Sensitivity,
    /// The shortest note.
    MinNote,
    /// The lowest key.
    Lowest,
    /// The highest key.
    Highest,
    /// The pitch drift of one note.
    Drift,
    /// Open and closed hats.
    OpenHats,
    /// Beats timing.
    Timing,
    /// A new track or the selected one.
    Target,
}

impl NotesRow {
    /// Every row, top down.
    pub const ALL: [NotesRow; 9] = [
        NotesRow::Mode,
        NotesRow::Sensitivity,
        NotesRow::MinNote,
        NotesRow::Lowest,
        NotesRow::Highest,
        NotesRow::Drift,
        NotesRow::OpenHats,
        NotesRow::Timing,
        NotesRow::Target,
    ];

    fn title(self) -> &'static str {
        match self {
            NotesRow::Mode => "Mode",
            NotesRow::Sensitivity => "Sensitivity",
            NotesRow::MinNote => "Minimum note",
            NotesRow::Lowest => "Lowest note",
            NotesRow::Highest => "Highest note",
            NotesRow::Drift => "Pitch drift",
            NotesRow::OpenHats => "Open hats",
            NotesRow::Timing => "Beats timing",
            NotesRow::Target => "Put notes on",
        }
    }

    /// True when the row means something in `mode`.
    pub fn applies(self, mode: NotesMode) -> bool {
        match self {
            NotesRow::MinNote | NotesRow::Lowest | NotesRow::Highest | NotesRow::Drift => {
                mode == NotesMode::Pitched
            }
            NotesRow::OpenHats => mode == NotesMode::Drums,
            _ => true,
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

/// The Audio to Notes panel. See the module documentation.
#[derive(Debug)]
pub struct NotesView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    values: NotesValues,
    target: String,
    can_apply: bool,
    hover: Option<Point>,
    /// A drag on a value: the row and the vertical travel (up steps the value up).
    drag: Option<(NotesRow, StepDrag)>,
    repaint: bool,
}

impl NotesView {
    /// A panel with these settings; call [`Self::set_bounds`] and [`Self::set_target`].
    pub fn new(theme: Theme, values: NotesValues) -> Self {
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

    /// Says what EXTRACT would work on (`text`, shown under the title) and whether it can.
    pub fn set_target(&mut self, text: String, can_apply: bool) {
        if (&text, can_apply) != (&self.target, self.can_apply) {
            self.target = text;
            self.can_apply = can_apply;
            self.repaint = true;
        }
    }

    /// The settings.
    pub fn values(&self) -> &NotesValues {
        &self.values
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The text of a row's value (`n/a` for a row the mode does not use).
    pub fn value_text(&self, row: NotesRow) -> String {
        let v = &self.values;
        if !row.applies(v.mode) {
            return "n/a".to_owned();
        }
        match row {
            NotesRow::Mode => v.mode.label().to_owned(),
            NotesRow::Sensitivity => format!("{} %", v.sensitivity),
            NotesRow::MinNote => format!("{} ms", v.min_note_ms),
            NotesRow::Lowest => note_name(v.key_lo),
            NotesRow::Highest => note_name(v.key_hi),
            NotesRow::Drift => format!("{} cents", v.drift_cents),
            NotesRow::OpenHats => if v.open_hats { "On" } else { "Off" }.to_owned(),
            NotesRow::Timing => match v.grid {
                0 => "Free".to_owned(),
                g => format!("1/{g}"),
            },
            NotesRow::Target => if v.new_track {
                "New track"
            } else {
                "Selected note track"
            }
            .to_owned(),
        }
    }

    /// Steps a row by `dir` (+1 or -1): a click on `+` or `-`, or a wheel notch. A row that does
    /// not apply to the mode stays as it is.
    pub fn step(&mut self, row: NotesRow, dir: i32) {
        let v = &mut self.values;
        if !row.applies(v.mode) {
            return;
        }
        match row {
            NotesRow::Mode => {
                v.mode = match v.mode {
                    NotesMode::Pitched => NotesMode::Drums,
                    NotesMode::Drums => NotesMode::Pitched,
                };
            }
            NotesRow::Sensitivity => {
                let next = i64::from(v.sensitivity) + i64::from(dir) * i64::from(SENSITIVITY_STEP);
                v.sensitivity = next.clamp(0, 100) as u32;
            }
            NotesRow::MinNote => v.min_note_ms = step_list(&MIN_NOTE_MS, v.min_note_ms, dir),
            NotesRow::Lowest => {
                v.key_lo = (i32::from(v.key_lo) + dir).clamp(0, i32::from(v.key_hi)) as u8;
            }
            NotesRow::Highest => {
                v.key_hi = (i32::from(v.key_hi) + dir).clamp(i32::from(v.key_lo), 127) as u8;
            }
            NotesRow::Drift => v.drift_cents = step_list(&DRIFT_CENTS, v.drift_cents, dir),
            NotesRow::OpenHats => v.open_hats = !v.open_hats,
            NotesRow::Timing => v.grid = step_list(&GRIDS, v.grid, dir),
            NotesRow::Target => v.new_track = !v.new_track,
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
        HEADER_H + TARGET_H + NotesRow::ALL.len() as f64 * ROW_H + APPLY_H + PAD
    }

    fn rows_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + TARGET_H
    }

    /// The row of a value.
    pub fn row_rect(&self, row: NotesRow) -> Rect {
        let p = self.panel();
        let i = NotesRow::ALL.iter().position(|r| *r == row).unwrap_or(0);
        let y = self.rows_top() + i as f64 * ROW_H;
        Rect::new(p.x0, y, p.x1, y + ROW_H)
    }

    fn cluster_x(&self) -> f64 {
        self.panel().x0 + PAD + 100.0
    }

    /// The `-` button of a row.
    pub fn minus_rect(&self, row: NotesRow) -> Rect {
        let r = self.row_rect(row);
        let x = self.cluster_x();
        let y = r.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + STEP_BTN_W, y + BTN_H)
    }

    /// The value window of a row.
    pub fn value_rect(&self, row: NotesRow) -> Rect {
        let m = self.minus_rect(row);
        Rect::new(m.x1 + 3.0, m.y0, m.x1 + 3.0 + VALUE_W, m.y1)
    }

    /// The `+` button of a row.
    pub fn plus_rect(&self, row: NotesRow) -> Rect {
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

    /// The EXTRACT button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let top = self.rows_top() + NotesRow::ALL.len() as f64 * ROW_H;
        let y = top + (APPLY_H - BTN_H - 4.0) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 120.0, y + BTN_H + 4.0)
    }

    /// True while a value is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
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
                    out.push(UiAction::AudioToNotes(action));
                } else if let Some(row) = NotesRow::ALL
                    .into_iter()
                    .find(|r| self.value_rect(*r).contains(*pos))
                {
                    self.drag = Some((row, StepDrag::new(pos.y)));
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                self.repaint = true;
                let dir = wheel_steps(delta.y);
                if let Some(row) = NotesRow::ALL
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

    fn click(&mut self, pos: Point) -> Option<NotesAction> {
        if self.close_rect().contains(pos) {
            return Some(NotesAction::Close);
        }
        if self.apply_rect().contains(pos) {
            return self.can_apply.then_some(NotesAction::Apply);
        }
        for row in NotesRow::ALL {
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
            "AUDIO TO NOTES",
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
        for row in NotesRow::ALL {
            let on = row.applies(self.values.mode);
            let color = if on { t.text } else { t.text_muted };
            let r = self.row_rect(row);
            sp::label(
                c,
                row.title(),
                r.x0 + PAD,
                r.y0 + 17.0,
                10.5,
                color,
                Align::Start,
            );
            let (minus, value, plus) = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            sp::button(c, st, minus, "-", None, on && hovered(minus));
            sp::lcd(c, st, value, None);
            sp::label(
                c,
                &fit_label(&self.value_text(row), VALUE_W - 12.0, 10.5),
                value.x0 + 8.0,
                value.y0 + 14.0,
                10.5,
                color,
                Align::Start,
            );
            sp::button(c, st, plus, "+", None, on && hovered(plus));
        }
        let apply = self.apply_rect();
        sp::button(
            c,
            st,
            apply,
            "EXTRACT",
            self.can_apply.then_some(st.loop_on),
            self.can_apply && hovered(apply),
        );
        c.pop_clip();
        c.pop_transform();
    }
}
