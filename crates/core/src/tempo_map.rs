//! The tempo map and the meter map: the one place where song ticks, seconds, frames and bars
//! are converted into each other.
//!
//! Both maps are compiled once, off the audio thread, into a table of segments
//! ([`compile_tempo`], [`compile_meter`]). The maths runs on a borrowed view of the table
//! ([`TempoMap`], [`MeterMap`]), which is `Copy` and allocation-free, so the RT player, the
//! bounce, the model and the UI all use the same code. [`TempoTable`] and [`MeterTable`] own a
//! compiled table for the callers that are not on the audio thread.
//!
//! - **Tempo.** An event holds a BPM from its tick; with `ramp` the tempo instead moves
//!   linearly in BPM over ticks to the next event's BPM (the last event is always constant).
//!   For a ramp `b(x) = b0 + (b1 - b0) x / L` over `L` ticks, the time is
//!   `60 / PPQ * L / (b1 - b0) * ln(b(x) / b0)` and its inverse uses `exp`; an equal-BPM ramp
//!   is a step. A segment's start time is accumulated once from its predecessors (no drift),
//!   and a step segment uses the plain `ticks * 60 / (bpm * PPQ)` division, so a map without
//!   ramps gives the same bits as the original step-only maths. Ticks before 0 extend the
//!   first tempo backwards.
//! - **Meter.** A meter event holds a time signature from a bar line (1-based bar). Bars
//!   always hold whole beats of the meter in effect, so a bar's tick follows from the events.

use crate::{PPQ, Tempo};

/// Why a tempo or meter map cannot be compiled.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MapError {
    /// The tempo map is empty or does not start at tick 0.
    #[error("tempo map must start at tick 0")]
    TempoStart,
    /// Tempo ticks do not strictly increase.
    #[error("tempo ticks must strictly increase")]
    TempoOrder,
    /// A tempo is outside 20..=999 BPM.
    #[error("tempo {0} BPM is outside 20..=999")]
    TempoRange(f64),
    /// The meter map is empty or does not start at bar 1.
    #[error("meter map must start at bar 1")]
    MeterStart,
    /// Meter bars do not strictly increase.
    #[error("meter bars must strictly increase")]
    MeterOrder,
    /// A time signature outside 1..=32 over 2, 4, 8 or 16.
    #[error("time signature {0}/{1} is not supported")]
    MeterRange(u8, u8),
}

/// A tempo event: `bpm` from `tick`; with `ramp` the tempo moves linearly (in BPM, over ticks)
/// to the next event's `bpm`. `ramp` on the last event has no effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoPoint {
    /// Tick where the event takes effect (the first is at 0).
    pub tick: i64,
    /// Tempo in BPM (20..=999).
    pub bpm: f64,
    /// Ramp to the next event.
    pub ramp: bool,
}

/// One segment of a compiled tempo map: from `tick` to `end_tick` the tempo goes from `bpm`
/// to `end_bpm` (equal for a step); `seconds` is the time at `tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoSegment {
    /// First tick of the segment (the first segment is at 0).
    pub tick: i64,
    /// Seconds at `tick`, accumulated once from integer tick spans.
    pub seconds: f64,
    /// BPM at `tick`.
    pub bpm: f64,
    /// First tick of the next segment; `i64::MAX` for the last.
    pub end_tick: i64,
    /// BPM at `end_tick`; equal to `bpm` unless the segment ramps.
    pub end_bpm: f64,
}

impl TempoSegment {
    /// A one-segment map at a constant tempo.
    pub fn constant(tempo: Tempo) -> Self {
        Self {
            tick: 0,
            seconds: 0.0,
            bpm: tempo.bpm(),
            end_tick: i64::MAX,
            end_bpm: tempo.bpm(),
        }
    }

    fn ramps(&self) -> bool {
        (self.end_bpm - self.bpm).abs() > 0.0
    }

    /// Seconds spanned from the segment start to `tick` (ticks before the start extend the
    /// start tempo backwards).
    fn span(&self, tick: f64) -> f64 {
        let x = tick - self.tick as f64;
        let flat = (x * 60.0) / (self.bpm * PPQ as f64);
        if !self.ramps() || x <= 0.0 {
            return flat;
        }
        let len = (self.end_tick - self.tick) as f64;
        let u = (self.end_bpm - self.bpm) / self.bpm * (x / len).min(1.0);
        flat * ln_ratio(u)
    }

    /// Ticks from the segment start at `seconds` after it.
    fn ticks_in(&self, seconds: f64) -> f64 {
        let flat = seconds * (self.bpm * PPQ as f64) / 60.0;
        if !self.ramps() || seconds <= 0.0 {
            return flat;
        }
        let len = (self.end_tick - self.tick) as f64;
        let w = seconds * PPQ as f64 * (self.end_bpm - self.bpm) / (60.0 * len);
        (flat * exp_ratio(w)).min(len)
    }

    fn bpm_at(&self, tick: f64) -> f64 {
        if !self.ramps() {
            return self.bpm;
        }
        let x = ((tick - self.tick as f64) / (self.end_tick - self.tick) as f64).clamp(0.0, 1.0);
        self.bpm + (self.end_bpm - self.bpm) * x
    }
}

/// `ln(1 + u) / u`, continuous at 0.
fn ln_ratio(u: f64) -> f64 {
    if u.abs() < 1e-9 {
        1.0 - u / 2.0
    } else {
        u.ln_1p() / u
    }
}

/// `(e^w - 1) / w`, continuous at 0.
fn exp_ratio(w: f64) -> f64 {
    if w.abs() < 1e-9 {
        1.0 + w / 2.0
    } else {
        w.exp_m1() / w
    }
}

/// Validates a tempo map and compiles its segment table (non-RT; allocates).
pub fn compile_tempo(points: &[TempoPoint]) -> Result<Vec<TempoSegment>, MapError> {
    match points.first() {
        Some(first) if first.tick == 0 => {}
        _ => return Err(MapError::TempoStart),
    }
    if points.windows(2).any(|w| w[1].tick <= w[0].tick) {
        return Err(MapError::TempoOrder);
    }
    if let Some(p) = points.iter().find(|p| Tempo::new(p.bpm).is_err()) {
        return Err(MapError::TempoRange(p.bpm));
    }
    let mut segments: Vec<TempoSegment> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        let seconds = segments
            .last()
            .map_or(0.0, |prev| prev.seconds + prev.span(p.tick as f64));
        let next = points.get(i + 1);
        let (end_tick, end_bpm) = match next {
            Some(n) if p.ramp => (n.tick, n.bpm),
            Some(n) => (n.tick, p.bpm),
            None => (i64::MAX, p.bpm),
        };
        segments.push(TempoSegment {
            tick: p.tick,
            seconds,
            bpm: p.bpm,
            end_tick,
            end_bpm,
        });
    }
    Ok(segments)
}

/// Tempo maths over a segment table (a compiled map, or a constant one-segment map). Pure and
/// allocation-free.
#[derive(Debug, Clone, Copy)]
pub struct TempoMap<'a>(pub &'a [TempoSegment]);

impl<'a> TempoMap<'a> {
    fn seg(self, tick: f64) -> &'a TempoSegment {
        let k = self.0.partition_point(|s| s.tick as f64 <= tick);
        &self.0[k.saturating_sub(1)]
    }

    /// Seconds at a (fractional) tick.
    pub fn seconds(self, tick: f64) -> f64 {
        let s = self.seg(tick);
        s.seconds + s.span(tick)
    }

    /// Fractional song frame at a tick.
    pub fn frame(self, tick: f64, sample_rate: f64) -> f64 {
        self.seconds(tick) * sample_rate
    }

    /// The integer song frame where events at `tick` are emitted (nearest, ties up; >= 0).
    pub fn song_frame(self, tick: i64, sample_rate: f64) -> u64 {
        let f = self.frame(tick as f64, sample_rate).round();
        if f > 0.0 { f as u64 } else { 0 }
    }

    /// Fractional tick at a song frame (the exact inverse of [`TempoMap::frame`]).
    pub fn tick(self, frame: f64, sample_rate: f64) -> f64 {
        let seconds = frame / sample_rate;
        let k = self.0.partition_point(|s| s.seconds <= seconds);
        let s = &self.0[k.saturating_sub(1)];
        s.tick as f64 + s.ticks_in(seconds - s.seconds)
    }

    /// Tempo in BPM at a (fractional) tick; inside a ramp the instantaneous tempo.
    pub fn bpm(self, tick: f64) -> f64 {
        self.seg(tick).bpm_at(tick)
    }

    /// Ticks per sample frame at a tick (the tempo there).
    pub fn ticks_per_frame(self, tick: f64, sample_rate: f64) -> f64 {
        self.bpm(tick) * PPQ as f64 / (60.0 * sample_rate)
    }
}

/// An owned compiled tempo map, for callers off the audio thread.
#[derive(Debug, Clone, PartialEq)]
pub struct TempoTable {
    segments: Vec<TempoSegment>,
}

impl TempoTable {
    /// Compiles `points` ([`compile_tempo`]).
    pub fn new(points: &[TempoPoint]) -> Result<Self, MapError> {
        compile_tempo(points).map(|segments| Self { segments })
    }

    /// A constant-tempo table.
    pub fn constant(tempo: Tempo) -> Self {
        Self {
            segments: vec![TempoSegment::constant(tempo)],
        }
    }

    /// The maths view.
    pub fn map(&self) -> TempoMap<'_> {
        TempoMap(&self.segments)
    }

    /// The compiled segments.
    pub fn segments(&self) -> &[TempoSegment] {
        &self.segments
    }
}

#[cfg(test)]
mod tests;
