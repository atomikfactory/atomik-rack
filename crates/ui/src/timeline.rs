//! Pure time-axis helpers for the sequencer views: tick ↔ x
//! mapping with zoom and scroll, meter maths, snap, bar/beat labels, ruler grid lines and the
//! bars.beats.ticks / mm:ss.mmm readouts.
//!
//! The song's meter changes along the timeline: [`Bars`] holds the meter map (the maths is
//! `atomik_core::MeterMap`) and every function that needs bar lines, beat lines or a snap grid
//! takes it. A [`Meter`] is one time signature (the piano roll shows the meter in effect at
//! its clip).
//!
//! Everything here is deterministic integer or `f64` arithmetic with no time source.

use crate::event::Modifiers;
use atomik_core::{MeterMap, MeterSegment, MeterTable, Tempo, TempoPoint, TempoTable};
use atomik_model::{MeterEvent, Song, TempoEvent};
use std::sync::Arc;

/// Ticks per quarter note (re-exported from `atomik_core`).
pub const PPQ: i64 = atomik_core::PPQ;

/// A time signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meter {
    /// Beats per bar (numerator).
    pub beats_per_bar: u8,
    /// Beat unit (denominator: 2, 4, 8, 16).
    pub beat_unit: u8,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            beats_per_bar: 4,
            beat_unit: 4,
        }
    }
}

impl Meter {
    /// The song's meter at bar 1.
    pub fn of(song: &Song) -> Self {
        let (beats_per_bar, beat_unit) = song.meter();
        Self {
            beats_per_bar,
            beat_unit,
        }
    }

    /// The song's meter in effect at `tick`.
    pub fn at(song: &Song, tick: i64) -> Self {
        Bars::of(song).meter_at(tick)
    }

    /// Ticks per beat (`PPQ · 4 / beat_unit`).
    pub fn beat_ticks(self) -> i64 {
        PPQ * 4 / i64::from(self.beat_unit.max(1))
    }

    /// Ticks per bar.
    pub fn bar_ticks(self) -> i64 {
        self.beat_ticks() * i64::from(self.beats_per_bar.max(1))
    }

    /// `"3/4"`.
    pub fn label(self) -> String {
        format!("{}/{}", self.beats_per_bar, self.beat_unit)
    }
}

/// The song's meter map for the time views: bar lines, beat lines and bar numbers along the
/// timeline. Cheap to clone and compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bars {
    segments: Arc<[MeterSegment]>,
}

impl Default for Bars {
    fn default() -> Self {
        Self::single(Meter::default())
    }
}

impl Bars {
    /// The song's meter map (4/4 when the song's map is invalid).
    pub fn of(song: &Song) -> Self {
        Self {
            segments: song.meter_table().segments().into(),
        }
    }

    /// One meter from bar 1.
    pub fn single(meter: Meter) -> Self {
        Self {
            segments: MeterTable::constant(meter.beats_per_bar, meter.beat_unit)
                .segments()
                .into(),
        }
    }

    /// The maths view.
    pub fn map(&self) -> MeterMap<'_> {
        MeterMap(&self.segments)
    }

    /// The segments: one per meter event.
    pub fn segments(&self) -> &[MeterSegment] {
        &self.segments
    }

    /// The signature in effect at `tick` (the first one before 0).
    pub fn meter_at(&self, tick: i64) -> Meter {
        let (beats_per_bar, beat_unit) = self.map().meter_at(tick);
        Meter {
            beats_per_bar,
            beat_unit,
        }
    }

    /// Ticks per bar of the bar holding `tick`.
    pub fn bar_ticks_at(&self, tick: i64) -> i64 {
        self.meter_at(tick).bar_ticks()
    }

    /// Ticks per beat at `tick`.
    pub fn beat_ticks_at(&self, tick: i64) -> i64 {
        self.meter_at(tick).beat_ticks()
    }

    /// The bar holding `tick` (1-based). Before tick 0 the first meter continues backwards, so
    /// the bars count down through 0, -1...
    pub fn bar_of(&self, tick: i64) -> i64 {
        if tick < 0 {
            tick.div_euclid(self.bar_ticks_at(0)) + 1
        } else {
            i64::from(self.map().bar_at(tick))
        }
    }

    /// The tick where `bar` starts (see [`Self::bar_of`]).
    pub fn bar_start(&self, bar: i64) -> i64 {
        if bar < 1 {
            (bar - 1) * self.bar_ticks_at(0)
        } else {
            self.map().bar_start(u32::try_from(bar).unwrap_or(u32::MAX))
        }
    }

    /// `(bar, beat, tick)` of a song tick: bar and beat are 1-based, `tick` is the remainder
    /// inside the beat.
    pub fn bbt(&self, tick: i64) -> (i64, i64, i64) {
        let bar = self.bar_of(tick);
        let in_bar = tick - self.bar_start(bar);
        let beat_ticks = self.beat_ticks_at(tick.max(0));
        (bar, in_bar / beat_ticks + 1, in_bar % beat_ticks)
    }

    /// The first bar line strictly after `tick`.
    pub fn next_bar_line(&self, tick: i64) -> i64 {
        self.bar_start(self.bar_of(tick) + 1)
    }

    /// The last bar line strictly before `tick` (a bar line at or before tick 0 is the first
    /// bar line, tick 0).
    pub fn prev_bar_line(&self, tick: i64) -> i64 {
        let bar = self.bar_of(tick);
        let start = self.bar_start(bar);
        if start < tick {
            start.max(0)
        } else {
            self.bar_start(bar - 1).max(0)
        }
    }

    /// The meter map of `events` (4/4 when it does not compile).
    pub fn from_events(events: &[MeterEvent]) -> Self {
        MeterTable::new(events).map_or_else(
            |_| Self::default(),
            |t| Self {
                segments: t.segments().into(),
            },
        )
    }

    /// The nearest bar line to `tick` (halves round up).
    pub fn nearest_bar_line(&self, tick: i64) -> i64 {
        let start = self.bar_start(self.bar_of(tick));
        let next = self.next_bar_line(tick);
        if tick - start < next - tick {
            start
        } else {
            next
        }
    }
}

/// `"bar.beat.ticks"` with the tick remainder zero-padded to five digits, e.g. `"3.2.07680"`.
pub fn format_bbt(tick: i64, bars: &Bars) -> String {
    let (bar, beat, t) = bars.bbt(tick);
    format!("{bar}.{beat}.{t:05}")
}

/// A ruler label: `"5"` on a bar line, `"5.3"` on a beat line, `"5.3.01920"` elsewhere.
pub fn bar_beat_label(tick: i64, bars: &Bars) -> String {
    let (bar, beat, t) = bars.bbt(tick);
    if beat == 1 && t == 0 {
        format!("{bar}")
    } else if t == 0 {
        format!("{bar}.{beat}")
    } else {
        format_bbt(tick, bars)
    }
}

/// Seconds at a song tick through a tempo map (steps and ramps; events sorted by tick, the
/// first at 0; an empty or invalid map means 120 BPM). Negative ticks give 0.
pub fn tick_to_seconds(tick: i64, tempo: &[TempoEvent]) -> f64 {
    let points: Vec<TempoPoint> = tempo
        .iter()
        .map(|e| TempoPoint {
            tick: e.tick,
            bpm: e.bpm,
            ramp: e.ramp,
        })
        .collect();
    let table = TempoTable::new(&points).unwrap_or_else(|_| {
        TempoTable::constant(
            Tempo::new(120.0).unwrap_or_else(|_| unreachable!("120 BPM is a valid tempo")),
        )
    });
    table.map().seconds(tick.max(0) as f64)
}

/// `"mm:ss.mmm"` (minutes keep growing past 59), e.g. `"01:04.250"`. Negative gives zero.
pub fn format_time(seconds: f64) -> String {
    let ms_total = (seconds.max(0.0) * 1000.0).round() as u64;
    let (min, rest) = (ms_total / 60_000, ms_total % 60_000);
    format!("{min:02}:{:02}.{:03}", rest / 1000, rest % 1000)
}
/// Snap resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Snap {
    /// No snapping (tick resolution).
    Off,
    /// Whole bars.
    Bar,
    /// Beats of the meter.
    Beat,
    /// Eighth notes (`PPQ / 2`).
    Eighth,
    /// Sixteenth notes (`PPQ / 4`).
    #[default]
    Sixteenth,
    /// Thirty-second notes (`PPQ / 8`).
    ThirtySecond,
    /// Sixty-fourth notes (`PPQ / 16`).
    SixtyFourth,
    /// Quarter-note triplets (`PPQ * 2 / 3`: three in the time of two beats).
    QuarterTriplet,
    /// Eighth-note triplets (`PPQ / 3`).
    EighthTriplet,
    /// Sixteenth-note triplets (`PPQ / 6`).
    SixteenthTriplet,
    /// Thirty-second-note triplets (`PPQ / 12`).
    ThirtySecondTriplet,
    /// The visible grid: the step of the finest ruler line the zoom shows
    /// ([`grid_step`], at most one bar), so snapping follows what is drawn.
    Grid,
}

/// Minimum distance in pixels between the lines of the grid the views draw, and so the step of
/// [`Snap::Grid`].
pub const GRID_MIN_PX: f64 = 7.0;

impl Snap {
    /// Every resolution, in selector order.
    pub const ALL: [Snap; 12] = [
        Snap::Bar,
        Snap::Beat,
        Snap::Eighth,
        Snap::Sixteenth,
        Snap::ThirtySecond,
        Snap::SixtyFourth,
        Snap::QuarterTriplet,
        Snap::EighthTriplet,
        Snap::SixteenthTriplet,
        Snap::ThirtySecondTriplet,
        Snap::Grid,
        Snap::Off,
    ];

    /// Grid step in ticks, `None` when off. [`Snap::Grid`] needs the zoom
    /// ([`Self::step_zoomed`]); without it, it is a sixteenth.
    pub fn step(self, meter: Meter) -> Option<i64> {
        match self {
            Snap::Off => None,
            Snap::Bar => Some(meter.bar_ticks()),
            Snap::Beat => Some(meter.beat_ticks()),
            Snap::Eighth => Some(PPQ / 2),
            Snap::Sixteenth | Snap::Grid => Some(PPQ / 4),
            Snap::ThirtySecond => Some(PPQ / 8),
            Snap::SixtyFourth => Some(PPQ / 16),
            Snap::QuarterTriplet => Some(PPQ * 2 / 3),
            Snap::EighthTriplet => Some(PPQ / 3),
            Snap::SixteenthTriplet => Some(PPQ / 6),
            Snap::ThirtySecondTriplet => Some(PPQ / 12),
        }
    }

    /// Like [`Self::step`], with [`Snap::Grid`] resolved to the visible grid at `px_per_tick`:
    /// the step [`grid_lines`] draws with, at most one bar. A zoom that is not a positive
    /// number gives a sixteenth.
    pub fn step_zoomed(self, meter: Meter, px_per_tick: f64) -> Option<i64> {
        match self {
            Snap::Grid if px_per_tick > 0.0 => {
                Some(grid_step(px_per_tick, meter, GRID_MIN_PX).min(meter.bar_ticks()))
            }
            other => other.step(meter),
        }
    }

    /// The step for a gesture: Alt held disables snapping.
    pub fn step_with(self, meter: Meter, modifiers: Modifiers) -> Option<i64> {
        if modifiers.alt {
            None
        } else {
            self.step(meter)
        }
    }

    /// Short label for the selector.
    pub fn label(self) -> &'static str {
        match self {
            Snap::Off => "Off",
            Snap::Bar => "Bar",
            Snap::Beat => "Beat",
            Snap::Eighth => "1/8",
            Snap::Sixteenth => "1/16",
            Snap::ThirtySecond => "1/32",
            Snap::SixtyFourth => "1/64",
            Snap::QuarterTriplet => "1/4T",
            Snap::EighthTriplet => "1/8T",
            Snap::SixteenthTriplet => "1/16T",
            Snap::ThirtySecondTriplet => "1/32T",
            Snap::Grid => "Grid",
        }
    }

    /// The resolution `steps` places along [`Self::ALL`] (positive: later in the selector),
    /// stopping at the ends: what a drag or the wheel on the selector does.
    pub fn offset(self, steps: i32) -> Snap {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[crate::step_drag::step_index(i, steps, Self::ALL.len())]
    }

    /// The next resolution in [`Self::ALL`] (wrapping); `forward = false` goes back.
    pub fn cycle(self, forward: bool) -> Snap {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        let n = Self::ALL.len();
        Self::ALL[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }]
    }
}

/// A snap grid: the resolution over the song's meter map. Bar and beat lines move with meter
/// changes, so the grid restarts at every bar line (a step never crosses one).
#[derive(Debug, Clone, PartialEq)]
pub struct SnapGrid {
    snap: Snap,
    bars: Bars,
    /// Zoom for [`Snap::Grid`] (0 = unknown: a sixteenth).
    px_per_tick: f64,
}

impl SnapGrid {
    /// The grid of `snap` over `bars`.
    pub fn new(snap: Snap, bars: Bars) -> Self {
        Self {
            snap,
            bars,
            px_per_tick: 0.0,
        }
    }

    /// The same grid at a zoom, which [`Snap::Grid`] follows.
    pub fn zoomed(mut self, px_per_tick: f64) -> Self {
        self.px_per_tick = px_per_tick;
        self
    }

    /// No snapping.
    pub fn off() -> Self {
        Self::new(Snap::Off, Bars::default())
    }

    /// The step of the bar holding `tick`, `None` when off.
    pub fn step_at(&self, tick: i64) -> Option<i64> {
        match self.snap {
            Snap::Off => None,
            Snap::Bar => Some(self.bars.bar_ticks_at(tick)),
            Snap::Beat => Some(self.bars.beat_ticks_at(tick)),
            other => other.step_zoomed(self.bars.meter_at(tick), self.px_per_tick),
        }
    }

    /// The grid points around `tick`: `(floor, ceil)` within the bar that holds it (the next
    /// bar line caps the last step). `None` when snapping is off. Before tick 0 the grid is
    /// uniform.
    fn cell(&self, tick: i64) -> Option<(i64, i64)> {
        let step = self.step_at(tick.max(0))?;
        if tick < 0 {
            let lo = tick.div_euclid(step) * step;
            return Some((lo, lo + step));
        }
        let start = self.bars.bar_start(self.bars.bar_of(tick));
        let next = self.bars.next_bar_line(tick);
        let lo = start + (tick - start) / step * step;
        Some((lo, (lo + step).min(next)))
    }
}

/// Rounding to a grid: a uniform step (`Option<i64>`, `None` = off) or a [`SnapGrid`].
pub trait Quant {
    /// The nearest grid point (halves round up).
    fn round_to(&self, tick: i64) -> i64;
    /// The grid point at or before `tick`.
    fn floor_to(&self, tick: i64) -> i64;
    /// The grid point at or after `tick`.
    fn ceil_to(&self, tick: i64) -> i64;
    /// A representative step (the first bar's): the least length of an item, `None` when off.
    fn unit(&self) -> Option<i64>;
}

impl Quant for Option<i64> {
    fn round_to(&self, tick: i64) -> i64 {
        match *self {
            Some(s) if s > 0 => (tick + s / 2).div_euclid(s) * s,
            _ => tick,
        }
    }
    fn floor_to(&self, tick: i64) -> i64 {
        match *self {
            Some(s) if s > 0 => tick.div_euclid(s) * s,
            _ => tick,
        }
    }
    fn ceil_to(&self, tick: i64) -> i64 {
        match *self {
            Some(s) if s > 0 => -(-tick).div_euclid(s) * s,
            _ => tick,
        }
    }
    fn unit(&self) -> Option<i64> {
        *self
    }
}

impl Quant for SnapGrid {
    fn round_to(&self, tick: i64) -> i64 {
        match self.cell(tick) {
            Some((lo, hi)) if tick - lo < hi - tick => lo,
            Some((_, hi)) => hi,
            None => tick,
        }
    }
    fn floor_to(&self, tick: i64) -> i64 {
        self.cell(tick).map_or(tick, |(lo, _)| lo)
    }
    fn ceil_to(&self, tick: i64) -> i64 {
        match self.cell(tick) {
            Some((lo, _)) if lo == tick => tick,
            Some((_, hi)) => hi,
            None => tick,
        }
    }
    fn unit(&self) -> Option<i64> {
        self.step_at(0)
    }
}

impl<T: Quant + ?Sized> Quant for &T {
    fn round_to(&self, tick: i64) -> i64 {
        (**self).round_to(tick)
    }
    fn floor_to(&self, tick: i64) -> i64 {
        (**self).floor_to(tick)
    }
    fn ceil_to(&self, tick: i64) -> i64 {
        (**self).ceil_to(tick)
    }
    fn unit(&self) -> Option<i64> {
        (**self).unit()
    }
}

/// Rounds to the nearest grid point (halves round up); identity when off.
pub fn snap_round(tick: i64, grid: impl Quant) -> i64 {
    grid.round_to(tick)
}

/// Rounds down to a grid point; identity when off.
pub fn snap_floor(tick: i64, grid: impl Quant) -> i64 {
    grid.floor_to(tick)
}

/// Rounds up to a grid point; identity when off.
pub fn snap_ceil(tick: i64, grid: impl Quant) -> i64 {
    grid.ceil_to(tick)
}

/// A horizontal time axis: `x = x0 + (tick − scroll) · px_per_tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeAxis {
    /// Logical x of the axis origin (left edge of the timeline area).
    pub x0: f64,
    /// Zoom: logical pixels per tick.
    pub px_per_tick: f64,
    /// Tick shown at `x0` (horizontal scroll, ≥ 0).
    pub scroll: f64,
}

/// Smallest zoom (pixels per tick): a 4/4 bar is 8 px wide.
pub const MIN_PX_PER_TICK: f64 = 8.0 / (4.0 * PPQ as f64);
/// Largest zoom: 16 px per tick. A tick is 1/15360 of a quarter, which at 120 BPM and 48 kHz is
/// 1.5625 frames, so the deepest zoom shows a sample about 10 px wide (9 px at 44.1 kHz, 5 px at
/// 96 kHz; 2 to 3 px per frame as slow as 40 BPM). A 1/128 note (480 ticks) is 7680 px wide
/// there and spans a 1920 px screen from about 4 px per tick.
pub const MAX_PX_PER_TICK: f64 = 16.0;

impl TimeAxis {
    /// An axis with a 4/4 bar `bar_px` wide, unscrolled.
    pub fn new(x0: f64, bar_px: f64) -> Self {
        Self {
            x0,
            px_per_tick: bar_px / (4 * PPQ) as f64,
            scroll: 0.0,
        }
    }

    /// Logical x of a tick.
    pub fn tick_to_x(&self, tick: i64) -> f64 {
        self.x0 + (tick as f64 - self.scroll) * self.px_per_tick
    }

    /// The tick at a logical x (rounded to the nearest tick).
    pub fn x_to_tick(&self, x: f64) -> i64 {
        (self.scroll + (x - self.x0) / self.px_per_tick).round() as i64
    }

    /// A pixel distance in ticks (rounded).
    pub fn dx_to_ticks(&self, dx: f64) -> i64 {
        (dx / self.px_per_tick).round() as i64
    }

    /// Zooms by `factor` (clamped to the zoom range) keeping the tick under `anchor_x` fixed.
    pub fn zoom_at(&mut self, factor: f64, anchor_x: f64) {
        let t = self.scroll + (anchor_x - self.x0) / self.px_per_tick;
        self.px_per_tick = (self.px_per_tick * factor).clamp(MIN_PX_PER_TICK, MAX_PX_PER_TICK);
        self.scroll = (t - (anchor_x - self.x0) / self.px_per_tick).max(0.0);
    }

    /// Scrolls by a pixel delta (positive = later), never before tick 0.
    pub fn scroll_px(&mut self, dx: f64) {
        self.scroll = (self.scroll + dx / self.px_per_tick).max(0.0);
    }

    /// How many ticks fit between the axis origin and the right edge `x1`.
    pub fn visible_ticks(&self, x1: f64) -> f64 {
        ((x1 - self.x0) / self.px_per_tick).max(0.0)
    }

    /// Pages the view so `tick` is visible (follow): when it is left of the view or past its
    /// right edge, the view scrolls so the tick sits a tenth of the width from the left.
    /// Returns whether it scrolled.
    pub fn page_to(&mut self, tick: i64, x1: f64) -> bool {
        let width = self.visible_ticks(x1);
        let t = tick as f64;
        if t >= self.scroll && t <= self.scroll + width {
            return false;
        }
        self.scroll = (t - width * 0.1).max(0.0);
        true
    }

    /// Zooms and scrolls so `[start, end)` fills the view up to the right edge `x1` (a margin
    /// of 12 px each side, at least a sixteenth of span).
    pub fn fit_span(&mut self, start: i64, end: i64, x1: f64) {
        let width = (x1 - self.x0 - 24.0).max(40.0);
        let span = (end - start).max(PPQ / 4) as f64;
        self.px_per_tick = (width / span).clamp(MIN_PX_PER_TICK, MAX_PX_PER_TICK);
        self.scroll = (start as f64 - 12.0 / self.px_per_tick).max(0.0);
    }
}

/// Importance of a grid line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GridLevel {
    /// Subdivision of a beat (or a bar line between labelled bar groups at far zoom).
    Sub,
    /// Beat line.
    Beat,
    /// Bar line.
    Bar,
}

/// One grid line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridLine {
    /// Song tick.
    pub tick: i64,
    /// Importance.
    pub level: GridLevel,
}

/// Grid steps below a bar, finest first: a tick, then the divisors of a sixteenth down to
/// 1/1024 of a quarter (15 ticks), the 1/128 note (`PPQ / 32` = 480 ticks) and 1/64 up to the
/// eighth. Each step divides the next, so a coarser grid is a subset of a finer one.
const SUBDIVISIONS: [i64; 12] = [
    1,
    5,
    PPQ / 1024,
    PPQ / 512,
    PPQ / 256,
    PPQ / 128,
    PPQ / 64,
    PPQ / 32,
    PPQ / 16,
    PPQ / 8,
    PPQ / 4,
    PPQ / 2,
];

/// Lines finer than a sixteenth are at least this many pixels apart, however small `min_px`
/// is, so a deep zoom shows a calm grid instead of a line every few pixels.
pub const FINE_GRID_PX: f64 = 40.0;

/// The most lines [`grid_lines`] draws for one meter segment: a guard for zooms and ranges
/// that do not belong together; real views stay below a thousand.
const MAX_SEGMENT_LINES: i64 = 20_000;

/// The finest grid step (ticks) whose lines are at least `min_px` apart (lines finer than a
/// sixteenth at least [`FINE_GRID_PX`]): a tick, 5 ticks, then 1/1024 up to 1/128, 1/64, 1/32,
/// 1/16, 1/8 and the beat (the steps that fit in a beat), then the bar, then 2, 4, 8… bars.
/// Never zero.
pub fn grid_step(px_per_tick: f64, meter: Meter, min_px: f64) -> i64 {
    let beat = meter.beat_ticks();
    let fine_px = min_px.max(FINE_GRID_PX);
    let mut candidates: Vec<i64> = SUBDIVISIONS.into_iter().filter(|s| *s < beat).collect();
    candidates.push(beat);
    if let Some(s) = candidates.into_iter().find(|s| {
        let need = if *s < PPQ / 4 { fine_px } else { min_px };
        *s as f64 * px_per_tick >= need
    }) {
        return s;
    }
    let mut bars = 1;
    loop {
        let s = meter.bar_ticks() * bars;
        if s as f64 * px_per_tick >= min_px || bars >= 1 << 20 {
            return s;
        }
        bars *= 2;
    }
}

/// Grid lines covering `[t0, t1]` with lines at least `min_px` apart, each classified as a
/// bar, beat or subdivision line. Every meter segment has its own grid, anchored at its bar
/// line (bar lines at far zoom are the bars `1, 1 + n, ...` of the song).
pub fn grid_lines(t0: i64, t1: i64, px_per_tick: f64, bars: &Bars, min_px: f64) -> Vec<GridLine> {
    let mut out = Vec::new();
    let segs = bars.segments();
    for (i, seg) in segs.iter().enumerate() {
        let end = segs.get(i + 1).map_or(i64::MAX, |n| n.tick);
        let (a, b) = (t0.max(seg.tick).max(0), t1.min(end - 1));
        if a > b {
            continue;
        }
        let meter = Meter {
            beats_per_bar: seg.num,
            beat_unit: seg.den,
        };
        let mut step = grid_step(px_per_tick, meter, min_px);
        while (b - a) / step > MAX_SEGMENT_LINES {
            step *= 2;
        }
        let (bar, beat) = (meter.bar_ticks(), meter.beat_ticks());
        if step >= bar {
            let n = u32::try_from(step / bar).unwrap_or(u32::MAX);
            let mut k = (a - seg.tick + bar - 1) / bar;
            while !(u64::from(seg.bar) + k as u64 - 1).is_multiple_of(u64::from(n)) {
                k += 1;
            }
            while seg.tick + k * bar <= b {
                out.push(GridLine {
                    tick: seg.tick + k * bar,
                    level: GridLevel::Bar,
                });
                k += i64::from(n);
            }
        } else {
            let mut t = seg.tick + (a - seg.tick + step - 1) / step * step;
            while t <= b {
                let level = if (t - seg.tick) % bar == 0 {
                    GridLevel::Bar
                } else if (t - seg.tick) % beat == 0 {
                    GridLevel::Beat
                } else {
                    GridLevel::Sub
                };
                out.push(GridLine { tick: t, level });
                t += step;
            }
        }
    }
    out
}

/// `"bar.beat.sixteenth.tick"` for a tick inside a beat: the sixteenth is 1-based, the tick the
/// remainder inside it (four digits). Whole beats shorten to `"bar.beat"`, whole sixteenths to
/// `"bar.beat.sixteenth"`.
pub fn deep_label(tick: i64, bars: &Bars) -> String {
    let (bar, beat, t) = bars.bbt(tick);
    let sixteenth = PPQ / 4;
    match (t / sixteenth, t % sixteenth) {
        (0, 0) => format!("{bar}.{beat}"),
        (s, 0) => format!("{bar}.{beat}.{}", s + 1),
        (s, rest) => format!("{bar}.{beat}.{}.{rest:04}", s + 1),
    }
}

/// Labels for the ruler lines that are not bar lines, once the zoom shows lines finer than a
/// beat: every beat and subdivision line at least `min_px` from the next one, labelled with
/// [`deep_label`]. Empty while the grid is a beat or coarser (bar numbers alone label the ruler).
/// Every returned tick is a line of [`grid_lines`] at the drawing `min_px`: the label step is a
/// multiple of the drawn one.
pub fn deep_labels(
    t0: i64,
    t1: i64,
    px_per_tick: f64,
    bars: &Bars,
    min_px: f64,
) -> Vec<(i64, String)> {
    let lines = grid_lines(t0, t1, px_per_tick, bars, min_px);
    if !lines.iter().any(|g| g.level == GridLevel::Sub) {
        return Vec::new();
    }
    lines
        .into_iter()
        .filter(|g| g.level != GridLevel::Bar)
        .map(|g| (g.tick, deep_label(g.tick, bars)))
        .collect()
}

/// Bars between ruler labels so that labels are at least `min_px` apart (1, 2, 4, 8…).
pub fn label_every_bars(px_per_tick: f64, meter: Meter, min_px: f64) -> i64 {
    let mut n = 1;
    while (meter.bar_ticks() * n) as f64 * px_per_tick < min_px && n < 1 << 20 {
        n *= 2;
    }
    n
}

/// `(tick, bar)` of the bar lines in `[t0, t1]` that carry a ruler label: every `n`th bar of
/// the song (`1, 1 + n, ...`, see [`label_every_bars`]) and the first bar of every meter
/// segment, so a change of meter is always numbered.
pub fn bar_labels(t0: i64, t1: i64, px_per_tick: f64, bars: &Bars, min_px: f64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let segs = bars.segments();
    for (i, seg) in segs.iter().enumerate() {
        let end = segs.get(i + 1).map_or(i64::MAX, |n| n.tick);
        let (a, b) = (t0.max(seg.tick).max(0), t1.min(end - 1));
        if a > b {
            continue;
        }
        let meter = Meter {
            beats_per_bar: seg.num,
            beat_unit: seg.den,
        };
        let every = label_every_bars(px_per_tick, meter, min_px);
        let bar_ticks = meter.bar_ticks();
        let mut k = (a - seg.tick) / bar_ticks;
        while seg.tick + k * bar_ticks <= b {
            let tick = seg.tick + k * bar_ticks;
            let bar = i64::from(seg.bar) + k;
            if tick >= a && (k == 0 || (bar - 1) % every == 0) {
                out.push((tick, bar));
            }
            k += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: i64 = 4 * PPQ;

    fn m(b: u8, u: u8) -> Meter {
        Meter {
            beats_per_bar: b,
            beat_unit: u,
        }
    }

    fn bars(events: &[(u32, u8, u8)]) -> Bars {
        let events: Vec<atomik_model::MeterEvent> = events
            .iter()
            .map(|&(bar, num, den)| atomik_model::MeterEvent { bar, num, den })
            .collect();
        Bars {
            segments: MeterTable::new(&events).unwrap().segments().into(),
        }
    }

    #[test]
    fn meter_ticks() {
        assert_eq!(m(4, 4).beat_ticks(), PPQ);
        assert_eq!(m(4, 4).bar_ticks(), BAR);
        assert_eq!(m(6, 8).beat_ticks(), PPQ / 2);
        assert_eq!(m(6, 8).bar_ticks(), 3 * PPQ);
        assert_eq!(m(3, 4).bar_ticks(), 3 * PPQ);
        assert_eq!(m(7, 16).bar_ticks(), 7 * PPQ / 4);
    }

    #[test]
    fn bbt_and_labels_are_meter_aware() {
        let b = |x: Meter| Bars::single(x);
        assert_eq!(b(m(4, 4)).bbt(0), (1, 1, 0));
        assert_eq!(b(m(4, 4)).bbt(BAR + PPQ + 120), (2, 2, 120));
        assert_eq!(b(m(3, 4)).bbt(3 * PPQ), (2, 1, 0));
        assert_eq!(b(m(6, 8)).bbt(5 * PPQ / 2), (1, 6, 0));
        assert_eq!(b(m(4, 4)).bbt(-1), (0, 4, PPQ - 1));
        assert_eq!(
            format_bbt(2 * BAR + PPQ + PPQ / 2, &b(m(4, 4))),
            "3.2.07680"
        );
        assert_eq!(format_bbt(0, &b(m(4, 4))), "1.1.00000");
        assert_eq!(bar_beat_label(4 * BAR, &b(m(4, 4))), "5");
        assert_eq!(bar_beat_label(4 * BAR + 2 * PPQ, &b(m(4, 4))), "5.3");
        assert_eq!(bar_beat_label(3 * PPQ, &b(m(3, 4))), "2");
        assert_eq!(bar_beat_label(PPQ / 4, &b(m(4, 4))), "1.1.03840");
    }

    #[test]
    fn seconds_through_a_step_tempo_map() {
        let t = |tick, bpm| TempoEvent::step(tick, bpm);
        let map = [t(0, 120.0)];
        assert!((tick_to_seconds(PPQ, &map) - 0.5).abs() < 1e-12);
        assert!((tick_to_seconds(BAR, &map) - 2.0).abs() < 1e-12);
        let map = [t(0, 120.0), t(BAR, 60.0)];
        assert!((tick_to_seconds(BAR + PPQ, &map) - 3.0).abs() < 1e-12);
        assert!((tick_to_seconds(BAR / 2, &map) - 1.0).abs() < 1e-12);
        assert!(tick_to_seconds(-5, &map).abs() < 1e-12);
        assert!((tick_to_seconds(PPQ, &[]) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn time_format() {
        assert_eq!(format_time(0.0), "00:00.000");
        assert_eq!(format_time(64.25), "01:04.250");
        assert_eq!(format_time(3600.0), "60:00.000");
        assert_eq!(format_time(-3.0), "00:00.000");
        assert_eq!(format_time(1.0005), "00:01.001");
    }

    #[test]
    fn snap_steps_and_cycle() {
        let mt = m(4, 4);
        assert_eq!(Snap::Bar.step(mt), Some(BAR));
        assert_eq!(Snap::Beat.step(m(6, 8)), Some(PPQ / 2));
        assert_eq!(Snap::Eighth.step(mt), Some(PPQ / 2));
        assert_eq!(Snap::Sixteenth.step(mt), Some(PPQ / 4));
        assert_eq!(Snap::Off.step(mt), None);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(Snap::Beat.step_with(mt, alt), None);
        assert_eq!(Snap::Beat.step_with(mt, Modifiers::SHIFT), Some(PPQ));
        assert_eq!(Snap::Off.cycle(true), Snap::Bar);
        assert_eq!(Snap::Bar.cycle(false), Snap::Off);
        assert_eq!(Snap::Beat.cycle(true), Snap::Eighth);
        assert_eq!(Snap::Sixteenth.label(), "1/16");
    }

    #[test]
    fn triplet_and_fine_snaps_have_their_exact_steps() {
        let mt = m(4, 4);
        assert_eq!(Snap::ThirtySecond.step(mt), Some(PPQ / 8));
        assert_eq!(Snap::SixtyFourth.step(mt), Some(PPQ / 16));
        // Three triplets fit where two plain notes of the next size up fit.
        assert_eq!(Snap::QuarterTriplet.step(mt), Some(10_240));
        assert_eq!(3 * 10_240, 2 * PPQ);
        assert_eq!(Snap::EighthTriplet.step(mt), Some(5_120));
        assert_eq!(3 * 5_120, PPQ);
        assert_eq!(Snap::SixteenthTriplet.step(mt), Some(2_560));
        assert_eq!(3 * 2_560, PPQ / 2);
        assert_eq!(Snap::ThirtySecondTriplet.step(mt), Some(1_280));
        assert_eq!(3 * 1_280, PPQ / 4);
        for s in Snap::ALL {
            assert_ne!(s.step(mt), Some(0), "{s:?}");
            assert!(!s.label().is_empty());
        }
        let all: std::collections::BTreeSet<_> = Snap::ALL.iter().collect();
        assert_eq!(all.len(), Snap::ALL.len(), "no value twice in the selector");
        let mut s = Snap::Bar;
        for _ in 0..Snap::ALL.len() {
            s = s.cycle(true);
        }
        assert_eq!(s, Snap::Bar, "the cycle visits every value and wraps");
        assert_eq!(Snap::Beat.offset(2), Snap::Sixteenth);
        assert_eq!(Snap::Sixteenth.offset(-2), Snap::Beat);
        assert_eq!(Snap::Off.offset(3), Snap::Off, "the ends stop, no wrap");
        assert_eq!(Snap::Bar.offset(-1), Snap::Bar);
    }

    #[test]
    fn triplet_grid_rounds_to_thirds_and_restarts_at_bar_lines() {
        let g = SnapGrid::new(Snap::EighthTriplet, Bars::single(m(4, 4)));
        assert_eq!(snap_round(2_559, &g), 0);
        assert_eq!(snap_round(2_560, &g), 5_120, "halves round up");
        assert_eq!(snap_round(5_119, &g), 5_120);
        assert_eq!(snap_floor(10_239, &g), 5_120);
        assert_eq!(snap_ceil(5_121, &g), 10_240);
        assert_eq!(snap_round(PPQ, &g), PPQ, "three triplets make a beat");
        assert_eq!(snap_round(-5_000, &g), -5_120);
        // 3/4 bars hold 1.5 half-note triplets: the last cell stops at the bar line.
        let g = SnapGrid::new(Snap::QuarterTriplet, Bars::single(m(3, 4)));
        let bar = 3 * PPQ;
        assert_eq!(snap_ceil(bar - 1, &g), bar, "never past the bar line");
        assert_eq!(
            snap_round(bar + 10_000, &g),
            bar + 10_240,
            "restarts in bar 2"
        );
        // A meter change moves the grid with the bar lines.
        let g = SnapGrid::new(Snap::SixteenthTriplet, bars(&[(1, 4, 4), (2, 3, 8)]));
        assert_eq!(snap_round(BAR + 2_559, &g), BAR + 2_560);
        assert_eq!(snap_floor(BAR + 5_000, &g), BAR + 2_560);
    }

    #[test]
    fn grid_snap_follows_the_visible_grid() {
        let mt = m(4, 4);
        let ppt = 96.0 / BAR as f64;
        let want = grid_step(ppt, mt, GRID_MIN_PX).min(BAR);
        assert_eq!(Snap::Grid.step_zoomed(mt, ppt), Some(want));
        assert_eq!(
            Snap::Grid.step_zoomed(mt, MAX_PX_PER_TICK),
            Some(grid_step(MAX_PX_PER_TICK, mt, GRID_MIN_PX))
        );
        assert_eq!(
            Snap::Grid.step_zoomed(mt, MIN_PX_PER_TICK / 64.0),
            Some(BAR),
            "far out the grid is capped at a bar"
        );
        assert_eq!(Snap::Grid.step_zoomed(mt, 0.0), Some(PPQ / 4), "no zoom");
        assert_eq!(Snap::Grid.step_zoomed(mt, f64::NAN), Some(PPQ / 4));
        assert_eq!(
            Snap::Beat.step_zoomed(mt, 5.0),
            Some(PPQ),
            "others ignore it"
        );
        // The step is the one the drawn lines have.
        let ppt = 0.05;
        let step = Snap::Grid.step_zoomed(mt, ppt).unwrap();
        let lines = grid_lines(0, BAR, ppt, &Bars::single(mt), GRID_MIN_PX);
        assert_eq!(lines[1].tick - lines[0].tick, step);
        let g = SnapGrid::new(Snap::Grid, Bars::single(mt)).zoomed(ppt);
        assert_eq!(snap_round(step + step / 2 - 1, &g), step);
        let unzoomed = SnapGrid::new(Snap::Grid, Bars::single(mt));
        assert_eq!(unzoomed.step_at(0), Some(PPQ / 4));
    }

    #[test]
    fn the_grid_stays_readable_at_every_zoom() {
        let mt = m(4, 4);
        let b = Bars::single(mt);
        let mut ppt = MAX_PX_PER_TICK;
        while ppt > MIN_PX_PER_TICK / 4.0 {
            let step = grid_step(ppt, mt, GRID_MIN_PX);
            assert!(step >= 1, "{ppt}");
            assert!(step as f64 * ppt >= GRID_MIN_PX, "lines at {ppt} too close");
            if step < PPQ / 4 {
                assert!(step as f64 * ppt >= FINE_GRID_PX, "fine lines at {ppt}");
            }
            // A 1920 px screen at this zoom.
            let t1 = (1920.0 / ppt) as i64;
            let n = grid_lines(0, t1, ppt, &b, GRID_MIN_PX).len();
            assert!(n <= 1920 / 7 + 2, "{n} lines at {ppt} px/tick");
            ppt /= 1.37;
        }
    }

    #[test]
    fn grid_steps_walk_down_to_single_ticks() {
        let mt = m(4, 4);
        assert_eq!(grid_step(16.0, mt, 7.0), 5);
        assert_eq!(grid_step(16.0, mt, 100.0), 15);
        assert_eq!(grid_step(8.0, mt, 7.0), 5);
        assert_eq!(grid_step(1.0, mt, 7.0), 60);
        assert_eq!(grid_step(0.125, mt, 7.0), PPQ / 32, "the 1/128 note");
        assert_eq!(grid_step(0.03125, mt, 7.0), PPQ / 8);
        assert_eq!(
            grid_step(0.0078125, mt, 7.0),
            PPQ / 4,
            "sixteenths from 7 px"
        );
        assert_eq!(
            grid_step(1.0, m(6, 8), 1.0),
            60,
            "fine lines keep their distance"
        );
        assert_eq!(grid_step(1e9, m(7, 16), 7.0), 1);
        // Every step divides the next one, so a coarser grid is a subset of a finer one.
        for w in SUBDIVISIONS.windows(2) {
            assert_eq!(w[1] % w[0], 0, "{} / {}", w[1], w[0]);
        }
    }

    #[test]
    fn degenerate_zooms_give_a_finite_nonzero_step_and_bounded_lines() {
        let mt = m(4, 4);
        for ppt in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e-300, 1e300] {
            let step = grid_step(ppt, mt, GRID_MIN_PX);
            assert!(step >= 1, "{ppt}: {step}");
            let lines = grid_lines(0, 100 * BAR, ppt, &Bars::single(mt), GRID_MIN_PX);
            assert!(lines.len() <= 30_000, "{ppt}: {} lines", lines.len());
        }
        // A range far too long for the zoom is thinned, not drawn line by line.
        let lines = grid_lines(0, i64::MAX / 4, MAX_PX_PER_TICK, &Bars::default(), 7.0);
        assert!(lines.len() <= 20_002, "{}", lines.len());
        let far = i64::MAX / 2;
        let lines = grid_lines(far, far + 10, 16.0, &Bars::default(), 7.0);
        assert!(lines.len() <= 10);
    }

    #[test]
    fn deep_labels_name_bar_beat_sixteenth_and_tick() {
        let b = Bars::single(m(4, 4));
        assert_eq!(deep_label(0, &b), "1.1");
        assert_eq!(deep_label(PPQ, &b), "1.2");
        assert_eq!(deep_label(BAR + PPQ / 4, &b), "2.1.2");
        assert_eq!(deep_label(BAR + PPQ + PPQ / 2 + 480, &b), "2.2.3.0480");
        assert_eq!(deep_label(1, &b), "1.1.1.0001");
        // Beat-sized grids label nothing besides the bars; deep ones label the lines between.
        let ppt = 96.0 / BAR as f64;
        assert!(deep_labels(0, BAR, ppt, &b, 64.0).is_empty());
        let zoom = 480.0 / PPQ as f64;
        let deep = deep_labels(0, PPQ, zoom, &b, 64.0);
        assert_eq!(deep.first(), Some(&(PPQ / 4, "1.1.2".to_string())));
        assert!(deep.iter().all(|(t, _)| *t != 0), "bars keep their numbers");
        // Every labelled tick is a drawn line.
        let drawn: Vec<i64> = grid_lines(0, PPQ, zoom, &b, 7.0)
            .iter()
            .map(|g| g.tick)
            .collect();
        assert!(deep.iter().all(|(t, _)| drawn.contains(t)));
        // At the deepest zoom the ticks themselves are named.
        let ticks = deep_labels(0, 20, MAX_PX_PER_TICK, &b, 64.0);
        assert_eq!(ticks.len(), 4, "{ticks:?}");
        assert!(ticks.iter().all(|(_, l)| l.starts_with("1.1.1.")));
    }

    #[test]
    fn paging_keeps_a_tick_visible_and_fitting_fills_the_view() {
        let mut a = TimeAxis::new(100.0, 96.0);
        let ppt = a.px_per_tick;
        let x1 = 100.0 + 960.0;
        assert!((a.visible_ticks(x1) - 960.0 / ppt).abs() < 1e-6);
        assert!(!a.page_to(PPQ, x1), "already visible");
        assert!(a.page_to(100 * BAR, x1));
        assert!(a.tick_to_x(100 * BAR) > a.x0 && a.tick_to_x(100 * BAR) < x1);
        assert!(a.page_to(0, x1), "left of the view pages back");
        assert_eq!(a.scroll, 0.0);
        a.fit_span(4 * BAR, 6 * BAR, x1);
        assert!(a.tick_to_x(4 * BAR) >= a.x0 && a.tick_to_x(6 * BAR) <= x1);
        assert!(a.tick_to_x(6 * BAR) > x1 - 40.0, "it fills the view");
        a.fit_span(0, 1, x1);
        assert!(a.px_per_tick <= MAX_PX_PER_TICK, "a tiny span is clamped");
    }

    #[test]
    fn snapping_rounds() {
        let s = Some(PPQ);
        assert_eq!(snap_round(PPQ / 2 - 1, s), 0);
        assert_eq!(snap_round(PPQ / 2, s), PPQ);
        assert_eq!(snap_round(-PPQ / 2 - 1, s), -PPQ);
        assert_eq!(snap_floor(2 * PPQ - 1, s), PPQ);
        assert_eq!(snap_floor(-1, s), -PPQ);
        assert_eq!(snap_ceil(PPQ + 1, s), 2 * PPQ);
        assert_eq!(snap_ceil(PPQ, s), PPQ);
        assert_eq!(snap_ceil(-1, s), 0);
        assert_eq!(snap_round(1234, None), 1234);
        assert_eq!(snap_floor(1234, None), 1234);
    }

    #[test]
    fn axis_maps_both_ways_with_zoom_and_scroll() {
        let mut a = TimeAxis::new(100.0, 96.0);
        assert!((a.tick_to_x(BAR) - 196.0).abs() < 1e-9);
        assert_eq!(a.x_to_tick(196.0), BAR);
        a.scroll = BAR as f64;
        assert!((a.tick_to_x(BAR) - 100.0).abs() < 1e-9);
        assert_eq!(a.x_to_tick(148.0), BAR + BAR / 2);
        assert_eq!(a.dx_to_ticks(48.0), BAR / 2);
        for t in [0, 1, 777, BAR * 3 + 5] {
            assert!((a.x_to_tick(a.tick_to_x(t)) - t).abs() <= 1);
        }
    }

    #[test]
    fn zoom_keeps_the_anchor_tick_and_clamps() {
        let mut a = TimeAxis::new(100.0, 96.0);
        a.scroll = 2.0 * BAR as f64;
        let anchor = 300.0;
        let before = a.x_to_tick(anchor);
        a.zoom_at(2.0, anchor);
        assert!((a.px_per_tick - 2.0 * 96.0 / BAR as f64).abs() < 1e-12);
        assert!((a.x_to_tick(anchor) - before).abs() <= 1);
        a.zoom_at(1e9, anchor);
        assert!((a.px_per_tick - MAX_PX_PER_TICK).abs() < 1e-15);
        a.zoom_at(1e-12, anchor);
        assert!((a.px_per_tick - MIN_PX_PER_TICK).abs() < 1e-15);
        assert!(a.scroll >= 0.0);
        a.scroll_px(-1e9);
        assert!(a.scroll.abs() < 1e-12);
    }

    #[test]
    fn grid_adapts_to_zoom() {
        let mt = m(4, 4);
        // 96 px per bar: beats are 24 px, sixteenths 6 px.
        let ppt = 96.0 / BAR as f64;
        assert_eq!(grid_step(ppt, mt, 8.0), PPQ / 2);
        assert_eq!(grid_step(ppt, mt, 20.0), PPQ);
        assert_eq!(grid_step(ppt, mt, 50.0), BAR);
        assert_eq!(grid_step(ppt, mt, 150.0), 2 * BAR);
        let lines = grid_lines(0, BAR, ppt, &Bars::single(mt), 20.0);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].level, GridLevel::Bar);
        assert_eq!(lines[1].level, GridLevel::Beat);
        assert_eq!(lines[4].tick, BAR);
        let fine = grid_lines(0, PPQ, 480.0 / PPQ as f64, &Bars::single(mt), 8.0);
        assert_eq!(fine[1].level, GridLevel::Sub);
        assert_eq!(fine[1].tick, PPQ / 8);

        assert_eq!(label_every_bars(ppt, mt, 40.0), 1);
        assert_eq!(label_every_bars(ppt / 4.0, mt, 40.0), 2);
    }

    #[test]
    fn seconds_through_a_tempo_ramp() {
        // 60 -> 180 BPM over two bars: ln(3) * 60 / PPQ * L / 120 seconds, then 180 BPM.
        let map = [
            TempoEvent {
                tick: 0,
                bpm: 60.0,
                ramp: true,
            },
            TempoEvent::step(2 * BAR, 180.0),
        ];
        let ramp = 60.0 / PPQ as f64 * (2 * BAR) as f64 / 120.0 * 3.0f64.ln();
        assert!((tick_to_seconds(2 * BAR, &map) - ramp).abs() < 1e-9);
        assert!((tick_to_seconds(2 * BAR + PPQ, &map) - ramp - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn bars_convert_through_meter_changes() {
        // 4/4 bars 1-2, 3/4 bars 3-4, 6/8 from bar 5.
        let b = bars(&[(1, 4, 4), (3, 3, 4), (5, 6, 8)]);
        let b34 = 3 * PPQ;
        assert_eq!(b.bar_start(1), 0);
        assert_eq!(b.bar_start(3), 2 * BAR);
        assert_eq!(b.bar_start(5), 2 * BAR + 2 * b34);
        assert_eq!(b.bar_start(0), -BAR);
        assert_eq!(b.bar_of(2 * BAR - 1), 2);
        assert_eq!(b.bar_of(2 * BAR), 3);
        assert_eq!(b.bar_of(-1), 0);
        assert_eq!(b.meter_at(2 * BAR), m(3, 4));
        assert_eq!(b.meter_at(-4), m(4, 4));
        assert_eq!(b.bbt(2 * BAR + PPQ + 5), (3, 2, 5));
        assert_eq!(b.bbt(b.bar_start(5) + 5 * PPQ / 2), (5, 6, 0));
        assert_eq!(b.next_bar_line(2 * BAR), 2 * BAR + b34);
        assert_eq!(b.prev_bar_line(2 * BAR), BAR);
        assert_eq!(b.prev_bar_line(2 * BAR + 1), 2 * BAR);
        assert_eq!(b.prev_bar_line(0), 0);
        assert_eq!(b.nearest_bar_line(2 * BAR + b34 / 2 - 1), 2 * BAR);
        assert_eq!(b.nearest_bar_line(2 * BAR + b34 / 2), 2 * BAR + b34);
        assert_eq!(bar_beat_label(2 * BAR, &b), "3");
        assert_eq!(bar_beat_label(2 * BAR + PPQ, &b), "3.2");
        assert_eq!(Bars::of(&atomik_model::empty_song(120.0)), Bars::default());
    }

    #[test]
    fn the_snap_grid_follows_the_meter_map() {
        // 7/8 (3.5 beats) then 4/4: the beat grid of the 4/4 bars starts at the bar line.
        let b = bars(&[(1, 7, 8), (2, 4, 4)]);
        let seven8 = 7 * PPQ / 2;
        let beat = SnapGrid::new(Snap::Beat, b.clone());
        assert_eq!(beat.step_at(0), Some(PPQ / 2));
        assert_eq!(beat.step_at(seven8), Some(PPQ));
        assert_eq!(beat.round_to(seven8 + PPQ + 10), seven8 + PPQ);
        assert_eq!(beat.floor_to(seven8 + PPQ - 1), seven8);
        assert_eq!(beat.ceil_to(seven8 + 1), seven8 + PPQ);
        assert_eq!(beat.ceil_to(seven8), seven8);
        let bar = SnapGrid::new(Snap::Bar, b.clone());
        assert_eq!(bar.round_to(seven8 / 2 - 1), 0);
        assert_eq!(bar.round_to(seven8 / 2), seven8);
        assert_eq!(bar.round_to(seven8 + 2 * PPQ), seven8 + BAR);
        assert_eq!(bar.floor_to(seven8 + BAR - 1), seven8);
        assert_eq!(bar.ceil_to(seven8 + 1), seven8 + BAR);
        // A step never crosses a bar line: the last half note step of a 7/8 bar is cut.
        let eighth = SnapGrid::new(Snap::Eighth, b);
        assert_eq!(eighth.round_to(seven8 - 10), seven8);
        assert_eq!(eighth.ceil_to(seven8 - PPQ / 4), seven8);
        assert_eq!(eighth.floor_to(seven8 - 1), seven8 - PPQ / 2);
        // Off and uniform steps behave as the plain functions.
        let off = SnapGrid::off();
        assert_eq!(off.round_to(1234), 1234);
        assert_eq!(off.unit(), None);
        assert_eq!(snap_round(-PPQ / 2 - 1, Some(PPQ)), -PPQ);
        assert_eq!(snap_ceil(PPQ + 1, Some(PPQ)), 2 * PPQ);
        let sixteenth = SnapGrid::new(Snap::Sixteenth, Bars::default());
        for t in [-9_000, -1, 0, 1, 1_919, 1_920, 3_840, 40_001] {
            assert_eq!(
                sixteenth.round_to(t),
                snap_round(t, Some(PPQ / 4)),
                "round {t}"
            );
            assert_eq!(
                sixteenth.floor_to(t),
                snap_floor(t, Some(PPQ / 4)),
                "floor {t}"
            );
            assert_eq!(
                sixteenth.ceil_to(t),
                snap_ceil(t, Some(PPQ / 4)),
                "ceil {t}"
            );
        }
        assert_eq!(sixteenth.unit(), Some(PPQ / 4));
    }

    #[test]
    fn grid_lines_restart_at_every_meter_change() {
        let b = bars(&[(1, 4, 4), (2, 3, 4)]);
        let ppt = 96.0 / BAR as f64;
        let lines = grid_lines(0, BAR + 3 * PPQ, ppt, &b, 20.0);
        let ticks: Vec<(i64, GridLevel)> = lines.iter().map(|g| (g.tick, g.level)).collect();
        // 24 px beats: bar 1 in 4/4, bar 2 in 3/4.
        assert_eq!(
            ticks,
            vec![
                (0, GridLevel::Bar),
                (PPQ, GridLevel::Beat),
                (2 * PPQ, GridLevel::Beat),
                (3 * PPQ, GridLevel::Beat),
                (BAR, GridLevel::Bar),
                (BAR + PPQ, GridLevel::Beat),
                (BAR + 2 * PPQ, GridLevel::Beat),
                (BAR + 3 * PPQ, GridLevel::Bar),
            ]
        );
        // Far zoom: multi-bar lines are the bars 1, 1 + n, ... of the song.
        let far = bars(&[(1, 4, 4), (4, 3, 4)]);
        let lines = grid_lines(0, 12 * BAR, 96.0 / BAR as f64 / 8.0, &far, 20.0);
        assert!(lines.iter().all(|g| g.level == GridLevel::Bar));
        let bars_hit: Vec<i64> = lines.iter().map(|g| Bars::bar_of(&far, g.tick)).collect();
        assert_eq!(bars_hit, vec![1, 3, 5, 9, 13]);
    }

    #[test]
    fn labels_number_every_change_of_meter() {
        let b = bars(&[(1, 4, 4), (4, 3, 4)]);
        let ppt = 96.0 / BAR as f64;
        let labels = bar_labels(0, 8 * BAR, ppt, &b, 34.0);
        assert_eq!(labels.first(), Some(&(0, 1)));
        assert_eq!(labels.iter().filter(|l| l.1 == 4).count(), 1);
        assert_eq!(labels.iter().find(|l| l.1 == 4).unwrap().0, 3 * BAR);
        assert_eq!(labels.len(), 10, "bars 1-3 in 4/4, then 4-10 in 3/4");
        // Zoomed out, every second bar is labelled, plus the first bar of a segment.
        let labels = bar_labels(0, 8 * BAR, ppt / 4.0, &b, 34.0);
        let nums: Vec<i64> = labels.iter().map(|l| l.1).collect();
        assert_eq!(nums, vec![1, 3, 4, 5, 7, 9]);
    }
}
