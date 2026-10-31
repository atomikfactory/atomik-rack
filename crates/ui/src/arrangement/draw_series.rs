//! Drawing automation with the pencil: a drag over an automation clip writes a series of points,
//! one per snap step, thinned as it goes so a long stroke does not leave hundreds of points.
//!
//! * Plain drag: the value follows the pointer (a point per step, a pointer that moves faster than
//!   a step is joined by a straight line, so no step is skipped).
//! * The constrain role (Shift): the points lie exactly on the straight line from where the
//!   stroke began to the pointer.
//! * The flat-range role: the value where the stroke began is held up to the pointer, as two
//!   points.
//!
//! The series replaces the points of the clip inside the range it covers; points outside stay.
//! Everything here is pure, the gesture plumbing is in [`super`].

use crate::timeline::{PPQ, Quant, SnapGrid};
use atomik_core::ParamDescriptor;
use atomik_model::automation_rec::thin_points;
use atomik_model::{AutomationPoint, ClipId, Curve};
use std::collections::BTreeMap;

/// How a stroke turns the pointer's path into points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesMode {
    /// The value follows the pointer.
    Freehand,
    /// A straight line from the press to the pointer.
    Line,
    /// The value of the press, held up to the pointer.
    Flat,
}

/// The snap steps from `from` to `to` inclusive, ascending (`from <= to`); both ends are always
/// there. With snapping off a sixty-fourth note is the step, so a stroke stays bounded.
pub fn steps_between(grid: &SnapGrid, from: i64, to: i64) -> Vec<i64> {
    let mut out = vec![from];
    let mut t = from;
    while t < to {
        let next = if grid.unit().is_some() {
            grid.ceil_to(t + 1)
        } else {
            t + PPQ / 16
        };
        if next <= t {
            break;
        }
        t = next.min(to);
        out.push(t);
    }
    out
}

fn lerp(a: (i64, f32), b: (i64, f32), tick: i64) -> f32 {
    if b.0 == a.0 {
        return b.1;
    }
    let f = (tick - a.0) as f64 / (b.0 - a.0) as f64;
    (f64::from(a.1) + (f64::from(b.1) - f64::from(a.1)) * f) as f32
}

/// The points a stroke writes, before thinning: `path` is the pointer's samples, each a snapped
/// content tick and a value, the first one where the stroke began.
pub fn stroke(path: &[(i64, f32)], mode: SeriesMode, grid: &SnapGrid) -> Vec<(i64, f32)> {
    let Some((&first, &last)) = path.first().zip(path.last()) else {
        return Vec::new();
    };
    let mut out: BTreeMap<i64, f32> = BTreeMap::new();
    let span = |a: (i64, f32), b: (i64, f32), out: &mut BTreeMap<i64, f32>| {
        let (lo, hi) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        for t in steps_between(grid, lo.0, hi.0) {
            out.insert(t, lerp(lo, hi, t));
        }
    };
    match mode {
        SeriesMode::Freehand => {
            out.insert(first.0, first.1);
            for w in path.windows(2) {
                span(w[0], w[1], &mut out);
            }
        }
        SeriesMode::Line => span(first, last, &mut out),
        SeriesMode::Flat => {
            out.insert(first.0, first.1);
            out.insert(last.0, first.1);
        }
    }
    out.into_iter().collect()
}

/// `base` with the stroke laid over it: the points of `base` inside the stroke's range go, the
/// stroke's points (thinned with `tolerance`, in value units) take their place.
pub fn lay_over(
    base: &[AutomationPoint],
    drawn: &[(i64, f32)],
    tolerance: f32,
) -> Vec<AutomationPoint> {
    let (Some(lo), Some(hi)) = (drawn.first().map(|p| p.0), drawn.last().map(|p| p.0)) else {
        return base.to_vec();
    };
    let points: Vec<AutomationPoint> = drawn
        .iter()
        .map(|&(tick, value)| AutomationPoint {
            tick,
            value,
            curve: Curve::Linear,
        })
        .collect();
    let mut out: Vec<AutomationPoint> = base
        .iter()
        .filter(|p| p.tick < lo || p.tick > hi)
        .copied()
        .collect();
    out.extend(thin_points(&points, tolerance));
    out.sort_by_key(|p| p.tick);
    out
}

/// The state of a stroke in progress.
#[derive(Debug, Clone)]
pub struct SeriesDrag {
    /// The clip.
    pub id: ClipId,
    /// The drag-merge id (the stroke is one undo step).
    pub merge: u64,
    /// The points of the clip when the stroke began (what Escape restores).
    pub base: Vec<AutomationPoint>,
    /// The pointer's samples: content tick and value.
    pub path: Vec<(i64, f32)>,
    /// Song tick minus content tick of the window the stroke is in.
    pub shift: i64,
    /// The lane's value range on screen: the top (the maximum) and the height in pixels.
    pub top: f64,
    /// The height of the value range on screen, in pixels.
    pub height: f64,
    /// The parameter the lane drives.
    pub param: ParamDescriptor,
    /// The content ticks the stroke may reach.
    pub window: (i64, i64),
    /// The thinning tolerance in value units.
    pub tolerance: f32,
    /// The points last sent.
    pub last: Vec<AutomationPoint>,
}

impl SeriesDrag {
    /// Adds a pointer sample (a tick and value; the tick is clamped to the window) and returns
    /// the clip's new points when they differ from the last ones sent.
    pub fn sample(
        &mut self,
        tick: i64,
        value: f32,
        mode: SeriesMode,
        grid: &SnapGrid,
    ) -> Option<Vec<AutomationPoint>> {
        let tick = tick.clamp(self.window.0, self.window.1.max(self.window.0));
        let several = self.path.len() > 1;
        match self.path.last_mut() {
            Some(last) if last.0 == tick && several => last.1 = value,
            _ => self.path.push((tick, value)),
        }
        let points = lay_over(&self.base, &stroke(&self.path, mode, grid), self.tolerance);
        (points != self.last).then(|| {
            self.last.clone_from(&points);
            points
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Bars, Snap};

    fn grid() -> SnapGrid {
        SnapGrid::new(Snap::Sixteenth, Bars::default())
    }

    fn p(tick: i64, value: f32) -> AutomationPoint {
        AutomationPoint {
            tick,
            value,
            curve: Curve::Linear,
        }
    }

    #[test]
    fn steps_are_the_snap_steps_with_both_ends() {
        let g = grid();
        let s = steps_between(&g, 0, 4 * PPQ);
        assert_eq!(s.len(), 17, "four beats of sixteenths and the end");
        assert_eq!(s[1], PPQ / 4);
        assert_eq!(steps_between(&g, 100, 100), [100]);
        // Off the grid the ends stay and the step is bounded.
        let off = SnapGrid::off();
        let s = steps_between(&off, 0, PPQ);
        assert_eq!((s[0], *s.last().unwrap()), (0, PPQ));
        assert_eq!(s.len(), 17);
    }

    #[test]
    fn a_fast_stroke_is_joined_by_a_line_so_no_step_is_skipped() {
        let g = grid();
        let drawn = stroke(&[(0, 0.0), (4 * PPQ, 1.0)], SeriesMode::Freehand, &g);
        assert_eq!(drawn.len(), 17);
        assert!((drawn[8].1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn shift_puts_every_point_exactly_on_the_line_from_the_press() {
        let g = grid();
        // The hand wandered, the line is from the press to where the pointer is now.
        let path = [(0, 0.2), (PPQ, 0.9), (2 * PPQ, 0.1), (4 * PPQ, 0.6)];
        let drawn = stroke(&path, SeriesMode::Line, &g);
        for (t, v) in &drawn {
            let want = 0.2 + 0.4 * (*t as f32 / (4 * PPQ) as f32);
            assert!((v - want).abs() < 1e-5, "{t}: {v} vs {want}");
        }
        assert_eq!((drawn[0].0, drawn.last().unwrap().0), (0, 4 * PPQ));
    }

    #[test]
    fn the_flat_range_holds_the_pressed_value_as_two_points() {
        let g = grid();
        let drawn = stroke(
            &[(PPQ, 0.3), (2 * PPQ, 0.9), (3 * PPQ, 0.8)],
            SeriesMode::Flat,
            &g,
        );
        assert_eq!(drawn, [(PPQ, 0.3), (3 * PPQ, 0.3)]);
        // Going backwards covers the same span.
        let back = stroke(&[(3 * PPQ, 0.3), (PPQ, 0.9)], SeriesMode::Flat, &g);
        assert_eq!(back, [(PPQ, 0.3), (3 * PPQ, 0.3)]);
    }

    #[test]
    fn a_stroke_replaces_what_it_covers_and_keeps_the_rest_sorted() {
        let base = [p(0, 0.5), p(PPQ, 0.5), p(2 * PPQ, 0.5), p(8 * PPQ, 0.5)];
        let drawn = [(PPQ, 0.1), (2 * PPQ, 0.2), (3 * PPQ, 0.3)];
        let out = lay_over(&base, &drawn, 0.0);
        let ticks: Vec<i64> = out.iter().map(|p| p.tick).collect();
        assert_eq!(ticks, [0, PPQ, 2 * PPQ, 3 * PPQ, 8 * PPQ]);
        assert!(
            (out[1].value - 0.1).abs() < 1e-6,
            "the old point at the start is replaced"
        );
        assert_eq!(lay_over(&base, &[], 0.1), base);
    }

    #[test]
    fn thinning_drops_the_points_a_line_reproduces() {
        let drawn: Vec<(i64, f32)> = (0..=64).map(|i| (i * PPQ / 4, i as f32 / 64.0)).collect();
        assert_eq!(lay_over(&[], &drawn, 0.0).len(), 65);
        assert_eq!(lay_over(&[], &drawn, 0.005).len(), 2);
    }

    #[test]
    fn samples_accumulate_and_report_only_changes() {
        let g = grid();
        let mut s = SeriesDrag {
            id: ClipId(1),
            merge: 1,
            base: vec![p(0, 0.5)],
            shift: 0,
            top: 0.0,
            height: 100.0,
            param: crate::seq_paint::FALLBACK_PARAM,
            path: vec![(PPQ, 0.4)],
            window: (0, 4 * PPQ),
            tolerance: 0.0,
            last: vec![p(0, 0.5)],
        };
        let a = s.sample(2 * PPQ, 0.6, SeriesMode::Freehand, &g).unwrap();
        assert_eq!(a.first().unwrap().tick, 0);
        assert!(a.len() > 3);
        // The same sample again changes nothing.
        assert!(s.sample(2 * PPQ, 0.6, SeriesMode::Freehand, &g).is_none());
        // A sample beyond the window stops at the window.
        let b = s.sample(99 * PPQ, 0.6, SeriesMode::Freehand, &g).unwrap();
        assert_eq!(b.last().unwrap().tick, 4 * PPQ);
    }
}
