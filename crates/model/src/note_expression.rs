//! Per-note expression: the pitch, pressure and timbre curves a note carries.
//!
//! A controller that plays every note on a channel of its own (MPE) bends, presses and slides
//! each note by itself. A clip keeps what was played as one [`NoteExpression`] per note, found by
//! the note's start tick and key (a clip never holds two notes at one start and key). The curves
//! are plain points in ticks from the note's start; a note without an entry plays exactly as a
//! note always did.
//!
//! * **Pitch** is an offset in semitones (up to [`PITCH_LIMIT`] either way), added to the note's
//!   own pitch.
//! * **Pressure** is `0..=1`, the force held on the note after the strike.
//! * **Timbre** is `0..=1`, the slide of the finger (the third MPE dimension).
//!
//! A curve holds its first value before its first point and its last value after its last
//! point; the player sends each point as it comes (the instruments smooth the steps). The data
//! rides in the clip's [`crate::MidiData`], so a split, a join, a loop conversion and a clip
//! copy carry it with the notes, and it is saved only while a clip has some (an older reader
//! drops it and plays the notes without it). A note moved or retuned in the editors loses its
//! curves, because they are kept by the note's start and key.

use crate::song::MAX_TICK;

/// The most points one curve keeps.
pub const MAX_EXPRESSION_POINTS: usize = 1024;
/// The most notes with expression one clip keeps.
pub const MAX_EXPRESSION_NOTES: usize = 16_384;
/// The largest pitch offset of a curve, in semitones either way.
pub const PITCH_LIMIT: f32 = 96.0;
/// Points closer than this to the one before them (in ticks) are folded into it when a take is
/// recorded: about 5 ms at 120 BPM.
pub const RECORD_GAP: i64 = atomik_core::PPQ / 96;

/// Which curve of a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExpressionKind {
    /// Pitch offset in semitones.
    Pitch,
    /// Pressure, `0..=1`.
    Pressure,
    /// Timbre (slide), `0..=1`.
    Timbre,
}

impl ExpressionKind {
    /// Every curve, in saved order.
    pub const ALL: [ExpressionKind; 3] = [
        ExpressionKind::Pitch,
        ExpressionKind::Pressure,
        ExpressionKind::Timbre,
    ];

    /// The lowest and highest value of the curve.
    pub fn range(self) -> (f32, f32) {
        match self {
            ExpressionKind::Pitch => (-PITCH_LIMIT, PITCH_LIMIT),
            _ => (0.0, 1.0),
        }
    }

    /// `value` held inside the range (a value that is not finite becomes zero).
    pub fn clamp(self, value: f32) -> f32 {
        if !value.is_finite() {
            return 0.0;
        }
        let (lo, hi) = self.range();
        value.clamp(lo, hi)
    }
}

/// One point of a curve: `tick` after the note's start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpressionPoint {
    /// Ticks after the start of the note, `0..=MAX_TICK`.
    pub tick: i64,
    /// The value, inside the range of the curve.
    pub value: f32,
}

impl ExpressionPoint {
    /// A point.
    pub const fn new(tick: i64, value: f32) -> Self {
        Self { tick, value }
    }
}

/// The curves of one note, found by its start and key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NoteExpression {
    /// Start tick of the note in the clip's content ticks.
    pub tick: i64,
    /// Key of the note, `0..=127`.
    pub key: u8,
    /// Pitch offset in semitones.
    pub pitch: Vec<ExpressionPoint>,
    /// Pressure.
    pub pressure: Vec<ExpressionPoint>,
    /// Timbre (slide).
    pub timbre: Vec<ExpressionPoint>,
}

impl NoteExpression {
    /// An entry without curves for the note at `tick` on `key`.
    pub fn new(tick: i64, key: u8) -> Self {
        Self {
            tick,
            key,
            ..Self::default()
        }
    }

    /// The curve of `kind`.
    pub fn curve(&self, kind: ExpressionKind) -> &[ExpressionPoint] {
        match kind {
            ExpressionKind::Pitch => &self.pitch,
            ExpressionKind::Pressure => &self.pressure,
            ExpressionKind::Timbre => &self.timbre,
        }
    }

    /// The curve of `kind`, to change.
    pub fn curve_mut(&mut self, kind: ExpressionKind) -> &mut Vec<ExpressionPoint> {
        match kind {
            ExpressionKind::Pitch => &mut self.pitch,
            ExpressionKind::Pressure => &mut self.pressure,
            ExpressionKind::Timbre => &mut self.timbre,
        }
    }

    /// True when no curve has a point.
    pub fn is_empty(&self) -> bool {
        self.pitch.is_empty() && self.pressure.is_empty() && self.timbre.is_empty()
    }

    /// The value of the curve `kind` `after` ticks into the note (linear between points, held
    /// before the first and after the last); `None` for a curve without points.
    pub fn value_at(&self, kind: ExpressionKind, after: i64) -> Option<f32> {
        value_at(self.curve(kind), after)
    }

    /// The entry in canonical form: every curve sorted by tick (a later point at an equal tick
    /// wins), values clamped into their range, ticks at or above zero.
    pub fn canonical(mut self) -> Self {
        for kind in ExpressionKind::ALL {
            let curve = self.curve_mut(kind);
            for p in curve.iter_mut() {
                p.tick = p.tick.clamp(0, MAX_TICK);
                p.value = kind.clamp(p.value);
            }
            curve.sort_by_key(|p| p.tick);
            let mut out: Vec<ExpressionPoint> = Vec::with_capacity(curve.len());
            for p in curve.drain(..) {
                match out.last_mut() {
                    Some(last) if last.tick == p.tick => *last = p,
                    _ => out.push(p),
                }
            }
            out.truncate(MAX_EXPRESSION_POINTS);
            *curve = out;
        }
        self
    }

    /// Checks the entry: the note's tick and key in range, each curve sorted with strictly
    /// increasing ticks inside `0..=max_tick`, values finite and inside the range of the curve,
    /// at most [`MAX_EXPRESSION_POINTS`] points. `Err` names the first fault.
    pub fn check(&self, max_tick: i64) -> Result<(), String> {
        if !(0..=max_tick).contains(&self.tick) {
            return Err(format!(
                "note expression: tick {} is outside 0..={max_tick}",
                self.tick
            ));
        }
        if self.key > 127 {
            return Err("note expression: key beyond 127".to_owned());
        }
        for kind in ExpressionKind::ALL {
            let curve = self.curve(kind);
            if curve.len() > MAX_EXPRESSION_POINTS {
                return Err(format!(
                    "a note expression curve keeps at most {MAX_EXPRESSION_POINTS} points"
                ));
            }
            let (lo, hi) = kind.range();
            for (i, p) in curve.iter().enumerate() {
                if !(0..=max_tick).contains(&p.tick) {
                    return Err(format!(
                        "note expression {kind:?} point {i}: tick {} is outside 0..={max_tick}",
                        p.tick
                    ));
                }
                if !p.value.is_finite() || !(lo..=hi).contains(&p.value) {
                    return Err(format!(
                        "note expression {kind:?} point {i}: value {} is outside {lo}..={hi}",
                        p.value
                    ));
                }
                if i > 0 && curve[i - 1].tick >= p.tick {
                    return Err(format!(
                        "note expression {kind:?} points must be sorted by tick without \
                         duplicates (point {i})"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The value of `points` (sorted, strictly increasing ticks) `after` ticks in: linear between
/// points, held before the first and after the last; `None` without points.
pub fn value_at(points: &[ExpressionPoint], after: i64) -> Option<f32> {
    let first = points.first()?;
    let n = points.partition_point(|p| p.tick <= after);
    let Some(prev) = n.checked_sub(1).map(|i| points[i]) else {
        return Some(first.value);
    };
    match points.get(n) {
        Some(next) if next.tick > prev.tick => {
            let x = (after - prev.tick) as f64 / (next.tick - prev.tick) as f64;
            Some(
                (f64::from(prev.value) + (f64::from(next.value) - f64::from(prev.value)) * x)
                    as f32,
            )
        }
        _ => Some(prev.value),
    }
}

/// Builds the points of a recorded curve from `raw` `(tick after the note start, value)` pairs in
/// arrival order: a burst closer than [`RECORD_GAP`] keeps its last value at the burst's first
/// tick, a point that repeats the value before it is dropped, and a curve is cut at
/// [`MAX_EXPRESSION_POINTS`].
pub fn record_curve(kind: ExpressionKind, raw: &[(i64, f32)]) -> Vec<ExpressionPoint> {
    let mut sorted: Vec<(i64, f32)> = raw
        .iter()
        .map(|&(t, v)| (t.max(0), kind.clamp(v)))
        .collect();
    sorted.sort_by_key(|p| p.0); // stable: equal ticks keep their arrival order
    let mut out: Vec<ExpressionPoint> = Vec::new();
    for (tick, value) in sorted {
        match out.last_mut() {
            Some(last) if tick - last.tick < RECORD_GAP => {
                last.value = value;
            }
            _ => {
                if out.last().is_some_and(|l| l.value.to_bits() == value.to_bits()) {
                    continue;
                }
                if out.len() >= MAX_EXPRESSION_POINTS {
                    break;
                }
                out.push(ExpressionPoint { tick, value });
            }
        }
    }
    // A burst can leave a point equal to the one before it: drop the repeat.
    out.dedup_by(|b, a| a.value.to_bits() == b.value.to_bits());
    out
}

/// The entries of `list` in canonical form: sorted by `(tick, key)` (a later entry for the same
/// note wins), entries without a point dropped, at most [`MAX_EXPRESSION_NOTES`] kept.
pub fn canonical_list(mut list: Vec<NoteExpression>) -> Vec<NoteExpression> {
    list.retain(|e| !e.is_empty());
    list.sort_by_key(|e| (e.tick, e.key));
    let mut out: Vec<NoteExpression> = Vec::with_capacity(list.len());
    for e in list {
        let e = e.canonical();
        match out.last_mut() {
            Some(last) if (last.tick, last.key) == (e.tick, e.key) => *last = e,
            _ => out.push(e),
        }
    }
    out.truncate(MAX_EXPRESSION_NOTES);
    out
}

/// The entry for the note at `tick` on `key`, in a list sorted by `(tick, key)`.
pub fn find(list: &[NoteExpression], tick: i64, key: u8) -> Option<&NoteExpression> {
    list.binary_search_by_key(&(tick, key), |e| (e.tick, e.key))
        .ok()
        .map(|i| &list[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(tick: i64, value: f32) -> ExpressionPoint {
        ExpressionPoint::new(tick, value)
    }

    #[test]
    fn a_curve_holds_its_ends_and_is_linear_between_points() {
        let c = [p(100, 1.0), p(300, 3.0)];
        assert_eq!(value_at(&c, 0), Some(1.0));
        assert_eq!(value_at(&c, 100), Some(1.0));
        assert_eq!(value_at(&c, 200), Some(2.0));
        assert_eq!(value_at(&c, 300), Some(3.0));
        assert_eq!(value_at(&c, 9999), Some(3.0));
        assert_eq!(value_at(&[], 5), None);
    }

    #[test]
    fn canonical_sorts_clamps_and_keeps_the_later_point_of_a_tick() {
        let mut e = NoteExpression::new(10, 60);
        e.pitch = vec![p(50, 200.0), p(10, -3.0), p(50, 1.5)];
        e.pressure = vec![p(5, f32::NAN), p(4, 2.0)];
        let e = e.canonical();
        assert_eq!(e.pitch, vec![p(10, -3.0), p(50, 1.5)]);
        assert_eq!(e.pressure, vec![p(4, 1.0), p(5, 0.0)]);
        assert_eq!(e.check(MAX_TICK), Ok(()));
    }

    #[test]
    fn check_refuses_unsorted_points_and_out_of_range_values() {
        let mut e = NoteExpression::new(0, 60);
        e.timbre = vec![p(10, 0.5), p(10, 0.6)];
        assert!(e.check(MAX_TICK).is_err());
        e.timbre = vec![p(10, 1.5)];
        assert!(e.check(MAX_TICK).is_err());
        e.timbre = vec![p(-1, 0.5)];
        assert!(e.check(MAX_TICK).is_err());
    }

    #[test]
    fn a_recorded_burst_keeps_its_last_value_and_repeats_are_dropped() {
        let raw = [(0, 0.1), (10, 0.2), (20, 0.3), (400, 0.3), (800, 0.6)];
        let c = record_curve(ExpressionKind::Pressure, &raw);
        assert_eq!(c, vec![p(0, 0.3), p(800, 0.6)]);
    }

    #[test]
    fn the_list_is_sorted_and_loses_empty_entries() {
        let mut a = NoteExpression::new(20, 60);
        a.pitch = vec![p(0, 1.0)];
        let mut b = NoteExpression::new(10, 64);
        b.pressure = vec![p(0, 0.5)];
        let c = NoteExpression::new(5, 50);
        let list = canonical_list(vec![a.clone(), c, b.clone()]);
        assert_eq!(list, vec![b, a]);
        assert!(find(&list, 20, 60).is_some());
        assert!(find(&list, 20, 61).is_none());
    }
}
