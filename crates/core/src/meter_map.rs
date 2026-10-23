//! The meter map: time signatures along the song and the bar <-> tick conversions.
//!
//! A [`MeterEvent`] holds a time signature from the start of a bar (1-based). Meter changes only
//! happen on bar lines, so every bar holds whole beats of the signature in effect and a bar's
//! tick follows from the events. Like the tempo map, the map is compiled once off the audio
//! thread ([`compile_meter`]); [`MeterMap`] is a `Copy` view with allocation-free maths and
//! [`MeterTable`] owns a compiled table.

use crate::PPQ;
use crate::tempo_map::MapError;

/// Largest accepted beats per bar.
pub const MAX_BEATS_PER_BAR: u8 = 32;

/// Accepted beat units (time signature denominators).
pub const BEAT_UNITS: [u8; 4] = [2, 4, 8, 16];

/// A meter event: the time signature `num/den` from the start of `bar` (1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterEvent {
    /// Bar where the signature takes effect (the first is bar 1).
    pub bar: u32,
    /// Beats per bar, 1..=[`MAX_BEATS_PER_BAR`].
    pub num: u8,
    /// Beat unit, one of [`BEAT_UNITS`].
    pub den: u8,
}

impl MeterEvent {
    /// A time signature `num/den` from the start of `bar`.
    pub const fn new(bar: u32, num: u8, den: u8) -> Self {
        Self { bar, num, den }
    }
}

/// One segment of a compiled meter map: from `bar` (at `tick`) on, the signature `num/den`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterSegment {
    /// Song tick of the first bar line of the segment.
    pub tick: i64,
    /// The bar that starts at `tick` (1-based).
    pub bar: u32,
    /// Beats per bar.
    pub num: u8,
    /// Beat unit.
    pub den: u8,
}

impl MeterSegment {
    /// The 4/4 segment a song without a meter map has.
    pub const FOUR_FOUR: Self = Self {
        tick: 0,
        bar: 1,
        num: 4,
        den: 4,
    };

    /// Ticks per beat (`PPQ * 4 / den`; at least 1).
    pub fn beat_ticks(&self) -> i64 {
        (PPQ * 4 / i64::from(self.den.max(1))).max(1)
    }

    /// Ticks per bar (at least 1).
    pub fn bar_ticks(&self) -> i64 {
        self.beat_ticks() * i64::from(self.num.max(1))
    }
}

/// Whether `num/den` is a supported time signature.
pub fn meter_ok(num: u8, den: u8) -> bool {
    (1..=MAX_BEATS_PER_BAR).contains(&num) && BEAT_UNITS.contains(&den)
}

/// Validates a meter map and compiles its segment table (non-RT; allocates).
pub fn compile_meter(events: &[MeterEvent]) -> Result<Vec<MeterSegment>, MapError> {
    match events.first() {
        Some(first) if first.bar == 1 => {}
        _ => return Err(MapError::MeterStart),
    }
    if events.windows(2).any(|w| w[1].bar <= w[0].bar) {
        return Err(MapError::MeterOrder);
    }
    if let Some(e) = events.iter().find(|e| !meter_ok(e.num, e.den)) {
        return Err(MapError::MeterRange(e.num, e.den));
    }
    let mut segments: Vec<MeterSegment> = Vec::with_capacity(events.len());
    for e in events {
        let tick = segments.last().map_or(0, |prev| {
            prev.tick + i64::from(e.bar - prev.bar) * prev.bar_ticks()
        });
        segments.push(MeterSegment {
            tick,
            bar: e.bar,
            num: e.num,
            den: e.den,
        });
    }
    Ok(segments)
}

/// Meter maths over a segment table. Pure and allocation-free. Ticks before 0 read as 0.
#[derive(Debug, Clone, Copy)]
pub struct MeterMap<'a>(pub &'a [MeterSegment]);

impl<'a> MeterMap<'a> {
    fn at_tick(self, tick: i64) -> &'a MeterSegment {
        let k = self.0.partition_point(|s| s.tick <= tick);
        &self.0[k.saturating_sub(1)]
    }

    fn at_bar(self, bar: u32) -> &'a MeterSegment {
        let k = self.0.partition_point(|s| s.bar <= bar);
        &self.0[k.saturating_sub(1)]
    }

    /// The signature `(num, den)` in effect at `tick`.
    pub fn meter_at(self, tick: i64) -> (u8, u8) {
        let s = self.at_tick(tick);
        (s.num, s.den)
    }

    /// Ticks per beat at `tick`.
    pub fn beat_ticks_at(self, tick: i64) -> i64 {
        self.at_tick(tick).beat_ticks()
    }

    /// Ticks per bar of the bar that holds `tick`.
    pub fn bar_ticks_at(self, tick: i64) -> i64 {
        self.at_tick(tick).bar_ticks()
    }

    /// The tick where `bar` (1-based; 0 reads as 1) starts.
    pub fn bar_start(self, bar: u32) -> i64 {
        let bar = bar.max(1);
        let s = self.at_bar(bar);
        s.tick + i64::from(bar - s.bar) * s.bar_ticks()
    }

    /// The bar (1-based) that holds `tick`; the number saturates at `u32::MAX`.
    pub fn bar_at(self, tick: i64) -> u32 {
        let tick = tick.max(0);
        let s = self.at_tick(tick);
        let bars = u32::try_from((tick - s.tick) / s.bar_ticks()).unwrap_or(u32::MAX);
        s.bar.saturating_add(bars)
    }

    /// The tick of the first bar line strictly after `tick`.
    pub fn next_bar_line(self, tick: i64) -> i64 {
        self.bar_start(self.bar_at(tick).saturating_add(1))
    }

    /// The tick of the last bar line strictly before `tick` (0 at or before the first bar).
    pub fn prev_bar_line(self, tick: i64) -> i64 {
        let bar = self.bar_at(tick);
        let start = self.bar_start(bar);
        if start < tick {
            start
        } else {
            self.bar_start(bar.saturating_sub(1))
        }
    }

    /// A tick as a 1-based bar, a 1-based beat and the ticks into that beat.
    pub fn bbt(self, tick: i64) -> (u32, u32, u32) {
        let tick = tick.max(0);
        let bar = self.bar_at(tick);
        let in_bar = tick - self.bar_start(bar);
        let beat = self.at_bar(bar).beat_ticks();
        let sat = |x: i64| u32::try_from(x).unwrap_or(u32::MAX);
        (bar, sat(in_bar / beat + 1), sat(in_bar % beat))
    }

    /// The inverse of [`MeterMap::bbt`]: `(bar, beat, ticks)` (bar and beat 1-based; 0 reads
    /// as 1) to a tick.
    pub fn from_bbt(self, bbt: (u32, u32, u32)) -> i64 {
        let (bar, beat, ticks) = bbt;
        self.bar_start(bar)
            + i64::from(beat.saturating_sub(1)) * self.at_bar(bar.max(1)).beat_ticks()
            + i64::from(ticks)
    }

    /// The beat line at or before `tick` (at least 0) and whether it is a downbeat (the first
    /// beat of a bar).
    pub fn beat_at_or_before(self, tick: i64) -> (i64, bool) {
        let tick = tick.max(0);
        let s = self.at_tick(tick);
        let idx = (tick - s.tick) / s.beat_ticks();
        (
            s.tick + idx * s.beat_ticks(),
            idx % i64::from(s.num.max(1)) == 0,
        )
    }

    /// The beat line after the beat line `beat` and whether it is a downbeat. A change of
    /// meter starts on a bar line, so the next beat is never skipped.
    pub fn beat_after(self, beat: i64) -> (i64, bool) {
        let s = self.at_tick(beat.max(0));
        let next = beat + s.beat_ticks();
        let k = self.0.partition_point(|x| x.tick <= beat);
        if let Some(n) = self.0.get(k)
            && next >= n.tick
        {
            return (n.tick, true);
        }
        let idx = (next - s.tick) / s.beat_ticks();
        (next, idx % i64::from(s.num.max(1)) == 0)
    }
}

/// An owned compiled meter map, for callers off the audio thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterTable {
    segments: Vec<MeterSegment>,
}

impl MeterTable {
    /// Compiles `events` ([`compile_meter`]).
    pub fn new(events: &[MeterEvent]) -> Result<Self, MapError> {
        compile_meter(events).map(|segments| Self { segments })
    }

    /// A map with one signature from bar 1.
    pub fn constant(num: u8, den: u8) -> Self {
        Self {
            segments: vec![MeterSegment {
                tick: 0,
                bar: 1,
                num,
                den,
            }],
        }
    }

    /// The maths view.
    pub fn map(&self) -> MeterMap<'_> {
        MeterMap(&self.segments)
    }

    /// The compiled segments.
    pub fn segments(&self) -> &[MeterSegment] {
        &self.segments
    }
}

#[cfg(test)]
mod tests;
