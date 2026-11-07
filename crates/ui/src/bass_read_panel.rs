//! The "Will the bass read?" panel: the strip to listen to (the master or a track), a meter of how
//! much of its low-end note energy a small speaker still plays through the harmonics of the note,
//! and the verdict.
//!
//! ```text
//! WILL THE BASS READ?                                  [ MEASURE ]  [ CLOSE ]
//! Master
//! Bass                                            (the strips of the mixer)
//! ...
//! |=========|-------|---------------|          16 %
//! lost      thin    reads
//! Yes: the harmonics carry the bass
//! one line on what to do
//! ```
//!
//! A modal view the app places over the window like the mix check. The app fills it with a
//! [`BassReadData`] ([`BassReadView::set_data`]); the view only draws and reports what was pressed
//! as a [`UiAction::BassRead`]:
//!
//! * A row chooses the strip ([`BassReadAction::Choose`]; also Up and Down; the wheel scrolls).
//! * `MEASURE` ([`BassReadAction::Measure`]; also Enter) renders the chosen strip and reads it;
//!   while it runs the button reads `STOP` ([`BassReadAction::Stop`]) and the meter shows the
//!   progress.
//! * `CLOSE` emits [`BassReadAction::Close`]; the app also closes the panel on Esc.
//!
//! The meter runs from 0 to [`METER_FULL`] of the note energy; the two ticks are where the
//! verdict changes ([`atomik_dsp::bass_read::PARTLY_FROM`], [`atomik_dsp::bass_read::READS_FROM`]).

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_dsp::bass_read::{PARTLY_FROM, READS_FROM};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const ROW_H: f64 = 22.0;
const METER_H: f64 = 120.0;
const FOOT_H: f64 = 26.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 640.0;
const BTN_H: f64 = 22.0;
const FONT: f32 = 10.5;
/// The strips the list shows at once.
pub const ROWS: usize = 7;
/// The share of the note energy at the right end of the meter (0 to 1).
pub const METER_FULL: f32 = 0.3;

/// How the reading came out, for the colour of the meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BassLevel {
    /// There were no low notes to judge.
    None,
    /// The bass disappears.
    Lost,
    /// The bass is thin.
    Partly,
    /// The bass reads.
    Reads,
}

/// What one measurement found.
#[derive(Debug, Clone, PartialEq)]
pub struct BassReadingRow {
    /// The strip that was measured.
    pub source: String,
    /// The share of the note energy that survives, 0 to 1.
    pub share: f32,
    /// The verdict, one sentence.
    pub verdict: String,
    /// What to do about it, one line.
    pub advice: String,
    /// A line with the numbers (the pitch of the low notes, how many were found).
    pub detail: String,
    /// The colour class of the verdict.
    pub level: BassLevel,
    /// The song changed after the measurement.
    pub stale: bool,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BassReadData {
    /// The strips to choose from; the first is the master.
    pub sources: Vec<String>,
    /// The chosen strip.
    pub chosen: usize,
    /// How far a running measurement is (`0.0..=1.0`); `None` when none runs.
    pub running: Option<f32>,
    /// The last measurement.
    pub reading: Option<BassReadingRow>,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BassReadAction {
    /// Choose strip `n` of the list.
    Choose(usize),
    /// Render the chosen strip and read it.
    Measure,
    /// Stop the measurement.
    Stop,
    /// Close the panel.
    Close,
}

/// The bass-read panel. See the module documentation.
#[derive(Debug)]
pub struct BassReadView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: BassReadData,
    top: usize,
    hover: Option<Point>,
    repaint: bool,
}

impl BassReadView {
    /// A panel with nothing measured yet; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 960.0, 600.0),
            scale: 1.0,
            data: BassReadData::default(),
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

    /// Shows `data`. The scroll follows the chosen strip.
    pub fn set_data(&mut self, data: BassReadData) {
        if self.data != data {
            self.data = data;
            self.keep_chosen_in_view();
            self.repaint = true;
        }
    }

    /// What the panel shows.
    pub fn data(&self) -> &BassReadData {
        &self.data
    }

    /// The first strip the list shows.
    pub fn top(&self) -> usize {
        self.top
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    fn count(&self) -> usize {
        self.data.sources.len()
    }

    fn keep_chosen_in_view(&mut self) {
        let chosen = self.data.chosen;
        if chosen < self.top {
            self.top = chosen;
        } else if chosen >= self.top + ROWS {
            self.top = chosen + 1 - ROWS;
        }
        self.top = self.top.min(self.count().saturating_sub(ROWS));
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
        HEADER_H + ROWS as f64 * ROW_H + METER_H + FOOT_H + 4.0 * PAD
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

    /// The MEASURE (or STOP) button.
    pub fn measure_rect(&self) -> Rect {
        let close = self.close_rect();
        Rect::new(close.x0 - 10.0 - 100.0, close.y0, close.x0 - 10.0, close.y1)
    }

    /// The list of strips.
    pub fn list_rect(&self) -> Rect {
        let p = self.panel();
        let y0 = p.y0 + HEADER_H + PAD;
        Rect::new(p.x0 + PAD, y0, p.x1 - PAD, y0 + ROWS as f64 * ROW_H)
    }

    /// The row in slot `i` of the list (the strip `top() + i`).
    pub fn row_rect(&self, i: usize) -> Rect {
        let l = self.list_rect();
        Rect::new(
            l.x0,
            l.y0 + i as f64 * ROW_H,
            l.x1,
            l.y0 + (i + 1) as f64 * ROW_H,
        )
    }

    /// The meter bar.
    pub fn meter_rect(&self) -> Rect {
        let l = self.list_rect();
        let y0 = l.y1 + PAD + 4.0;
        Rect::new(l.x0, y0, l.x1 - 70.0, y0 + 22.0)
    }

    fn slots(&self) -> usize {
        self.count().saturating_sub(self.top).min(ROWS)
    }

    /// The x of `share` (0 to 1) on the meter.
    fn meter_x(&self, share: f32) -> f64 {
        let m = self.meter_rect();
        m.x0 + m.width() * f64::from((share / METER_FULL).clamp(0.0, 1.0))
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        if self.measure_rect().contains(pos) {
            return if self.data.running.is_some() {
                help(self.measure_rect(), "Stop the measurement")
            } else {
                help(
                    self.measure_rect(),
                    "Render the chosen strip and read how much of its bass a small speaker keeps",
                )
            };
        }
        if self.list_rect().contains(pos) && self.count() > 0 {
            return help(
                self.list_rect(),
                "Choose the master or a track: the meter reads the strip as it sounds in the mix",
            );
        }
        if self.meter_rect().inflate(0.0, 6.0).contains(pos) {
            return help(
                self.meter_rect(),
                "How much of the bass note's energy a small speaker plays in its harmonics",
            );
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = UiAction::BassRead;
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
                    out.push(act(BassReadAction::Close));
                } else if self.measure_rect().contains(*pos) {
                    out.push(act(if self.data.running.is_some() {
                        BassReadAction::Stop
                    } else {
                        BassReadAction::Measure
                    }));
                } else if let Some(i) = (0..self.slots()).find(|i| self.row_rect(*i).contains(*pos))
                {
                    out.push(act(BassReadAction::Choose(self.top + i)));
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Up if self.data.chosen > 0 => {
                    out.push(act(BassReadAction::Choose(self.data.chosen - 1)));
                }
                Key::Down if self.data.chosen + 1 < self.count() => {
                    out.push(act(BassReadAction::Choose(self.data.chosen + 1)));
                }
                Key::Enter if self.data.running.is_none() => {
                    out.push(act(BassReadAction::Measure));
                }
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
            "WILL THE BASS READ?",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let measure = self.measure_rect();
        let label = if self.data.running.is_some() {
            "STOP"
        } else {
            "MEASURE"
        };
        sp::button(c, st, measure, label, None, hovered(measure));
        self.paint_list(c);
        self.paint_meter(c);
        let foot = match (&self.data.reading, self.data.running) {
            (_, Some(_)) => "Rendering the strip: the song keeps playing meanwhile",
            (None, None) => "Measure renders the chosen strip and reads its low notes",
            (Some(r), None) if r.stale => "The song changed since this reading: measure again",
            (Some(_), None) => "A small speaker plays the harmonics and the ear rebuilds the note",
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
        if self.data.sources.is_empty() {
            sp::label(
                c,
                "No strip to measure",
                list.center().x,
                list.center().y,
                FONT,
                t.text_muted,
                Align::Center,
            );
            return;
        }
        for i in 0..self.slots() {
            let n = self.top + i;
            let Some(name) = self.data.sources.get(n) else {
                continue;
            };
            let r = self.row_rect(i);
            if n == self.data.chosen {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            sp::label(
                c,
                &fit_label(name, r.width() - 16.0, FONT),
                r.x0 + 8.0,
                r.y0 + 15.0,
                FONT,
                t.text,
                Align::Start,
            );
        }
        if self.count() > ROWS {
            sp::label(
                c,
                &format!(
                    "{}-{} of {}",
                    self.top + 1,
                    self.top + self.slots(),
                    self.count()
                ),
                list.x1 - 4.0,
                list.y1 + 11.0,
                9.0,
                t.text_muted,
                Align::End,
            );
        }
    }

    fn paint_meter(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let m = self.meter_rect();
        sp::lcd(c, st, m.inflate(2.0, 2.0), None);
        if let Some(progress) = self.data.running {
            let w = m.width() * f64::from(progress.clamp(0.0, 1.0));
            sp::rect(
                c,
                Rect::new(m.x0, m.y0, m.x0 + w, m.y1),
                mix(t.lcd, t.focus, 0.6),
            );
            sp::label(
                c,
                &format!("{:.0} %", progress.clamp(0.0, 1.0) * 100.0),
                m.x1 + 8.0,
                m.center().y + 4.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
        } else if let Some(r) = &self.data.reading {
            let colour = match r.level {
                BassLevel::None => t.text_muted,
                BassLevel::Lost => t.clip,
                BassLevel::Partly => t.warning,
                BassLevel::Reads => t.io_connected,
            };
            let x = self.meter_x(r.share);
            let colour = if r.stale {
                mix(t.lcd, colour, 0.4)
            } else {
                colour
            };
            if r.level != BassLevel::None {
                sp::rect(c, Rect::new(m.x0, m.y0, x, m.y1), colour);
            }
            sp::label(
                c,
                &format!("{:.0} %", r.share * 100.0),
                m.x1 + 8.0,
                m.center().y + 4.0,
                13.0,
                if r.stale { t.text_muted } else { t.text },
                Align::Start,
            );
            sp::label(
                c,
                &fit_label(
                    &format!("{}: {}", r.source, r.verdict),
                    self.panel().width() - 2.0 * PAD,
                    11.5,
                ),
                m.x0,
                m.y1 + 24.0,
                11.5,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &fit_label(&r.advice, self.panel().width() - 2.0 * PAD, 9.5),
                m.x0,
                m.y1 + 42.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
            sp::label(
                c,
                &fit_label(&r.detail, self.panel().width() - 2.0 * PAD, 9.5),
                m.x0,
                m.y1 + 58.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
        } else {
            sp::label(
                c,
                "Not measured yet",
                m.center().x,
                m.center().y + 4.0,
                FONT,
                t.text_muted,
                Align::Center,
            );
        }
        // The two thresholds of the verdict.
        for (share, text) in [(PARTLY_FROM, "thin"), (READS_FROM, "reads")] {
            let x = self.meter_x(share);
            sp::line(c, x, m.y0 - 3.0, x, m.y1 + 3.0, t.text_muted);
            sp::label(
                c,
                text,
                x + 3.0,
                m.y0 - 5.0,
                8.5,
                t.text_muted,
                Align::Start,
            );
        }
    }
}
