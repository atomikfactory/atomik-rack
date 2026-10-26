//! Flattening of clips into song-time events (what the app turns into the engine `SongInput`).
//!
//! Rules (all integer tick maths, deterministic):
//! - **Audible spans.** On a lane, unmuted clips are ordered by `(start, id)`; a later
//!   clip hides every earlier one where they overlap. A clip is audible on its span
//!   `[start, start + length)` minus the spans of all later unmuted clips. Muted clips neither
//!   play nor hide anything. Audible spans of one lane never overlap.
//! - **Note lanes.** A track's note lanes flatten one by one with the rules below (clips of
//!   different lanes never hide each other) and a muted lane contributes nothing. The notes of
//!   the lanes merge, sorted by `(tick, key)` with the earlier lane first at an equal tick and
//!   key; the controller points of a kind merge by tick, the later lane winning at an equal tick.
//! - **Content mapping.** At song tick `t` a clip shows content position
//!   `u = offset + (t - start)`; with `loop_length = Some(n)` the content window `[0, n)`
//!   repeats, so the position is `u mod n` and each repetition is a separate piece.
//! - **Notes.** A note plays when its content tick falls inside a piece of an audible span; it
//!   is cut at the end of that piece (the clip end, the loop repetition end, or where a later
//!   clip takes over). Notes outside the loop window or before the offset never play.
//! - **Swing.** A note clip with a [`StepGrid`](super::StepGrid) swings
//!   its notes before anything else: with `s` the step length in ticks, a note whose content
//!   tick `t` (`Note::tick`, counted from the content start, so the clip offset and the loop
//!   do not move the steps) has `t mod 2s` in `[s, 2s)` starts `(swing * s + 100) / 200` ticks
//!   later (integer division); its length is unchanged and notes in even steps do not move.
//!   Every loop pass swings alike. The existing note rule then applies to the *swung* start:
//!   a note belongs to a piece when its unswung content tick is inside the piece's content
//!   window (so a note before the offset never plays), and it plays only if its swung start
//!   is still inside the window (a note the delay pushes past the clip end, the loop end or
//!   the start of a later clip is dropped, like any note that starts outside the piece); it
//!   is cut at the piece end as before.
//! - **Automation.** Each piece emits a point at its start carrying the content value there
//!   (interpolated if between points, the first value before the first point), then every
//!   content point inside the piece. A ramp still running at the end of a piece gets a closing
//!   point on the piece's last tick with the interpolated value, so the flattened curve equals
//!   the content at every integer tick of every audible span. The last point of a piece is
//!   always `Step`: between pieces and clips the value holds. In a looped clip the content
//!   value inside the window is computed from all points, so a ramp towards a point at or after
//!   the loop end is honoured up to the loop end.
//! - **Controllers.** [`flatten_controllers`] treats each controller lane of a note clip like an
//!   automation lane (same pieces, offsets, loops and hiding), with one difference: a lane says
//!   nothing before its first point, so a piece emits a point at its start only when the lane
//!   already has a value there (an earlier point, or a ramp running through it); a point inside
//!   the piece is emitted at its own tick. Lanes of the same kind in several clips of the lane
//!   join into one list per kind. Between pieces the value holds.
//! - **Audio.** See [`flatten_audio`]: the same audible spans, converted from ticks to
//!   frames with the tempo map, one [`AudioRegion`] per span.

use super::{
    AudioClipContent, AutomationPoint, Clip, ClipContent, ClipId, Curve, FadeCurve, Lane, LaneKind,
    Song, Track,
};
use crate::controller::{ControllerKind, value_at as lane_value_at};
use crate::warp::{WarpJob, warp_job};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The audible spans `(clip, start, end)` of a lane's clips, sorted by start (rules in
/// the module docs). The UI can draw the hidden parts of a clip from it.
pub fn audible_spans(lane: &Lane) -> Vec<(ClipId, i64, i64)> {
    let mut clips: Vec<&Clip> = lane.clips.iter().filter(|c| !c.muted).collect();
    clips.sort_by_key(|c| (c.start, c.id));
    let mut out = Vec::new();
    for (i, c) in clips.iter().enumerate() {
        let mut spans = vec![(c.start, c.end())];
        for later in &clips[i + 1..] {
            if later.start >= c.end() {
                break;
            }
            let (ls, le) = (later.start, later.end());
            spans = spans
                .into_iter()
                .flat_map(|(a, b)| {
                    [(a, b.min(ls)), (a.max(le), b)]
                        .into_iter()
                        .filter(|(x, y)| x < y)
                })
                .collect();
        }
        out.extend(spans.into_iter().map(|(a, b)| (c.id, a, b)));
    }
    out.sort_by_key(|&(id, a, _)| (a, id));
    out
}

/// The pieces `(song start, song end, content start)` of clip `c` over the song span `[a, b)`:
/// one piece without a loop, one per loop repetition with one.
fn pieces(c: &Clip, a: i64, b: i64) -> Vec<(i64, i64, i64)> {
    match c.loop_length {
        None => vec![(a, b, c.offset + (a - c.start))],
        Some(n) => {
            let mut out = Vec::new();
            let mut s = a;
            while s < b {
                let cs = (c.offset + (s - c.start)).rem_euclid(n);
                let e = (s + n - cs).min(b);
                out.push((s, e, cs));
                s = e;
            }
            out
        }
    }
}

/// Each audible piece of a lane: `(clip, song start, song end, content start)` in song order.
fn audible_pieces(lane: &Lane) -> Vec<(&Clip, i64, i64, i64)> {
    let by_id: BTreeMap<ClipId, &Clip> = lane.clips.iter().map(|c| (c.id, c)).collect();
    audible_spans(lane)
        .into_iter()
        .flat_map(|(id, a, b)| {
            let c = by_id[&id];
            pieces(c, a, b)
                .into_iter()
                .map(move |(s, e, cs)| (c, s, e, cs))
        })
        .collect()
}

/// The notes a track's unmuted note lanes play, as `(song tick, length, key, velocity)` sorted
/// by `(tick, key)` (rules in the module docs). The lanes' notes merge; at an equal tick and key
/// the earlier lane comes first. Track mute and solo are not applied here; see
/// [`effective_tracks`].
pub fn flatten_notes(track: &Track) -> Vec<(i64, i64, u8, f32)> {
    let mut out = Vec::new();
    for lane in track.note_lanes().filter(|l| !l.muted) {
        for (clip, s, e, cs) in audible_pieces(lane) {
            let ClipContent::Notes(notes) = &clip.content else {
                continue;
            };
            let ce = cs + (e - s);
            let from = notes.partition_point(|n| n.tick < cs);
            for n in notes[from..].iter().take_while(|n| n.tick < ce) {
                let tick = clip.grid.map_or(n.tick, |g| g.swung(n.tick));
                if n.muted || tick >= ce {
                    continue; // muted, or swung out of the piece
                }
                let t = s + (tick - cs);
                out.push((t, n.length.min(e - t), n.key, n.velocity));
            }
        }
    }
    // Stable: equal `(tick, key)` keep the lane order.
    out.sort_by_key(|&(t, _, k, _)| (t, k));
    out
}

/// The content value at content position `c` and the shape of the curve leaving it.
/// `points` is non-empty and sorted by tick.
fn value_at(points: &[AutomationPoint], c: i64) -> (f32, Curve) {
    let i = points.partition_point(|p| p.tick <= c);
    if i == 0 {
        return (points[0].value, Curve::Step);
    }
    let p = points[i - 1];
    match points.get(i) {
        Some(n) if p.curve == Curve::Linear => {
            let v0 = f64::from(p.value);
            let frac = (c - p.tick) as f64 / (n.tick - p.tick) as f64;
            (
                (v0 + (f64::from(n.value) - v0) * frac) as f32,
                Curve::Linear,
            )
        }
        _ => (p.value, Curve::Step),
    }
}

/// The automation of lane `lane` of a track as `(song tick, value, curve)` with strictly
/// increasing ticks (rules in the module docs). Empty for the note lane or a bad index.
pub fn flatten_automation(track: &Track, lane: usize) -> Vec<(i64, f32, Curve)> {
    let Some(lane) = track
        .lanes
        .get(lane)
        .filter(|l| matches!(l.kind, LaneKind::Automation(_)))
    else {
        return Vec::new();
    };
    let mut out: Vec<(i64, f32, Curve)> = Vec::new();
    for (clip, s, e, cs) in audible_pieces(lane) {
        let ClipContent::Automation(points) = &clip.content else {
            continue;
        };
        if points.is_empty() {
            continue;
        }
        let ce = cs + (e - s);
        out.push((s, value_at(points, cs).0, value_at(points, cs).1));
        let from = points.partition_point(|p| p.tick <= cs);
        for p in points[from..].iter().take_while(|p| p.tick < ce) {
            out.push((s + (p.tick - cs), p.value, value_at(points, p.tick).1));
        }
        let last = out.len() - 1;
        if out[last].2 == Curve::Linear {
            if out[last].0 < e - 1 {
                out.push((e - 1, value_at(points, ce - 1).0, Curve::Step));
            } else {
                out[last].2 = Curve::Step;
            }
        }
    }
    out
}

/// One kind's flattened controller points: `(song tick, value, curve)`.
pub type FlatController = (ControllerKind, Vec<(i64, f32, Curve)>);

/// The controller data a track's unmuted note lanes play: for each [`ControllerKind`] in use
/// (sorted by kind) the points `(song tick, value, curve)` with strictly increasing ticks
/// (rules in the module docs). The lanes' points of a kind merge by tick; at an equal tick the
/// later lane wins. Empty without controller lanes. Track mute and solo are not applied here.
pub fn flatten_controllers(track: &Track) -> Vec<FlatController> {
    let mut by_kind: BTreeMap<ControllerKind, Vec<(i64, f32, Curve)>> = BTreeMap::new();
    let mut lanes = track.note_lanes().filter(|l| !l.muted);
    let first = lanes.next().map(lane_controllers).unwrap_or_default();
    // One lane is the common case and needs no merge.
    let rest: Vec<_> = lanes.map(lane_controllers).collect();
    if rest.is_empty() {
        by_kind = first;
    } else {
        for map in std::iter::once(first).chain(rest) {
            for (kind, points) in map {
                by_kind.entry(kind).or_default().extend(points);
            }
        }
        for points in by_kind.values_mut() {
            // Stable by tick, so of two points at one tick the later lane's comes last: keep it.
            points.sort_by_key(|p| p.0);
            let mut merged: Vec<(i64, f32, Curve)> = Vec::with_capacity(points.len());
            for p in points.drain(..) {
                match merged.last_mut() {
                    Some(last) if last.0 == p.0 => *last = p,
                    _ => merged.push(p),
                }
            }
            *points = merged;
        }
    }
    by_kind.retain(|_, v| !v.is_empty());
    by_kind.into_iter().collect()
}

/// The controller points of one note lane, per kind.
fn lane_controllers(lane: &Lane) -> BTreeMap<ControllerKind, Vec<(i64, f32, Curve)>> {
    let mut by_kind: BTreeMap<ControllerKind, Vec<(i64, f32, Curve)>> = BTreeMap::new();
    for (clip, s, e, cs) in audible_pieces(lane) {
        let ce = cs + (e - s);
        for ctl in &clip.controllers {
            let points = &ctl.points;
            let out = by_kind.entry(ctl.kind).or_default();
            let before = out.len();
            if let Some((v, curve)) = lane_value_at(points, cs) {
                out.push((s, v, curve));
            }
            let from = points.partition_point(|p| p.tick <= cs);
            for p in points[from..].iter().take_while(|p| p.tick < ce) {
                out.push((s + (p.tick - cs), p.value, p.curve));
            }
            if out.len() == before {
                continue;
            }
            let last = out.len() - 1;
            if out[last].2 == Curve::Linear {
                // A ramp still running at the end of the piece: close it on the last tick.
                let at_end = lane_value_at(points, ce - 1).map_or(out[last].1, |(v, _)| v);
                if out[last].0 < e - 1 {
                    out.push((e - 1, at_end, Curve::Step));
                } else {
                    out[last].2 = Curve::Step;
                }
            }
        }
    }
    by_kind
}

/// The tracks that play: unmuted tracks, and if any track is soloed, only the soloed ones
/// among them. Display order is kept.
pub fn effective_tracks(song: &Song) -> Vec<&Track> {
    let any_solo = song.tracks.iter().any(|t| t.solo);
    song.tracks
        .iter()
        .filter(|t| !t.muted && (!any_solo || t.solo))
        .collect()
}

/// One stretch of audio the track player plays, in song frames. Regions of one track are
/// sorted by `(song_start_frame, end)`; they overlap only where a crossfade mixes two clips
/// (see [`flatten_audio`]), never more than two at a time.
///
/// The player starts reading `asset` at frame `source_offset` at song frame `song_start_frame` and
/// plays `frames` frames, cut earlier where the asset ends (the model does not know the
/// asset's length here). A `reverse` region reads downwards from `source_offset`: region frame
/// `i` plays asset frame `source_offset - i`; frames at or past the asset's end are silent.
///
/// **Fades are measured within the original clip, not reset at overlap cut points.** A clip
/// that a later clip covers in part is flattened into several regions; each region carries the
/// whole clip's fade lengths plus where it sits in the clip (`clip_pos`, `clip_frames`), so the
/// fade-in plays only over the clip's first `fade_in` frames and the fade-out only over its
/// last `fade_out` frames. A cut inside a clip makes no fade of its own (the cut is a hard
/// edge; the covering clip supplies its own fade-in, or a crossfade).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioRegion {
    /// Asset id.
    pub asset: String,
    /// Song frame where the region starts (`Song::frame_at` of its tick).
    pub song_start_frame: u64,
    /// Asset frame read at the first region frame (the highest one, with `reverse`).
    pub source_offset: u64,
    /// Region length in frames (at least 1).
    pub frames: u64,
    /// Linear gain from the clip's `gain_db`.
    pub gain: f32,
    /// Fade-in length in frames of the original clip (at most `clip_frames`).
    pub fade_in: u64,
    /// Fade-out length in frames of the original clip (at most `clip_frames`).
    pub fade_out: u64,
    /// Fade shape.
    pub fade_curve: FadeCurve,
    /// Frames from the original clip's start to the region's start (0 for an uncut clip).
    pub clip_pos: u64,
    /// Length in frames of the original clip (`frame_at(end) - frame_at(start)`).
    pub clip_frames: u64,
    /// Plays the asset backwards.
    pub reverse: bool,
    /// The clip's gain envelope as `(frame from the clip start, linear gain)` pairs.
    pub envelope: Vec<(u64, f32)>,
    /// Frames of the crossfade-in at the region's start (0 for none): the region is the first
    /// of a clip that crossfades with the clip before it.
    pub x_in: u64,
    /// Frames of the crossfade-out: the region is the tail of an earlier clip that a later
    /// clip crossfades over, and `x_out == frames`. 0 for none.
    pub x_out: u64,
    /// Shape of the crossfade (the later clip's `xfade`).
    pub x_curve: FadeCurve,
    /// For a warped clip: the render that replaces the asset. The player then reads that
    /// buffer (the clip's whole span, so frame `clip_pos + i` for region frame `i`) instead of
    /// the asset; `asset` and `source_offset` are what the clip plays without it (the same
    /// frames in real time) while the buffer is not ready.
    pub warp: Option<Arc<WarpJob>>,
}

impl AudioRegion {
    /// The fade gain (in `0.0..=1.0`, without the clip gain) at region frame `i`.
    ///
    /// With `n = clip_pos + i` the frame within the original clip: the fade-in gives
    /// `curve(n / fade_in)` for `n < fade_in`, the fade-out gives
    /// `curve((clip_frames - n) / fade_out)` for `n >= clip_frames - fade_out`, and both
    /// multiply (they overlap only when `fade_in + fade_out > clip_frames`). `curve` is
    /// [`FadeCurve::eval`]. This is the reference definition; the engine evaluates the same
    /// curves with platform-independent arithmetic.
    pub fn fade_gain(&self, i: u64) -> f32 {
        let n = self.clip_pos + i;
        let mut g = 1.0f64;
        if n < self.fade_in {
            g *= self.fade_curve.eval(n as f64 / self.fade_in as f64);
        }
        if n + self.fade_out >= self.clip_frames && self.fade_out > 0 {
            g *= self
                .fade_curve
                .eval((self.clip_frames - n) as f64 / self.fade_out as f64);
        }
        g as f32
    }

    /// The crossfade gain at region frame `i`: `curve(i / x_in)` while `i < x_in`,
    /// `curve((x_out - i) / x_out)` while `i < x_out`, else 1.
    pub fn xfade_gain(&self, i: u64) -> f32 {
        let mut g = 1.0f64;
        if i < self.x_in {
            g *= self.x_curve.eval(i as f64 / self.x_in as f64);
        }
        if i < self.x_out {
            g *= self
                .x_curve
                .eval((self.x_out - i) as f64 / self.x_out as f64);
        }
        g as f32
    }

    /// The whole gain at region frame `i`: clip gain times fade, times the envelope, times the
    /// crossfade (the last two only when the region has them).
    pub fn total_gain(&self, i: u64) -> f32 {
        let mut g = self.gain * self.fade_gain(i);
        if let Some(e) = super::envelope_gain(&self.envelope, self.clip_pos + i) {
            g *= e;
        }
        if self.x_in > 0 || self.x_out > 0 {
            g *= self.xfade_gain(i);
        }
        g
    }
}

/// A crossfade between two audio clips of a lane, in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crossfade {
    /// The clip that fades in (the later one).
    pub later: ClipId,
    /// The clip that fades out (the earlier one).
    pub earlier: ClipId,
    /// First tick of the overlap (the later clip's start).
    pub start: i64,
    /// End tick of the overlap.
    pub end: i64,
    /// The later clip's crossfade shape.
    pub curve: FadeCurve,
}

/// The crossfades of an audio lane: the rule of [`flatten_audio`] on ticks. A clip with
/// `xfade: Some(curve)` whose first audible span begins at its own start, right after an
/// audible span of an earlier audio clip that continues past that start and ends inside the
/// later clip, crossfades over `[start, min(earlier end, end of the later clip's span))`.
pub fn crossfades(lane: &Lane) -> Vec<Crossfade> {
    let by_id: BTreeMap<ClipId, &Clip> = lane.clips.iter().map(|c| (c.id, c)).collect();
    let spans = audible_spans(lane);
    let mut out = Vec::new();
    for (k, &(id, a, b)) in spans.iter().enumerate() {
        let clip = by_id[&id];
        let ClipContent::Audio(audio) = &clip.content else {
            continue;
        };
        let (Some(curve), true, Some(&(pid, _, pend))) = (
            audio.xfade,
            a == clip.start,
            k.checked_sub(1).and_then(|p| spans.get(p)),
        ) else {
            continue;
        };
        let Some(prev) = by_id.get(&pid) else {
            continue;
        };
        if pend == a
            && prev.content.is_audio()
            && prev.end() > a
            && prev.end() <= clip.end()
            && prev.end().min(b) > a
        {
            out.push(Crossfade {
                later: id,
                earlier: pid,
                start: a,
                end: prev.end().min(b),
                curve,
            });
        }
    }
    out
}

/// The audio regions an audio track plays, sorted by start frame (rules in the module docs).
///
/// - Only tracks that play: a track that [`effective_tracks`] leaves out (muted, or another
///   track is soloed) gives no regions, and neither does a track without an audio lane.
/// - The audible spans are the note lane's rules: muted clips neither play nor hide
///   others, a later clip (by `(start, id)`) hides the earlier ones where they overlap.
/// - Ticks become frames with the song's tempo map and `sample_rate` ([`Song::frame_at`]):
///   a span `[a, b)` is `[frame_at(a), frame_at(b))`; spans that round to no frame are
///   dropped. Neighbouring spans share their boundary frame, so no frame is lost or doubled.
/// - Audio does not stretch unless the clip is warped: the source offset advances one asset
///   frame per song frame from the clip start, so a region that starts `d` frames into its clip
///   reads from `source_offset + d`. A reversed clip of `n` frames plays the asset frames
///   `source_offset .. source_offset + n` backwards, so its region that starts `d` frames in
///   starts at `source_offset + n - 1 - d` and counts down. A warped clip (see [`crate::warp`])
///   carries a [`WarpJob`] on each region: the player reads the rendered buffer at frame
///   `clip_pos + i`, and the fields above are the real-time reading it falls back to.
/// - `gain` is `db_to_gain(gain_db)`; fades and the envelope are the original clip's (see
///   [`AudioRegion`]).
/// - **Crossfades.** A clip `B` with `xfade: Some(curve)` whose first audible span begins at
///   its own start, and which starts inside an earlier audible clip `P` that ends inside
///   `B` (`P.end <= B.end`), crossfades with it: over the overlap `[B.start, P.end)`, shortened
///   to where `B` stops being audible if a later clip covers `B` first, `B` fades in with
///   `curve` and `P` fades out with the mirrored curve, and both play (an extra region, the tail
///   of `P`, covers the overlap). With the equal-power curve the two gains square-sum to 1. A
///   clip that lies wholly inside an earlier one hides it with hard edges, as does a clip with
///   `xfade: None`. `P`'s and `B`'s own fades and gains apply on top of the crossfade.
pub fn flatten_audio(track: &Track, song: &Song, sample_rate: u32) -> Vec<AudioRegion> {
    if !effective_tracks(song)
        .iter()
        .any(|t| t.device == track.device)
    {
        return Vec::new();
    }
    let Some(lane) = track.lanes.iter().find(|l| l.kind.is_audio()) else {
        return Vec::new();
    };
    let by_id: BTreeMap<ClipId, &Clip> = lane.clips.iter().map(|c| (c.id, c)).collect();
    let table = song.tempo_table();
    let map = table.map();
    let sr = f64::from(sample_rate);
    let spans = audible_spans(lane);
    let mut out = Vec::new();
    // The region of `clip` over the song frames `[start, end)`, `start` at or after the clip's.
    let region = |clip: &Clip, audio: &AudioClipContent, start: u64, end: u64| {
        let clip_start = map.song_frame(clip.start, sr);
        let clip_frames = map.song_frame(clip.end(), sr) - clip_start;
        let clip_pos = start - clip_start;
        let source_offset = if audio.reverse {
            audio.source_offset + clip_frames.saturating_sub(1 + clip_pos)
        } else {
            audio.source_offset + clip_pos
        };
        AudioRegion {
            asset: audio.asset.clone(),
            song_start_frame: start,
            source_offset,
            frames: end - start,
            gain: atomik_core::db_to_gain(audio.gain_db),
            fade_in: audio.fade_in.min(clip_frames),
            fade_out: audio.fade_out.min(clip_frames),
            fade_curve: audio.fade_curve,
            clip_pos,
            clip_frames,
            reverse: audio.reverse,
            envelope: audio.envelope_linear(),
            x_in: 0,
            x_out: 0,
            x_curve: FadeCurve::EqualPower,
            warp: if audio.reverse {
                None
            } else {
                warp_job(song, sample_rate, clip.start, clip.length, audio).map(Arc::new)
            },
        }
    };
    let zones = crossfades(lane);
    for &(id, a, b) in &spans {
        let clip = by_id[&id];
        let ClipContent::Audio(audio) = &clip.content else {
            continue;
        };
        let (start, end) = (map.song_frame(a, sr), map.song_frame(b, sr));
        if end <= start {
            continue;
        }
        let mut here = region(clip, audio, start, end);
        if let Some(x) = zones.iter().find(|x| x.later == id && x.start == a) {
            let prev = by_id[&x.earlier];
            if let ClipContent::Audio(prev_audio) = &prev.content {
                let zone_end = map.song_frame(x.end, sr);
                if zone_end > start {
                    let z = zone_end - start;
                    let mut tail = region(prev, prev_audio, start, zone_end);
                    tail.x_out = z;
                    tail.x_curve = x.curve;
                    here.x_in = z;
                    here.x_curve = x.curve;
                    out.push(tail);
                }
            }
        }
        out.push(here);
    }
    out
}
