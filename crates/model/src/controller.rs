//! MIDI controller data in note clips: pitch bend, aftertouch and controller (CC) lanes.
//!
//! A note clip may carry [`ControllerLane`]s next to its notes (`Clip::controllers`). A lane
//! holds the points of one [`ControllerKind`], in the clip's content ticks, with the same
//! [`AutomationPoint`] shape as an automation lane (`tick`, `value`, `Linear` or `Step`
//! towards the next point). Values are normalised like [`NoteKind`]: pitch bend `-1..=1` (0 is
//! the centre), everything else `0..=1`. The lanes are sorted by kind and unique per kind, so a
//! clip's controller data is canonical.
//!
//! The model keeps the lanes as drawn or recorded; the song flattening
//! ([`crate::song::flatten_controllers`]) turns them into the discrete events the engine plays.
//! Pure data and arithmetic: no engine and no UI.

use crate::song::{AutomationPoint, Curve};
use atomik_core::NoteKind;

/// Which MIDI controller a lane carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ControllerKind {
    /// Channel pitch bend, `-1..=1`.
    PitchBend,
    /// Channel (mono) aftertouch, `0..=1`.
    ChannelPressure,
    /// A continuous controller, `0..=127`, value `0..=1`.
    Cc(u8),
    /// Polyphonic aftertouch of one key, `0..=127`, value `0..=1`.
    PolyPressure(u8),
}

/// The CC numbers the lane picker names (the rest are offered as "CC n").
pub const COMMON_CCS: [u8; 6] = [1, 7, 10, 11, 64, 74];

impl ControllerKind {
    /// The lowest and highest value of the kind.
    pub fn range(self) -> (f32, f32) {
        match self {
            ControllerKind::PitchBend => (-1.0, 1.0),
            _ => (0.0, 1.0),
        }
    }

    /// The value at rest: the centre for pitch bend, zero for the others. Sent when playback
    /// stops so a bend or a held pedal does not stay on.
    pub fn rest(self) -> f32 {
        0.0
    }

    /// True when a stopped or relocated transport returns the controller to its rest value:
    /// pitch bend, both aftertouches and the sustain pedal (CC 64). Other controllers (volume,
    /// expression, a filter cutoff) keep the last value they were given, as on a hardware synth.
    pub fn resets(self) -> bool {
        match self {
            ControllerKind::Cc(n) => n == 64,
            _ => true,
        }
    }

    /// `value` clamped to the kind's range (NaN becomes the rest value).
    pub fn clamp(self, value: f32) -> f32 {
        let (lo, hi) = self.range();
        if value.is_nan() {
            self.rest()
        } else {
            value.clamp(lo, hi)
        }
    }

    /// The note event that sets the controller to `value` (clamped).
    pub fn note_kind(self, value: f32) -> NoteKind {
        let value = self.clamp(value);
        match self {
            ControllerKind::PitchBend => NoteKind::PitchBend { value },
            ControllerKind::ChannelPressure => NoteKind::ChannelPressure { value },
            ControllerKind::Cc(controller) => NoteKind::Cc { controller, value },
            ControllerKind::PolyPressure(key) => NoteKind::PolyPressure { key, value },
        }
    }

    /// The kind and value of a live controller event; `None` for notes, `AllOff` and
    /// per-note expressions.
    pub fn from_note(kind: NoteKind) -> Option<(ControllerKind, f32)> {
        match kind {
            NoteKind::PitchBend { value } => Some((ControllerKind::PitchBend, value)),
            NoteKind::ChannelPressure { value } => Some((ControllerKind::ChannelPressure, value)),
            NoteKind::Cc { controller, value } => Some((ControllerKind::Cc(controller), value)),
            NoteKind::PolyPressure { key, value } => {
                Some((ControllerKind::PolyPressure(key), value))
            }
            _ => None,
        }
    }

    /// A label for menus and lane headers: "Pitch Bend", "Aftertouch", "Mod Wheel (CC1)", ...
    pub fn label(self) -> String {
        match self {
            ControllerKind::PitchBend => "Pitch Bend".to_owned(),
            ControllerKind::ChannelPressure => "Aftertouch".to_owned(),
            ControllerKind::Cc(n) => match cc_name(n) {
                Some(name) => format!("{name} (CC{n})"),
                None => format!("CC {n}"),
            },
            ControllerKind::PolyPressure(key) => format!("Poly Aftertouch (key {key})"),
        }
    }
}

/// The conventional name of a controller number, for the ones a lane picker offers.
pub fn cc_name(n: u8) -> Option<&'static str> {
    Some(match n {
        1 => "Mod Wheel",
        7 => "Volume",
        10 => "Pan",
        11 => "Expression",
        64 => "Sustain",
        74 => "Brightness",
        _ => return None,
    })
}

/// One lane of controller points in a note clip (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerLane {
    /// What the lane controls.
    pub kind: ControllerKind,
    /// Points in content ticks, strictly increasing, values inside the kind's range. Never
    /// empty in a stored clip (an empty lane is dropped).
    pub points: Vec<AutomationPoint>,
}

impl ControllerLane {
    /// A lane of `kind` with `points`.
    pub fn new(kind: ControllerKind, points: Vec<AutomationPoint>) -> Self {
        Self { kind, points }
    }

    /// The value at content tick `tick` and the shape of the curve leaving it: nothing before the
    /// first point, the previous point's value (held, or ramping to the next one) after.
    pub fn value_at(&self, tick: i64) -> Option<(f32, Curve)> {
        value_at(&self.points, tick)
    }
}

/// The value of sorted `points` at `tick` (see [`ControllerLane::value_at`]).
pub(crate) fn value_at(points: &[AutomationPoint], tick: i64) -> Option<(f32, Curve)> {
    let i = points.partition_point(|p| p.tick <= tick);
    let p = points[..i].last()?;
    match points.get(i) {
        Some(n) if p.curve == Curve::Linear => {
            let v0 = f64::from(p.value);
            let frac = (tick - p.tick) as f64 / (n.tick - p.tick) as f64;
            Some((
                (v0 + (f64::from(n.value) - v0) * frac) as f32,
                Curve::Linear,
            ))
        }
        _ => Some((p.value, Curve::Step)),
    }
}

/// Checks a clip's controller lanes: kinds unique and sorted, CC and key numbers `0..=127`,
/// points sorted by tick without duplicates, ticks in `0..=max_tick`, values finite and inside
/// the kind's range. `Err` names the first fault.
pub(crate) fn check_lanes(lanes: &[ControllerLane], max_tick: i64) -> Result<(), String> {
    for (i, lane) in lanes.iter().enumerate() {
        let n = match lane.kind {
            ControllerKind::Cc(n) | ControllerKind::PolyPressure(n) => Some(n),
            _ => None,
        };
        if n.is_some_and(|n| n > 127) {
            return Err(format!("controller lane {i}: number beyond 127"));
        }
        if i > 0 && lanes[i - 1].kind >= lane.kind {
            return Err(format!(
                "controller lanes must be sorted by kind without duplicates (lane {i})"
            ));
        }
        if lane.points.is_empty() {
            return Err(format!("controller lane {i} has no points"));
        }
        let (lo, hi) = lane.kind.range();
        for (j, p) in lane.points.iter().enumerate() {
            if !(0..=max_tick).contains(&p.tick) {
                return Err(format!(
                    "controller lane {i} point {j}: tick {} is outside 0..={max_tick}",
                    p.tick
                ));
            }
            if !(lo..=hi).contains(&p.value) {
                return Err(format!(
                    "controller lane {i} point {j}: value {} is outside {lo}..={hi}",
                    p.value
                ));
            }
            if j > 0 && lane.points[j - 1].tick >= p.tick {
                return Err(format!(
                    "controller lane {i}: points must be sorted by tick without duplicates (point {j})"
                ));
            }
        }
    }
    Ok(())
}

/// The part of `lane` inside the content window `[from, to)` as points relative to `from`,
/// moved by `shift`: the points inside, a point at the window start carrying the value there
/// when an earlier point gave the lane one, and a closing point on the last tick when a ramp is
/// still running at the end (so the cut window plays like the window of the whole lane).
pub(crate) fn window_points(
    lane: &ControllerLane,
    from: i64,
    to: i64,
    shift: i64,
) -> Vec<AutomationPoint> {
    let mut out = Vec::new();
    if let Some((value, curve)) = value_at(&lane.points, from) {
        out.push(AutomationPoint {
            tick: from,
            value,
            curve,
        });
    }
    let first = lane.points.partition_point(|p| p.tick <= from);
    out.extend(
        lane.points[first..]
            .iter()
            .take_while(|p| p.tick < to)
            .copied(),
    );
    if let Some(last) = out.last().copied()
        && last.curve == Curve::Linear
    {
        match value_at(&lane.points, to - 1) {
            Some((value, _)) if last.tick < to - 1 => out.push(AutomationPoint {
                tick: to - 1,
                value,
                curve: Curve::Step,
            }),
            _ => out.last_mut().expect("non-empty").curve = Curve::Step,
        }
    }
    for p in &mut out {
        p.tick += shift - from;
    }
    out
}

/// The lanes in canonical form: sorted by kind, lanes with no points dropped. Used by the
/// callers that build lanes (the recorder, the editors) before they send them.
pub fn canonical(mut lanes: Vec<ControllerLane>) -> Vec<ControllerLane> {
    lanes.retain(|l| !l.points.is_empty());
    lanes.sort_by_key(|l| l.kind);
    lanes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
        AutomationPoint { tick, value, curve }
    }

    #[test]
    fn kinds_sort_bend_pressure_cc_poly() {
        let mut v = vec![
            ControllerKind::PolyPressure(3),
            ControllerKind::Cc(64),
            ControllerKind::Cc(1),
            ControllerKind::ChannelPressure,
            ControllerKind::PitchBend,
        ];
        v.sort();
        assert_eq!(
            v,
            [
                ControllerKind::PitchBend,
                ControllerKind::ChannelPressure,
                ControllerKind::Cc(1),
                ControllerKind::Cc(64),
                ControllerKind::PolyPressure(3),
            ]
        );
    }

    #[test]
    fn note_kind_round_trips_and_clamps() {
        for k in [
            ControllerKind::PitchBend,
            ControllerKind::ChannelPressure,
            ControllerKind::Cc(11),
            ControllerKind::PolyPressure(60),
        ] {
            let v = if k == ControllerKind::PitchBend {
                -0.5
            } else {
                0.25
            };
            assert_eq!(ControllerKind::from_note(k.note_kind(v)), Some((k, v)));
        }
        assert_eq!(
            ControllerKind::PitchBend.note_kind(3.0),
            NoteKind::PitchBend { value: 1.0 }
        );
        assert_eq!(
            ControllerKind::Cc(1).note_kind(-1.0),
            NoteKind::Cc {
                controller: 1,
                value: 0.0
            }
        );
        assert_eq!(ControllerKind::from_note(NoteKind::AllOff), None);
        assert_eq!(
            ControllerKind::from_note(NoteKind::On {
                key: 60,
                velocity: 1.0
            }),
            None
        );
    }

    #[test]
    fn value_at_is_nothing_before_the_first_point_then_held_or_ramped() {
        let lane = ControllerLane::new(
            ControllerKind::Cc(1),
            vec![
                pt(100, 0.0, Curve::Linear),
                pt(200, 1.0, Curve::Step),
                pt(300, 0.5, Curve::Step),
            ],
        );
        assert_eq!(lane.value_at(99), None);
        assert_eq!(lane.value_at(100), Some((0.0, Curve::Linear)));
        assert_eq!(lane.value_at(150), Some((0.5, Curve::Linear)));
        assert_eq!(lane.value_at(200), Some((1.0, Curve::Step)));
        assert_eq!(lane.value_at(299), Some((1.0, Curve::Step)));
        assert_eq!(lane.value_at(9999), Some((0.5, Curve::Step)));
    }

    #[test]
    fn check_lanes_rejects_every_fault() {
        let ok = |kind, points| ControllerLane::new(kind, points);
        let good = ok(ControllerKind::PitchBend, vec![pt(0, -1.0, Curve::Step)]);
        assert!(check_lanes(std::slice::from_ref(&good), 1000).is_ok());
        let cases: Vec<Vec<ControllerLane>> = vec![
            vec![ok(ControllerKind::Cc(1), vec![])],
            vec![ok(ControllerKind::Cc(200), vec![pt(0, 0.0, Curve::Step)])],
            vec![ok(ControllerKind::Cc(1), vec![pt(-1, 0.0, Curve::Step)])],
            vec![ok(ControllerKind::Cc(1), vec![pt(2000, 0.0, Curve::Step)])],
            vec![ok(ControllerKind::Cc(1), vec![pt(0, -0.1, Curve::Step)])],
            vec![ok(
                ControllerKind::Cc(1),
                vec![pt(0, f32::NAN, Curve::Step)],
            )],
            vec![ok(ControllerKind::PitchBend, vec![pt(0, 1.5, Curve::Step)])],
            vec![ok(
                ControllerKind::Cc(1),
                vec![pt(5, 0.0, Curve::Step), pt(5, 1.0, Curve::Step)],
            )],
            vec![
                ok(ControllerKind::Cc(2), vec![pt(0, 0.0, Curve::Step)]),
                ok(ControllerKind::Cc(1), vec![pt(0, 0.0, Curve::Step)]),
            ],
            vec![
                ok(ControllerKind::Cc(1), vec![pt(0, 0.0, Curve::Step)]),
                ok(ControllerKind::Cc(1), vec![pt(0, 0.0, Curve::Step)]),
            ],
        ];
        for (i, c) in cases.iter().enumerate() {
            assert!(check_lanes(c, 1000).is_err(), "case {i} should fail");
        }
    }

    #[test]
    fn canonical_sorts_and_drops_empty_lanes() {
        let lanes = canonical(vec![
            ControllerLane::new(ControllerKind::Cc(7), vec![pt(0, 1.0, Curve::Step)]),
            ControllerLane::new(ControllerKind::Cc(1), vec![]),
            ControllerLane::new(ControllerKind::PitchBend, vec![pt(0, 0.0, Curve::Step)]),
        ]);
        let kinds: Vec<_> = lanes.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [ControllerKind::PitchBend, ControllerKind::Cc(7)]);
    }

    #[test]
    fn window_points_cut_a_ramp_at_both_ends() {
        let lane = ControllerLane::new(
            ControllerKind::Cc(1),
            vec![pt(0, 0.0, Curve::Linear), pt(100, 1.0, Curve::Step)],
        );
        let w = window_points(&lane, 25, 75, 1000);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0], pt(1000, 0.25, Curve::Linear));
        assert_eq!((w[1].tick, w[1].curve), (1049, Curve::Step));
        assert!((w[1].value - 0.74).abs() < 1e-6);
        // Before the first point there is no value to carry.
        let early = ControllerLane::new(ControllerKind::Cc(1), vec![pt(50, 1.0, Curve::Step)]);
        assert_eq!(window_points(&early, 0, 100, 0), [pt(50, 1.0, Curve::Step)]);
        assert!(window_points(&early, 0, 40, 0).is_empty());
    }

    #[test]
    fn labels_name_the_common_controllers() {
        assert_eq!(ControllerKind::Cc(1).label(), "Mod Wheel (CC1)");
        assert_eq!(ControllerKind::Cc(64).label(), "Sustain (CC64)");
        assert_eq!(ControllerKind::Cc(99).label(), "CC 99");
        assert_eq!(ControllerKind::ChannelPressure.label(), "Aftertouch");
    }

    #[test]
    fn only_bend_pressure_and_sustain_reset() {
        assert!(ControllerKind::PitchBend.resets());
        assert!(ControllerKind::ChannelPressure.resets());
        assert!(ControllerKind::PolyPressure(60).resets());
        assert!(ControllerKind::Cc(64).resets());
        assert!(!ControllerKind::Cc(7).resets());
        assert!(!ControllerKind::Cc(1).resets());
    }
}
