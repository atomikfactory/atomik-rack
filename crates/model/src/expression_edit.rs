//! Editing the per-note expression curves by hand: the pure steps behind the piano roll's
//! curve strip (see [`crate::note_expression`] for what a curve is).
//!
//! Every function takes a curve (sorted points in ticks after the note's start) and returns the
//! new curve, so the view can derive each step of a gesture from the curve it started with and
//! send one `SetClipMidi` per step. A point keeps inside the note (`0..=length` ticks) and inside
//! the range of its curve; a curve keeps at most [`MAX_EXPRESSION_POINTS`] points.

use crate::MidiData;
use crate::automation_rec::thin_points;
use crate::note_expression::{
    ExpressionKind, ExpressionPoint, MAX_EXPRESSION_NOTES, MAX_EXPRESSION_POINTS, NoteExpression,
};
use crate::song::{AutomationPoint, Curve};

/// The span thinning measures its tolerance against for the pitch curve, in semitones (the whole
/// range of the curve is far wider than any bend a player draws).
const PITCH_THIN_SPAN: f32 = 24.0;

/// `value` of `kind` held to its range (a value that is not finite becomes zero).
pub fn fit_value(kind: ExpressionKind, value: f32) -> f32 {
    kind.clamp(value)
}

/// The pitch `value` rounded to a whole semitone (the Ctrl snap of the strip).
pub fn snap_semitone(value: f32) -> f32 {
    if value.is_finite() {
        value.round()
    } else {
        0.0
    }
}

fn fit(kind: ExpressionKind, length: i64, tick: i64, value: f32) -> ExpressionPoint {
    ExpressionPoint::new(tick.clamp(0, length.max(0)), kind.clamp(value))
}

/// `points` with a point at `tick` (inside the note) and `value`: a point that is already at that
/// tick takes the value. Returns the new curve and the index of the point, or `None` when the
/// curve is full.
pub fn set_point(
    points: &[ExpressionPoint],
    kind: ExpressionKind,
    length: i64,
    tick: i64,
    value: f32,
) -> Option<(Vec<ExpressionPoint>, usize)> {
    let p = fit(kind, length, tick, value);
    let mut out = points.to_vec();
    match out.binary_search_by_key(&p.tick, |q| q.tick) {
        Ok(i) => {
            out[i] = p;
            Some((out, i))
        }
        Err(i) => {
            if out.len() >= MAX_EXPRESSION_POINTS {
                return None;
            }
            out.insert(i, p);
            Some((out, i))
        }
    }
}

/// `points` without the points at `indices` (indices past the end are ignored).
pub fn remove_points(points: &[ExpressionPoint], indices: &[usize]) -> Vec<ExpressionPoint> {
    points
        .iter()
        .enumerate()
        .filter(|(i, _)| !indices.contains(i))
        .map(|(_, p)| *p)
        .collect()
}

/// `points` with the points at `indices` moved by `dtick` and `dvalue` together. The shift is
/// limited so that no moved point leaves the note or the range of the curve (the group keeps its
/// shape), and a moved point replaces an unselected point at its tick. Returns the curve and the
/// new indices of the moved points.
pub fn move_points(
    points: &[ExpressionPoint],
    kind: ExpressionKind,
    length: i64,
    indices: &[usize],
    dtick: i64,
    dvalue: f32,
) -> (Vec<ExpressionPoint>, Vec<usize>) {
    let length = length.max(0);
    let (lo, hi) = kind.range();
    let moved: Vec<ExpressionPoint> = indices
        .iter()
        .filter_map(|i| points.get(*i))
        .copied()
        .collect();
    if moved.is_empty() {
        return (points.to_vec(), Vec::new());
    }
    let (mut min_t, mut max_t) = (i64::MAX, i64::MIN);
    let (mut min_v, mut max_v) = (f32::MAX, f32::MIN);
    for p in &moved {
        min_t = min_t.min(p.tick);
        max_t = max_t.max(p.tick);
        min_v = min_v.min(p.value);
        max_v = max_v.max(p.value);
    }
    let dt = dtick.clamp(-min_t, (length - max_t).max(-min_t));
    let dv = if dvalue.is_finite() { dvalue } else { 0.0 }.clamp(lo - min_v, hi - max_v);
    let mut entries: Vec<(ExpressionPoint, bool)> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        if indices.contains(&i) {
            let tick = p.tick + dt;
            entries.push((ExpressionPoint::new(tick, kind.clamp(p.value + dv)), true));
        } else {
            entries.push((*p, false));
        }
    }
    let moved_ticks: Vec<i64> = entries.iter().filter(|e| e.1).map(|e| e.0.tick).collect();
    entries.retain(|(p, was_moved)| *was_moved || !moved_ticks.contains(&p.tick));
    entries.sort_by_key(|(p, _)| p.tick);
    let new_indices = entries
        .iter()
        .enumerate()
        .filter(|(_, (_, m))| *m)
        .map(|(i, _)| i)
        .collect();
    (entries.into_iter().map(|(p, _)| p).collect(), new_indices)
}

/// `points` simplified: a point the straight line through its neighbours reproduces within
/// `fraction` of the span of the curve goes (the same simplification as Cleanup Automation).
pub fn thin(
    points: &[ExpressionPoint],
    kind: ExpressionKind,
    fraction: f32,
) -> Vec<ExpressionPoint> {
    let span = match kind {
        ExpressionKind::Pitch => PITCH_THIN_SPAN,
        _ => 1.0,
    };
    let auto: Vec<AutomationPoint> = points
        .iter()
        .map(|p| AutomationPoint {
            tick: p.tick,
            value: p.value,
            curve: Curve::Linear,
        })
        .collect();
    thin_points(&auto, fraction * span)
        .into_iter()
        .map(|p| ExpressionPoint::new(p.tick, p.value))
        .collect()
}

/// `points` with a freehand `stroke` of `(tick, value)` pairs drawn over them: the points inside
/// the span of the stroke are replaced by the stroke thinned to `fraction` (see [`thin`]), the
/// points outside stay. The stroke is taken in arrival order; ticks are held inside the note and
/// values inside the range. A result over the point limit is thinned harder until it fits.
/// Returns the curve and the indices of the drawn points.
pub fn draw_stroke(
    points: &[ExpressionPoint],
    kind: ExpressionKind,
    length: i64,
    stroke: &[(i64, f32)],
    fraction: f32,
) -> (Vec<ExpressionPoint>, Vec<usize>) {
    if stroke.is_empty() {
        return (points.to_vec(), Vec::new());
    }
    // Sort by tick (equal ticks keep the arrival order, the later one wins).
    let mut drawn: Vec<ExpressionPoint> = stroke
        .iter()
        .map(|&(t, v)| fit(kind, length, t, v))
        .collect();
    drawn.sort_by_key(|p| p.tick);
    let mut line: Vec<ExpressionPoint> = Vec::with_capacity(drawn.len());
    for p in drawn {
        match line.last_mut() {
            Some(last) if last.tick == p.tick => *last = p,
            _ => line.push(p),
        }
    }
    let (from, to) = (line[0].tick, line[line.len() - 1].tick);
    let outside: Vec<ExpressionPoint> = points
        .iter()
        .filter(|p| p.tick < from || p.tick > to)
        .copied()
        .collect();
    let room = MAX_EXPRESSION_POINTS.saturating_sub(outside.len()).max(1);
    let mut f = fraction.max(0.0);
    let mut thinned = thin(&line, kind, f);
    while thinned.len() > room && f < 1.0 {
        f = if f <= 0.0 { 0.005 } else { f * 2.0 };
        thinned = thin(&line, kind, f);
    }
    thinned.truncate(room);
    let mut out = outside;
    out.extend(thinned.iter().copied());
    out.sort_by_key(|p| p.tick);
    let indices = out
        .iter()
        .enumerate()
        .filter(|(_, p)| p.tick >= from && p.tick <= to)
        .map(|(i, _)| i)
        .collect();
    (out, indices)
}

/// `points` of a note `from_length` ticks long stretched to a note `to_length` ticks long: every
/// tick scales with the length (points that meet at one tick keep the later one).
pub fn scale_to_length(
    points: &[ExpressionPoint],
    from_length: i64,
    to_length: i64,
) -> Vec<ExpressionPoint> {
    let (from, to) = (from_length.max(1), to_length.max(0));
    let mut out: Vec<ExpressionPoint> = Vec::with_capacity(points.len());
    for p in points {
        let tick = (i128::from(p.tick) * i128::from(to) / i128::from(from)) as i64;
        let q = ExpressionPoint::new(tick.clamp(0, to), p.value);
        match out.last_mut() {
            Some(last) if last.tick == q.tick => *last = q,
            _ => out.push(q),
        }
    }
    out
}

/// `midi` with the curve `kind` of the note at `tick` on `key` replaced by `points` (an entry
/// that is left without any point goes). `None` when the note has no entry yet and the clip is at
/// its limit of notes with expression.
pub fn with_curve(
    midi: &MidiData,
    tick: i64,
    key: u8,
    kind: ExpressionKind,
    points: Vec<ExpressionPoint>,
) -> Option<MidiData> {
    let mut entry = midi
        .expression_of(tick, key)
        .cloned()
        .unwrap_or_else(|| NoteExpression::new(tick, key));
    *entry.curve_mut(kind) = points;
    if entry.is_empty() {
        return Some(
            midi.without_expression(tick, key)
                .unwrap_or_else(|| midi.clone()),
        );
    }
    midi.with_expression(entry)
}

/// `midi` with the curve `kind` of the note at (`tick`, `key`) copied to every note of `targets`
/// (`(start tick, key, length)`), stretched to each note's length. `source_length` is the length
/// of the note the curve belongs to; the source itself is left out of the targets. `None` when
/// nothing changes.
pub fn apply_curve(
    midi: &MidiData,
    source: (i64, u8),
    source_length: i64,
    kind: ExpressionKind,
    targets: &[(i64, u8, i64)],
) -> Option<MidiData> {
    let curve = midi
        .expression_of(source.0, source.1)
        .map(|e| e.curve(kind).to_vec())
        .unwrap_or_default();
    let mut out = midi.clone();
    for &(tick, key, length) in targets {
        if (tick, key) == source {
            continue;
        }
        let scaled = scale_to_length(&curve, source_length, length);
        let at = out.expression.len();
        let had = out.expression_of(tick, key).is_some();
        if !had && scaled.is_empty() {
            continue;
        }
        if !had && at >= MAX_EXPRESSION_NOTES {
            continue;
        }
        out = with_curve(&out, tick, key, kind, scaled)?;
    }
    (out != *midi).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(tick: i64, value: f32) -> ExpressionPoint {
        ExpressionPoint::new(tick, value)
    }

    const P: ExpressionKind = ExpressionKind::Pitch;
    const T: ExpressionKind = ExpressionKind::Timbre;

    #[test]
    fn a_point_is_added_inside_the_note_and_inside_the_range() {
        let (c, i) = set_point(&[p(0, 0.0), p(100, 1.0)], T, 200, 50, 0.4).unwrap();
        assert_eq!((c.len(), i), (3, 1));
        assert_eq!(c[1], p(50, 0.4));
        // Past the end and beyond the range: held inside both.
        let (c, i) = set_point(&c, T, 200, 900, 7.0).unwrap();
        assert_eq!(c[i], p(200, 1.0));
        let (c, i) = set_point(&c, P, 200, -5, -500.0).unwrap();
        assert_eq!(c[i], p(0, -96.0));
        // A point at the same tick takes the value instead of adding one.
        let (c2, j) = set_point(&c, P, 200, 0, 3.0).unwrap();
        assert_eq!((c2.len(), c2[j]), (c.len(), p(0, 3.0)));
    }

    #[test]
    fn a_full_curve_takes_no_new_point_but_still_moves_one() {
        let full: Vec<ExpressionPoint> = (0..MAX_EXPRESSION_POINTS as i64)
            .map(|t| p(t, 0.5))
            .collect();
        assert!(set_point(&full, T, 5000, 4000, 0.1).is_none());
        let (c, _) = set_point(&full, T, 5000, 10, 0.9).unwrap();
        assert_eq!(c.len(), MAX_EXPRESSION_POINTS);
    }

    #[test]
    fn points_are_removed_by_index() {
        let c = [p(0, 0.1), p(10, 0.2), p(20, 0.3), p(30, 0.4)];
        assert_eq!(remove_points(&c, &[1, 3]), [p(0, 0.1), p(20, 0.3)]);
        assert_eq!(remove_points(&c, &[9]), c);
    }

    #[test]
    fn a_group_moves_together_and_stops_at_the_edges() {
        let c = [p(0, 0.2), p(50, 0.5), p(100, 0.9)];
        let (m, idx) = move_points(&c, T, 200, &[1, 2], 40, 0.05);
        assert_eq!(idx, [1, 2]);
        assert_eq!(m[1], p(90, 0.55));
        assert!((m[2].value - 0.95).abs() < 1e-6 && m[2].tick == 140);
        // Far right: limited by the note's end, the shape is kept.
        let (m, _) = move_points(&c, T, 200, &[1, 2], 10_000, 0.0);
        assert_eq!((m[1].tick, m[2].tick), (150, 200));
        // Far up: limited by the range, the distance between the values is kept.
        let (m, _) = move_points(&c, T, 200, &[0, 2], 0, 5.0);
        assert!((m[2].value - 1.0).abs() < 1e-6 && (m[0].value - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_moved_point_replaces_an_unselected_point_at_its_tick_and_the_curve_stays_sorted() {
        let c = [p(0, 0.1), p(50, 0.5), p(100, 0.9)];
        let (m, idx) = move_points(&c, T, 200, &[0], 50, 0.0);
        assert_eq!(m, [p(50, 0.1), p(100, 0.9)]);
        assert_eq!(idx, [0]);
        // A point dragged past its neighbour re-sorts and reports its new place.
        let (m, idx) = move_points(&c, T, 200, &[0], 70, 0.0);
        assert_eq!(m, [p(50, 0.5), p(70, 0.1), p(100, 0.9)]);
        assert_eq!(idx, [1]);
    }

    #[test]
    fn a_freehand_stroke_replaces_what_it_covers_and_is_thinned() {
        let c = [p(0, 0.9), p(500, 0.9)];
        // A straight rise sampled every 10 ticks thins to its two ends.
        let stroke: Vec<(i64, f32)> = (10..=100)
            .step_by(10)
            .map(|t| (t, t as f32 / 200.0))
            .collect();
        let (out, idx) = draw_stroke(&c, T, 600, &stroke, 0.005);
        assert_eq!(out, [p(0, 0.9), p(10, 0.05), p(100, 0.5), p(500, 0.9)]);
        assert_eq!(idx, [1, 2]);
        // A corner survives.
        let corner = [(0, 0.0), (50, 0.5), (100, 1.0), (150, 0.5), (200, 0.0)];
        let (out, _) = draw_stroke(&[], T, 600, &corner, 0.005);
        assert_eq!(out, [p(0, 0.0), p(100, 1.0), p(200, 0.0)]);
    }

    #[test]
    fn a_stroke_never_passes_the_point_limit() {
        let stroke: Vec<(i64, f32)> = (0..4000)
            .map(|t| (t, if t % 2 == 0 { 0.1 } else { 0.9 }))
            .collect();
        let (out, _) = draw_stroke(&[], T, 5000, &stroke, 0.005);
        assert!(out.len() <= MAX_EXPRESSION_POINTS);
        assert!(out.windows(2).all(|w| w[0].tick < w[1].tick));
    }

    #[test]
    fn a_curve_scales_with_the_length_of_the_note() {
        let c = [p(0, 0.0), p(100, 1.0), p(200, 0.5)];
        assert_eq!(
            scale_to_length(&c, 200, 400),
            [p(0, 0.0), p(200, 1.0), p(400, 0.5)]
        );
        assert_eq!(
            scale_to_length(&c, 200, 100),
            [p(0, 0.0), p(50, 1.0), p(100, 0.5)]
        );
        // Squeezed to nothing: the points that meet keep the later one.
        assert_eq!(scale_to_length(&c, 200, 0), [p(0, 0.5)]);
    }

    #[test]
    fn a_curve_is_copied_to_other_notes_scaled_to_each() {
        let mut e = NoteExpression::new(0, 60);
        e.pitch = vec![p(0, 0.0), p(100, 2.0)];
        e.timbre = vec![p(0, 0.5)];
        let midi = MidiData {
            expression: vec![e],
            ..Default::default()
        };
        let targets = [(0, 60, 100), (200, 62, 400), (900, 64, 50)];
        let out = apply_curve(&midi, (0, 60), 100, P, &targets).unwrap();
        assert_eq!(
            out.expression_of(200, 62).unwrap().pitch,
            [p(0, 0.0), p(400, 2.0)]
        );
        assert_eq!(
            out.expression_of(900, 64).unwrap().pitch,
            [p(0, 0.0), p(50, 2.0)]
        );
        // Only the curve that was copied; the source keeps all of its own.
        assert!(out.expression_of(200, 62).unwrap().timbre.is_empty());
        assert_eq!(out.expression_of(0, 60).unwrap().timbre, [p(0, 0.5)]);
        // Nothing to change: no new data.
        assert!(apply_curve(&out, (0, 60), 100, P, &targets).is_none());
    }

    #[test]
    fn copying_an_empty_curve_clears_it_on_the_targets() {
        let mut a = NoteExpression::new(0, 60);
        a.timbre = vec![p(0, 0.5)];
        let mut b = NoteExpression::new(100, 62);
        b.pitch = vec![p(0, 1.0)];
        let midi = MidiData {
            expression: vec![a, b],
            ..Default::default()
        };
        let out = apply_curve(&midi, (0, 60), 100, P, &[(100, 62, 100)]).unwrap();
        assert!(
            out.expression_of(100, 62).is_none(),
            "the entry had nothing else"
        );
    }

    #[test]
    fn replacing_the_last_curve_of_a_note_removes_its_entry() {
        let mut a = NoteExpression::new(0, 60);
        a.pitch = vec![p(0, 1.0)];
        let midi = MidiData {
            expression: vec![a],
            ..Default::default()
        };
        let out = with_curve(&midi, 0, 60, P, Vec::new()).unwrap();
        assert!(out.expression.is_empty());
        let out = with_curve(&out, 0, 60, T, vec![p(0, 0.3)]).unwrap();
        assert_eq!(out.expression_of(0, 60).unwrap().timbre, [p(0, 0.3)]);
    }

    #[test]
    fn the_semitone_snap_rounds_and_survives_bad_values() {
        assert_eq!(snap_semitone(1.4), 1.0);
        assert_eq!(snap_semitone(-2.6), -3.0);
        assert_eq!(snap_semitone(f32::NAN), 0.0);
    }
}
