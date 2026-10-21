//! Pure editing helpers for the sequencer views: clip content windows (loop
//! repeats), hit testing of spans and points, drag maths, rubber-band selection, the
//! selection model, note normalization, quantize, and automation values.

use crate::timeline::{snap_ceil, snap_floor, snap_round};
use atomik_core::ParamDescriptor;
use atomik_model::{AutomationPoint, Clip, Curve, Note};
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect};

// ---------------------------------------------------------------- clip content windows

/// A stretch of a clip in which song tick `t` shows content tick
/// `content_start + (t − song_start)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentWindow {
    /// First song tick of the window.
    pub song_start: i64,
    /// End song tick (exclusive).
    pub song_end: i64,
    /// Content tick shown at `song_start`.
    pub content_start: i64,
}

/// The windows of a clip: one window without a loop; with a loop of `n` ticks, the content
/// range `[0, n)` repeats, starting at `offset mod n`. Windows after the first start a loop
/// repeat.
pub fn content_windows(clip: &Clip) -> Vec<ContentWindow> {
    let end = clip.start + clip.length;
    match clip.loop_length {
        Some(n) if n > 0 => {
            let mut out = Vec::new();
            let mut t = clip.start;
            let mut c = clip.offset.rem_euclid(n);
            while t < end {
                let e = (t + n - c).min(end);
                out.push(ContentWindow {
                    song_start: t,
                    song_end: e,
                    content_start: c,
                });
                t = e;
                c = 0;
            }
            out
        }
        _ => vec![ContentWindow {
            song_start: clip.start,
            song_end: end,
            content_start: clip.offset,
        }],
    }
}

/// The content tick shown at song tick `tick`, if the tick is inside the clip.
pub fn song_to_content(clip: &Clip, tick: i64) -> Option<i64> {
    content_windows(clip)
        .into_iter()
        .find(|w| (w.song_start..w.song_end).contains(&tick))
        .map(|w| w.content_start + tick - w.song_start)
}

// ---------------------------------------------------------------- hit testing

/// Which part of a horizontal span was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanPart {
    /// The body (move).
    Body,
    /// The right edge (resize).
    End,
}

/// Hit-tests `x` against the span `[x0, x1)`. The right edge zone is `edge_px` wide inside
/// the span, shrunk to a third of the span for narrow spans so the body stays grabbable.
pub fn hit_span(x: f64, x0: f64, x1: f64, edge_px: f64) -> Option<SpanPart> {
    if !(x0..x1).contains(&x) {
        return None;
    }
    let edge = edge_px.min((x1 - x0) / 3.0);
    Some(if x >= x1 - edge {
        SpanPart::End
    } else {
        SpanPart::Body
    })
}

/// The point nearest to `pos` within `radius` (the last one wins ties, as it is drawn on top).
pub fn nearest_point(points: &[Point], pos: Point, radius: f64) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in points.iter().enumerate() {
        let d = p.distance(pos);
        if d <= radius && best.is_none_or(|(_, b)| d <= b) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Indices of the items whose rectangles intersect the rubber band (`a`–`b`, any corners).
pub fn rubber_band(a: Point, b: Point, items: &[Rect]) -> BTreeSet<usize> {
    let band = Rect::from_points(a, b);
    items
        .iter()
        .enumerate()
        .filter(|(_, r)| r.x0 <= band.x1 && r.x1 >= band.x0 && r.y0 <= band.y1 && r.y1 >= band.y0)
        .map(|(i, _)| i)
        .collect()
}

// ---------------------------------------------------------------- drag maths

/// New start for a body drag: the original start plus the pointer delta, rounded to the
/// grid and never before 0.
pub fn drag_start(orig_start: i64, delta: i64, step: Option<i64>) -> i64 {
    snap_round(orig_start + delta, step).max(0)
}

/// New length for a right-edge drag: the original end plus the delta, rounded to the grid,
/// and at least `min_len`.
pub fn drag_length(start: i64, orig_end: i64, delta: i64, step: Option<i64>, min_len: i64) -> i64 {
    (snap_round(orig_end + delta, step) - start).max(min_len)
}

/// A drawn span from a press at `t0` to the pointer at `t1` (either order): the start
/// floors to the grid, the end ceils, and the span is at least one step (or `min_len`).
pub fn draw_span(t0: i64, t1: i64, step: Option<i64>, min_len: i64) -> (i64, i64) {
    let (a, b) = (t0.min(t1).max(0), t0.max(t1).max(0));
    let start = snap_floor(a, step);
    let end = snap_ceil(b, step);
    (start, (end - start).max(min_length(step, min_len)))
}

/// Minimum length of a clip or note for a snap step: the step, or `fallback` when off.
pub fn min_length(step: Option<i64>, fallback: i64) -> i64 {
    step.unwrap_or(fallback).max(1)
}

// ---------------------------------------------------------------- selection

/// An ordered set of selected items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection<T: Ord> {
    items: BTreeSet<T>,
}

impl<T: Ord> Default for Selection<T> {
    fn default() -> Self {
        Self {
            items: BTreeSet::new(),
        }
    }
}

impl<T: Ord + Clone> Selection<T> {
    /// An empty selection.
    pub fn new() -> Self {
        Self::default()
    }
    /// Whether `item` is selected.
    pub fn contains(&self, item: &T) -> bool {
        self.items.contains(item)
    }
    /// Number of selected items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// The selected items in order.
    pub fn items(&self) -> &BTreeSet<T> {
        &self.items
    }
    /// Clears the selection.
    pub fn clear(&mut self) {
        self.items.clear();
    }
    /// Selects only `item`.
    pub fn select_only(&mut self, item: T) {
        self.items.clear();
        self.items.insert(item);
    }
    /// Adds `item`.
    pub fn add(&mut self, item: T) {
        self.items.insert(item);
    }
    /// Toggles `item`.
    pub fn toggle(&mut self, item: T) {
        if !self.items.remove(&item) {
            self.items.insert(item);
        }
    }
    /// Replaces the selection with `items`.
    pub fn set(&mut self, items: impl IntoIterator<Item = T>) {
        self.items = items.into_iter().collect();
    }
    /// Keeps only the items for which `keep` is true.
    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        self.items.retain(keep);
    }
    /// A click on `item`: Ctrl/Cmd toggles it, Shift adds it, a plain click on an unselected
    /// item selects only it, and a plain click on a selected item keeps the selection (so a
    /// group can be dragged).
    pub fn click(&mut self, item: T, toggle: bool, add: bool) {
        if toggle {
            self.toggle(item);
        } else if add {
            self.add(item);
        } else if !self.contains(&item) {
            self.select_only(item);
        }
    }
}

// ---------------------------------------------------------------- notes

/// Highest MIDI key.
pub const MAX_KEY: u8 = 127;
/// Smallest velocity (1/127).
pub const MIN_VELOCITY: f32 = 1.0 / 127.0;

/// Makes a note valid: key 0..=127, tick ≥ 0, length ≥ 1, velocity in (0, 1] (NaN → 0.8).
pub fn clamp_note(n: Note) -> Note {
    let velocity = if n.velocity.is_nan() {
        0.8
    } else {
        n.velocity.clamp(MIN_VELOCITY, 1.0)
    };
    Note {
        tick: n.tick.max(0),
        length: n.length.max(1),
        key: n.key.min(MAX_KEY),
        velocity,
        ..n
    }
}

/// Clamps every note and sorts by (tick, key, length), keeping track of where the notes at
/// `tracked` indices end up.
pub fn normalize_notes(
    notes: Vec<Note>,
    tracked: &BTreeSet<usize>,
) -> (Vec<Note>, BTreeSet<usize>) {
    let mut tagged: Vec<(Note, bool)> = notes
        .into_iter()
        .enumerate()
        .map(|(i, n)| (clamp_note(n), tracked.contains(&i)))
        .collect();
    tagged.sort_by_key(|(n, _)| (n.tick, n.key, n.length));
    let sel = tagged
        .iter()
        .enumerate()
        .filter(|(_, (_, t))| *t)
        .map(|(i, _)| i)
        .collect();
    (tagged.into_iter().map(|(n, _)| n).collect(), sel)
}

fn map_selected(notes: &[Note], sel: &BTreeSet<usize>, f: impl Fn(&Note) -> Note) -> Vec<Note> {
    notes
        .iter()
        .enumerate()
        .map(|(i, n)| if sel.contains(&i) { f(n) } else { *n })
        .collect()
}

/// Moves the `sel` notes by `dt` ticks and `dkey` keys. Both deltas are limited so every
/// moved note stays at tick ≥ 0 and key 0..=127 (the group keeps its shape).
pub fn move_notes(notes: &[Note], sel: &BTreeSet<usize>, dt: i64, dkey: i32) -> Vec<Note> {
    let moved = || {
        notes
            .iter()
            .enumerate()
            .filter(|(i, _)| sel.contains(i))
            .map(|(_, n)| n)
    };
    let min_tick = moved().map(|n| n.tick).min().unwrap_or(0);
    let min_key = moved().map(|n| i32::from(n.key)).min().unwrap_or(0);
    let max_key = moved().map(|n| i32::from(n.key)).max().unwrap_or(0);
    let dt = dt.max(-min_tick);
    let dkey = dkey.clamp(-min_key, i32::from(MAX_KEY) - max_key);
    map_selected(notes, sel, |n| Note {
        tick: n.tick + dt,
        key: (i32::from(n.key) + dkey).clamp(0, i32::from(MAX_KEY)) as u8,
        ..*n
    })
}

/// Changes the length of the `sel` notes by `dlen` ticks, each at least `min_len`.
pub fn resize_notes(notes: &[Note], sel: &BTreeSet<usize>, dlen: i64, min_len: i64) -> Vec<Note> {
    map_selected(notes, sel, |n| Note {
        length: (n.length + dlen).max(min_len.max(1)),
        ..*n
    })
}

/// Moves the starts of the `sel` notes towards the grid by `strength` (0..=1, 1 = fully on
/// the grid), keeping lengths. With no `step` the notes are unchanged.
pub fn quantize_notes(
    notes: &[Note],
    sel: &BTreeSet<usize>,
    step: Option<i64>,
    strength: f32,
) -> Vec<Note> {
    if step.is_none() {
        return notes.to_vec();
    }
    let strength = f64::from(strength.clamp(0.0, 1.0));
    map_selected(notes, sel, |n| {
        let q = snap_round(n.tick, step);
        let t = n.tick as f64 + (q - n.tick) as f64 * strength;
        Note {
            tick: (t.round() as i64).max(0),
            ..*n
        }
    })
}

/// Sets the velocity of the `sel` notes by adding `delta` to each original velocity (clamped).
pub fn shift_velocity(notes: &[Note], sel: &BTreeSet<usize>, delta: f32) -> Vec<Note> {
    map_selected(notes, sel, |n| Note {
        velocity: (n.velocity + delta).clamp(MIN_VELOCITY, 1.0),
        ..*n
    })
}

/// A velocity from a y position in a lane `[top, top + height]` (top = 1, bottom = min).
pub fn velocity_from_y(y: f64, top: f64, height: f64) -> f32 {
    let v = 1.0 - (y - top) / height.max(1e-9);
    (v as f32).clamp(MIN_VELOCITY, 1.0)
}

// ---------------------------------------------------------------- automation

/// The normalized 0..=1 position of a plain value for drawing (taper-aware).
pub fn value_to_norm(p: &ParamDescriptor, value: f32) -> f32 {
    p.to_normalized(value)
}

/// A plain value from a y position in a lane `[top, top + height]` (top = max), clamped and
/// rounded by the descriptor.
pub fn value_from_y(p: &ParamDescriptor, y: f64, top: f64, height: f64) -> f32 {
    let n = 1.0 - (y - top) / height.max(1e-9);
    p.from_normalized(n.clamp(0.0, 1.0) as f32)
}

/// The automation value at a content tick: before the first point it is the first value,
/// after the last the last value, `Linear` segments interpolate and `Step` holds. `None`
/// when there are no points.
pub fn automation_value_at(points: &[AutomationPoint], tick: i64) -> Option<f32> {
    let first = points.first()?;
    if tick <= first.tick {
        return Some(first.value);
    }
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if tick < b.tick {
            return Some(match a.curve {
                Curve::Step => a.value,
                Curve::Linear => {
                    let t = (tick - a.tick) as f64 / (b.tick - a.tick).max(1) as f64;
                    (f64::from(a.value) + (f64::from(b.value) - f64::from(a.value)) * t) as f32
                }
            });
        }
    }
    points.last().map(|p| p.value)
}

/// Inserts a point keeping the list sorted by tick (a point at the same tick is replaced).
/// Returns the new list and the index of the inserted point.
pub fn insert_point(
    points: &[AutomationPoint],
    point: AutomationPoint,
) -> (Vec<AutomationPoint>, usize) {
    let mut v = points.to_vec();
    match v.binary_search_by_key(&point.tick, |p| p.tick) {
        Ok(i) => {
            v[i] = point;
            (v, i)
        }
        Err(i) => {
            v.insert(i, point);
            (v, i)
        }
    }
}

/// Moves point `index` to `(tick, value)`, limited to stay strictly between its neighbours
/// (and at tick ≥ 0). Returns the new list; the index is unchanged.
pub fn move_point(
    points: &[AutomationPoint],
    index: usize,
    tick: i64,
    value: f32,
) -> Vec<AutomationPoint> {
    let mut v = points.to_vec();
    if index >= v.len() {
        return v;
    }
    let lo = if index == 0 { 0 } else { v[index - 1].tick + 1 };
    let hi = v.get(index + 1).map_or(i64::MAX, |p| p.tick - 1);
    v[index].tick = tick.clamp(lo, hi.max(lo));
    v[index].value = value;
    v
}

/// Removes point `index`.
pub fn remove_point(points: &[AutomationPoint], index: usize) -> Vec<AutomationPoint> {
    let mut v = points.to_vec();
    if index < v.len() {
        v.remove(index);
    }
    v
}

/// Toggles point `index` between linear and step.
pub fn toggle_curve(points: &[AutomationPoint], index: usize) -> Vec<AutomationPoint> {
    let mut v = points.to_vec();
    if let Some(p) = v.get_mut(index) {
        p.curve = match p.curve {
            Curve::Linear => Curve::Step,
            Curve::Step => Curve::Linear,
        };
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::PPQ;
    use atomik_core::{ParamId, Taper, Unit};
    use atomik_model::{ClipContent, ClipId};

    const BAR: i64 = 4 * PPQ;

    fn clip(start: i64, length: i64, offset: i64, looped: Option<i64>) -> Clip {
        Clip {
            id: ClipId(1),
            start,
            length,
            offset,
            loop_length: looped,
            muted: false,
            grid: None,
            content: ClipContent::Notes(Vec::new()),
        }
    }

    fn note(tick: i64, key: u8) -> Note {
        Note::new(tick, PPQ, key, 0.8)
    }

    fn set(v: &[usize]) -> BTreeSet<usize> {
        v.iter().copied().collect()
    }

    fn ap(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
        AutomationPoint { tick, value, curve }
    }

    fn cutoff() -> ParamDescriptor {
        ParamDescriptor {
            id: ParamId(0),
            key: "cutoff",
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            taper: Taper::Exponential,
            unit: Unit::Hz,
            discrete: false,
        }
    }

    #[test]
    fn windows_without_loop() {
        let w = content_windows(&clip(BAR, 2 * BAR, PPQ, None));
        assert_eq!(
            w,
            vec![ContentWindow {
                song_start: BAR,
                song_end: 3 * BAR,
                content_start: PPQ
            }]
        );
        let c = clip(BAR, 2 * BAR, PPQ, None);
        assert_eq!(song_to_content(&c, BAR), Some(PPQ));
        assert_eq!(song_to_content(&c, 3 * BAR), None);
        assert_eq!(song_to_content(&c, BAR - 1), None);
    }

    #[test]
    fn windows_with_loop_repeat_from_offset() {
        // 1-bar loop, 2.5 bars long, starting half a bar into the loop.
        let c = clip(0, 5 * BAR / 2, BAR / 2, Some(BAR));
        let w = content_windows(&c);
        assert_eq!(w.len(), 3);
        assert_eq!(
            w[0],
            ContentWindow {
                song_start: 0,
                song_end: BAR / 2,
                content_start: BAR / 2
            }
        );
        assert_eq!(
            w[1],
            ContentWindow {
                song_start: BAR / 2,
                song_end: 3 * BAR / 2,
                content_start: 0
            }
        );
        assert_eq!(
            w[2],
            ContentWindow {
                song_start: 3 * BAR / 2,
                song_end: 5 * BAR / 2,
                content_start: 0
            }
        );
        assert_eq!(song_to_content(&c, BAR), Some(BAR / 2));
        assert_eq!(song_to_content(&c, 2 * BAR), Some(BAR / 2));
        // Offset beyond the loop wraps; a loop longer than the clip gives one window.
        let w = content_windows(&clip(0, BAR, 3 * BAR / 2, Some(BAR)));
        assert_eq!(w[0].content_start, BAR / 2);
        assert_eq!(content_windows(&clip(0, BAR, 0, Some(4 * BAR))).len(), 1);
    }

    #[test]
    fn span_hits_body_and_edge() {
        assert_eq!(hit_span(50.0, 0.0, 100.0, 6.0), Some(SpanPart::Body));
        assert_eq!(hit_span(96.0, 0.0, 100.0, 6.0), Some(SpanPart::End));
        assert_eq!(hit_span(100.0, 0.0, 100.0, 6.0), None);
        assert_eq!(hit_span(-0.1, 0.0, 100.0, 6.0), None);
        // A 9 px span: the edge zone shrinks to 3 px.
        assert_eq!(hit_span(5.0, 0.0, 9.0, 6.0), Some(SpanPart::Body));
        assert_eq!(hit_span(7.0, 0.0, 9.0, 6.0), Some(SpanPart::End));
    }

    #[test]
    fn nearest_point_within_radius() {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 0.0),
        ];
        assert_eq!(nearest_point(&pts, Point::new(1.0, 1.0), 4.0), Some(0));
        assert_eq!(nearest_point(&pts, Point::new(9.0, 0.0), 4.0), Some(2));
        assert_eq!(nearest_point(&pts, Point::new(5.0, 9.0), 4.0), None);
    }

    #[test]
    fn rubber_band_any_direction() {
        let items = [
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(20.0, 0.0, 30.0, 10.0),
            Rect::new(0.0, 20.0, 10.0, 30.0),
        ];
        assert_eq!(
            rubber_band(Point::new(25.0, 5.0), Point::new(5.0, 8.0), &items),
            set(&[0, 1])
        );
        assert_eq!(
            rubber_band(Point::new(12.0, 12.0), Point::new(18.0, 18.0), &items),
            set(&[])
        );
        assert_eq!(
            rubber_band(Point::new(-1.0, 40.0), Point::new(11.0, 9.0), &items),
            set(&[0, 2])
        );
    }

    #[test]
    fn drag_maths_snaps_and_clamps() {
        let beat = Some(PPQ);
        assert_eq!(drag_start(BAR, PPQ / 2 + 1, beat), BAR + PPQ);
        assert_eq!(drag_start(BAR, PPQ / 2 - 1, beat), BAR);
        assert_eq!(drag_start(BAR, -10 * BAR, beat), 0);
        assert_eq!(drag_start(100, 23, None), 123);
        assert_eq!(drag_length(0, BAR, PPQ - 100, beat, PPQ), BAR + PPQ);
        assert_eq!(drag_length(0, BAR, -10 * BAR, beat, PPQ), PPQ);
        assert_eq!(drag_length(BAR, 2 * BAR, 7, None, 1), BAR + 7);
        assert_eq!(draw_span(PPQ + 10, 3 * PPQ - 10, beat, PPQ), (PPQ, 2 * PPQ));
        assert_eq!(draw_span(3 * PPQ - 10, PPQ + 10, beat, PPQ), (PPQ, 2 * PPQ));
        assert_eq!(draw_span(PPQ + 10, PPQ + 20, beat, PPQ), (PPQ, PPQ));
        assert_eq!(draw_span(-50, 10, None, 240), (0, 240));
        assert_eq!(min_length(Some(PPQ), 240), PPQ);
        assert_eq!(min_length(None, 240), 240);
    }

    #[test]
    fn selection_click_rules() {
        let mut s = Selection::new();
        s.click(3, false, false);
        assert_eq!(s.items(), &set(&[3]));
        s.click(5, false, true);
        assert_eq!(s.items(), &set(&[3, 5]));
        s.click(5, false, false);
        assert_eq!(
            s.items(),
            &set(&[3, 5]),
            "plain click on a selected item keeps the group"
        );
        s.click(7, false, false);
        assert_eq!(s.items(), &set(&[7]));
        s.click(7, true, false);
        assert!(s.is_empty());
        s.toggle(2);
        s.toggle(4);
        s.toggle(2);
        assert_eq!(s.items(), &set(&[4]));
        s.select_only(9);
        assert_eq!(s.items(), &set(&[9]));
        assert!(s.contains(&9) && s.len() == 1);
    }

    #[test]
    fn notes_are_clamped_and_sorted_with_tracking() {
        let bad = Note::new(-5, 0, 200, 1.5);
        let c = clamp_note(bad);
        assert_eq!((c.tick, c.length, c.key), (0, 1, MAX_KEY));
        assert!((c.velocity - 1.0).abs() < 1e-9);
        let z = clamp_note(Note {
            velocity: 0.0,
            ..note(0, 60)
        });
        assert!((z.velocity - MIN_VELOCITY).abs() < 1e-9);
        let nan = clamp_note(Note {
            velocity: f32::NAN,
            ..note(0, 60)
        });
        assert!((nan.velocity - 0.8).abs() < 1e-9);
        let notes = vec![note(2 * PPQ, 60), note(0, 64), note(0, 60)];
        let (sorted, tracked) = normalize_notes(notes, &set(&[0, 1]));
        assert_eq!(sorted, vec![note(0, 60), note(0, 64), note(2 * PPQ, 60)]);
        assert_eq!(tracked, set(&[1, 2]));
    }

    #[test]
    fn moving_notes_keeps_the_group_in_range() {
        let notes = [note(PPQ, 60), note(2 * PPQ, 125), note(0, 10)];
        let moved = move_notes(&notes, &set(&[0, 1]), PPQ, 5);
        assert_eq!((moved[0].tick, moved[0].key), (2 * PPQ, 62));
        assert_eq!((moved[1].tick, moved[1].key), (3 * PPQ, 127));
        assert_eq!(moved[2], notes[2]);
        let moved = move_notes(&notes, &set(&[0, 1]), -5 * PPQ, -200);
        assert_eq!((moved[0].tick, moved[0].key), (0, 0));
        assert_eq!((moved[1].tick, moved[1].key), (PPQ, 65));
    }

    #[test]
    fn resize_quantize_and_velocity() {
        let notes = [note(PPQ + 100, 60), note(2 * PPQ - 100, 62), note(10, 64)];
        let r = resize_notes(&notes, &set(&[0, 1]), -2 * PPQ, PPQ / 4);
        assert_eq!(
            (r[0].length, r[1].length, r[2].length),
            (PPQ / 4, PPQ / 4, PPQ)
        );
        let q = quantize_notes(&notes, &set(&[0, 1]), Some(PPQ), 1.0);
        assert_eq!((q[0].tick, q[1].tick, q[2].tick), (PPQ, 2 * PPQ, 10));
        assert_eq!(q[0].length, PPQ, "quantize keeps lengths");
        let h = quantize_notes(&notes, &set(&[0, 1]), Some(PPQ), 0.5);
        assert_eq!((h[0].tick, h[1].tick), (PPQ + 50, 2 * PPQ - 50));
        assert_eq!(
            quantize_notes(&notes, &set(&[0]), None, 1.0),
            notes.to_vec()
        );
        let v = shift_velocity(&notes, &set(&[0, 2]), 0.5);
        assert!((v[0].velocity - 1.0).abs() < 1e-6 && (v[1].velocity - 0.8).abs() < 1e-6);
        let v = shift_velocity(&notes, &set(&[0]), -5.0);
        assert!((v[0].velocity - MIN_VELOCITY).abs() < 1e-6);
        assert!((velocity_from_y(10.0, 10.0, 100.0) - 1.0).abs() < 1e-6);
        assert!((velocity_from_y(60.0, 10.0, 100.0) - 0.5).abs() < 1e-6);
        assert!((velocity_from_y(500.0, 10.0, 100.0) - MIN_VELOCITY).abs() < 1e-6);
    }

    #[test]
    fn automation_values_follow_the_taper() {
        let p = cutoff();
        // Exponential: the middle of the lane is the geometric mean.
        let mid = value_from_y(&p, 50.0, 0.0, 100.0);
        assert!((mid - (20.0f32 * 20_000.0).sqrt()).abs() < 0.5, "{mid}");
        assert!((value_from_y(&p, -40.0, 0.0, 100.0) - 20_000.0).abs() < 1e-3);
        assert!((value_from_y(&p, 140.0, 0.0, 100.0) - 20.0).abs() < 1e-3);
        assert!((value_to_norm(&p, mid) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn automation_interpolation() {
        let pts = [
            ap(0, 0.0, Curve::Linear),
            ap(100, 1.0, Curve::Step),
            ap(200, 0.5, Curve::Linear),
        ];
        assert_eq!(automation_value_at(&[], 5), None);
        assert!((automation_value_at(&pts, -10).unwrap() - 0.0).abs() < 1e-6);
        assert!((automation_value_at(&pts, 50).unwrap() - 0.5).abs() < 1e-6);
        assert!((automation_value_at(&pts, 150).unwrap() - 1.0).abs() < 1e-6);
        assert!((automation_value_at(&pts, 200).unwrap() - 0.5).abs() < 1e-6);
        assert!((automation_value_at(&pts, 999).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn automation_point_edits() {
        let pts = [ap(0, 0.0, Curve::Linear), ap(100, 1.0, Curve::Linear)];
        let (v, i) = insert_point(&pts, ap(50, 0.3, Curve::Linear));
        assert_eq!(i, 1);
        assert_eq!(
            v.iter().map(|p| p.tick).collect::<Vec<_>>(),
            vec![0, 50, 100]
        );
        let (v2, i2) = insert_point(&v, ap(100, 0.7, Curve::Linear));
        assert_eq!((v2.len(), i2), (3, 2));
        assert!((v2[2].value - 0.7).abs() < 1e-6);
        let (v3, i3) = insert_point(&[], ap(10, 0.1, Curve::Step));
        assert_eq!((v3.len(), i3), (1, 0));
        let m = move_point(&v, 1, 500, 0.9);
        assert_eq!(m[1].tick, 99, "limited before the next point");
        let m = move_point(&v, 1, -500, 0.9);
        assert_eq!(m[1].tick, 1, "limited after the previous point");
        let m = move_point(&v, 0, -500, 0.2);
        assert_eq!(m[0].tick, 0);
        assert_eq!(remove_point(&v, 1), pts.to_vec());
        let t = toggle_curve(&v, 1);
        assert_eq!(t[1].curve, Curve::Step);
        assert_eq!(toggle_curve(&t, 1)[1].curve, Curve::Linear);
    }
}
