//! Controller data of a song track: pitch bend, aftertouch and CC lanes that the song player
//! sends to the track's device next to its notes.
//!
//! [`SongController`] is the engine-native input (a kind and points in song ticks, the same
//! points as an automation lane). [`expand`] (non-RT, in `compile_song`) turns a lane into
//! discrete events: every point at its own tick with its exact value, and for a `Linear` point
//! the values on the way to the next one every [`RAMP_STEP`] ticks, rounded to the kind's MIDI
//! resolution (14 bit for pitch bend, 7 bit for the others) and left out when the rounded value
//! did not change. The events join the track's note events (sorted by tick, controllers after
//! note-offs and before note-ons at the same tick), so the RT player only walks an index and
//! pushes events, as it does for notes. The same expansion gives the chase index
//! ([`CtlLane`]): the value a controller has at any position, so a seek can restore it.

use crate::seq::{SongCurve, SongPoint};
use atomik_core::{NoteKind, PPQ};

/// Ticks between the intermediate values of a ramp: 40 ticks, about a millisecond at 120 BPM.
pub const RAMP_STEP: i64 = PPQ / 384;

/// Which MIDI controller a [`SongController`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SongControllerKind {
    /// Channel pitch bend, values `-1..=1`.
    PitchBend,
    /// Channel aftertouch, `0..=1`.
    ChannelPressure,
    /// Controller number `0..=127`, `0..=1`.
    Cc(u8),
    /// Polyphonic aftertouch of key `0..=127`, `0..=1`.
    PolyPressure(u8),
}

impl SongControllerKind {
    /// The event that sets the controller to `value`.
    pub fn note(self, value: f32) -> NoteKind {
        match self {
            Self::PitchBend => NoteKind::PitchBend { value },
            Self::ChannelPressure => NoteKind::ChannelPressure { value },
            Self::Cc(controller) => NoteKind::Cc { controller, value },
            Self::PolyPressure(key) => NoteKind::PolyPressure { key, value },
        }
    }

    /// The lowest and highest value.
    fn range(self) -> (f32, f32) {
        match self {
            Self::PitchBend => (-1.0, 1.0),
            _ => (0.0, 1.0),
        }
    }

    /// Steps per unit of value: the MIDI resolution (pitch bend 14 bit, the rest 7 bit).
    fn steps(self) -> f32 {
        match self {
            Self::PitchBend => 8192.0,
            _ => 127.0,
        }
    }

    /// True when stop, a locate and a position before the lane's first point return the
    /// controller to zero: pitch bend, both aftertouches and the sustain pedal (CC 64). The
    /// other controllers keep the last value they were given.
    pub(crate) fn resets(self) -> bool {
        match self {
            Self::Cc(n) => n == 64,
            _ => true,
        }
    }

    fn round(self, v: f32) -> f32 {
        let (lo, hi) = self.range();
        ((v * self.steps()).round() / self.steps()).clamp(lo, hi)
    }
}

/// One controller lane of a track: a kind and its points in song ticks (any order within equal
/// ticks is kept: a jump).
#[derive(Debug, Clone, PartialEq)]
pub struct SongController {
    /// What the lane controls; at most one lane per kind in a track.
    pub kind: SongControllerKind,
    /// Points; `Linear` ramps to the next point, `Step` holds. Before the first point the lane
    /// sends nothing, after the last it holds.
    pub points: Vec<SongPoint>,
}

/// The values a controller takes over time, for chase: `ticks` ascending, `values` alongside.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CtlLane {
    pub(crate) kind: SongControllerKind,
    pub(crate) ticks: Vec<i64>,
    pub(crate) values: Vec<f32>,
}

/// Checks a lane's points: ticks at or above zero, finite values inside the kind's range.
pub(crate) fn validate(c: &SongController) -> Result<(), String> {
    let (lo, hi) = c.kind.range();
    match c
        .points
        .iter()
        .find(|p| p.tick.0 < 0 || !(lo..=hi).contains(&p.value))
    {
        Some(p) => Err(format!("controller point {p:?} for {:?}", c.kind)),
        None => Ok(()),
    }
}

/// Expands a validated lane into `(tick, value)` events in time order (module docs).
pub(crate) fn expand(c: &SongController) -> CtlLane {
    let mut points = c.points.clone();
    points.sort_by_key(|p| p.tick); // stable: equal ticks keep their order
    let mut lane = CtlLane {
        kind: c.kind,
        ticks: Vec::with_capacity(points.len()),
        values: Vec::with_capacity(points.len()),
    };
    let mut push = |tick: i64, value: f32| {
        if lane.values.last() != Some(&value) {
            lane.ticks.push(tick);
            lane.values.push(value);
        }
    };
    for (i, p) in points.iter().enumerate() {
        push(p.tick.0, p.value);
        let Some(next) = points.get(i + 1) else {
            continue;
        };
        if p.curve != SongCurve::Linear || next.tick.0 <= p.tick.0 {
            continue;
        }
        let span = (next.tick.0 - p.tick.0) as f64;
        let mut t = p.tick.0 + RAMP_STEP;
        while t < next.tick.0 {
            let x = (t - p.tick.0) as f64 / span;
            let v = f64::from(p.value) + (f64::from(next.value) - f64::from(p.value)) * x;
            push(t, c.kind.round(v as f32));
            t += RAMP_STEP;
        }
    }
    lane
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::Ticks;

    fn pt(tick: i64, value: f32, curve: SongCurve) -> SongPoint {
        SongPoint {
            tick: Ticks(tick),
            value,
            curve,
        }
    }

    fn lane(kind: SongControllerKind, points: Vec<SongPoint>) -> SongController {
        SongController { kind, points }
    }

    #[test]
    fn steps_emit_each_point_once_and_skip_repeats() {
        let l = expand(&lane(
            SongControllerKind::Cc(1),
            vec![
                pt(100, 0.5, SongCurve::Step),
                pt(200, 0.5, SongCurve::Step),
                pt(300, 1.0, SongCurve::Step),
            ],
        ));
        assert_eq!(l.ticks, [100, 300]);
        assert_eq!(l.values, [0.5, 1.0]);
    }

    #[test]
    fn a_ramp_steps_through_the_midi_resolution_and_ends_on_the_point() {
        let l = expand(&lane(
            SongControllerKind::Cc(7),
            vec![pt(0, 0.0, SongCurve::Linear), pt(PPQ, 1.0, SongCurve::Step)],
        ));
        // 127 levels at most (the 0 start included), ascending, one per change.
        assert!(
            l.values.len() <= 128 && l.values.len() > 100,
            "{}",
            l.values.len()
        );
        assert_eq!((l.ticks[0], l.values[0]), (0, 0.0));
        assert!(*l.ticks.last().unwrap() <= PPQ);
        assert_eq!(*l.values.last().unwrap(), 1.0);
        assert!(l.values.windows(2).all(|w| w[0] < w[1]));
        assert!(l.ticks.windows(2).all(|w| w[0] < w[1]));
        // Every intermediate value is a 7-bit level.
        for v in &l.values[1..l.values.len() - 1] {
            assert!(((v * 127.0).round() - v * 127.0).abs() < 1e-3, "{v}");
        }
    }

    #[test]
    fn pitch_bend_ramps_in_fourteen_bits_and_never_leaves_the_range() {
        let l = expand(&lane(
            SongControllerKind::PitchBend,
            vec![
                pt(0, -1.0, SongCurve::Linear),
                pt(PPQ, 1.0, SongCurve::Step),
            ],
        ));
        assert!(l.values.iter().all(|v| (-1.0..=1.0).contains(v)));
        // One value per ramp step (the ramp changes faster than the resolution).
        assert_eq!(l.values.len() as i64, PPQ / RAMP_STEP + 1);
    }

    #[test]
    fn equal_ticks_keep_their_order_and_a_zero_span_ramp_is_a_jump() {
        let l = expand(&lane(
            SongControllerKind::PitchBend,
            vec![pt(10, 0.0, SongCurve::Linear), pt(10, 0.5, SongCurve::Step)],
        ));
        assert_eq!(
            (l.ticks.clone(), l.values.clone()),
            (vec![10, 10], vec![0.0, 0.5])
        );
    }

    #[test]
    fn validation_rejects_bad_ticks_and_values() {
        let bad = |kind, p| validate(&lane(kind, vec![p])).is_err();
        assert!(bad(SongControllerKind::Cc(1), pt(-1, 0.0, SongCurve::Step)));
        assert!(bad(SongControllerKind::Cc(1), pt(0, 1.1, SongCurve::Step)));
        assert!(bad(
            SongControllerKind::Cc(1),
            pt(0, f32::NAN, SongCurve::Step)
        ));
        assert!(bad(
            SongControllerKind::PitchBend,
            pt(0, -1.1, SongCurve::Step)
        ));
        assert!(!bad(
            SongControllerKind::PitchBend,
            pt(0, -1.0, SongCurve::Step)
        ));
    }

    #[test]
    fn kinds_map_to_note_events_and_only_some_reset() {
        assert_eq!(
            SongControllerKind::Cc(64).note(1.0),
            NoteKind::Cc {
                controller: 64,
                value: 1.0
            }
        );
        assert_eq!(
            SongControllerKind::PolyPressure(60).note(0.5),
            NoteKind::PolyPressure {
                key: 60,
                value: 0.5
            }
        );
        assert!(SongControllerKind::Cc(64).resets());
        assert!(!SongControllerKind::Cc(1).resets());
        assert!(SongControllerKind::PitchBend.resets());
    }
}
