//! Pure time-axis helpers for the sequencer views: tick ↔ x
//! mapping with zoom and scroll, meter maths, snap, bar/beat labels, ruler grid lines and the
//! bars.beats.ticks / mm:ss.mmm readouts.
//!
//! Everything here is deterministic integer or `f64` arithmetic with no time source.

use crate::event::Modifiers;
use atomik_model::{Song, TempoEvent};

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
    /// The song's meter.
    pub fn of(song: &Song) -> Self {
        Self {
            beats_per_bar: song.beats_per_bar,
            beat_unit: song.beat_unit,
        }
    }

    /// Ticks per beat (`PPQ · 4 / beat_unit`).
    pub fn beat_ticks(self) -> i64 {
        PPQ * 4 / i64::from(self.beat_unit.max(1))
    }

    /// Ticks per bar.
    pub fn bar_ticks(self) -> i64 {
        self.beat_ticks() * i64::from(self.beats_per_bar.max(1))
    }

    /// `(bar, beat, tick)` of a song tick: bar and beat are 1-based, `tick` is the remainder
    /// inside the beat. Negative ticks floor towards earlier bars.
    pub fn bbt(self, tick: i64) -> (i64, i64, i64) {
        let bar = tick.div_euclid(self.bar_ticks());
        let in_bar = tick.rem_euclid(self.bar_ticks());
        let beat = in_bar / self.beat_ticks();
        (bar + 1, beat + 1, in_bar % self.beat_ticks())
    }
}

/// `"bar.beat.ticks"` with the tick remainder zero-padded to five digits, e.g. `"3.2.07680"`.
pub fn format_bbt(tick: i64, meter: Meter) -> String {
    let (bar, beat, t) = meter.bbt(tick);
    format!("{bar}.{beat}.{t:05}")
}

/// A ruler label: `"5"` on a bar line, `"5.3"` on a beat line, `"5.3.01920"` elsewhere.
pub fn bar_beat_label(tick: i64, meter: Meter) -> String {
    let (bar, beat, t) = meter.bbt(tick);
    if beat == 1 && t == 0 {
        format!("{bar}")
    } else if t == 0 {
        format!("{bar}.{beat}")
    } else {
        format_bbt(tick, meter)
    }
}

/// Seconds at a song tick through a step tempo map (events sorted by tick; an empty map
/// means 120 BPM). Negative ticks give 0.
pub fn tick_to_seconds(tick: i64, tempo: &[TempoEvent]) -> f64 {
    let tick = tick.max(0);
    let mut secs = 0.0;
    let mut pos = 0_i64;
    let mut bpm = tempo.first().map_or(120.0, |e| e.bpm);
    for e in tempo.iter().skip(1) {
        if e.tick >= tick {
            break;
        }
        let at = e.tick.max(pos);
        secs += (at - pos) as f64 / PPQ as f64 * 60.0 / bpm;
        pos = at;
        bpm = e.bpm;
    }
    secs + (tick - pos) as f64 / PPQ as f64 * 60.0 / bpm
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
}

impl Snap {
    /// Every resolution, in selector order.
    pub const ALL: [Snap; 5] = [
        Snap::Bar,
        Snap::Beat,
        Snap::Eighth,
        Snap::Sixteenth,
        Snap::Off,
    ];

    /// Grid step in ticks, `None` when off.
    pub fn step(self, meter: Meter) -> Option<i64> {
        match self {
            Snap::Off => None,
            Snap::Bar => Some(meter.bar_ticks()),
            Snap::Beat => Some(meter.beat_ticks()),
            Snap::Eighth => Some(PPQ / 2),
            Snap::Sixteenth => Some(PPQ / 4),
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
        }
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

/// Rounds to the nearest multiple of `step` (halves round up); identity when `None`.
pub fn snap_round(tick: i64, step: Option<i64>) -> i64 {
    match step {
        Some(s) if s > 0 => (tick + s / 2).div_euclid(s) * s,
        _ => tick,
    }
}

/// Rounds down to a multiple of `step`; identity when `None`.
pub fn snap_floor(tick: i64, step: Option<i64>) -> i64 {
    match step {
        Some(s) if s > 0 => tick.div_euclid(s) * s,
        _ => tick,
    }
}

/// Rounds up to a multiple of `step`; identity when `None`.
pub fn snap_ceil(tick: i64, step: Option<i64>) -> i64 {
    match step {
        Some(s) if s > 0 => -(-tick).div_euclid(s) * s,
        _ => tick,
    }
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
/// Largest zoom: a sixteenth is 480 px wide.
pub const MAX_PX_PER_TICK: f64 = 480.0 / (PPQ as f64 / 4.0);

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

/// The finest grid step (ticks) whose lines are at least `min_px` apart: 1/16, 1/8, beat,
/// bar, then 2, 4, 8… bars.
pub fn grid_step(px_per_tick: f64, meter: Meter, min_px: f64) -> i64 {
    let mut candidates = vec![PPQ / 4, PPQ / 2, meter.beat_ticks()];
    candidates.retain(|s| *s <= meter.beat_ticks());
    candidates.sort_unstable();
    candidates.dedup();
    let mut bars = 1;
    loop {
        if let Some(s) = candidates
            .iter()
            .copied()
            .find(|s| *s as f64 * px_per_tick >= min_px)
        {
            return s;
        }
        let s = meter.bar_ticks() * bars;
        if s as f64 * px_per_tick >= min_px || bars >= 1 << 20 {
            return s;
        }
        bars *= 2;
        candidates.clear();
    }
}

/// Grid lines covering `[t0, t1]` with lines at least `min_px` apart, each classified as a
/// bar, beat or subdivision line.
pub fn grid_lines(t0: i64, t1: i64, px_per_tick: f64, meter: Meter, min_px: f64) -> Vec<GridLine> {
    let step = grid_step(px_per_tick, meter, min_px);
    let (bar, beat) = (meter.bar_ticks(), meter.beat_ticks());
    let mut out = Vec::new();
    let mut t = snap_ceil(t0.max(0), Some(step));
    while t <= t1 {
        let level = if t % bar == 0 {
            GridLevel::Bar
        } else if t % beat == 0 {
            GridLevel::Beat
        } else {
            GridLevel::Sub
        };
        out.push(GridLine { tick: t, level });
        t += step;
    }
    out
}

/// Bars between ruler labels so that labels are at least `min_px` apart (1, 2, 4, 8…).
pub fn label_every_bars(px_per_tick: f64, meter: Meter, min_px: f64) -> i64 {
    let mut n = 1;
    while (meter.bar_ticks() * n) as f64 * px_per_tick < min_px && n < 1 << 20 {
        n *= 2;
    }
    n
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
        assert_eq!(m(4, 4).bbt(0), (1, 1, 0));
        assert_eq!(m(4, 4).bbt(BAR + PPQ + 120), (2, 2, 120));
        assert_eq!(m(3, 4).bbt(3 * PPQ), (2, 1, 0));
        assert_eq!(m(6, 8).bbt(5 * PPQ / 2), (1, 6, 0));
        assert_eq!(m(4, 4).bbt(-1), (0, 4, PPQ - 1));
        assert_eq!(format_bbt(2 * BAR + PPQ + PPQ / 2, m(4, 4)), "3.2.07680");
        assert_eq!(format_bbt(0, m(4, 4)), "1.1.00000");
        assert_eq!(bar_beat_label(4 * BAR, m(4, 4)), "5");
        assert_eq!(bar_beat_label(4 * BAR + 2 * PPQ, m(4, 4)), "5.3");
        assert_eq!(bar_beat_label(3 * PPQ, m(3, 4)), "2");
        assert_eq!(bar_beat_label(PPQ / 4, m(4, 4)), "1.1.03840");
    }

    #[test]
    fn seconds_through_a_step_tempo_map() {
        let t = |tick, bpm| TempoEvent { tick, bpm };
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
        let lines = grid_lines(0, BAR, ppt, mt, 20.0);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].level, GridLevel::Bar);
        assert_eq!(lines[1].level, GridLevel::Beat);
        assert_eq!(lines[4].tick, BAR);
        let fine = grid_lines(0, PPQ, 480.0 / PPQ as f64, mt, 8.0);
        assert_eq!(fine[1].level, GridLevel::Sub);
        assert_eq!(fine[1].tick, PPQ / 4);
        // 6/8: beats are eighths, so no separate eighth subdivision.
        assert_eq!(grid_step(1.0, m(6, 8), 1.0), PPQ / 4);
        assert_eq!(label_every_bars(ppt, mt, 40.0), 1);
        assert_eq!(label_every_bars(ppt / 4.0, mt, 40.0), 2);
    }
}
