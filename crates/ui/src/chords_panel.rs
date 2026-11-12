//! The Detect Chords panel: the chords found in audio, shown over its span before they go into the
//! chord lane.
//!
//! ```text
//! DETECT CHORDS                                                         [ CLOSE ]
//! From: Piano.wav, bars 1 to 9
//! | C        | G        | Am       | F/A      |                       (the chord strip)
//! Changes        [-]  Every bar            [+]
//! Chords         [-]  Full (sevenths)      [+]
//! Slash chords   [-]  On                   [+]
//! [x] Set song key to A minor
//! [ REPLACE IN RANGE ] [ FILL GAPS ONLY ] [ CANCEL ]
//! ```
//!
//! A modal view the app places over the window like the Audio to Notes panel. It has no document:
//! the app gives it what it reads ([`ChordsView::set_target`]), how far the analysis has got
//! ([`ChordsView::set_progress`]) and, when it is done, the chords to show
//! ([`ChordsView::set_preview`]), and reads the settings back ([`ChordsView::values`]):
//!
//! * A change of CHANGES, CHORDS or SLASH emits [`ChordsAction::Rescore`]: the app scores the
//!   analysis it keeps again, with no new listening, and sends the new preview.
//! * `REPLACE IN RANGE` and `FILL GAPS ONLY` emit [`ChordsAction::Replace`] and
//!   [`ChordsAction::Merge`] (only with chords to put); `CANCEL` and `CLOSE` emit
//!   [`ChordsAction::Close`].
//! * While the analysis runs the CANCEL button stops it ([`ChordsAction::Cancel`]) and a bar
//!   shows how far it is.
//!
//! The chord strip draws each chord as a box over its share of the span with its symbol; a chord
//! the analysis is unsure of is dimmed. The check box under the rows appears only when the song
//! has no key and the audio points to one.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, TextRun};
use crate::event::{PointerButton, UiEvent};
use crate::notes_panel::NotesProgress;
use crate::paint::{fit_label, fit_label_raw};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::step_drag::wheel_steps;
use crate::theme::{Theme, mix};
use crate::tooltip::{Help, stepper_help};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const TARGET_H: f64 = 24.0;
const STRIP_H: f64 = 56.0;
const ROW_H: f64 = 26.0;
const KEY_H: f64 = 26.0;
const APPLY_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 560.0;
const BTN_H: f64 = 20.0;
const STEP_BTN_W: f64 = 24.0;
const VALUE_W: f64 = 170.0;
const ACTION_W: f64 = 132.0;
/// Below this confidence a chord is drawn dimmed.
pub const LOW_CONFIDENCE: f32 = 0.5;

/// Draws `text`, already translated and fitted by [`fit_label`], as it is (a second lookup would
/// try to translate the French).
fn label_fitted(
    c: &mut dyn Canvas,
    text: &str,
    x: f64,
    y: f64,
    size: f32,
    color: vello::peniko::Color,
    align: Align,
) {
    c.text(&TextRun::verbatim(text, Point::new(x, y), size, color).align(align));
}

/// How often the chords may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordChanges {
    /// On any beat.
    Beat,
    /// On the first beat of either half of a bar.
    HalfBar,
    /// Once a bar.
    Bar,
}

impl ChordChanges {
    /// Every setting, in order.
    pub const ALL: [ChordChanges; 3] =
        [ChordChanges::Beat, ChordChanges::HalfBar, ChordChanges::Bar];

    /// The value in the panel.
    pub fn label(self) -> &'static str {
        match self {
            ChordChanges::Beat => "Every beat",
            ChordChanges::HalfBar => "Every half bar",
            ChordChanges::Bar => "Every bar",
        }
    }
}

/// The settings of the panel; the app keeps them between openings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordsValues {
    /// How often the chords may change.
    pub changes: ChordChanges,
    /// Sevenths and sixths as well as triads.
    pub full: bool,
    /// Slash chords (a bass that is not the root).
    pub slash: bool,
    /// Also set the song key (when the song has none).
    pub set_key: bool,
}

impl Default for ChordsValues {
    fn default() -> Self {
        Self {
            changes: ChordChanges::HalfBar,
            full: true,
            slash: true,
            set_key: true,
        }
    }
}

/// One chord of the preview.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewChord {
    /// Song tick it starts at.
    pub tick: i64,
    /// Length in ticks.
    pub length: i64,
    /// The symbol (`Am7`, `C/G`).
    pub symbol: String,
    /// How sure the analysis is, `0..=1`.
    pub confidence: f32,
}

/// What the analysis found, for the panel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChordsPreview {
    /// The ticks the chords are put over.
    pub span: (i64, i64),
    /// The chords, in order.
    pub chords: Vec<PreviewChord>,
    /// The key the audio points to, offered when the song has none (`A minor`).
    pub key: Option<String>,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordsAction {
    /// Score the analysis again with the new settings.
    Rescore,
    /// Put the chords in the lane, replacing those of the span (one undo step).
    Replace,
    /// Put the chords in the gaps of the lane only (one undo step).
    Merge,
    /// Stop the analysis that runs.
    Cancel,
    /// Close the panel.
    Close,
}

/// A row of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordsRow {
    /// How often the chords may change.
    Changes,
    /// Triads only, or sevenths too.
    Chords,
    /// Slash chords.
    Slash,
}

impl ChordsRow {
    /// Every row, top down.
    pub const ALL: [ChordsRow; 3] = [ChordsRow::Changes, ChordsRow::Chords, ChordsRow::Slash];

    fn title(self) -> &'static str {
        match self {
            ChordsRow::Changes => "Changes",
            ChordsRow::Chords => "Chords",
            ChordsRow::Slash => "Slash chords",
        }
    }
}

/// The Detect Chords panel. See the module documentation.
#[derive(Debug)]
pub struct ChordsView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    values: ChordsValues,
    target: String,
    progress: Option<NotesProgress>,
    preview: Option<ChordsPreview>,
    hover: Option<Point>,
    repaint: bool,
}

impl ChordsView {
    /// A panel with these settings; call [`Self::set_bounds`] and [`Self::set_target`].
    pub fn new(theme: Theme, values: ChordsValues) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            values,
            target: String::new(),
            progress: None,
            preview: None,
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

    /// Says what the chords are read from (shown under the title).
    pub fn set_target(&mut self, text: String) {
        if text != self.target {
            self.target = text;
            self.repaint = true;
        }
    }

    /// Says how far the analysis has got (`None`: it is not running).
    pub fn set_progress(&mut self, progress: Option<NotesProgress>) {
        if progress != self.progress {
            self.progress = progress;
            self.repaint = true;
        }
    }

    /// The chords found (`None`: none yet).
    pub fn set_preview(&mut self, preview: Option<ChordsPreview>) {
        if preview != self.preview {
            self.preview = preview;
            self.repaint = true;
        }
    }

    /// The preview shown.
    pub fn preview(&self) -> Option<&ChordsPreview> {
        self.preview.as_ref()
    }

    /// The settings.
    pub fn values(&self) -> &ChordsValues {
        &self.values
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the chords can be put in the lane: the analysis is over and found some.
    pub fn can_apply(&self) -> bool {
        self.progress.is_none() && self.preview.as_ref().is_some_and(|p| !p.chords.is_empty())
    }

    /// Whether the key check box is shown.
    fn offers_key(&self) -> bool {
        self.preview.as_ref().is_some_and(|p| p.key.is_some())
    }

    /// The text of a row's value.
    pub fn value_text(&self, row: ChordsRow) -> &'static str {
        let v = &self.values;
        match row {
            ChordsRow::Changes => v.changes.label(),
            ChordsRow::Chords => {
                if v.full {
                    "Full (sevenths)"
                } else {
                    "Simple (triads)"
                }
            }
            ChordsRow::Slash => {
                if v.slash {
                    "On"
                } else {
                    "Off"
                }
            }
        }
    }

    /// Steps a row by `dir` (+1 or -1).
    pub fn step(&mut self, row: ChordsRow, dir: i32) {
        let v = &mut self.values;
        match row {
            ChordsRow::Changes => {
                let at = ChordChanges::ALL
                    .iter()
                    .position(|c| *c == v.changes)
                    .unwrap_or(0) as i32;
                v.changes = ChordChanges::ALL[(at + dir).clamp(0, 2) as usize];
            }
            ChordsRow::Chords => v.full = !v.full,
            ChordsRow::Slash => v.slash = !v.slash,
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
        HEADER_H + TARGET_H + STRIP_H + ChordsRow::ALL.len() as f64 * ROW_H + KEY_H + APPLY_H + PAD
    }

    /// The chord strip.
    pub fn strip_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + TARGET_H;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + STRIP_H - 10.0)
    }

    /// The row of a value.
    pub fn row_rect(&self, row: ChordsRow) -> Rect {
        let p = self.panel();
        let i = ChordsRow::ALL.iter().position(|r| *r == row).unwrap_or(0);
        let y = p.y0 + HEADER_H + TARGET_H + STRIP_H + i as f64 * ROW_H;
        Rect::new(p.x0, y, p.x1, y + ROW_H)
    }

    /// The `-` button of a row.
    pub fn minus_rect(&self, row: ChordsRow) -> Rect {
        let r = self.row_rect(row);
        let x = self.panel().x0 + PAD + 100.0;
        let y = r.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + STEP_BTN_W, y + BTN_H)
    }

    /// The value window of a row.
    pub fn value_rect(&self, row: ChordsRow) -> Rect {
        let m = self.minus_rect(row);
        Rect::new(m.x1 + 3.0, m.y0, m.x1 + 3.0 + VALUE_W, m.y1)
    }

    /// The `+` button of a row.
    pub fn plus_rect(&self, row: ChordsRow) -> Rect {
        let v = self.value_rect(row);
        Rect::new(v.x1 + 3.0, v.y0, v.x1 + 3.0 + STEP_BTN_W, v.y1)
    }

    /// The "Set song key" check box (with its text).
    pub fn key_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.row_rect(ChordsRow::Slash).y1;
        Rect::new(p.x0 + PAD, y + 4.0, p.x1 - PAD, y + KEY_H - 4.0)
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

    fn action_rect(&self, i: usize) -> Rect {
        let p = self.panel();
        let top = self.row_rect(ChordsRow::Slash).y1 + KEY_H;
        let y = top + (APPLY_H - BTN_H - 4.0) / 2.0;
        let x = p.x0 + PAD + i as f64 * (ACTION_W + 8.0);
        Rect::new(x, y, x + ACTION_W, y + BTN_H + 4.0)
    }

    /// The REPLACE IN RANGE button.
    pub fn replace_rect(&self) -> Rect {
        self.action_rect(0)
    }

    /// The FILL GAPS ONLY button.
    pub fn merge_rect(&self) -> Rect {
        self.action_rect(1)
    }

    /// The CANCEL button (it stops the analysis while one runs).
    pub fn cancel_rect(&self) -> Rect {
        self.action_rect(2)
    }

    // ------------------------------------------------------------------ events

    /// What the button or value under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(
                self.close_rect(),
                "Close the panel; the lane is left as it is",
            );
        }
        if self.replace_rect().contains(pos) {
            return help(
                self.replace_rect(),
                "Put these chords in the lane over the span, replacing its chords (one undo step)",
            );
        }
        if self.merge_rect().contains(pos) {
            return help(
                self.merge_rect(),
                "Put these chords only where the lane has none (one undo step)",
            );
        }
        if self.cancel_rect().contains(pos) {
            let text = if self.progress.is_some() {
                "Stop listening to the audio"
            } else {
                "Close the panel; the lane is left as it is"
            };
            return help(self.cancel_rect(), text);
        }
        if self.offers_key() && self.key_rect().contains(pos) {
            return help(
                self.key_rect(),
                "Also set the song key the audio points to, in the same undo step",
            );
        }
        if self.strip_rect().contains(pos) {
            return help(
                self.strip_rect(),
                "The chords found; a dimmed chord is a less sure guess",
            );
        }
        ChordsRow::ALL.into_iter().find_map(|row| {
            let rects = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            stepper_help(pos, row.title(), rects, false)
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
                    out.push(UiAction::DetectChords(action));
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                if let Some(row) = ChordsRow::ALL
                    .into_iter()
                    .find(|r| self.row_rect(*r).contains(*pos))
                {
                    self.step(row, wheel_steps(delta.y));
                    out.push(UiAction::DetectChords(ChordsAction::Rescore));
                }
            }
            _ => {}
        }
        out
    }

    fn click(&mut self, pos: Point) -> Option<ChordsAction> {
        if self.close_rect().contains(pos) {
            return Some(ChordsAction::Close);
        }
        if self.cancel_rect().contains(pos) {
            return Some(if self.progress.is_some() {
                ChordsAction::Cancel
            } else {
                ChordsAction::Close
            });
        }
        if self.replace_rect().contains(pos) {
            return self.can_apply().then_some(ChordsAction::Replace);
        }
        if self.merge_rect().contains(pos) {
            return self.can_apply().then_some(ChordsAction::Merge);
        }
        if self.offers_key() && self.key_rect().contains(pos) {
            self.values.set_key = !self.values.set_key;
            return None;
        }
        for row in ChordsRow::ALL {
            let dir = if self.minus_rect(row).contains(pos) {
                -1
            } else if self.plus_rect(row).contains(pos) || self.value_rect(row).contains(pos) {
                1
            } else {
                continue;
            };
            self.step(row, dir);
            return Some(ChordsAction::Rescore);
        }
        None
    }

    // ------------------------------------------------------------------ painting

    fn paint_strip(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let r = self.strip_rect();
        sp::lcd(c, st, r, None);
        let message = |c: &mut dyn Canvas, text: &str| {
            label_fitted(
                c,
                &fit_label(text, r.width() - 16.0, 10.5),
                r.x0 + 8.0,
                r.y0 + r.height() / 2.0 + 4.0,
                10.5,
                t.text_muted,
                Align::Start,
            );
        };
        if let Some(progress) = &self.progress {
            message(c, &progress.text);
            let bar = Rect::new(r.x0 + 8.0, r.y1 - 10.0, r.x1 - 8.0, r.y1 - 5.0);
            sp::rect(c, bar, st.separator);
            let done = f64::from(progress.fraction.clamp(0.0, 1.0));
            sp::rect(
                c,
                Rect::new(bar.x0, bar.y0, bar.x0 + bar.width() * done, bar.y1),
                st.loop_on,
            );
            return;
        }
        let Some(preview) = &self.preview else {
            message(c, "Listening\u{2026}");
            return;
        };
        if preview.chords.is_empty() {
            message(c, "No chords found in the audio");
            return;
        }
        let (lo, hi) = preview.span;
        let width = (hi - lo).max(1) as f64;
        let x_at = |tick: i64| r.x0 + 2.0 + (r.width() - 4.0) * ((tick - lo) as f64 / width);
        for chord in &preview.chords {
            let (x0, x1) = (x_at(chord.tick), x_at(chord.tick + chord.length));
            let cell = Rect::new(x0 + 1.0, r.y0 + 4.0, (x1 - 1.0).max(x0 + 2.0), r.y1 - 4.0);
            let sure = chord.confidence >= LOW_CONFIDENCE;
            let fill = if sure {
                mix(st.loop_on, t.lcd, 0.55)
            } else {
                mix(st.loop_on, t.lcd, 0.85)
            };
            sp::rect(c, cell, fill);
            let color = if sure { t.text } else { t.text_muted };
            let text = fit_label_raw(&chord.symbol, cell.width() - 6.0, 11.0);
            c.text(
                &TextRun::verbatim(
                    text,
                    Point::new(cell.x0 + 4.0, cell.y0 + cell.height() / 2.0 + 4.0),
                    11.0,
                    color,
                )
                .align(Align::Start),
            );
        }
    }

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
            "DETECT CHORDS",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let width = p.width() - 2.0 * PAD;
        let (from, color) = if self.target.is_empty() {
            ("no audio to read".to_owned(), t.text_muted)
        } else {
            (self.target.clone(), t.text)
        };
        label_fitted(
            c,
            &fit_label(&format!("From: {from}"), width, 10.0),
            p.x0 + PAD,
            p.y0 + HEADER_H + 15.0,
            10.0,
            color,
            Align::Start,
        );
        self.paint_strip(c);
        for row in ChordsRow::ALL {
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
            label_fitted(
                c,
                &fit_label(self.value_text(row), VALUE_W - 12.0, 10.5),
                value.x0 + 8.0,
                value.y0 + 14.0,
                10.5,
                t.text,
                Align::Start,
            );
            sp::button(c, st, plus, "+", None, hovered(plus));
        }
        if let Some(key) = self.preview.as_ref().and_then(|p| p.key.as_ref()) {
            let k = self.key_rect();
            let bx = Rect::new(k.x0, k.y0 + 2.0, k.x0 + 14.0, k.y0 + 16.0);
            sp::lcd(c, st, bx, None);
            if self.values.set_key {
                sp::rect(c, bx.inset(-3.5), st.loop_on);
            }
            label_fitted(
                c,
                &fit_label(&format!("Set song key to {key}"), k.width() - 24.0, 10.5),
                bx.x1 + 8.0,
                k.y0 + 13.0,
                10.5,
                t.text,
                Align::Start,
            );
        }
        let ready = self.can_apply();
        let (replace, merge, cancel) = (self.replace_rect(), self.merge_rect(), self.cancel_rect());
        sp::button(
            c,
            st,
            replace,
            "REPLACE IN RANGE",
            ready.then_some(st.loop_on),
            ready && hovered(replace),
        );
        sp::button(
            c,
            st,
            merge,
            "FILL GAPS ONLY",
            None,
            ready && hovered(merge),
        );
        sp::button(c, st, cancel, "CANCEL", None, hovered(cancel));
        c.pop_clip();
        c.pop_transform();
    }
}
