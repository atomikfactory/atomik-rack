//! The tempo lane and the meter changes of the arrangement: the pure edits of the tempo map
//! and the meter map (a command takes the whole new map) and the painting of the tempo curve.
//!
//! The lane sits between the ruler and the track lanes. It plots the BPM against the same time
//! axis: a step is a horizontal run and a jump, a ramp a straight line to the next point
//! (the tempo moves linearly in BPM over ticks). Every edit returns the whole new map; the
//! view sends it as `SetTempoMap` / `DragTempoMap` (and the meter twins), so one gesture is one
//! undo step.

use crate::canvas::{Align, Canvas, CanvasExt, pt};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::mix;
use crate::timeline::{Bars, TimeAxis};
use atomik_core::{TempoPoint, TempoTable};
use atomik_model::{MAX_TICK, MeterEvent, TEMPO_MAX, TEMPO_MIN, TempoEvent};
use vello::kurbo::{BezPath, Circle, Point, Rect};

/// Height of the tempo lane.
pub const TEMPO_LANE_HEIGHT: f64 = 76.0;
/// Space above and below the plotted range.
const PAD: f64 = 9.0;
/// Radius of a point's hit area.
pub const POINT_HIT: f64 = 7.0;
/// Radius of a drawn point.
const POINT_R: f64 = 3.6;
/// Highest bar a meter change may take (keeps its tick inside the song limits).
const MAX_BAR: u32 = 100_000;

/// The meters of the insert menu.
pub const COMMON_METERS: [(u8, u8); 7] = [(2, 4), (3, 4), (4, 4), (5, 4), (6, 8), (7, 8), (12, 8)];

// ---------------------------------------------------------------------------- scale

/// The BPM range the lane shows for a tempo map: the map's range with 40 BPM of room, in
/// steps of 10 and at least 80 BPM tall, inside the tempo limits.
pub fn bpm_range(events: &[TempoEvent]) -> (f64, f64) {
    let lo = events.iter().map(|e| e.bpm).fold(f64::INFINITY, f64::min);
    let hi = events
        .iter()
        .map(|e| e.bpm)
        .fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() || !hi.is_finite() {
        return (80.0, 160.0);
    }
    let mut a = (((lo - 40.0) / 10.0).floor() * 10.0).max(TEMPO_MIN);
    let mut b = (((hi + 40.0) / 10.0).ceil() * 10.0).min(TEMPO_MAX);
    if b - a < 80.0 {
        let grow = 80.0 - (b - a);
        a = (a - grow / 2.0).max(TEMPO_MIN);
        b = (a + 80.0).max(b).min(TEMPO_MAX);
        a = a.min(b - 80.0).max(TEMPO_MIN);
    }
    (a, b)
}

/// The y of a BPM in `lane` for the range `range`.
pub fn bpm_to_y(bpm: f64, lane: Rect, range: (f64, f64)) -> f64 {
    let h = (lane.height() - 2.0 * PAD).max(1.0);
    lane.y1 - PAD - (bpm - range.0) / (range.1 - range.0) * h
}

/// The BPM at a y in `lane` (the inverse of [`bpm_to_y`], clamped to the range).
pub fn y_to_bpm(y: f64, lane: Rect, range: (f64, f64)) -> f64 {
    let h = (lane.height() - 2.0 * PAD).max(1.0);
    let bpm = range.0 + (lane.y1 - PAD - y) / h * (range.1 - range.0);
    bpm.clamp(range.0.max(TEMPO_MIN), range.1.min(TEMPO_MAX))
}

/// A BPM rounded to a whole number, or to a tenth with `fine`.
pub fn round_bpm(bpm: f64, fine: bool) -> f64 {
    let r = if fine { 10.0 } else { 1.0 };
    ((bpm * r).round() / r).clamp(TEMPO_MIN, TEMPO_MAX)
}

/// The position of an event's point in the lane.
pub fn point_pos(e: &TempoEvent, lane: Rect, axis: &TimeAxis, range: (f64, f64)) -> Point {
    Point::new(axis.tick_to_x(e.tick), bpm_to_y(e.bpm, lane, range))
}

/// The tempo at `tick` through a map (a ramp gives the tempo inside it).
pub fn tempo_at(map: &[TempoEvent], tick: i64) -> f64 {
    let points: Vec<TempoPoint> = map
        .iter()
        .map(|e| TempoPoint {
            tick: e.tick,
            bpm: e.bpm,
            ramp: e.ramp,
        })
        .collect();
    TempoTable::new(&points).map_or_else(
        |_| map.first().map_or(120.0, |e| e.bpm),
        |t| t.map().bpm(tick as f64),
    )
}

// ---------------------------------------------------------------------------- tempo edits

/// The map with the last event's ramp cleared (a ramp needs a next event).
fn normalized(mut map: Vec<TempoEvent>) -> Vec<TempoEvent> {
    if let Some(last) = map.last_mut() {
        last.ramp = false;
    }
    map
}

/// The map with a new event at `tick` (after tick 0, no event there yet) and its index. The
/// event inside a ramp splits it: the ramp before the new point and, if the old segment ramped,
/// a ramp after it too.
pub fn add_point(map: &[TempoEvent], tick: i64, bpm: f64) -> Option<(Vec<TempoEvent>, usize)> {
    if tick <= 0 || tick > MAX_TICK || map.iter().any(|e| e.tick == tick) {
        return None;
    }
    let at = map.partition_point(|e| e.tick < tick);
    let ramp = at > 0 && map[at - 1].ramp;
    let mut out = map.to_vec();
    out.insert(
        at,
        TempoEvent {
            tick,
            bpm: bpm.clamp(TEMPO_MIN, TEMPO_MAX),
            ramp,
        },
    );
    Some((normalized(out), at))
}

/// The map with event `i` moved to `tick` and `bpm`. The first event stays at tick 0; the others
/// stay between their neighbours.
pub fn move_point(map: &[TempoEvent], i: usize, tick: i64, bpm: f64) -> Vec<TempoEvent> {
    let mut out = map.to_vec();
    if let Some(e) = out.get_mut(i) {
        e.bpm = bpm.clamp(TEMPO_MIN, TEMPO_MAX);
        if i > 0 {
            let lo = map[i - 1].tick + 1;
            let hi = map.get(i + 1).map_or(MAX_TICK, |n| n.tick - 1);
            e.tick = tick.clamp(lo, hi.max(lo));
        }
    }
    out
}

/// The map without event `i`; the first event cannot go.
pub fn delete_point(map: &[TempoEvent], i: usize) -> Option<Vec<TempoEvent>> {
    if i == 0 || i >= map.len() {
        return None;
    }
    let mut out = map.to_vec();
    out.remove(i);
    Some(normalized(out))
}

/// The map with the ramp of event `i` flipped; the last event has no next event to ramp to.
pub fn toggle_ramp(map: &[TempoEvent], i: usize) -> Option<Vec<TempoEvent>> {
    if i + 1 >= map.len() {
        return None;
    }
    let mut out = map.to_vec();
    out[i].ramp = !out[i].ramp;
    Some(out)
}

// ---------------------------------------------------------------------------- meter edits

/// The map with `num/den` from `bar`: a new change, or the signature of the change already
/// there. `None` for a bar outside the supported range.
pub fn insert_meter(map: &[MeterEvent], bar: u32, num: u8, den: u8) -> Option<Vec<MeterEvent>> {
    if bar == 0 || bar > MAX_BAR || !atomik_core::meter_ok(num, den) {
        return None;
    }
    let mut out = map.to_vec();
    match out.binary_search_by_key(&bar, |e| e.bar) {
        Ok(i) => (out[i].num, out[i].den) = (num, den),
        Err(i) => out.insert(i, MeterEvent { bar, num, den }),
    }
    Some(out)
}

/// The map with the signature of change `i` replaced.
pub fn set_meter(map: &[MeterEvent], i: usize, num: u8, den: u8) -> Option<Vec<MeterEvent>> {
    if i >= map.len() || !atomik_core::meter_ok(num, den) {
        return None;
    }
    let mut out = map.to_vec();
    (out[i].num, out[i].den) = (num, den);
    Some(out)
}

/// The map with change `i` moved to `bar`, between its neighbours; the first change stays at
/// bar 1.
pub fn move_meter(map: &[MeterEvent], i: usize, bar: u32) -> Vec<MeterEvent> {
    let mut out = map.to_vec();
    if i > 0
        && let Some(e) = out.get_mut(i)
    {
        let lo = map[i - 1].bar + 1;
        let hi = map.get(i + 1).map_or(MAX_BAR, |n| n.bar - 1);
        e.bar = bar.clamp(lo, hi.max(lo));
    }
    out
}

/// The map without change `i`; the first change cannot go.
pub fn delete_meter(map: &[MeterEvent], i: usize) -> Option<Vec<MeterEvent>> {
    if i == 0 || i >= map.len() {
        return None;
    }
    let mut out = map.to_vec();
    out.remove(i);
    Some(out)
}

// ---------------------------------------------------------------------------- painting

/// What the lane paints besides the map.
pub struct LanePaint<'a> {
    /// The tempo map.
    pub events: &'a [TempoEvent],
    /// The BPM range of the scale.
    pub range: (f64, f64),
    /// The selected event.
    pub selected: Option<usize>,
    /// The event under the pointer.
    pub hover: Option<usize>,
    /// The meter map of the grid lines.
    pub bars: &'a Bars,
    /// The playhead tick and its colour.
    pub playhead: (i64, vello::peniko::Color),
}

/// A scale step (10, 20, 50 or 100 BPM...) whose lines are at least 16 px apart.
fn scale_step(lane: Rect, range: (f64, f64)) -> f64 {
    let per_bpm = (lane.height() - 2.0 * PAD) / (range.1 - range.0);
    [10.0, 20.0, 50.0, 100.0, 200.0]
        .into_iter()
        .find(|s| s * per_bpm >= 16.0)
        .unwrap_or(200.0)
}

/// Paints the lane over `lane` (clipped to the timeline by the caller).
pub fn paint<C: Canvas + ?Sized>(
    c: &mut C,
    st: &SeqTheme,
    axis: &TimeAxis,
    lane: Rect,
    p: &LanePaint<'_>,
) {
    let t = &st.base;
    sp::rect(c, lane, st.lane_auto);
    sp::grid(c, st, axis, lane, p.bars);
    // The BPM scale.
    let step = scale_step(lane, p.range);
    let mut v = (p.range.0 / step).ceil() * step;
    while v <= p.range.1 {
        let y = bpm_to_y(v, lane, p.range).round() + 0.5;
        sp::line(c, lane.x0, y, lane.x1, y, st.grid_beat);
        sp::label(
            c,
            &format!("{v:.0}"),
            lane.x0 + 4.0,
            y - 2.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
        v += step;
    }
    // The curve: steps run flat and jump, a ramp is the straight line to the next point.
    let colour = t.highlight;
    if let Some(first) = p.events.first() {
        let pos = |e: &TempoEvent| point_pos(e, lane, axis, p.range);
        let mut path = BezPath::new();
        let p0 = pos(first);
        path.move_to(p0);
        for (i, e) in p.events.iter().enumerate() {
            let here = pos(e);
            match p.events.get(i + 1) {
                Some(next) if e.ramp => path.line_to(pos(next)),
                Some(next) => {
                    path.line_to(Point::new(pos(next).x, here.y));
                    path.line_to(pos(next));
                }
                None => path.line_to(Point::new(lane.x1 + 4.0, here.y)),
            }
        }
        let mut area = path.clone();
        let last_y = pos(p.events.last().unwrap_or(first)).y;
        area.line_to(Point::new(lane.x1 + 4.0, last_y));
        area.line_to(Point::new(lane.x1 + 4.0, lane.y1));
        area.line_to(Point::new(p0.x, lane.y1));
        area.close_path();
        c.fill_with(area, colour.with_alpha(0.1));
        c.stroke_with(path, 1.6, colour);
        for (i, e) in p.events.iter().enumerate() {
            let at = pos(e);
            if at.x < lane.x0 - 8.0 || at.x > lane.x1 + 8.0 {
                continue;
            }
            let lit = p.selected == Some(i) || p.hover == Some(i);
            let fill = if p.selected == Some(i) {
                st.selection
            } else {
                t.text
            };
            c.fill_with(Circle::new(at, POINT_R + if lit { 1.2 } else { 0.0 }), fill);
            c.stroke_with(
                Circle::new(at, POINT_R + 1.2),
                1.0,
                mix(colour, t.background, 0.4),
            );
            if lit {
                let (above, x) = (at.y - 7.0, at.x.clamp(lane.x0 + 14.0, lane.x1 - 14.0));
                let y = if above < lane.y0 + 9.0 {
                    at.y + 14.0
                } else {
                    above
                };
                sp::label(
                    c,
                    &format!("{:.2}", e.bpm),
                    x,
                    y,
                    9.0,
                    t.text,
                    Align::Center,
                );
            }
        }
    }
    let x = axis.tick_to_x(p.playhead.0);
    if x >= lane.x0 - 1.0 && x <= lane.x1 + 1.0 {
        let x = x.round() + 0.5;
        c.stroke_with(
            vello::kurbo::Line::new(pt(x, lane.y0), pt(x, lane.y1)),
            1.5,
            p.playhead.1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::PPQ;

    const BAR: i64 = 4 * PPQ;

    fn step(tick: i64, bpm: f64) -> TempoEvent {
        TempoEvent::step(tick, bpm)
    }

    fn ramp(tick: i64, bpm: f64) -> TempoEvent {
        TempoEvent {
            tick,
            bpm,
            ramp: true,
        }
    }

    #[test]
    fn the_range_has_room_and_stays_inside_the_limits() {
        assert_eq!(bpm_range(&[step(0, 120.0)]), (80.0, 160.0));
        assert_eq!(
            bpm_range(&[step(0, 100.0), step(BAR, 180.0)]),
            (60.0, 220.0)
        );
        let (a, b) = bpm_range(&[step(0, 20.0)]);
        assert_eq!((a, b), (20.0, 100.0));
        let (a, b) = bpm_range(&[step(0, 999.0)]);
        assert!(
            a >= 20.0 && (b - 999.0).abs() < 1e-9 && b - a >= 80.0,
            "{a} {b}"
        );
        assert_eq!(bpm_range(&[]), (80.0, 160.0));
    }

    #[test]
    fn y_and_bpm_invert_each_other_inside_the_range() {
        let lane = Rect::new(0.0, 100.0, 400.0, 100.0 + TEMPO_LANE_HEIGHT);
        let range = (80.0, 160.0);
        for bpm in [80.0, 100.0, 120.5, 160.0] {
            let y = bpm_to_y(bpm, lane, range);
            assert!((y_to_bpm(y, lane, range) - bpm).abs() < 1e-9);
        }
        assert!(bpm_to_y(160.0, lane, range) < bpm_to_y(80.0, lane, range));
        assert_eq!(y_to_bpm(0.0, lane, range), 160.0, "above the lane clamps");
        assert_eq!(y_to_bpm(1e6, lane, range), 80.0);
        assert_eq!(round_bpm(120.46, false), 120.0);
        assert_eq!(round_bpm(120.46, true), 120.5);
        assert_eq!(round_bpm(3.0, false), 20.0);
    }

    #[test]
    fn adding_a_point_splits_a_ramp_and_refuses_tick_zero_and_duplicates() {
        let map = [ramp(0, 60.0), step(4 * BAR, 180.0)];
        let (out, i) = add_point(&map, BAR, 90.0).unwrap();
        assert_eq!(i, 1);
        assert_eq!(
            out,
            vec![ramp(0, 60.0), ramp(BAR, 90.0), step(4 * BAR, 180.0)]
        );
        let (out, i) = add_point(&map, 5 * BAR, 100.0).unwrap();
        assert_eq!((i, out.len()), (2, 3));
        assert!(!out[2].ramp, "the last event never ramps");
        assert!(add_point(&map, 0, 90.0).is_none());
        assert!(add_point(&map, 4 * BAR, 90.0).is_none());
        assert_eq!(add_point(&map, BAR, 5000.0).unwrap().0[1].bpm, 999.0);
    }

    #[test]
    fn moving_a_point_stays_between_its_neighbours() {
        let map = [step(0, 100.0), step(BAR, 120.0), step(3 * BAR, 140.0)];
        assert_eq!(move_point(&map, 1, 2 * BAR, 130.0)[1], step(2 * BAR, 130.0));
        assert_eq!(move_point(&map, 1, -50, 130.0)[1].tick, 1);
        assert_eq!(move_point(&map, 1, 9 * BAR, 130.0)[1].tick, 3 * BAR - 1);
        assert_eq!(
            move_point(&map, 0, 500, 90.0)[0],
            step(0, 90.0),
            "the first is pinned"
        );
        assert_eq!(
            move_point(&map, 2, 99 * BAR, 5000.0)[2],
            step(99 * BAR, 999.0)
        );
        assert_eq!(move_point(&map, 7, 5, 100.0), map.to_vec());
    }

    #[test]
    fn deleting_and_ramping_keep_the_map_valid() {
        let map = [ramp(0, 100.0), ramp(BAR, 120.0), step(2 * BAR, 90.0)];
        assert_eq!(delete_point(&map, 0), None);
        assert_eq!(delete_point(&map, 3), None);
        assert_eq!(
            delete_point(&map, 1),
            Some(vec![ramp(0, 100.0), step(2 * BAR, 90.0)])
        );
        // Deleting the last event leaves a ramp that has nowhere to go: it is cleared.
        assert_eq!(
            delete_point(&map, 2),
            Some(vec![ramp(0, 100.0), step(BAR, 120.0)])
        );
        assert_eq!(toggle_ramp(&map, 2), None);
        assert_eq!(toggle_ramp(&map, 1).unwrap()[1], step(BAR, 120.0));
        assert_eq!(toggle_ramp(&[step(0, 120.0)], 0), None);
        assert!(!toggle_ramp(&map, 0).unwrap()[0].ramp);
    }

    #[test]
    fn the_tempo_at_a_tick_follows_a_ramp() {
        let map = [ramp(0, 60.0), step(2 * BAR, 180.0)];
        assert_eq!(tempo_at(&map, 0), 60.0);
        assert!((tempo_at(&map, BAR) - 120.0).abs() < 1e-9);
        assert_eq!(tempo_at(&map, 5 * BAR), 180.0);
        assert_eq!(tempo_at(&[], 5), 120.0);
    }

    fn mev(bar: u32, num: u8, den: u8) -> MeterEvent {
        MeterEvent { bar, num, den }
    }

    #[test]
    fn meter_changes_insert_replace_move_and_delete() {
        let map = [mev(1, 4, 4)];
        let two = insert_meter(&map, 5, 3, 4).unwrap();
        assert_eq!(two, vec![mev(1, 4, 4), mev(5, 3, 4)]);
        let three = insert_meter(&two, 3, 6, 8).unwrap();
        assert_eq!(three, vec![mev(1, 4, 4), mev(3, 6, 8), mev(5, 3, 4)]);
        assert_eq!(insert_meter(&three, 5, 7, 8).unwrap()[2], mev(5, 7, 8));
        assert_eq!(insert_meter(&three, 1, 5, 4).unwrap()[0], mev(1, 5, 4));
        assert!(insert_meter(&three, 0, 4, 4).is_none());
        assert!(insert_meter(&three, 4, 4, 3).is_none());
        assert!(insert_meter(&three, 200_000, 4, 4).is_none());
        assert_eq!(set_meter(&three, 1, 2, 4).unwrap()[1], mev(3, 2, 4));
        assert!(set_meter(&three, 9, 2, 4).is_none());
        assert_eq!(move_meter(&three, 1, 4)[1].bar, 4);
        assert_eq!(
            move_meter(&three, 1, 1)[1].bar,
            2,
            "after the previous change"
        );
        assert_eq!(move_meter(&three, 1, 9)[1].bar, 4, "before the next change");
        assert_eq!(move_meter(&three, 0, 9), three.to_vec(), "bar 1 is pinned");
        assert_eq!(move_meter(&three, 2, 99)[2].bar, 99);
        assert_eq!(delete_meter(&three, 0), None);
        assert_eq!(
            delete_meter(&three, 1),
            Some(vec![mev(1, 4, 4), mev(5, 3, 4)])
        );
    }
}
