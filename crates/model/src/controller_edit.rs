//! Editing several controller points at once: copy, paste, delete, move and thin.
//!
//! The piano roll's controller area selects points of the lane it shows. Everything here is a
//! pure function from a clip's lane list (and the indices of the selected points of one lane)
//! to the whole, canonical lane list that goes into one `SongCommand::SetClipControllers`, so
//! an edit is one undo step and the model's checks apply as usual. Indices count the points of
//! the lane of the given kind, in tick order; the functions that move or thin points also
//! return where the selected points ended up.
//!
//! * [`ControllerSnapshot`] is the clipboard form of the copied points: their kind and the
//!   points relative to the earliest one. [`paste_points`] lays it into any lane at a tick,
//!   mapping the values to the target's range (a bend pasted into a CC keeps its position in the
//!   range), replacing the points it lands on and dropping what falls past the clip.
//! * [`remove_points`], [`move_points`] and [`thin_selection`] work on the selected points
//!   only; points outside the selection stay as they are.

use crate::automation_rec::thin_points;
use crate::controller::{ControllerKind, ControllerLane, canonical};
use crate::song::{AutomationPoint, Curve, MAX_TICK};
use std::collections::BTreeSet;

/// Controller points copied from one lane: the kind and the points, relative to the earliest.
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerSnapshot {
    /// The lane the points came from (their values are in its range).
    pub kind: ControllerKind,
    /// The points sorted by tick; `tick` is relative to the earliest copied point.
    pub points: Vec<AutomationPoint>,
    /// From the first to the last copied point (0 for a single point).
    pub span: i64,
}

impl ControllerSnapshot {
    /// Copies the points of `kind` at `indices`; `None` when the lane or every index is missing.
    pub fn capture(
        lanes: &[ControllerLane],
        kind: ControllerKind,
        indices: &[usize],
    ) -> Option<Self> {
        let lane = lanes.iter().find(|l| l.kind == kind)?;
        let chosen: BTreeSet<usize> = indices
            .iter()
            .copied()
            .filter(|i| *i < lane.points.len())
            .collect();
        let first = lane.points[*chosen.first()?].tick;
        let last = lane.points[*chosen.last()?].tick;
        Some(Self {
            kind,
            points: chosen
                .iter()
                .map(|i| AutomationPoint {
                    tick: lane.points[*i].tick - first,
                    ..lane.points[*i]
                })
                .collect(),
            span: last - first,
        })
    }
}

/// `value` of `from` expressed in the range of `to` (the same position between lowest and
/// highest), rounded to the MIDI resolution of `to`.
pub fn convert_value(from: ControllerKind, to: ControllerKind, value: f32) -> f32 {
    if from == to {
        return value;
    }
    let (flo, fhi) = from.range();
    let (tlo, thi) = to.range();
    let f = if fhi > flo {
        (value - flo) / (fhi - flo)
    } else {
        0.0
    };
    to.quantize(tlo + f.clamp(0.0, 1.0) * (thi - tlo))
}

/// The lane of `kind` of `lanes` as a mutable list, created empty when missing.
fn lane_mut(lanes: &mut Vec<ControllerLane>, kind: ControllerKind) -> &mut ControllerLane {
    match lanes.iter().position(|l| l.kind == kind) {
        Some(i) => &mut lanes[i],
        None => {
            lanes.push(ControllerLane::new(kind, Vec::new()));
            lanes.last_mut().expect("just pushed")
        }
    }
}

/// `lanes` with the points of `kind` at `indices` removed (the lane goes with its last point).
pub fn remove_points(
    lanes: &[ControllerLane],
    kind: ControllerKind,
    indices: &[usize],
) -> Vec<ControllerLane> {
    let drop: BTreeSet<usize> = indices.iter().copied().collect();
    let mut out = lanes.to_vec();
    if let Some(lane) = out.iter_mut().find(|l| l.kind == kind) {
        let mut i = 0;
        lane.points.retain(|_| {
            i += 1;
            !drop.contains(&(i - 1))
        });
    }
    canonical(out)
}

/// `lanes` with the points of `kind` at `indices` moved by `dtick` and `dvalue` together: the
/// shift is limited so that no moved point leaves `0..=MAX_TICK` or the kind's range (the group
/// keeps its shape), and a moved point replaces an unselected point at its tick. Returns the
/// lanes and the new indices of the moved points.
pub fn move_points(
    lanes: &[ControllerLane],
    kind: ControllerKind,
    indices: &[usize],
    dtick: i64,
    dvalue: f32,
) -> (Vec<ControllerLane>, Vec<usize>) {
    let Some(lane) = lanes.iter().find(|l| l.kind == kind) else {
        return (lanes.to_vec(), Vec::new());
    };
    let picked: BTreeSet<usize> = indices
        .iter()
        .copied()
        .filter(|i| *i < lane.points.len())
        .collect();
    if picked.is_empty() {
        return (lanes.to_vec(), Vec::new());
    }
    let moved: Vec<&AutomationPoint> = picked.iter().map(|i| &lane.points[*i]).collect();
    let (min_tick, max_tick) = (moved[0].tick, moved[moved.len() - 1].tick);
    let dtick = dtick.clamp(-min_tick, MAX_TICK - max_tick);
    let (lo, hi) = kind.range();
    let min_v = moved.iter().map(|p| p.value).fold(f32::INFINITY, f32::min);
    let max_v = moved
        .iter()
        .map(|p| p.value)
        .fold(f32::NEG_INFINITY, f32::max);
    let dvalue = if dvalue.is_finite() {
        dvalue.clamp(lo - min_v, hi - max_v)
    } else {
        0.0
    };
    let shifted: Vec<AutomationPoint> = moved
        .iter()
        .map(|p| AutomationPoint {
            tick: p.tick + dtick,
            value: kind.clamp(p.value + dvalue),
            curve: p.curve,
        })
        .collect();
    let ticks: BTreeSet<i64> = shifted.iter().map(|p| p.tick).collect();
    let mut rest: Vec<(AutomationPoint, bool)> = lane
        .points
        .iter()
        .enumerate()
        .filter(|(i, p)| !picked.contains(i) && !ticks.contains(&p.tick))
        .map(|(_, p)| (*p, false))
        .collect();
    rest.extend(shifted.into_iter().map(|p| (p, true)));
    rest.sort_by_key(|(p, _)| p.tick);
    let new_indices = rest
        .iter()
        .enumerate()
        .filter(|(_, (_, m))| *m)
        .map(|(i, _)| i)
        .collect();
    let mut out = lanes.to_vec();
    lane_mut(&mut out, kind).points = rest.into_iter().map(|(p, _)| p).collect();
    (canonical(out), new_indices)
}

/// `lanes` with the points of `snapshot` laid into the lane of `target` from content tick
/// `at_tick` on: values mapped to the target's range, a pasted point replaces the point at its
/// tick, points at or after `limit` (the clip's content length) or past `MAX_TICK` are
/// dropped. Returns the lanes and the indices of the pasted points, or `None` when nothing is
/// left to paste.
pub fn paste_points(
    lanes: &[ControllerLane],
    target: ControllerKind,
    at_tick: i64,
    snapshot: &ControllerSnapshot,
    limit: i64,
) -> Option<(Vec<ControllerLane>, Vec<usize>)> {
    let at = at_tick.max(0);
    let end = limit.min(MAX_TICK + 1);
    let pasted: Vec<AutomationPoint> = snapshot
        .points
        .iter()
        .map(|p| AutomationPoint {
            tick: at + p.tick,
            value: convert_value(snapshot.kind, target, p.value),
            curve: match p.curve {
                Curve::Bend(_) => Curve::Linear,
                c => c,
            },
        })
        .filter(|p| p.tick < end)
        .collect();
    if pasted.is_empty() {
        return None;
    }
    let ticks: BTreeSet<i64> = pasted.iter().map(|p| p.tick).collect();
    let mut out = lanes.to_vec();
    let lane = lane_mut(&mut out, target);
    let mut all: Vec<(AutomationPoint, bool)> = lane
        .points
        .iter()
        .filter(|p| !ticks.contains(&p.tick))
        .map(|p| (*p, false))
        .collect();
    all.extend(pasted.into_iter().map(|p| (p, true)));
    all.sort_by_key(|(p, _)| p.tick);
    let indices = all
        .iter()
        .enumerate()
        .filter(|(_, (_, new))| *new)
        .map(|(i, _)| i)
        .collect();
    lane.points = all.into_iter().map(|(p, _)| p).collect();
    Some((canonical(out), indices))
}

/// `lanes` with the selected points of `kind` thinned: every run of neighbouring selected points
/// is simplified like Cleanup Automation does (a ramp point the line through its neighbours
/// reproduces within `fraction` of the range goes; so does a held point that repeats the value
/// before it), keeping the first and the last point of a run. Points outside the selection are
/// untouched. Returns the lanes and the new indices of the surviving selected points.
pub fn thin_selection(
    lanes: &[ControllerLane],
    kind: ControllerKind,
    indices: &[usize],
    fraction: f32,
) -> (Vec<ControllerLane>, Vec<usize>) {
    let Some(lane) = lanes.iter().find(|l| l.kind == kind) else {
        return (lanes.to_vec(), Vec::new());
    };
    let picked: BTreeSet<usize> = indices
        .iter()
        .copied()
        .filter(|i| *i < lane.points.len())
        .collect();
    let (lo, hi) = kind.range();
    let tolerance = fraction * (hi - lo);
    let mut points: Vec<AutomationPoint> = Vec::with_capacity(lane.points.len());
    let mut kept: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < lane.points.len() {
        if !picked.contains(&i) {
            points.push(lane.points[i]);
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < lane.points.len() && picked.contains(&(j + 1)) {
            j += 1;
        }
        let run = thin_run(&lane.points[i..=j], tolerance);
        for p in run {
            kept.push(points.len());
            points.push(p);
        }
        i = j + 1;
    }
    let mut out = lanes.to_vec();
    lane_mut(&mut out, kind).points = points;
    (canonical(out), kept)
}

/// One run of selected points thinned (see [`thin_selection`]).
fn thin_run(run: &[AutomationPoint], tolerance: f32) -> Vec<AutomationPoint> {
    let line = thin_points(run, tolerance);
    let mut out: Vec<AutomationPoint> = Vec::with_capacity(line.len());
    for (n, p) in line.iter().enumerate() {
        let repeats = out.last().is_some_and(|prev| {
            prev.curve == Curve::Step
                && p.curve == Curve::Step
                && (p.value - prev.value).abs() <= tolerance
        });
        // The last point of the run stays: it joins the unselected points after it.
        if repeats && n + 1 < line.len() {
            continue;
        }
        out.push(*p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CC: ControllerKind = ControllerKind::Cc(1);

    fn pt(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
        AutomationPoint { tick, value, curve }
    }

    fn lane(kind: ControllerKind, points: Vec<AutomationPoint>) -> Vec<ControllerLane> {
        vec![ControllerLane::new(kind, points)]
    }

    fn ticks(lanes: &[ControllerLane], kind: ControllerKind) -> Vec<i64> {
        lanes
            .iter()
            .find(|l| l.kind == kind)
            .map(|l| l.points.iter().map(|p| p.tick).collect())
            .unwrap_or_default()
    }

    #[test]
    fn capture_is_relative_and_ignores_missing_indices() {
        let l = lane(
            CC,
            vec![
                pt(100, 0.1, Curve::Linear),
                pt(200, 0.2, Curve::Step),
                pt(500, 0.5, Curve::Linear),
            ],
        );
        let s = ControllerSnapshot::capture(&l, CC, &[2, 1, 9]).unwrap();
        assert_eq!(s.span, 300);
        assert_eq!(s.points[0], pt(0, 0.2, Curve::Step));
        assert_eq!(s.points[1].tick, 300);
        assert!(ControllerSnapshot::capture(&l, CC, &[7]).is_none());
        assert!(ControllerSnapshot::capture(&l, ControllerKind::Cc(7), &[0]).is_none());
        assert!(ControllerSnapshot::capture(&[], CC, &[0]).is_none());
    }

    #[test]
    fn remove_drops_the_selection_and_an_emptied_lane() {
        let l = lane(
            CC,
            vec![
                pt(0, 0.0, Curve::Step),
                pt(10, 0.5, Curve::Step),
                pt(20, 1.0, Curve::Step),
            ],
        );
        let r = remove_points(&l, CC, &[0, 2]);
        assert_eq!(ticks(&r, CC), [10]);
        assert!(remove_points(&l, CC, &[0, 1, 2]).is_empty());
        assert_eq!(remove_points(&l, CC, &[]), l);
    }

    #[test]
    fn a_group_move_keeps_its_shape_inside_the_limits() {
        let l = lane(
            CC,
            vec![
                pt(100, 0.2, Curve::Linear),
                pt(200, 0.9, Curve::Linear),
                pt(300, 0.5, Curve::Step),
            ],
        );
        let (m, sel) = move_points(&l, CC, &[0, 1], 50, 0.05);
        assert_eq!(ticks(&m, CC), [150, 250, 300]);
        assert_eq!(sel, [0, 1]);
        let p = &m[0].points;
        assert!((p[0].value - 0.25).abs() < 1e-6 && (p[1].value - 0.95).abs() < 1e-6);
        // Pushed up past the top: limited by the highest point, the shape is kept.
        let (m, _) = move_points(&l, CC, &[0, 1], 0, 0.5);
        let p = &m[0].points;
        assert!((p[1].value - 1.0).abs() < 1e-6);
        assert!((p[0].value - 0.3).abs() < 1e-6);
        // Pushed before the start.
        let (m, _) = move_points(&l, CC, &[0, 1], -1000, 0.0);
        assert_eq!(ticks(&m, CC), [0, 100, 300]);
        // A moved point replaces an unselected one at its tick.
        let (m, sel) = move_points(&l, CC, &[1], 100, 0.0);
        assert_eq!(ticks(&m, CC), [100, 300]);
        assert_eq!(sel, [1]);
        assert_eq!(m[0].points[1].curve, Curve::Linear, "the moved point won");
        // Nothing selected, or not a lane: unchanged.
        assert_eq!(move_points(&l, CC, &[], 5, 0.1).0, l);
        assert_eq!(move_points(&l, ControllerKind::Cc(9), &[0], 5, 0.1).0, l);
    }

    #[test]
    fn moving_pitch_bend_stays_in_range() {
        let bend = ControllerKind::PitchBend;
        let l = lane(
            bend,
            vec![pt(0, -0.5, Curve::Step), pt(10, 0.75, Curve::Step)],
        );
        let (m, _) = move_points(&l, bend, &[0, 1], 0, 5.0);
        assert!((m[0].points[1].value - 1.0).abs() < 1e-6);
        assert!((m[0].points[0].value + 0.25).abs() < 1e-6);
        crate::controller::check_lanes(&m, MAX_TICK).unwrap();
    }

    #[test]
    fn paste_lays_points_replaces_and_clips_to_the_clip() {
        let l = lane(
            CC,
            vec![pt(100, 0.1, Curve::Step), pt(900, 0.9, Curve::Step)],
        );
        let snap = ControllerSnapshot {
            kind: CC,
            points: vec![pt(0, 0.4, Curve::Linear), pt(100, 0.6, Curve::Step)],
            span: 100,
        };
        let (p, sel) = paste_points(&l, CC, 100, &snap, 1000).unwrap();
        assert_eq!(ticks(&p, CC), [100, 200, 900]);
        assert_eq!(sel, [0, 1]);
        assert_eq!(
            p[0].points[0].curve,
            Curve::Linear,
            "replaced the point at 100"
        );
        // Past the clip end: the late point is dropped, the early one stays.
        let (p, sel) = paste_points(&l, CC, 880, &snap, 950).unwrap();
        assert_eq!(ticks(&p, CC), [100, 880, 900]);
        assert_eq!(sel, [1]);
        // Nothing left inside.
        assert!(paste_points(&l, CC, 2000, &snap, 1000).is_none());
    }

    #[test]
    fn paste_maps_the_range_between_kinds_and_creates_the_lane() {
        let snap = ControllerSnapshot {
            kind: ControllerKind::PitchBend,
            points: vec![
                pt(0, -1.0, Curve::Step),
                pt(10, 0.0, Curve::Step),
                pt(20, 1.0, Curve::Step),
            ],
            span: 20,
        };
        let (p, _) = paste_points(&[], CC, 0, &snap, 100).unwrap();
        let v: Vec<f32> = p[0].points.iter().map(|q| q.value).collect();
        assert_eq!(v[0], 0.0);
        assert!((v[1] - 64.0 / 127.0).abs() < 0.01 && (v[2] - 1.0).abs() < 1e-6);
        // Into a bend lane the CC fractions land across -1..1.
        let cc = ControllerSnapshot {
            kind: CC,
            points: vec![pt(0, 0.0, Curve::Step), pt(5, 1.0, Curve::Step)],
            span: 5,
        };
        let (p, _) = paste_points(&[], ControllerKind::PitchBend, 0, &cc, 100).unwrap();
        assert_eq!(p[0].points[0].value, -1.0);
        assert_eq!(p[0].points[1].value, 1.0);
        crate::controller::check_lanes(&p, MAX_TICK).unwrap();
    }

    #[test]
    fn thinning_removes_collinear_ramp_points_and_repeated_holds() {
        let ramp: Vec<AutomationPoint> = (0..=10)
            .map(|i| pt(i * 10, i as f32 / 10.0, Curve::Linear))
            .collect();
        let l = lane(CC, ramp);
        let all: Vec<usize> = (0..=10).collect();
        let (t, sel) = thin_selection(&l, CC, &all, 0.005);
        assert_eq!(ticks(&t, CC), [0, 100]);
        assert_eq!(sel, [0, 1]);
        // Only the selected middle is thinned; the outside stays.
        let (t, sel) = thin_selection(&l, CC, &[3, 4, 5, 6, 7], 0.005);
        assert_eq!(ticks(&t, CC), [0, 10, 20, 30, 70, 80, 90, 100]);
        assert_eq!(sel, [3, 4]);
        // Held values: the repeats go, a change stays.
        let holds = lane(
            CC,
            vec![
                pt(0, 0.5, Curve::Step),
                pt(10, 0.5, Curve::Step),
                pt(20, 0.5, Curve::Step),
                pt(30, 0.8, Curve::Step),
                pt(40, 0.8, Curve::Step),
            ],
        );
        let (t, _) = thin_selection(&holds, CC, &[0, 1, 2, 3, 4], 0.005);
        assert_eq!(
            ticks(&t, CC),
            [0, 30, 40],
            "the last point of the run stays"
        );
        // A tolerance of nothing keeps a held change below it only when it is real.
        let (t, _) = thin_selection(&holds, CC, &[0, 1, 2, 3, 4], 0.0);
        assert_eq!(ticks(&t, CC), [0, 30, 40]);
    }

    #[test]
    fn heavier_thinning_drops_more() {
        let wobble: Vec<AutomationPoint> = (0..=20)
            .map(|i| {
                pt(
                    i * 10,
                    0.5 + if i % 2 == 0 { 0.01 } else { -0.01 },
                    Curve::Linear,
                )
            })
            .collect();
        let l = lane(CC, wobble);
        let all: Vec<usize> = (0..=20).collect();
        let light = thin_selection(&l, CC, &all, 0.005).0[0].points.len();
        let heavy = thin_selection(&l, CC, &all, 0.05).0[0].points.len();
        assert!(heavy < light, "{heavy} < {light}");
    }
}
