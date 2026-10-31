//! The Big Meter: a large two-channel meter for one pair of the Audio I/O sockets, floating over
//! the rack (View > Big Meter). It reads the same per-socket peaks the I/O panel's small meters
//! do and adds the ballistics those leave out: VU (300 ms integration), PPM (fast attack, a fall
//! of 24 dB in 2.8 s) and a plain peak meter, a hold marker that lasts 5 s or until reset, and
//! latching clip lamps.
//!
//! Everything here is pure: the ballistics take the elapsed time as an argument (the view
//! passes the frame clock it is ticked with), the layout is a function of the overlay's origin
//! and the painter only reads a [`BigMeter`]. The rack view owns the state, feeds it the peaks
//! and routes the pointer to [`BigMeter::hit`].

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::Theme;
use vello::kurbo::{Point, Rect, RoundedRect};

/// Which ballistics the meter shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterMode {
    /// Average level: the needle reaches 99 % of a steady tone in [`VU_RISE_SECONDS`].
    #[default]
    Vu,
    /// Quasi-peak: rises at once, falls [`PPM_FALL_DB`] in [`PPM_FALL_SECONDS`].
    Ppm,
    /// Sample peak: rises at once, falls [`PEAK_FALL_DB_PER_SECOND`].
    Peak,
}

impl MeterMode {
    /// Every mode, in button order.
    pub const ALL: [MeterMode; 3] = [MeterMode::Vu, MeterMode::Ppm, MeterMode::Peak];

    /// The button label.
    pub fn label(self) -> &'static str {
        match self {
            MeterMode::Vu => "VU",
            MeterMode::Ppm => "PPM",
            MeterMode::Peak => "PEAK",
        }
    }
}

/// How long the hold marker stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoldMode {
    /// No marker.
    Off,
    /// The marker stays [`HOLD_SECONDS`] after its last rise, then drops to the level.
    #[default]
    Timed,
    /// The marker stays until it is reset.
    Infinite,
}

impl HoldMode {
    /// Every mode, in button order.
    pub const ALL: [HoldMode; 3] = [HoldMode::Off, HoldMode::Timed, HoldMode::Infinite];

    /// The button label.
    pub fn label(self) -> &'static str {
        match self {
            HoldMode::Off => "Off",
            HoldMode::Timed => "5 s",
            HoldMode::Infinite => "Inf",
        }
    }
}

/// Time a VU needle takes to reach 99 % of a steady level.
pub const VU_RISE_SECONDS: f64 = 0.3;
/// The PPM fall: this many dB ...
pub const PPM_FALL_DB: f64 = 24.0;
/// ... in this many seconds.
pub const PPM_FALL_SECONDS: f64 = 2.8;
/// Fall of the plain peak meter, dB per second.
pub const PEAK_FALL_DB_PER_SECOND: f64 = 20.0;
/// The timed hold marker's life, seconds.
pub const HOLD_SECONDS: f64 = 5.0;
/// Bottom of the scale and the level of silence, dBFS.
pub const FLOOR_DB: f32 = -60.0;
/// What 0 VU reads in dBFS until the user changes it.
pub const DEFAULT_VU_REFERENCE_DB: f32 = -18.0;
/// The reference can be set from this ...
pub const MIN_VU_REFERENCE_DB: f32 = -24.0;
/// ... to this dBFS.
pub const MAX_VU_REFERENCE_DB: f32 = -9.0;
/// Quantisation of a displayed level, dB (a quiet meter stops repainting).
const SHOWN_STEP_DB: f32 = 0.25;

fn db_of(linear: f32) -> f32 {
    if linear.is_finite() && linear > 0.0 {
        (20.0 * linear.log10()).max(FLOOR_DB)
    } else {
        FLOOR_DB
    }
}

fn linear_of(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// The ballistics of one channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
    /// The level shown, dBFS.
    pub db: f32,
    /// The hold marker, dBFS (at the level when the hold is off).
    pub hold_db: f32,
    /// Seconds since the hold marker last rose.
    hold_age: f64,
    /// A peak above full scale was seen since the last reset.
    pub clip: bool,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            db: FLOOR_DB,
            hold_db: FLOOR_DB,
            hold_age: 0.0,
            clip: false,
        }
    }
}

impl Channel {
    /// Folds in `peak` (the largest linear sample since the last call) after `dt` seconds.
    pub fn advance(&mut self, mode: MeterMode, hold: HoldMode, peak: f32, dt: f64) {
        let dt = dt.max(0.0);
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        let peak_db = db_of(peak);
        match mode {
            MeterMode::Vu => {
                // A first-order response whose step reaches 99 % in VU_RISE_SECONDS.
                let tau = VU_RISE_SECONDS / 100.0_f64.ln();
                let k = (1.0 - (-dt / tau).exp()) as f32;
                let level = linear_of(self.db);
                self.db = db_of(level + (peak - level) * k);
            }
            MeterMode::Ppm => self.fall(peak_db, PPM_FALL_DB / PPM_FALL_SECONDS * dt),
            MeterMode::Peak => self.fall(peak_db, PEAK_FALL_DB_PER_SECOND * dt),
        }
        if peak > 1.0 {
            self.clip = true;
        }
        match hold {
            HoldMode::Off => {
                self.hold_db = self.db;
                self.hold_age = 0.0;
            }
            HoldMode::Timed | HoldMode::Infinite => {
                if self.db >= self.hold_db {
                    self.hold_db = self.db;
                    self.hold_age = 0.0;
                } else {
                    self.hold_age += dt;
                    if hold == HoldMode::Timed && self.hold_age > HOLD_SECONDS {
                        self.hold_db = self.db;
                        self.hold_age = 0.0;
                    }
                }
            }
        }
    }

    /// Instant rise, a fall of at most `fall_db`.
    fn fall(&mut self, peak_db: f32, fall_db: f64) {
        self.db = if peak_db >= self.db {
            peak_db
        } else {
            (self.db - fall_db as f32).max(peak_db)
        };
    }

    /// Clears the hold marker (it drops to the level).
    pub fn reset_hold(&mut self) {
        self.hold_db = self.db;
        self.hold_age = 0.0;
    }

    /// The values the painter shows, quantised: a repaint is only needed when they change.
    fn shown(&self) -> (i32, i32, bool) {
        let q = |v: f32| (v / SHOWN_STEP_DB).round() as i32;
        (q(self.db), q(self.hold_db), self.clip)
    }
}

/// One pair of Audio I/O sockets the meter can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    /// `In 1-2`, `Out 3-4`, `Out 5` for an odd last channel.
    pub label: String,
    /// The socket indices (inputs first, as the I/O panel numbers them) of the left and right
    /// channel; the right one is missing for an odd last channel.
    pub sockets: [Option<usize>; 2],
}

/// The pairs of an interface with `inputs` host inputs and `outputs` host outputs: the inputs
/// two by two, then the outputs.
pub fn pairs(inputs: usize, outputs: usize) -> Vec<Pair> {
    let mut out = Vec::new();
    for (name, count, base) in [("In", inputs, 0), ("Out", outputs, inputs)] {
        let mut k = 0;
        while k < count {
            let right = (k + 1 < count).then_some(base + k + 1);
            let label = if right.is_some() {
                format!("{name} {}-{}", k + 1, k + 2)
            } else {
                format!("{name} {}", k + 1)
            };
            out.push(Pair {
                label,
                sockets: [Some(base + k), right],
            });
            k += 2;
        }
    }
    out
}

/// The sockets of pair number `index` of [`pairs`], without building the list (the per-frame feed
/// calls this).
pub fn pair_sockets(inputs: usize, outputs: usize, index: usize) -> Option<[Option<usize>; 2]> {
    let in_pairs = inputs.div_ceil(2);
    let (count, base, k) = if index < in_pairs {
        (inputs, 0, index * 2)
    } else {
        (outputs, inputs, (index - in_pairs) * 2)
    };
    (k < count).then(|| [Some(base + k), (k + 1 < count).then_some(base + k + 1)])
}

/// What a press on the overlay hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BigHit {
    /// The previous pair.
    Prev,
    /// The next pair.
    Next,
    /// A mode button.
    Mode(MeterMode),
    /// A hold button.
    Hold(HoldMode),
    /// Lower the VU reference by 1 dB.
    ReferenceDown,
    /// Raise the VU reference by 1 dB.
    ReferenceUp,
    /// A clip lamp (0 left, 1 right).
    Clip(usize),
    /// A bar: resets the hold marker of that channel.
    Bar(usize),
    /// The panel, nothing in particular.
    Body,
}

/// Width of the overlay.
pub const WIDTH: f64 = 240.0;
/// Height of the overlay.
pub const HEIGHT: f64 = 98.0;
const PAD: f64 = 8.0;
const BAR_X: (f64, f64) = (22.0, WIDTH - 24.0);
const BAR_Y: [(f64, f64); 2] = [(30.0, 42.0), (46.0, 58.0)];
const CHIP_H: (f64, f64) = (6.0, 22.0);

/// The rectangles of the overlay whose top left is at `origin`.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    origin: Point,
}

impl Layout {
    /// The layout at `origin`.
    pub fn at(origin: Point) -> Self {
        Self { origin }
    }

    fn rect(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(
            self.origin.x + x0,
            self.origin.y + y0,
            self.origin.x + x1,
            self.origin.y + y1,
        )
    }

    /// The whole overlay.
    pub fn panel(&self) -> Rect {
        self.rect(0.0, 0.0, WIDTH, HEIGHT)
    }

    /// The previous-pair button.
    pub fn prev(&self) -> Rect {
        self.rect(PAD, CHIP_H.0, PAD + 16.0, CHIP_H.1)
    }

    /// The pair's name between the pair buttons.
    pub fn label(&self) -> Rect {
        self.rect(PAD + 18.0, CHIP_H.0, 100.0, CHIP_H.1)
    }

    /// The next-pair button.
    pub fn next(&self) -> Rect {
        self.rect(102.0, CHIP_H.0, 118.0, CHIP_H.1)
    }

    /// The button of `mode`.
    pub fn mode(&self, mode: MeterMode) -> Rect {
        let i = MeterMode::ALL.iter().position(|m| *m == mode).unwrap_or(0) as f64;
        let x = 124.0 + i * 38.0;
        self.rect(x, CHIP_H.0, x + 36.0, CHIP_H.1)
    }

    /// The bar of channel `ch` (0 left, 1 right).
    pub fn bar(&self, ch: usize) -> Rect {
        let (y0, y1) = BAR_Y[ch.min(1)];
        self.rect(BAR_X.0, y0, BAR_X.1, y1)
    }

    /// The clip lamp of channel `ch`.
    pub fn clip(&self, ch: usize) -> Rect {
        let (y0, y1) = BAR_Y[ch.min(1)];
        self.rect(WIDTH - 20.0, y0, WIDTH - PAD, y1)
    }

    /// The button of `hold`.
    pub fn hold(&self, hold: HoldMode) -> Rect {
        let i = HoldMode::ALL.iter().position(|h| *h == hold).unwrap_or(0) as f64;
        let x = PAD + i * 33.0;
        self.rect(x, 76.0, x + 31.0, 92.0)
    }

    /// The VU reference's lower button.
    pub fn reference_down(&self) -> Rect {
        self.rect(112.0, 76.0, 128.0, 92.0)
    }

    /// The VU reference's text.
    pub fn reference_text(&self) -> Rect {
        self.rect(130.0, 76.0, 220.0, 92.0)
    }

    /// The VU reference's raise button.
    pub fn reference_up(&self) -> Rect {
        self.rect(WIDTH - PAD - 16.0, 76.0, WIDTH - PAD, 92.0)
    }

    /// Where the scale labels sit (baseline y).
    fn scale_y(&self) -> f64 {
        self.origin.y + 70.0
    }
}

/// The state of the Big Meter.
#[derive(Debug, Clone)]
pub struct BigMeter {
    /// The overlay shows.
    pub visible: bool,
    /// Index into [`pairs`].
    pub pair: usize,
    /// The ballistics.
    pub mode: MeterMode,
    /// The hold marker.
    pub hold: HoldMode,
    /// What 0 VU reads, dBFS.
    pub vu_reference_db: f32,
    /// The two channels.
    pub channels: [Channel; 2],
    /// The largest peak per channel since the last [`Self::advance`].
    raw: [f32; 2],
    /// The clock of the last advance.
    last: Option<f64>,
    /// What the last paint showed, to repaint only on a change.
    painted: [(i32, i32, bool); 2],
}

impl Default for BigMeter {
    fn default() -> Self {
        Self {
            visible: false,
            pair: 0,
            mode: MeterMode::Vu,
            hold: HoldMode::Timed,
            vu_reference_db: DEFAULT_VU_REFERENCE_DB,
            channels: [Channel::default(); 2],
            raw: [0.0; 2],
            last: None,
            painted: [(0, 0, false); 2],
        }
    }
}

impl BigMeter {
    /// Keeps the largest of the peaks the I/O feed delivers for the shown pair (`peaks` is
    /// indexed by socket).
    pub fn feed(&mut self, peaks: &[f32], sockets: [Option<usize>; 2]) {
        for (raw, socket) in self.raw.iter_mut().zip(sockets) {
            if let Some(p) = socket.and_then(|s| peaks.get(s)).copied()
                && p.is_finite()
            {
                *raw = raw.max(p);
            }
        }
    }

    /// Forgets the clock (the overlay was hidden): the next [`Self::advance`] only restarts it.
    pub fn pause(&mut self) {
        self.last = None;
        self.raw = [0.0; 2];
    }

    /// Moves the ballistics on to `now` (seconds, any monotonic clock) with what was fed since
    /// the last call. The first call only starts the clock; a long gap counts as 250 ms.
    /// Returns whether something shown changed.
    pub fn advance(&mut self, now: f64) -> bool {
        let Some(last) = self.last.replace(now) else {
            return false;
        };
        let dt = (now - last).clamp(0.0, 0.25);
        let raw = std::mem::take(&mut self.raw);
        for (ch, peak) in self.channels.iter_mut().zip(raw) {
            ch.advance(self.mode, self.hold, peak, dt);
        }
        let shown = [self.channels[0].shown(), self.channels[1].shown()];
        let changed = shown != self.painted;
        self.painted = shown;
        changed
    }

    /// Starts again on another pair: the levels, markers and clip lamps clear.
    pub fn select(&mut self, pair: usize) {
        self.pair = pair;
        self.channels = [Channel::default(); 2];
        self.raw = [0.0; 2];
        self.painted = [(0, 0, false); 2];
    }

    /// Clears the clip lamp of `ch`, or of both with `None`.
    pub fn reset_clip(&mut self, ch: Option<usize>) {
        for (i, c) in self.channels.iter_mut().enumerate() {
            if ch.is_none_or(|k| k == i) {
                c.clip = false;
            }
        }
    }

    /// Clears the hold marker of `ch`, or of both with `None`.
    pub fn reset_hold(&mut self, ch: Option<usize>) {
        for (i, c) in self.channels.iter_mut().enumerate() {
            if ch.is_none_or(|k| k == i) {
                c.reset_hold();
            }
        }
    }

    /// Changes the VU reference by `delta_db`, within its limits.
    pub fn nudge_reference(&mut self, delta_db: f32) {
        self.vu_reference_db =
            (self.vu_reference_db + delta_db).clamp(MIN_VU_REFERENCE_DB, MAX_VU_REFERENCE_DB);
    }

    /// The scale: `(dB shown, label)` ticks and the dB at the left and right end of a bar.
    pub fn scale(&self) -> (&'static [(f32, &'static str)], f32, f32) {
        match self.mode {
            MeterMode::Vu => (
                &[
                    (-20.0, "-20"),
                    (-10.0, "-10"),
                    (-5.0, "-5"),
                    (0.0, "0"),
                    (3.0, "+3"),
                ],
                -20.0,
                3.0,
            ),
            _ => (
                &[
                    (-60.0, "-60"),
                    (-40.0, "-40"),
                    (-20.0, "-20"),
                    (-10.0, "-10"),
                    (0.0, "0"),
                ],
                FLOOR_DB,
                0.0,
            ),
        }
    }

    /// Where a level in dBFS lands on a bar, 0..1.
    pub fn norm(&self, db: f32) -> f64 {
        let (_, lo, hi) = self.scale();
        let shown = if self.mode == MeterMode::Vu {
            db - self.vu_reference_db
        } else {
            db
        };
        f64::from(((shown - lo) / (hi - lo)).clamp(0.0, 1.0))
    }

    /// What a press at `pos` hits, for the overlay at `origin`; `None` outside it.
    pub fn hit(&self, origin: Point, pos: Point) -> Option<BigHit> {
        let l = Layout::at(origin);
        if !l.panel().contains(pos) {
            return None;
        }
        let hit = if l.prev().contains(pos) {
            BigHit::Prev
        } else if l.next().contains(pos) {
            BigHit::Next
        } else if let Some(m) = MeterMode::ALL.iter().find(|m| l.mode(**m).contains(pos)) {
            BigHit::Mode(*m)
        } else if let Some(h) = HoldMode::ALL.iter().find(|h| l.hold(**h).contains(pos)) {
            BigHit::Hold(*h)
        } else if self.mode == MeterMode::Vu && l.reference_down().contains(pos) {
            BigHit::ReferenceDown
        } else if self.mode == MeterMode::Vu && l.reference_up().contains(pos) {
            BigHit::ReferenceUp
        } else if let Some(ch) = (0..2).find(|c| l.clip(*c).contains(pos)) {
            BigHit::Clip(ch)
        } else if let Some(ch) = (0..2).find(|c| l.bar(*c).contains(pos)) {
            BigHit::Bar(ch)
        } else {
            BigHit::Body
        };
        Some(hit)
    }
}

/// Paints the overlay at `origin` for the pair labelled `label` (`None` when the interface has
/// no channels).
pub fn paint<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    meter: &BigMeter,
    origin: Point,
    label: Option<&str>,
) {
    let l = Layout::at(origin);
    let panel = RoundedRect::from_rect(l.panel(), 6.0);
    c.fill_with(panel, theme.plate);
    c.stroke_with(panel, 1.0, theme.section_line);
    let chip = |c: &mut C, r: Rect, text: &str, on: bool| {
        let shape = RoundedRect::from_rect(r, 4.0);
        if on {
            c.fill_with(shape, theme.selection);
        }
        c.stroke_with(shape, 1.0, theme.section_line);
        c.text(
            &TextRun::new(text, pt(r.center().x, r.y0 + 12.0), 10.0, theme.text)
                .align(Align::Center),
        );
    };
    chip(c, l.prev(), "<", false);
    chip(c, l.next(), ">", false);
    c.text(
        &TextRun::new(
            label.unwrap_or("No channels"),
            pt(l.label().center().x, l.label().y0 + 12.0),
            11.0,
            theme.text,
        )
        .align(Align::Center)
        .weight(600),
    );
    for m in MeterMode::ALL {
        chip(c, l.mode(m), m.label(), m == meter.mode);
    }
    let (ticks, ..) = meter.scale();
    for ch in 0..2 {
        let bar = l.bar(ch);
        let st = &meter.channels[ch];
        c.text(&TextRun::new(
            if ch == 0 { "L" } else { "R" },
            pt(PAD + origin.x, bar.y1 - 2.0),
            10.0,
            theme.text_muted,
        ));
        c.fill_with(RoundedRect::from_rect(bar, 2.0), theme.lcd);
        let fill = meter.norm(st.db);
        if fill > 0.0 {
            let hot = if meter.mode == MeterMode::Vu {
                st.db > meter.vu_reference_db
            } else {
                st.db > -6.0
            };
            c.fill_with(
                Rect::new(bar.x0, bar.y0, bar.x0 + bar.width() * fill, bar.y1),
                if hot {
                    theme.warning
                } else {
                    theme.io_connected
                },
            );
        }
        if meter.hold != HoldMode::Off && st.hold_db > FLOOR_DB {
            let x = bar.x0 + bar.width() * meter.norm(st.hold_db);
            c.fill_with(
                Rect::new((x - 1.0).max(bar.x0), bar.y0, x + 1.0, bar.y1),
                theme.text,
            );
        }
        for (db, _) in ticks {
            let x = bar.x0
                + bar.width()
                    * meter.norm(match meter.mode {
                        MeterMode::Vu => db + meter.vu_reference_db,
                        _ => *db,
                    });
            c.fill_with(
                Rect::new(x - 0.25, bar.y1 - 3.0, x + 0.25, bar.y1),
                theme.text_muted,
            );
        }
        c.fill_with(
            RoundedRect::from_rect(l.clip(ch), 2.0),
            if st.clip { theme.clip } else { theme.led_off },
        );
    }
    let bar = l.bar(0);
    for (db, text) in ticks {
        let x = bar.x0
            + bar.width()
                * meter.norm(match meter.mode {
                    MeterMode::Vu => db + meter.vu_reference_db,
                    _ => *db,
                });
        c.text(
            &TextRun::new(*text, pt(x, l.scale_y()), 8.0, theme.text_muted).align(Align::Center),
        );
    }
    for h in HoldMode::ALL {
        chip(c, l.hold(h), h.label(), h == meter.hold);
    }
    if meter.mode == MeterMode::Vu {
        chip(c, l.reference_down(), "-", false);
        chip(c, l.reference_up(), "+", false);
        c.text(
            &TextRun::new(
                format!("0 VU = {:.0} dBFS", meter.vu_reference_db),
                pt(l.reference_text().center().x, l.reference_text().y0 + 12.0),
                10.0,
                theme.text_muted,
            )
            .align(Align::Center),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 60.0;

    fn run(ch: &mut Channel, mode: MeterMode, hold: HoldMode, peak: f32, seconds: f64) {
        let steps = (seconds / 0.005).round() as usize;
        for _ in 0..steps {
            ch.advance(mode, hold, peak, 0.005);
        }
    }

    #[test]
    fn a_vu_needle_reaches_99_percent_of_a_zero_vu_tone_in_300_ms() {
        let tone = linear_of(DEFAULT_VU_REFERENCE_DB);
        let mut ch = Channel::default();
        run(&mut ch, MeterMode::Vu, HoldMode::Off, tone, 0.25);
        assert!(linear_of(ch.db) < 0.99 * tone, "not yet at 250 ms");
        let mut ch = Channel::default();
        run(&mut ch, MeterMode::Vu, HoldMode::Off, tone, 0.3);
        let reached = linear_of(ch.db) / tone;
        assert!(reached >= 0.989, "{reached}");
        assert!(reached <= 1.0001, "{reached}");
        // The step size does not matter: 60 Hz frames reach the same figure.
        let mut coarse = Channel::default();
        for _ in 0..18 {
            coarse.advance(MeterMode::Vu, HoldMode::Off, tone, FRAME);
        }
        assert!((coarse.db - ch.db).abs() < 0.2, "{} {}", coarse.db, ch.db);
    }

    #[test]
    fn a_vu_needle_falls_back_with_the_same_time() {
        let tone = linear_of(-18.0);
        let mut ch = Channel::default();
        run(&mut ch, MeterMode::Vu, HoldMode::Off, tone, 1.0);
        run(&mut ch, MeterMode::Vu, HoldMode::Off, 0.0, 0.3);
        assert!(linear_of(ch.db) <= 0.011 * tone, "{}", ch.db);
    }

    #[test]
    fn a_ppm_rises_at_once_and_falls_24_db_in_2_8_seconds() {
        let mut ch = Channel::default();
        ch.advance(MeterMode::Ppm, HoldMode::Off, 1.0, FRAME);
        assert_eq!(ch.db, 0.0);
        run(&mut ch, MeterMode::Ppm, HoldMode::Off, 0.0, 2.8);
        assert!((ch.db + 24.0).abs() < 0.05, "{}", ch.db);
        // It never falls below what is coming in.
        let mut ch = Channel::default();
        ch.advance(MeterMode::Ppm, HoldMode::Off, 1.0, FRAME);
        run(
            &mut ch,
            MeterMode::Ppm,
            HoldMode::Off,
            linear_of(-10.0),
            5.0,
        );
        assert!((ch.db + 10.0).abs() < 0.01, "{}", ch.db);
    }

    #[test]
    fn a_peak_meter_falls_faster_than_a_ppm() {
        let mut ch = Channel::default();
        ch.advance(MeterMode::Peak, HoldMode::Off, 1.0, FRAME);
        run(&mut ch, MeterMode::Peak, HoldMode::Off, 0.0, 1.0);
        assert!(
            (ch.db + PEAK_FALL_DB_PER_SECOND as f32).abs() < 0.05,
            "{}",
            ch.db
        );
    }

    #[test]
    fn the_hold_marker_stays_five_seconds_or_until_reset() {
        let mut ch = Channel::default();
        ch.advance(MeterMode::Peak, HoldMode::Timed, linear_of(-6.0), FRAME);
        let top = ch.hold_db;
        assert!((top + 6.0).abs() < 0.01);
        run(&mut ch, MeterMode::Peak, HoldMode::Timed, 0.0, 4.5);
        assert_eq!(ch.hold_db, top, "still held at 4.5 s");
        run(&mut ch, MeterMode::Peak, HoldMode::Timed, 0.0, 1.0);
        assert_eq!(ch.hold_db, ch.db, "dropped to the level after 5 s");
        // Infinite never drops; a reset does.
        let mut ch = Channel::default();
        ch.advance(MeterMode::Peak, HoldMode::Infinite, linear_of(-6.0), FRAME);
        run(&mut ch, MeterMode::Peak, HoldMode::Infinite, 0.0, 30.0);
        assert!((ch.hold_db + 6.0).abs() < 0.01);
        ch.reset_hold();
        assert_eq!(ch.hold_db, ch.db);
        // Off follows the level.
        let mut ch = Channel::default();
        ch.advance(MeterMode::Peak, HoldMode::Off, 0.5, FRAME);
        assert_eq!(ch.hold_db, ch.db);
    }

    #[test]
    fn a_hold_marker_that_is_passed_rises_with_the_level() {
        let mut ch = Channel::default();
        ch.advance(MeterMode::Peak, HoldMode::Timed, 0.25, FRAME);
        run(&mut ch, MeterMode::Peak, HoldMode::Timed, 0.0, 3.0);
        ch.advance(MeterMode::Peak, HoldMode::Timed, 0.5, FRAME);
        assert!((ch.hold_db + 6.0).abs() < 0.1);
        run(&mut ch, MeterMode::Peak, HoldMode::Timed, 0.0, 4.0);
        assert!(
            (ch.hold_db + 6.0).abs() < 0.1,
            "the age restarted at the rise"
        );
    }

    #[test]
    fn a_clip_latches_until_reset_and_full_scale_is_not_one() {
        let mut ch = Channel::default();
        ch.advance(MeterMode::Peak, HoldMode::Off, 1.0, FRAME);
        assert!(!ch.clip);
        ch.advance(MeterMode::Peak, HoldMode::Off, 1.2, FRAME);
        assert!(ch.clip);
        run(&mut ch, MeterMode::Peak, HoldMode::Off, 0.0, 5.0);
        assert!(ch.clip, "latched");
        let mut m = BigMeter::default();
        m.channels[0].clip = true;
        m.channels[1].clip = true;
        m.reset_clip(Some(0));
        assert!(!m.channels[0].clip && m.channels[1].clip);
        m.reset_clip(None);
        assert!(!m.channels[1].clip);
    }

    #[test]
    fn junk_peaks_read_as_silence() {
        let mut ch = Channel::default();
        for junk in [f32::NAN, f32::INFINITY, -1.0] {
            ch.advance(MeterMode::Ppm, HoldMode::Off, junk, FRAME);
            assert_eq!(ch.db, FLOOR_DB);
            assert!(!ch.clip);
        }
    }

    #[test]
    fn pairs_list_the_inputs_then_the_outputs() {
        let p = pairs(3, 4);
        let labels: Vec<_> = p.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["In 1-2", "In 3", "Out 1-2", "Out 3-4"]);
        assert_eq!(p[0].sockets, [Some(0), Some(1)]);
        assert_eq!(p[1].sockets, [Some(2), None]);
        assert_eq!(
            p[2].sockets,
            [Some(3), Some(4)],
            "outputs follow the inputs"
        );
        assert_eq!(p[3].sockets, [Some(5), Some(6)]);
        assert!(pairs(0, 0).is_empty());
        assert_eq!(pairs(0, 2)[0].sockets, [Some(0), Some(1)]);
        assert_eq!(pairs(32, 32).len(), 32);
    }

    #[test]
    fn the_feed_keeps_the_largest_peak_and_advance_spends_it() {
        let pair = pairs(2, 2)[0].sockets;
        let mut m = BigMeter {
            mode: MeterMode::Ppm,
            hold: HoldMode::Off,
            ..BigMeter::default()
        };
        assert!(!m.advance(0.0), "the first call starts the clock");
        m.feed(&[0.25, 0.5, 0.0, 0.0], pair);
        m.feed(&[0.125, 0.75, 0.0, 0.0], pair);
        assert!(m.advance(FRAME));
        assert!((m.channels[0].db - db_of(0.25)).abs() < 0.01);
        assert!((m.channels[1].db - db_of(0.75)).abs() < 0.01);
        // Nothing fed: the levels fall and the next advance reports the change, then settles.
        let mut t = FRAME;
        let mut changes = 0;
        for _ in 0..400 {
            t += FRAME;
            if m.advance(t) {
                changes += 1;
            }
        }
        assert!(changes > 0);
        assert!(!m.advance(t + FRAME), "a settled meter does not repaint");
        // A long gap counts as a quarter second.
        m.feed(&[1.0, 1.0, 0.0, 0.0], pair);
        m.advance(t + 100.0);
        assert_eq!(m.channels[0].db, 0.0);
    }

    #[test]
    fn selecting_a_pair_starts_clean() {
        let mut m = BigMeter::default();
        m.channels[0].db = -3.0;
        m.channels[0].clip = true;
        m.select(2);
        assert_eq!(m.pair, 2);
        assert_eq!(m.channels[0], Channel::default());
    }

    #[test]
    fn the_scale_follows_the_mode_and_the_reference() {
        let mut m = BigMeter::default();
        // 0 VU sits at the reference: 20/23 along the bar.
        assert!((m.norm(-18.0) - 20.0 / 23.0).abs() < 1e-6);
        m.nudge_reference(3.0);
        assert!((m.norm(-15.0) - 20.0 / 23.0).abs() < 1e-6);
        m.nudge_reference(100.0);
        assert_eq!(m.vu_reference_db, MAX_VU_REFERENCE_DB);
        m.nudge_reference(-100.0);
        assert_eq!(m.vu_reference_db, MIN_VU_REFERENCE_DB);
        m.mode = MeterMode::Ppm;
        assert_eq!(m.norm(0.0), 1.0);
        assert_eq!(m.norm(-60.0), 0.0);
        assert!((m.norm(-30.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn presses_land_on_the_right_control() {
        let m = BigMeter::default();
        let o = Point::new(100.0, 50.0);
        let l = Layout::at(o);
        assert_eq!(m.hit(o, Point::new(0.0, 0.0)), None);
        assert_eq!(m.hit(o, l.prev().center()), Some(BigHit::Prev));
        assert_eq!(m.hit(o, l.next().center()), Some(BigHit::Next));
        for mode in MeterMode::ALL {
            assert_eq!(m.hit(o, l.mode(mode).center()), Some(BigHit::Mode(mode)));
        }
        for hold in HoldMode::ALL {
            assert_eq!(m.hit(o, l.hold(hold).center()), Some(BigHit::Hold(hold)));
        }
        assert_eq!(
            m.hit(o, l.reference_down().center()),
            Some(BigHit::ReferenceDown)
        );
        assert_eq!(
            m.hit(o, l.reference_up().center()),
            Some(BigHit::ReferenceUp)
        );
        assert_eq!(m.hit(o, l.clip(1).center()), Some(BigHit::Clip(1)));
        assert_eq!(m.hit(o, l.bar(0).center()), Some(BigHit::Bar(0)));
        assert_eq!(
            m.hit(o, Point::new(o.x + 2.0, o.y + 2.0)),
            Some(BigHit::Body)
        );
        // The reference buttons exist only while the VU scale shows.
        let ppm = BigMeter {
            mode: MeterMode::Ppm,
            ..BigMeter::default()
        };
        assert_eq!(ppm.hit(o, l.reference_up().center()), Some(BigHit::Body));
    }

    #[test]
    fn the_controls_do_not_overlap_and_stay_inside() {
        let l = Layout::at(Point::ZERO);
        let mut rects = vec![l.prev(), l.next(), l.reference_down(), l.reference_up()];
        rects.extend(MeterMode::ALL.map(|m| l.mode(m)));
        rects.extend(HoldMode::ALL.map(|h| l.hold(h)));
        rects.extend((0..2).flat_map(|c| [l.bar(c), l.clip(c)]));
        for (i, a) in rects.iter().enumerate() {
            assert!(l.panel().contains_rect(*a), "{a:?}");
            for b in &rects[i + 1..] {
                let inter = a.intersect(*b);
                assert!(inter.width() <= 0.0 || inter.height() <= 0.0, "{a:?} {b:?}");
            }
        }
    }
}

#[cfg(test)]
mod pair_tests {
    use super::*;

    #[test]
    fn pair_sockets_agree_with_the_list() {
        for (i, o) in [(0, 0), (1, 0), (2, 2), (3, 4), (5, 1), (32, 32)] {
            let list = pairs(i, o);
            for (k, p) in list.iter().enumerate() {
                assert_eq!(pair_sockets(i, o, k), Some(p.sockets), "{i} {o} {k}");
            }
            assert_eq!(pair_sockets(i, o, list.len()), None);
        }
    }
}
