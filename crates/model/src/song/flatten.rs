//! Flattening of clips into song-time events (what the app turns into the engine `SongInput`).
//!
//! Rules (all integer tick maths, deterministic):
//! - **Audible spans.** On a lane, unmuted clips are ordered by `(start, id)`; a later
//!   clip hides every earlier one where they overlap. A clip is audible on its span
//!   `[start, start + length)` minus the spans of all later unmuted clips. Muted clips neither
//!   play nor hide anything. Audible spans of one lane never overlap.
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
    AutomationPoint, Clip, ClipContent, ClipId, Curve, FadeCurve, Lane, LaneKind, Song, Track,
};
use crate::controller::{ControllerKind, value_at as lane_value_at};
use std::collections::BTreeMap;

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

/// The notes a track's note lane plays, as `(song tick, length, key, velocity)` sorted by
/// `(tick, key)` (rules in the module docs). Track mute and solo are not applied here; see
/// [`effective_tracks`].
pub fn flatten_notes(track: &Track) -> Vec<(i64, i64, u8, f32)> {
    let Some(lane) = track.lanes.iter().find(|l| l.kind.is_notes()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
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

/// The controller data a track's note lane plays: for each [`ControllerKind`] in use (sorted by
/// kind) the points `(song tick, value, curve)` with strictly increasing ticks (rules in the
/// module docs). Empty without controller lanes. Track mute and solo are not applied here.
pub fn flatten_controllers(track: &Track) -> Vec<FlatController> {
    let Some(lane) = track.lanes.iter().find(|l| l.kind.is_notes()) else {
        return Vec::new();
    };
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
    by_kind.retain(|_, v| !v.is_empty());
    by_kind.into_iter().collect()
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

/// One stretch of audio the track player plays, in song frames. Regions of one
/// track are sorted by `song_start_frame` and never overlap.
///
/// The player starts reading `asset` at frame `source_offset` at song frame `song_start_frame`
/// and plays `frames` frames, cut earlier where the asset ends (the model does not know the
/// asset's length here).
///
/// **Fades are measured within the original clip, not reset at overlap cut points.** A clip
/// that a later clip covers in part is flattened into several regions; each region carries the
/// whole clip's fade lengths plus where it sits in the clip (`clip_pos`, `clip_frames`), so the
/// fade-in plays only over the clip's first `fade_in` frames and the fade-out only over its
/// last `fade_out` frames. A cut inside a clip makes no fade of its own (the cut is a hard
/// edge; the covering clip supplies its own fade-in).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioRegion {
    /// Asset id.
    pub asset: String,
    /// Song frame where the region starts (`Song::frame_at` of its tick).
    pub song_start_frame: u64,
    /// Asset frame read at the first region frame.
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
}

impl AudioRegion {
    /// The fade gain (in `0.0..=1.0`, without the clip gain) at region frame `i`.
    ///
    /// With `n = clip_pos + i` the frame within the original clip: the fade-in gives
    /// `curve(n / fade_in)` for `n < fade_in`, the fade-out gives
    /// `curve((clip_frames - n) / fade_out)` for `n >= clip_frames - fade_out`, and both
    /// multiply (they overlap only when `fade_in + fade_out > clip_frames`). `curve(t)` is
    /// `t` for [`FadeCurve::Linear`] and `sin(t * pi / 2)` for [`FadeCurve::EqualPower`]. This is
    /// the reference definition; the engine may evaluate it with a table.
    pub fn fade_gain(&self, i: u64) -> f32 {
        let n = self.clip_pos + i;
        let curve = |t: f64| match self.fade_curve {
            FadeCurve::Linear => t,
            FadeCurve::EqualPower => (t * std::f64::consts::FRAC_PI_2).sin(),
        };
        let mut g = 1.0f64;
        if n < self.fade_in {
            g *= curve(n as f64 / self.fade_in as f64);
        }
        if n + self.fade_out >= self.clip_frames && self.fade_out > 0 {
            g *= curve((self.clip_frames - n) as f64 / self.fade_out as f64);
        }
        g as f32
    }
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
/// - Audio does not stretch: the source offset advances one asset frame per song frame
///   from the clip start, so a region that starts `d` frames into its clip reads from
///   `source_offset + d`.
/// - `gain` is `db_to_gain(gain_db)`; fades are the original clip's (see [`AudioRegion`]).
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
    let mut out = Vec::new();
    for (id, a, b) in audible_spans(lane) {
        let clip = by_id[&id];
        let ClipContent::Audio(audio) = &clip.content else {
            continue;
        };
        let (clip_start, clip_end) = (
            map.song_frame(clip.start, sr),
            map.song_frame(clip.end(), sr),
        );
        let (start, end) = (map.song_frame(a, sr), map.song_frame(b, sr));
        if end <= start {
            continue;
        }
        let clip_frames = clip_end - clip_start;
        out.push(AudioRegion {
            asset: audio.asset.clone(),
            song_start_frame: start,
            source_offset: audio.source_offset + (start - clip_start),
            frames: end - start,
            gain: atomik_core::db_to_gain(audio.gain_db),
            fade_in: audio.fade_in.min(clip_frames),
            fade_out: audio.fade_out.min(clip_frames),
            fade_curve: audio.fade_curve,
            clip_pos: start - clip_start,
            clip_frames,
        });
    }
    out
}
