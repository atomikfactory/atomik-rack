//! An automation lane on a param a macro drives: the lane sets the base, the macro adds its range.
//!
//! A param with an assignment and a lane of its own plays
//! `clamp(lane(t) + (max - min) * share(macro(t)))`: at macro 0 (macro 1 when inverted) the lane
//! sounds as drawn, and the macro lifts it by up to the range of the assignment. The macro is the
//! bank's stored value, or its own lane when it has one; the result is clamped to the param's
//! range and rounded for a stepped param like any macro value.
//!
//! Both sides are flattened to song ticks and joined at the points of either one. Between two
//! points the sum of two straight (or held) segments is straight, so that case is exact; a bent
//! segment is cut into [`BEND_PARTS`] straight pieces. A jump (a held lane's next point, a step
//! of the macro) stays a jump: the segment before it ends one tick earlier.

use crate::expand::macro_value;
use crate::macros::{MacroAssign, assign_share, macro_key};
use crate::song::{effective_tracks, flatten_automation};
use crate::{Curve, DeviceKey, LaneKind, RackDoc};
use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;

/// A flattened lane: `(song tick, value, curve)` with strictly increasing ticks.
type Points = [(i64, f32, Curve)];

/// How many straight pieces a bent segment is cut into.
pub const BEND_PARTS: i64 = 8;

/// The value a lane value `base` plays with macro value `m` added by assignment `a`.
pub fn lane_value(a: &MacroAssign, base: f32, m: f32, p: &ParamDescriptor) -> f32 {
    macro_value(base, base + (a.max - a.min), assign_share(a, m), p)
}

/// The value of `points` at `tick`; `left` takes the limit from the left (what a segment ends on
/// when the next point jumps). Before the first point the first value holds.
fn eval(points: &Points, tick: i64, left: bool) -> f32 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    let at = if left {
        points.partition_point(|p| p.0 < tick)
    } else {
        points.partition_point(|p| p.0 <= tick)
    };
    let Some(i) = at.checked_sub(1) else {
        return first.1;
    };
    let (t0, v0, curve) = points[i];
    match points.get(i + 1) {
        Some(&(t1, v1, _)) if curve.ramps() && t1 > t0 => {
            let x = ((tick - t0) as f64 / (t1 - t0) as f64).clamp(0.0, 1.0);
            curve.blend(f64::from(v0), f64::from(v1), x) as f32
        }
        _ => v0,
    }
}

/// True when a segment of `points` that starts at or before `tick` is bent.
fn bent_at(points: &Points, tick: i64) -> bool {
    let i = points.partition_point(|p| p.0 <= tick);
    i.checked_sub(1)
        .is_some_and(|i| matches!(points[i].2, Curve::Bend(_)) && i + 1 < points.len())
}

/// The points `base` plays as under assignment `a` (see the module documentation). `macro_points`
/// is the macro's own lane (empty: the macro holds `macro_at`).
pub fn compose(
    a: &MacroAssign,
    p: &ParamDescriptor,
    base: &Points,
    macro_at: f32,
    macro_points: &Points,
) -> Vec<(i64, f32, Curve)> {
    if base.is_empty() {
        return Vec::new();
    }
    if macro_points.is_empty() {
        return base
            .iter()
            .map(|&(t, v, c)| (t, lane_value(a, v, macro_at, p), c))
            .collect();
    }
    let mut ticks: Vec<i64> = base.iter().chain(macro_points).map(|x| x.0).collect();
    ticks.sort_unstable();
    ticks.dedup();
    // A bent segment gets extra points so the straight pieces follow it.
    let mut cuts: Vec<i64> = Vec::new();
    for pair in ticks.windows(2) {
        let (t0, t1) = (pair[0], pair[1]);
        cuts.push(t0);
        if (bent_at(base, t0) || bent_at(macro_points, t0)) && t1 - t0 > BEND_PARTS {
            for k in 1..BEND_PARTS {
                cuts.push(t0 + (t1 - t0) * k / BEND_PARTS);
            }
        }
    }
    cuts.extend(ticks.last().copied());
    let sum = |tick: i64, left: bool| {
        lane_value(a, eval(base, tick, left), eval(macro_points, tick, left), p)
    };
    let mut out: Vec<(i64, f32, Curve)> = Vec::new();
    for (i, &t) in cuts.iter().enumerate() {
        let here = sum(t, false);
        let Some(&next) = cuts.get(i + 1) else {
            out.push((t, here, Curve::Step));
            break;
        };
        let end = sum(next, true);
        if end.to_bits() == here.to_bits() {
            out.push((t, here, Curve::Step));
            continue;
        }
        out.push((t, here, Curve::Linear));
        // The next point jumps (a held value changes there): end the ramp one tick early.
        if sum(next, false).to_bits() != end.to_bits() && next - 1 > t {
            out.push((next - 1, end, Curve::Step));
        }
    }
    out
}

impl RackDoc {
    /// The playing points of the lane of `device` on `param`, when it has clips: what the engine
    /// plays for the lane, before any macro is added.
    pub fn own_lane_points(
        &self,
        device: &DeviceKey,
        param: &str,
    ) -> Option<Vec<(i64, f32, Curve)>> {
        let track = effective_tracks(&self.song)
            .into_iter()
            .find(|t| &t.device == device && t.frozen.is_none())?;
        let i = track.automation_lane(param)?;
        let points = flatten_automation(track, i);
        (!points.is_empty()).then_some(points)
    }

    /// The points a lane on a param a macro drives plays as, or `None` when no live assignment
    /// drives the param or the lane has no points (see the module documentation).
    pub fn driven_lane_points(
        &self,
        registry: &Registry,
        device: &DeviceKey,
        param: &str,
    ) -> Option<Vec<(i64, f32, Curve)>> {
        let a = self.bank_driver(device, param)?;
        let base = self.own_lane_points(device, param)?;
        let p = registry
            .get(&self.resolve(device)?.type_id)?
            .descriptor
            .param_by_key(param)?;
        let macro_points = self
            .own_lane_points(&a.bank, &macro_key(a.index))
            .unwrap_or_default();
        let at = self.bank_macro_value(&a.bank, a.index);
        Some(compose(a, p, &base, at, &macro_points))
    }

    /// True when the lane of `device` on `param` has clips that play (so a macro lane for the
    /// param is folded into it, see [`RackDoc::driven_lane_points`]).
    pub fn has_own_lane_points(&self, device: &DeviceKey, param: &str) -> bool {
        self.own_lane_points(device, param).is_some()
    }

    /// True when some assignment of `bank` drives a param whose own lane plays (a move of the
    /// bank's macro then changes the song the engine plays).
    pub fn bank_shapes_lanes(&self, bank: &DeviceKey) -> bool {
        self.macros.assigns.iter().any(|a| {
            &a.bank == bank
                && self.macro_assign_live(a)
                && self.song.track(&a.device).is_some_and(|t| {
                    t.lanes.iter().any(|l| {
                        matches!(&l.kind, LaneKind::Automation(k) if *k == a.param)
                            && !l.clips.is_empty()
                    })
                })
        })
    }
}
