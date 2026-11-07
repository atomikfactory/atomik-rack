//! The masking map panel: the pairs of tracks that cover each other, ranked, with the two spectra
//! of the chosen pair and a button that turns the finding into a dynamic cut.
//!
//! ```text
//! MASKING MAP                                            [ ANALYSE ]  [ CLOSE ]
//! 1  Kick > Bass      250 Hz   82%  |  Bass under Kick   worst band 250 Hz
//! 2  Vox > Keys       500 Hz   41%  |  [spectra: one pair of bars per band, clash bands shaded]
//! 3  ...                            |  63  125  250  500  1k  2k  4k  8k  16k
//!                                   |  Time ranges:  [0:12.0-0:24.5] [1:02.1-1:10.0]
//!                                   |  [ DE-MASK 250 Hz ]  [ DE-MASK ]
//! note line
//! ```
//!
//! A modal view the app places over the window like the song information panel. The app fills it
//! with a [`MaskingData`] when an analysis finishes ([`MaskingView::set_data`]) and shows progress
//! while one runs ([`MaskingView::set_progress`]); the view only draws and reports what was
//! pressed as a [`UiAction::Masking`]:
//!
//! * `ANALYSE` ([`MaskingAction::Analyse`]); while an analysis runs the button says `CANCEL` and
//!   emits [`MaskingAction::Cancel`].
//! * A row of the list chooses the pair (also Up and Down).
//! * A band of the spectra chooses the band the de-mask works on (the worst band is the default).
//! * A time range emits [`MaskingAction::Locate`]: the app puts the playhead at its start.
//! * `DE-MASK` emits [`MaskingAction::Demask`] with the pair and the band.
//! * `CLOSE` emits [`MaskingAction::Close`]; the app also closes the panel on Esc.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_dsp::masking::{BANDS, band_centre, band_label};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const ROW_H: f64 = 24.0;
const LIST_W: f64 = 290.0;
const CHART_H: f64 = 170.0;
const AXIS_H: f64 = 16.0;
const RANGE_H: f64 = 22.0;
const FOOT_H: f64 = 30.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 800.0;
const BTN_H: f64 = 22.0;
const FONT: f32 = 10.5;
/// The pairs the list shows.
pub const LIST_ROWS: usize = 12;
/// The time ranges shown under the chart.
pub const RANGE_SLOTS: usize = 4;
/// The level at the foot of the chart, dB.
pub const CHART_FLOOR_DB: f32 = -80.0;

/// One ranked pair, as the panel draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskingPair {
    /// The louder track.
    pub masker: String,
    /// The track that is covered.
    pub masked: String,
    /// The band with the highest score.
    pub worst: usize,
    /// The pair score, percent.
    pub score: f32,
    /// The clash zone: the bands that are shaded.
    pub zone: [bool; BANDS],
    /// The mean level of the louder track per band, dB.
    pub masker_db: [f32; BANDS],
    /// The mean level of the covered track per band, dB.
    pub masked_db: [f32; BANDS],
    /// The time ranges of the clash as `(start, end)` in seconds of the song.
    pub ranges: Vec<(f64, f64)>,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaskingData {
    /// How many tracks were compared.
    pub tracks: usize,
    /// The worst pairs, highest score first.
    pub pairs: Vec<MaskingPair>,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaskingAction {
    /// Analyse the song (or the time selection) now.
    Analyse,
    /// Stop the analysis that runs.
    Cancel,
    /// Put the playhead at the start of a time range of a pair.
    Locate {
        /// The pair (its place in the list).
        pair: usize,
        /// The range of the pair.
        range: usize,
    },
    /// Make the covered track of a pair yield to the other in a band.
    Demask {
        /// The pair.
        pair: usize,
        /// The band, an index of [`atomik_dsp::masking`]'s bands.
        band: usize,
    },
    /// Close the panel.
    Close,
}

/// The masking map panel. See the module documentation.
#[derive(Debug)]
pub struct MaskingView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: Option<MaskingData>,
    progress: Option<f32>,
    note: String,
    pair: usize,
    band: Option<usize>,
    hover: Option<Point>,
    repaint: bool,
}

/// `1:05.3` for 65.3 seconds.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let minutes = (s / 60.0).floor();
    format!("{}:{:04.1}", minutes as u64, s - minutes * 60.0)
}

impl MaskingView {
    /// A panel with nothing analysed yet; call [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            data: None,
            progress: None,
            note: String::new(),
            pair: 0,
            band: None,
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

    /// Shows the result of an analysis (the first pair is chosen).
    pub fn set_data(&mut self, data: Option<MaskingData>) {
        if self.data != data {
            self.pair = 0;
            self.band = None;
            self.data = data;
            self.repaint = true;
        }
    }

    /// Shows the progress of the analysis that runs (`None`: none runs).
    pub fn set_progress(&mut self, progress: Option<f32>) {
        if self.progress != progress {
            self.progress = progress;
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

    /// The result that is shown.
    pub fn data(&self) -> Option<&MaskingData> {
        self.data.as_ref()
    }

    /// The pair that is chosen (its place in the list).
    pub fn pair(&self) -> usize {
        self.pair
    }

    /// The band the de-mask works on: the one that was clicked, else the worst of the pair.
    pub fn band(&self) -> Option<usize> {
        let pair = self.data.as_ref()?.pairs.get(self.pair)?;
        Some(self.band.unwrap_or(pair.worst))
    }

    /// Whether an analysis runs.
    pub fn busy(&self) -> bool {
        self.progress.is_some()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    fn current(&self) -> Option<&MaskingPair> {
        self.data.as_ref()?.pairs.get(self.pair)
    }

    fn choose(&mut self, pair: usize) {
        let n = self
            .data
            .as_ref()
            .map_or(0, |d| d.pairs.len().min(LIST_ROWS));
        if pair < n && pair != self.pair {
            self.pair = pair;
            self.band = None;
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
        HEADER_H + CHART_H + AXIS_H + 3.0 * RANGE_H + 2.0 * BTN_H + FOOT_H + 4.0 * PAD
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

    /// The ANALYSE (or CANCEL) button.
    pub fn analyse_rect(&self) -> Rect {
        let close = self.close_rect();
        Rect::new(close.x0 - 10.0 - 110.0, close.y0, close.x0 - 10.0, close.y1)
    }

    /// The list of pairs.
    pub fn list_rect(&self) -> Rect {
        let p = self.panel();
        let y0 = p.y0 + HEADER_H + PAD;
        Rect::new(
            p.x0 + PAD,
            y0,
            p.x0 + PAD + LIST_W,
            y0 + LIST_ROWS as f64 * ROW_H,
        )
    }

    /// The row of pair `i`.
    pub fn row_rect(&self, i: usize) -> Rect {
        let l = self.list_rect();
        Rect::new(
            l.x0,
            l.y0 + i as f64 * ROW_H,
            l.x1,
            l.y0 + (i + 1) as f64 * ROW_H,
        )
    }

    /// The spectra chart (the bars of the bands).
    pub fn chart_rect(&self) -> Rect {
        let p = self.panel();
        let l = self.list_rect();
        let y0 = p.y0 + HEADER_H + PAD + ROW_H;
        Rect::new(l.x1 + PAD, y0, p.x1 - PAD, y0 + CHART_H)
    }

    /// The slot of band `b` in the chart.
    pub fn band_rect(&self, b: usize) -> Rect {
        let c = self.chart_rect();
        let w = c.width() / BANDS as f64;
        Rect::new(c.x0 + b as f64 * w, c.y0, c.x0 + (b + 1) as f64 * w, c.y1)
    }

    /// The button of time range `i` of the chosen pair.
    pub fn range_rect(&self, i: usize) -> Rect {
        let c = self.chart_rect();
        let w = c.width() / RANGE_SLOTS as f64;
        let y0 = c.y1 + AXIS_H + RANGE_H;
        Rect::new(
            c.x0 + i as f64 * w,
            y0,
            c.x0 + (i + 1) as f64 * w - 6.0,
            y0 + BTN_H,
        )
    }

    /// The DE-MASK button.
    pub fn demask_rect(&self) -> Rect {
        let c = self.chart_rect();
        let y0 = c.y1 + AXIS_H + 3.0 * RANGE_H + PAD / 2.0;
        Rect::new(c.x0, y0, c.x0 + 190.0, y0 + BTN_H)
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        if self.analyse_rect().contains(pos) {
            return help(
                self.analyse_rect(),
                if self.busy() {
                    "Stop the analysis"
                } else {
                    "Render the channels and rank the pairs that mask each other"
                },
            );
        }
        if self.demask_rect().contains(pos) && self.current().is_some() {
            return help(
                self.demask_rect(),
                "Key a Dynamic EQ band on the covered track from the other (one undo step)",
            );
        }
        if self.chart_rect().contains(pos) && self.current().is_some() {
            return help(
                self.chart_rect(),
                "Click a band to de-mask it; shaded bands are where the tracks clash",
            );
        }
        let ranges = self
            .current()
            .map_or(0, |p| p.ranges.len().min(RANGE_SLOTS));
        if (0..ranges).any(|i| self.range_rect(i).contains(pos)) {
            return help(
                self.chart_rect(),
                "Move the playhead to the start of this clash",
            );
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = |a: MaskingAction| UiAction::Masking(a);
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
                let n = self
                    .data
                    .as_ref()
                    .map_or(0, |d| d.pairs.len().min(LIST_ROWS));
                if self.close_rect().contains(*pos) {
                    out.push(act(MaskingAction::Close));
                } else if self.analyse_rect().contains(*pos) {
                    out.push(act(if self.busy() {
                        MaskingAction::Cancel
                    } else {
                        MaskingAction::Analyse
                    }));
                } else if let Some(i) = (0..n).find(|i| self.row_rect(*i).contains(*pos)) {
                    self.choose(i);
                } else if self.current().is_some() && self.demask_rect().contains(*pos) {
                    if let Some(band) = self.band() {
                        out.push(act(MaskingAction::Demask {
                            pair: self.pair,
                            band,
                        }));
                    }
                } else if self.current().is_some() && self.chart_rect().contains(*pos) {
                    if let Some(b) = (0..BANDS).find(|b| self.band_rect(*b).contains(*pos)) {
                        self.band = Some(b);
                    }
                } else if let Some(ranges) = self.current().map(|p| p.ranges.len().min(RANGE_SLOTS))
                    && let Some(range) = (0..ranges).find(|i| self.range_rect(*i).contains(*pos))
                {
                    out.push(act(MaskingAction::Locate {
                        pair: self.pair,
                        range,
                    }));
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Up => self.choose(self.pair.saturating_sub(1)),
                Key::Down => self.choose(self.pair + 1),
                _ => {}
            },
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    fn bar_height(chart: Rect, db: f32) -> f64 {
        let t = f64::from((db - CHART_FLOOR_DB) / -CHART_FLOOR_DB).clamp(0.0, 1.0);
        (chart.height() - 4.0) * t
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
            "MASKING MAP",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let analyse = self.analyse_rect();
        let text = match self.progress {
            Some(f) => format!("CANCEL {:.0}%", f * 100.0),
            None => "ANALYSE".to_owned(),
        };
        sp::button(
            c,
            st,
            analyse,
            &text,
            self.busy().then_some(st.record),
            hovered(analyse),
        );
        self.paint_list(c);
        self.paint_chart(c);
        let foot = if self.note.is_empty() {
            match &self.data {
                None => "Analyse renders every channel of the song (or the selected time range)",
                Some(d) if d.pairs.is_empty() => "No pair of tracks masks each other",
                Some(_) => "Pick a pair, click a band, then DE-MASK",
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
        sp::label(
            c,
            "Worst pairs: the first track covers the second",
            list.x0,
            list.y0 - 5.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        let Some(data) = &self.data else {
            return;
        };
        for (i, pair) in data.pairs.iter().take(LIST_ROWS).enumerate() {
            let r = self.row_rect(i);
            if i == self.pair {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            let names = format!("{} > {}", pair.masker, pair.masked);
            sp::label(
                c,
                &fit_label(&names, LIST_W - 118.0, FONT),
                r.x0 + 6.0,
                r.y0 + 16.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &band_label(pair.worst),
                r.x1 - 56.0,
                r.y0 + 16.0,
                FONT,
                t.text_muted,
                Align::End,
            );
            sp::label(
                c,
                &format!("{:.0}%", pair.score),
                r.x1 - 6.0,
                r.y0 + 16.0,
                FONT,
                t.text,
                Align::End,
            );
        }
    }

    fn paint_chart(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let chart = self.chart_rect();
        sp::lcd(c, st, chart, None);
        let Some(pair) = self.current() else {
            sp::label(
                c,
                if self.data.is_some() {
                    "Nothing to show"
                } else {
                    "No analysis yet"
                },
                chart.center().x,
                chart.center().y,
                FONT,
                t.text_muted,
                Align::Center,
            );
            return;
        };
        let chosen = self.band();
        let shade = mix(t.lcd, st.record, 0.22);
        let covering = st.record;
        let covered = st.loop_on;
        for b in 0..BANDS {
            let slot = self.band_rect(b);
            if pair.zone[b] {
                sp::rect(c, slot, shade);
            }
            let half = (slot.width() - 3.0) / 2.0;
            for (k, (db, colour)) in [(pair.masker_db[b], covering), (pair.masked_db[b], covered)]
                .into_iter()
                .enumerate()
            {
                let h = Self::bar_height(chart, db);
                if h < 1.0 {
                    continue;
                }
                let x = slot.x0 + 1.5 + k as f64 * half;
                sp::rect(
                    c,
                    Rect::new(x, chart.y1 - 2.0 - h, x + half, chart.y1 - 2.0),
                    colour,
                );
            }
            if chosen == Some(b) {
                c.stroke_with(slot.inset(-0.5), 1.0, t.focus);
            }
        }
        // The axis: a label every octave or so.
        for b in (0..BANDS).step_by(3) {
            let slot = self.band_rect(b);
            sp::label(
                c,
                &band_label(b),
                slot.center().x,
                chart.y1 + 12.0,
                8.5,
                t.text_muted,
                Align::Center,
            );
        }
        let head = format!(
            "{} covers {}   (red: {}, blue: {})",
            pair.masker, pair.masked, pair.masker, pair.masked
        );
        sp::label(
            c,
            &fit_label(&head, chart.width() - 12.0, 9.5),
            chart.x0 + 6.0,
            chart.y0 + 12.0,
            9.5,
            t.text,
            Align::Start,
        );
        let y = chart.y1 + AXIS_H + RANGE_H - 6.0;
        let ranges = pair.ranges.len();
        let label = if ranges == 0 {
            "No time range".to_owned()
        } else {
            format!(
                "Where the worst band ({}) clashes{}:",
                band_label(pair.worst),
                if ranges > RANGE_SLOTS {
                    format!(" (first {RANGE_SLOTS} of {ranges})")
                } else {
                    String::new()
                }
            )
        };
        sp::label(c, &label, chart.x0, y, 9.5, t.text_muted, Align::Start);
        for (i, (from, to)) in pair.ranges.iter().take(RANGE_SLOTS).enumerate() {
            let r = self.range_rect(i);
            let hovered = self.hover.is_some_and(|h| r.contains(h));
            sp::button(
                c,
                st,
                r,
                &format!("{} - {}", clock(*from), clock(*to)),
                None,
                hovered,
            );
        }
        let demask = self.demask_rect();
        let band = chosen.map_or_else(String::new, |b| format!(" {:.0} Hz", band_centre(b)));
        sp::button(
            c,
            st,
            demask,
            &format!("DE-MASK{band}"),
            Some(st.loop_on),
            self.hover.is_some_and(|h| demask.contains(h)),
        );
    }
}
