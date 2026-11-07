//! The Sound Analyst panel ("recreate this sound"): the measurements of a sound file and the
//! starting patch that was made from them.
//!
//! ```text
//! SOUND ANALYST                      [ OPEN FILE ] [ MAKE PATCH ] [ MATCH ] [ CLOSE ]
//! pluck.wav, 1.2 s                                  (drop an audio file anywhere on the panel)
//! LEVEL         (target curve, and the patch's render in a second colour)       3.1 dB   66 %
//! BRIGHTNESS    ...                                                             0.4 oct  65 %
//! PITCH         ...                                                             0.1 st   83 %
//! Envelope: attack 2 ms, then it dies away in 300 ms        (the measured numbers)
//! Brightness: ...
//! Patch: a saw on the poly synth ...                        (what the patch does and why)
//! ```
//!
//! A modal view the app places over the window like the bass-read panel. The app fills it with a
//! [`SoundAnalystData`] ([`SoundAnalystView::set_data`]); the view only draws and reports what was
//! pressed as a [`UiAction::SoundAnalyst`]:
//!
//! * `OPEN FILE` ([`SoundAnalystAction::OpenFile`]) asks for an audio file; while a file is being
//!   analysed the button reads `STOP` ([`SoundAnalystAction::Stop`]) and the first curve box shows
//!   the progress. Audio files dropped on the window go to the analysis as well (the app routes
//!   them).
//! * `MAKE PATCH` ([`SoundAnalystAction::MakePatch`]; also Enter) adds the proposed patch on a new
//!   track. `MATCH` ([`SoundAnalystAction::Match`]) renders the patch that was made, as it is now,
//!   on the same note and measures how close it is.
//! * `CLOSE` emits [`SoundAnalystAction::Close`]; the app also closes the panel on Esc.
//!
//! Each curve box draws the measured curve of the target and, once there is one, the curve of the
//! render of the patch on the same time axis, with the distance between them and a score from 0 to
//! 100 % on its right (`1 / (1 + distance / half)`, see `atomik_dsp::sound_analysis::HALF_MATCH`).

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};
use vello::peniko::Color;

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const FILE_H: f64 = 24.0;
const CURVE_H: f64 = 62.0;
const CURVE_GAP: f64 = 16.0;
const LINE_H: f64 = 14.0;
const FOOT_H: f64 = 26.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 780.0;
const BTN_H: f64 = 22.0;
const FONT: f32 = 10.0;
/// The room the curves leave on the right for the distance and the score.
const SIDE_W: f64 = 120.0;
/// The lines of numbers the panel shows.
pub const NUMBER_LINES: usize = 4;
/// The lines of the plan the panel shows.
pub const PLAN_LINES: usize = 5;
/// The level curve runs from this many dB (the bottom of its box) to 0.
pub const LEVEL_FLOOR_DB: f32 = -48.0;
/// The brightness curve runs from this frequency (the bottom of its box)...
pub const BRIGHT_LOW_HZ: f32 = 50.0;
/// ...to this one.
pub const BRIGHT_HIGH_HZ: f32 = 20_000.0;
/// The pitch axis is at least this many semitones high.
pub const PITCH_MIN_SPAN: f32 = 6.0;

/// One curve of the target and of the render of the patch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Curve {
    /// The target's values per frame (level in dB, 0 or less; brightness and pitch in Hz, 0 where
    /// the frame has no value).
    pub target: Vec<f32>,
    /// Seconds between two frames of the target.
    pub target_hop_s: f32,
    /// The render of the patch, on the same scale; empty before there is one.
    pub made: Vec<f32>,
    /// Seconds between two frames of the render.
    pub made_hop_s: f32,
}

/// How far the patch's render is from the target on one curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    /// The distance in the unit of the curve (dB, octaves, semitones).
    pub value: f32,
    /// The unit, for the label.
    pub unit: &'static str,
    /// The score, 0 to 1.
    pub score: f32,
}

/// The match of the patch to the target.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchRow {
    /// What was rendered: the proposal, or the patch of the track as it is now.
    pub of: String,
    /// The level curve.
    pub level: Distance,
    /// The brightness curve.
    pub brightness: Distance,
    /// The pitch curve; `None` when the target has no pitch.
    pub pitch: Option<Distance>,
    /// The mean of the scores, 0 to 1.
    pub overall: f32,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SoundAnalystData {
    /// The file that was analysed (its name and length), when there is one.
    pub file: Option<String>,
    /// How far a running analysis is (`0.0..=1.0`); `None` when none runs.
    pub running: Option<f32>,
    /// The measured numbers, one line each (at most [`NUMBER_LINES`] are drawn).
    pub numbers: Vec<String>,
    /// What the proposed patch is and why, one line each (at most [`PLAN_LINES`] are drawn).
    pub plan: Vec<String>,
    /// The level curve.
    pub level: Curve,
    /// The brightness curve.
    pub brightness: Curve,
    /// The pitch curve; `None` for an unpitched sound.
    pub pitch: Option<Curve>,
    /// The match of the patch.
    pub matched: Option<MatchRow>,
    /// Whether a patch is proposed and Make Patch can add it.
    pub can_make: bool,
    /// Whether a patch was made and Match can render it.
    pub can_match: bool,
    /// The match is of a patch that has been changed since, or of the proposal when a patch was
    /// made: the label says which.
    pub stale: bool,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundAnalystAction {
    /// Pick an audio file to analyse.
    OpenFile,
    /// Stop the running analysis.
    Stop,
    /// Add the proposed patch on a new track.
    MakePatch,
    /// Render the patch that was made and measure it.
    Match,
    /// Close the panel.
    Close,
}

/// The Sound Analyst panel. See the module documentation.
#[derive(Debug)]
pub struct SoundAnalystView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: SoundAnalystData,
    hover: Option<Point>,
    repaint: bool,
}

impl SoundAnalystView {
    /// A panel with nothing analysed yet; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 960.0, 700.0),
            scale: 1.0,
            data: SoundAnalystData::default(),
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

    /// Shows `data`.
    pub fn set_data(&mut self, data: SoundAnalystData) {
        if self.data != data {
            self.data = data;
            self.repaint = true;
        }
    }

    /// What the panel shows.
    pub fn data(&self) -> &SoundAnalystData {
        &self.data
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
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
        HEADER_H
            + FILE_H
            + 3.0 * (CURVE_H + CURVE_GAP)
            + (NUMBER_LINES + PLAN_LINES) as f64 * LINE_H
            + FOOT_H
            + 5.0 * PAD
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 70.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    /// The MATCH button.
    pub fn match_rect(&self) -> Rect {
        let close = self.close_rect();
        Rect::new(close.x0 - 8.0 - 70.0, close.y0, close.x0 - 8.0, close.y1)
    }

    /// The MAKE PATCH button.
    pub fn make_rect(&self) -> Rect {
        let m = self.match_rect();
        Rect::new(m.x0 - 8.0 - 96.0, m.y0, m.x0 - 8.0, m.y1)
    }

    /// The OPEN FILE (or STOP) button.
    pub fn open_rect(&self) -> Rect {
        let m = self.make_rect();
        Rect::new(m.x0 - 8.0 - 92.0, m.y0, m.x0 - 8.0, m.y1)
    }

    /// The box of curve `i` (0 level, 1 brightness, 2 pitch).
    pub fn curve_rect(&self, i: usize) -> Rect {
        let p = self.panel();
        let y0 =
            p.y0 + HEADER_H + FILE_H + PAD + i as f64 * (CURVE_H + CURVE_GAP) + CURVE_GAP * 0.6;
        Rect::new(p.x0 + PAD + 82.0, y0, p.x1 - PAD - SIDE_W, y0 + CURVE_H)
    }

    fn numbers_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + FILE_H + 2.0 * PAD + 3.0 * (CURVE_H + CURVE_GAP) + 4.0
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        if self.match_rect().contains(pos) {
            return help(
                self.match_rect(),
                "Render the patch as it is now on the same note and compare its curves",
            );
        }
        if self.make_rect().contains(pos) {
            return help(
                self.make_rect(),
                "Add the proposed patch on a new track (one undo step)",
            );
        }
        if self.open_rect().contains(pos) {
            return if self.data.running.is_some() {
                help(self.open_rect(), "Stop the analysis")
            } else {
                help(
                    self.open_rect(),
                    "Pick an audio file to analyse (or drop one on the panel)",
                )
            };
        }
        let curves = [
            "The loudness over time: attack, decay, sustain and release",
            "The brightness over time: the spectral centroid of the sound",
            "The pitch over time, or nothing for an unpitched sound",
        ];
        for (i, text) in curves.iter().enumerate() {
            let r = self.curve_rect(i);
            if r.inflate(0.0, 4.0).contains(pos) {
                return help(r, text);
            }
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = UiAction::SoundAnalyst;
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
                if self.close_rect().contains(*pos) {
                    out.push(act(SoundAnalystAction::Close));
                } else if self.open_rect().contains(*pos) {
                    out.push(act(if self.data.running.is_some() {
                        SoundAnalystAction::Stop
                    } else {
                        SoundAnalystAction::OpenFile
                    }));
                } else if self.make_rect().contains(*pos) && self.data.can_make {
                    out.push(act(SoundAnalystAction::MakePatch));
                } else if self.match_rect().contains(*pos) && self.data.can_match {
                    out.push(act(SoundAnalystAction::Match));
                }
            }
            UiEvent::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } if self.data.can_make && self.data.running.is_none() => {
                out.push(act(SoundAnalystAction::MakePatch));
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
            "SOUND ANALYST",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        sp::button(
            c,
            st,
            self.close_rect(),
            "CLOSE",
            None,
            hovered(self.close_rect()),
        );
        sp::button(
            c,
            st,
            self.match_rect(),
            "MATCH",
            None,
            hovered(self.match_rect()),
        );
        sp::button(
            c,
            st,
            self.make_rect(),
            "MAKE PATCH",
            self.data.can_make.then_some(t.focus),
            hovered(self.make_rect()),
        );
        let open = if self.data.running.is_some() {
            "STOP"
        } else {
            "OPEN FILE"
        };
        sp::button(
            c,
            st,
            self.open_rect(),
            open,
            None,
            hovered(self.open_rect()),
        );
        let (file, colour) = match &self.data.file {
            Some(f) => (f.as_str(), t.text),
            None => (
                "Drop an audio file on the panel, or press OPEN FILE",
                t.text_muted,
            ),
        };
        sp::label(
            c,
            &fit_label(file, p.width() - 2.0 * PAD, 11.0),
            p.x0 + PAD,
            p.y0 + HEADER_H + 17.0,
            11.0,
            colour,
            Align::Start,
        );
        self.paint_curves(c);
        self.paint_lines(c);
        let foot = match (&self.data.matched, self.data.running) {
            (_, Some(_)) => "Analysing and trying a patch: the song keeps playing meanwhile",
            (None, None) if self.data.can_make => {
                "Make Patch adds the proposed patch on a new track"
            }
            (None, None) => "The analysis never changes the song; only Make Patch does",
            (Some(_), None) if self.data.stale => {
                "The patch changed since this match: press Match again"
            }
            (Some(_), None) => "The thin line is the patch, the thick line the sound you gave it",
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

    fn paint_curves(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let d = &self.data;
        let span = |curve: &Curve| {
            (curve.target.len() as f32 * curve.target_hop_s)
                .max(curve.made.len() as f32 * curve.made_hop_s)
        };
        let seconds = [Some(&d.level), Some(&d.brightness), d.pitch.as_ref()]
            .iter()
            .flatten()
            .map(|cv| span(cv))
            .fold(0.0f32, f32::max);
        let rows: [(&str, Option<&Curve>, Option<Distance>); 3] = [
            ("LEVEL", Some(&d.level), d.matched.as_ref().map(|m| m.level)),
            (
                "BRIGHTNESS",
                Some(&d.brightness),
                d.matched.as_ref().map(|m| m.brightness),
            ),
            (
                "PITCH",
                d.pitch.as_ref(),
                d.matched.as_ref().and_then(|m| m.pitch),
            ),
        ];
        for (i, (name, curve, distance)) in rows.into_iter().enumerate() {
            let r = self.curve_rect(i);
            sp::label(
                c,
                name,
                self.panel().x0 + PAD,
                r.y0 + 12.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
            sp::lcd(c, st, r.inflate(2.0, 2.0), None);
            let Some(curve) = curve.filter(|cv| !cv.target.is_empty() || !cv.made.is_empty())
            else {
                let text = match (i, &d.file) {
                    (2, Some(_)) => "unpitched",
                    (_, Some(_)) => "",
                    _ => "nothing analysed yet",
                };
                sp::label(
                    c,
                    text,
                    r.center().x,
                    r.center().y + 4.0,
                    FONT,
                    t.text_muted,
                    Align::Center,
                );
                continue;
            };
            if i == 0
                && let Some(progress) = d.running
            {
                let w = r.width() * f64::from(progress.clamp(0.0, 1.0));
                sp::rect(
                    c,
                    Rect::new(r.x0, r.y0, r.x0 + w, r.y1),
                    mix(t.lcd, t.focus, 0.4),
                );
            }
            let scale = AxisScale::of(i, curve);
            for (hz_or_db, text) in scale.ticks() {
                let y = scale.y(r, hz_or_db);
                sp::line(c, r.x0, y, r.x1, y, mix(t.lcd, t.text, 0.12));
                sp::label(c, &text, r.x0 - 4.0, y + 3.0, 8.0, t.text_muted, Align::End);
            }
            let thick = t.focus;
            let thin = mix(t.text, t.warning, 0.5);
            polyline(
                c,
                r,
                (&curve.made, curve.made_hop_s),
                seconds,
                &scale,
                thin,
                1.0,
            );
            polyline(
                c,
                r,
                (&curve.target, curve.target_hop_s),
                seconds,
                &scale,
                thick,
                2.0,
            );
            let side = Rect::new(r.x1 + 12.0, r.y0, self.panel().x1 - PAD, r.y1);
            if let Some(dist) = distance {
                sp::label(
                    c,
                    &format!("{:.2} {}", dist.value, dist.unit),
                    side.x0,
                    side.y0 + 16.0,
                    FONT + 1.0,
                    t.text,
                    Align::Start,
                );
                sp::label(
                    c,
                    &format!("{:.0} %", dist.score * 100.0),
                    side.x0,
                    side.y0 + 34.0,
                    13.0,
                    if d.stale { t.text_muted } else { t.text },
                    Align::Start,
                );
                let bar = Rect::new(side.x0, side.y0 + 44.0, side.x1, side.y0 + 52.0);
                sp::lcd(c, st, bar, None);
                let fill = mix(
                    t.lcd,
                    score_colour(t, dist.score),
                    if d.stale { 0.4 } else { 1.0 },
                );
                sp::rect(
                    c,
                    Rect::new(
                        bar.x0,
                        bar.y0,
                        bar.x0 + bar.width() * f64::from(dist.score.clamp(0.0, 1.0)),
                        bar.y1,
                    ),
                    fill,
                );
            }
        }
        if let Some(m) = &d.matched {
            let p = self.panel();
            sp::label(
                c,
                &fit_label(
                    &format!("{}: {:.0} % match", m.of, m.overall * 100.0),
                    SIDE_W + 90.0,
                    10.0,
                ),
                p.x1 - PAD,
                self.header_line(),
                10.0,
                if d.stale { t.text_muted } else { t.text },
                Align::End,
            );
        }
    }

    /// The baseline of the line with the file name, for what is on its right.
    fn header_line(&self) -> f64 {
        self.panel().y0 + HEADER_H + 17.0
    }

    fn paint_lines(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let p = self.panel();
        let width = p.width() - 2.0 * PAD;
        let mut y = self.numbers_top() + 8.0;
        for line in self.data.numbers.iter().take(NUMBER_LINES) {
            sp::label(
                c,
                &fit_label(line, width, 9.5),
                p.x0 + PAD,
                y,
                9.5,
                t.text,
                Align::Start,
            );
            y += LINE_H;
        }
        y = self.numbers_top() + 8.0 + NUMBER_LINES as f64 * LINE_H + 4.0;
        for line in self.data.plan.iter().take(PLAN_LINES) {
            sp::label(
                c,
                &fit_label(line, width, 9.5),
                p.x0 + PAD,
                y,
                9.5,
                t.text_muted,
                Align::Start,
            );
            y += LINE_H;
        }
    }
}

fn score_colour(t: &Theme, score: f32) -> Color {
    if score >= 0.66 {
        t.io_connected
    } else if score >= 0.4 {
        t.warning
    } else {
        t.clip
    }
}

/// How a curve's values map to the height of its box.
struct AxisScale {
    kind: usize,
    low: f32,
    high: f32,
}

impl AxisScale {
    fn of(kind: usize, curve: &Curve) -> Self {
        match kind {
            0 => Self {
                kind,
                low: LEVEL_FLOOR_DB,
                high: 0.0,
            },
            1 => Self {
                kind,
                low: BRIGHT_LOW_HZ.log2(),
                high: BRIGHT_HIGH_HZ.log2(),
            },
            _ => {
                let semis: Vec<f32> = curve
                    .target
                    .iter()
                    .chain(&curve.made)
                    .filter(|h| **h > 0.0)
                    .map(|h| 12.0 * (h / 440.0).log2())
                    .collect();
                let lo = semis.iter().copied().fold(f32::INFINITY, f32::min);
                let hi = semis.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let (lo, hi) = if lo.is_finite() && hi.is_finite() {
                    (lo, hi)
                } else {
                    (-3.0, 3.0)
                };
                let mid = 0.5 * (lo + hi);
                let half = (0.5 * (hi - lo) + 1.0).max(PITCH_MIN_SPAN / 2.0);
                Self {
                    kind,
                    low: mid - half,
                    high: mid + half,
                }
            }
        }
    }

    /// The value of the axis for a stored value, `None` where it has none.
    fn value(&self, v: f32) -> Option<f32> {
        match self.kind {
            0 => Some(v.max(LEVEL_FLOOR_DB)),
            _ if v <= 0.0 => None,
            1 => Some(v.log2()),
            _ => Some(12.0 * (v / 440.0).log2()),
        }
    }

    fn y(&self, r: Rect, value: f32) -> f64 {
        let share = ((value - self.low) / (self.high - self.low)).clamp(0.0, 1.0);
        r.y1 - r.height() * f64::from(share)
    }

    /// The gridlines: `(value of the axis, label)`.
    fn ticks(&self) -> Vec<(f32, String)> {
        match self.kind {
            0 => vec![(0.0, "0 dB".into()), (-24.0, "-24".into())],
            1 => [100.0f32, 1_000.0, 10_000.0]
                .iter()
                .map(|hz| {
                    (
                        hz.log2(),
                        if *hz >= 1_000.0 {
                            format!("{:.0}k", hz / 1_000.0)
                        } else {
                            format!("{hz:.0}")
                        },
                    )
                })
                .collect(),
            _ => {
                let mid = 0.5 * (self.low + self.high);
                let hz = 440.0 * (mid / 12.0).exp2();
                vec![(mid, format!("{hz:.0} Hz"))]
            }
        }
    }
}

/// Draws `values` (one per `hop_s`) as a line inside `r`, over `seconds` of time.
fn polyline(
    c: &mut dyn Canvas,
    r: Rect,
    (values, hop_s): (&[f32], f32),
    seconds: f32,
    scale: &AxisScale,
    colour: Color,
    width: f64,
) {
    if values.len() < 2 || seconds <= 0.0 || hop_s <= 0.0 {
        return;
    }
    // At most one segment per pixel.
    let step = ((values.len() as f64 / r.width().max(1.0)).ceil() as usize).max(1);
    let mut last: Option<(f64, f64)> = None;
    for i in (0..values.len()).step_by(step) {
        let Some(v) = scale.value(values[i]) else {
            last = None;
            continue;
        };
        let x = r.x0 + r.width() * f64::from(i as f32 * hop_s / seconds).min(1.0);
        let y = scale.y(r, v);
        if let Some((px, py)) = last {
            c.stroke_with(
                vello::kurbo::Line::new(Point::new(px, py), Point::new(x, y)),
                width,
                colour,
            );
        }
        last = Some((x, y));
    }
}
