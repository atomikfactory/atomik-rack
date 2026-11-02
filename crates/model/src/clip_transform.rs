//! Whole-clip transforms of a selection: reverse, scale tempo, chop and automation cleanup.
//!
//! * [`reverse_clips`] mirrors what each note, automation and controller clip shows inside its
//!   content window (the loop of a looped clip), and flips the `reverse` flag of audio clips;
//! * [`scale_clips`] changes the speed of a selection by a percentage: clip positions (relative
//!   to the earliest start), lengths, loops, notes, controller and automation points all scale,
//!   and audio clips are warped to follow;
//! * [`chop_clips`] cuts every clip into pieces of one length, from its own start.
//! * [`cleanup_automation`] thins the points of automation clips by a fraction of the range.
//!
//! Each is one `Edit::Compound`, so one undo step.

use crate::automation_rec::{ThinLevel, thin_points};
use crate::controller::ControllerLane;
use crate::history::Edit;
use crate::song::{AudioClipContent, AutomationPoint, Clip, ClipContent, GainPoint, Note};
use crate::song_cmd::{bad, check_geometry, find_clip, range, split_pair};
use crate::warp::{WARP_BPM_MAX, WARP_BPM_MIN, WarpMarker};
use crate::{ClipId, DeviceKey, ModelError, RackDoc};
use std::collections::{BTreeMap, BTreeSet};

/// The slowest and fastest a selection can be scaled to, in percent of its tempo.
pub const SCALE_PERCENT_MIN: f64 = 10.0;
/// See [`SCALE_PERCENT_MIN`].
pub const SCALE_PERCENT_MAX: f64 = 1000.0;

type Found<'d> = (DeviceKey, usize, &'d Clip);

fn selection<'d>(doc: &'d RackDoc, ids: &[ClipId]) -> Result<Vec<Found<'d>>, ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    if wanted.is_empty() {
        return Err(bad("no clips are selected"));
    }
    let mut clips = Vec::new();
    for id in &wanted {
        clips.push(find_clip(doc, *id)?);
    }
    clips.sort_by_key(|(_, _, c)| (c.start, c.id));
    Ok(clips)
}

fn replace(device: &DeviceKey, lane: usize, old: &Clip, new: Clip) -> Edit {
    Edit::Clip {
        device: device.clone(),
        lane,
        old: Some(old.clone()),
        new: Some(new),
    }
}

// ---------------------------------------------------------------------------------- reverse

/// The content window of a clip: what its content shows, or the range `[0, loop)` a loop
/// repeats.
fn window(c: &Clip) -> (i64, i64) {
    match c.loop_length {
        Some(n) => (0, n),
        None => (c.offset, c.offset + c.length),
    }
}

/// Mirrors the points inside `[ws, we)` about the middle of the window and leaves the others.
/// The curve that leaves a point belongs to the segment after it, so the curves move with the
/// segments: the first mirrored point takes the curve of the last segment, and the last takes
/// the curve the old last point carried, which keeps a double reverse exact.
pub(crate) fn mirror_points(points: &[AutomationPoint], ws: i64, we: i64) -> Vec<AutomationPoint> {
    let from = points.partition_point(|p| p.tick < ws);
    let to = points.partition_point(|p| p.tick < we);
    let inside = &points[from..to];
    let n = inside.len();
    let mut out: Vec<AutomationPoint> = points[..from].to_vec();
    for j in 0..n {
        let src = inside[n - 1 - j];
        let curve = if j + 1 < n {
            inside[n - 2 - j].curve
        } else {
            inside[n - 1].curve
        };
        out.push(AutomationPoint {
            tick: ws + we - 1 - src.tick,
            value: src.value,
            curve,
        });
    }
    out.extend_from_slice(&points[to..]);
    out
}

/// Mirrors the notes that start inside `[ws, we)`. `graphical` mirrors each note as a bar on a
/// piano roll (it ends where it began, so a long note lands at the other end); otherwise only
/// the starts are mirrored (`p` becomes the window length minus `p`, the first stays first) and
/// every note keeps its length. A note that runs past the window is cut at its end.
fn mirror_notes(notes: &[Note], ws: i64, we: i64, graphical: bool) -> Vec<Note> {
    let w = we - ws;
    let mut out: BTreeMap<(i64, u8), Note> = BTreeMap::new();
    for n in notes {
        let mut m = *n;
        if (ws..we).contains(&n.tick) {
            let rel = n.tick - ws;
            let length = n.length.min(we - n.tick);
            if graphical {
                m.tick = ws + (w - rel - length);
                m.length = length;
            } else {
                m.tick = ws + (w - rel) % w;
                m.length = length;
            }
        }
        out.insert((m.tick, m.key), m);
    }
    out.into_values().collect()
}

fn reverse_lane(lane: &ControllerLane, ws: i64, we: i64) -> ControllerLane {
    ControllerLane::new(lane.kind, mirror_points(&lane.points, ws, we))
}

/// Plans `ReverseClips`.
pub(crate) fn reverse_clips(
    doc: &RackDoc,
    ids: &[ClipId],
    graphical: bool,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let clips = selection(doc, ids)?;
    // Audio clips toggle together: they all play backwards unless they all do already.
    let to_reverse = clips
        .iter()
        .any(|(_, _, c)| matches!(&c.content, ClipContent::Audio(a) if !a.reverse));
    let mut edits = Vec::new();
    for (device, lane, c) in clips.iter().map(|(d, l, c)| (d, l, *c)) {
        let (ws, we) = window(c);
        let mut new = c.clone();
        match &c.content {
            ClipContent::Notes(notes) => {
                new.content = ClipContent::Notes(mirror_notes(notes, ws, we, graphical).into());
                new.controllers = c
                    .controllers
                    .iter()
                    .map(|l| reverse_lane(l, ws, we))
                    .collect();
            }
            ClipContent::Automation(points) => {
                new.content = ClipContent::Automation(mirror_points(points, ws, we));
            }
            ClipContent::Audio(a) => {
                if a.reverse != to_reverse {
                    if to_reverse && c.loop_length.is_some() {
                        return Err(bad(format!(
                            "clip {} loops and cannot play backwards: convert the loop to separate clips first",
                            c.id.0
                        )));
                    }
                    // A reversed clip cannot be warped or transposed, so reversing a warped
                    // clip switches the warp off (its mode, tempo and markers stay).
                    let mut warp = a.warp.clone();
                    if to_reverse {
                        warp.enabled = false;
                        warp.transpose_cents = 0;
                    }
                    new.content = ClipContent::Audio(AudioClipContent {
                        reverse: to_reverse,
                        warp,
                        ..a.clone()
                    });
                }
            }
        }
        if new != *c {
            edits.push(replace(device, *lane, c, new));
        }
    }
    if edits.is_empty() {
        return Err(bad("nothing in the selection changes when reversed"));
    }
    Ok((Edit::Compound(edits), None))
}

// ------------------------------------------------------------------------------------ scale

/// `x` scaled to a tempo of `percent` per cent: 200 halves it, 50 doubles it.
fn st(x: i64, percent: f64) -> i64 {
    (x as f64 * 100.0 / percent).round() as i64
}

fn su(x: u64, percent: f64) -> u64 {
    (x as f64 * 100.0 / percent).round() as u64
}

fn scale_points(points: &[AutomationPoint], percent: f64) -> Vec<AutomationPoint> {
    let mut out: BTreeMap<i64, AutomationPoint> = BTreeMap::new();
    for p in points {
        let tick = st(p.tick, percent);
        out.insert(tick, AutomationPoint { tick, ..*p });
    }
    out.into_values().collect()
}

fn scale_notes(notes: &[Note], percent: f64) -> Vec<Note> {
    let mut out: BTreeMap<(i64, u8), Note> = BTreeMap::new();
    for n in notes {
        let tick = st(n.tick, percent);
        let end = st(n.tick + n.length, percent);
        out.insert(
            (tick, n.key),
            Note {
                tick,
                length: (end - tick).max(1),
                ..*n
            },
        );
    }
    out.into_values().collect()
}

fn scale_audio(
    doc: &RackDoc,
    c: &Clip,
    a: &AudioClipContent,
    percent: f64,
) -> Result<AudioClipContent, ModelError> {
    if a.reverse {
        return Err(bad(format!(
            "clip {} plays backwards and cannot be warped: un-reverse it first",
            c.id.0
        )));
    }
    let mut out = a.clone();
    // The asset keeps its place in the clip: the clip gets shorter by the factor, so the
    // source tempo drops by it, which is how a warped clip plays that much faster.
    let current = if a.warp.enabled {
        a.warp.source_bpm
    } else {
        doc.song.tempo_table().map().bpm(c.start as f64)
    };
    let bpm = current * 100.0 / percent;
    if !(WARP_BPM_MIN..=WARP_BPM_MAX).contains(&bpm) {
        return Err(range(format!(
            "clip {}: a source tempo of {bpm:.1} BPM is outside {WARP_BPM_MIN}..={WARP_BPM_MAX}",
            c.id.0
        )));
    }
    out.warp.enabled = true;
    out.warp.source_bpm = bpm;
    let mut markers: Vec<WarpMarker> = Vec::new();
    for m in &a.warp.markers {
        let tick = st(m.tick, percent);
        if markers.last().is_none_or(|l| l.tick < tick) {
            markers.push(WarpMarker {
                tick,
                source: m.source,
            });
        }
    }
    out.warp.markers = markers;
    out.fade_in = su(a.fade_in, percent);
    out.fade_out = su(a.fade_out, percent);
    let mut env: Vec<GainPoint> = Vec::new();
    for p in &a.envelope {
        let frame = su(p.frame, percent);
        if env.last().is_none_or(|l| l.frame < frame) {
            env.push(GainPoint {
                frame,
                gain_db: p.gain_db,
            });
        }
    }
    out.envelope = env;
    Ok(out)
}

fn check_percent(percent: f64) -> Result<(), ModelError> {
    if !(SCALE_PERCENT_MIN..=SCALE_PERCENT_MAX).contains(&percent) {
        return Err(range(format!(
            "a tempo of {percent}% is outside {SCALE_PERCENT_MIN}..={SCALE_PERCENT_MAX}%"
        )));
    }
    if (percent - 100.0).abs() < f64::EPSILON {
        return Err(bad("a tempo of 100% changes nothing"));
    }
    Ok(())
}

/// One clip at a tempo of `percent` per cent, its geometry scaled about the tick `anchor`
/// (which stays where it is) and its content with it: notes, controller and automation points
/// scale from the start of the content, an audio clip is warped to follow.
fn scaled_clip(doc: &RackDoc, c: &Clip, percent: f64, anchor: i64) -> Result<Clip, ModelError> {
    let start = anchor + st(c.start - anchor, percent);
    let end = anchor + st(c.end() - anchor, percent);
    let mut new = Clip {
        start,
        length: (end - start).max(1),
        offset: st(c.offset, percent),
        loop_length: c.loop_length.map(|n| st(n, percent).max(1)),
        ..c.clone()
    };
    match &c.content {
        ClipContent::Notes(notes) => {
            new.content = ClipContent::Notes(scale_notes(notes, percent).into());
            new.controllers = c
                .controllers
                .iter()
                .map(|l| ControllerLane::new(l.kind, scale_points(&l.points, percent)))
                .collect();
        }
        ClipContent::Automation(points) => {
            new.content = ClipContent::Automation(scale_points(points, percent));
        }
        ClipContent::Audio(a) => {
            new.content = ClipContent::Audio(scale_audio(doc, c, a, percent)?);
        }
    }
    check_geometry(new.start, new.length, new.offset, new.loop_length).map_err(range)?;
    Ok(new)
}

/// Plans `ScaleTempo`.
pub(crate) fn scale_clips(
    doc: &RackDoc,
    ids: &[ClipId],
    percent: f64,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    check_percent(percent)?;
    let clips = selection(doc, ids)?;
    let base = clips.iter().map(|(_, _, c)| c.start).min().unwrap_or(0);
    let mut edits = Vec::new();
    for (device, lane, c) in clips.iter().map(|(d, l, c)| (d, l, *c)) {
        let new = scaled_clip(doc, c, percent, base)?;
        edits.push(replace(device, *lane, c, new));
    }
    Ok((Edit::Compound(edits), None))
}

/// Plans `StretchClips`: every clip is scaled by the same ratio about its own fixed edge, the
/// end when the start edge is dragged (`from_start`) and the start when the end edge is.
pub(crate) fn stretch_clips(
    doc: &RackDoc,
    ids: &[ClipId],
    percent: f64,
    from_start: bool,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    check_percent(percent)?;
    let clips = selection(doc, ids)?;
    let mut edits = Vec::new();
    for (device, lane, c) in clips.iter().map(|(d, l, c)| (d, l, *c)) {
        let anchor = if from_start { c.end() } else { c.start };
        let new = scaled_clip(doc, c, percent, anchor)?;
        edits.push(replace(device, *lane, c, new));
    }
    Ok((Edit::Compound(edits), None))
}

// ------------------------------------------------------------------------------------ chop

/// Plans `ChopClips`.
pub(crate) fn chop_clips(
    doc: &RackDoc,
    ids: &[ClipId],
    length: i64,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    if length <= 0 {
        return Err(bad("the pieces need a length above 0"));
    }
    let first = doc.song.next_clip_id;
    let mut next = first;
    let mut edits = Vec::new();
    for (device, lane, c) in selection(doc, ids)? {
        if c.length <= length {
            continue;
        }
        let mut rest = c.clone();
        let mut at = c.start + length;
        let mut left_of_original = true;
        while at < c.end() {
            let (left, mut right) = split_pair(doc, &rest, at)?;
            if left_of_original {
                edits.push(replace(&device, lane, c, left));
                left_of_original = false;
            } else {
                edits.push(Edit::Clip {
                    device: device.clone(),
                    lane,
                    old: None,
                    new: Some(left),
                });
            }
            right.id = ClipId(next);
            next = next
                .checked_add(1)
                .ok_or_else(|| bad("clip ids are exhausted"))?;
            rest = right;
            at += length;
        }
        edits.push(Edit::Clip {
            device,
            lane,
            old: None,
            new: Some(rest),
        });
    }
    if next == first {
        return Err(bad("every clip is already as short as the pieces"));
    }
    edits.push(Edit::NextClipId {
        old: first,
        new: next,
    });
    Ok((Edit::Compound(edits), Some(ClipId(next - 1))))
}

// ---------------------------------------------------------------------------------- cleanup

/// Plans `CleanupAutomation`: every automation clip of the selection is thinned with
/// [`thin_points`] at `level` (a fraction of its parameter's range). Other clips are left alone;
/// a selection where nothing changes rejects.
pub(crate) fn cleanup_automation(
    doc: &RackDoc,
    registry: &atomik_device_sdk::Registry,
    ids: &[ClipId],
    level: ThinLevel,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let clips = selection(doc, ids)?;
    let mut edits = Vec::new();
    for (device, lane, c) in clips.iter().map(|(d, l, c)| (d, *l, *c)) {
        let ClipContent::Automation(points) = &c.content else {
            continue;
        };
        let range = automation_range(doc, registry, device, lane);
        let thinned = thin_points(points, level.fraction() * range);
        if thinned.len() != points.len() {
            let mut new = c.clone();
            new.content = ClipContent::Automation(thinned);
            edits.push(replace(device, lane, c, new));
        }
    }
    if edits.is_empty() {
        return Err(bad("no automation point can be removed at this level"));
    }
    Ok((Edit::Compound(edits), None))
}

/// The size of the range of the parameter an automation lane drives (1 when it is unknown, as
/// for a plugin parameter that is gone).
fn automation_range(
    doc: &RackDoc,
    registry: &atomik_device_sdk::Registry,
    device: &DeviceKey,
    lane: usize,
) -> f32 {
    let param = doc
        .song
        .track(device)
        .and_then(|t| t.lanes.get(lane))
        .and_then(|l| match &l.kind {
            crate::song::LaneKind::Automation(p) => Some(p.clone()),
            _ => None,
        });
    let desc = crate::song_cmd::device_descriptor(doc, registry, device).ok();
    match (desc, param) {
        (Some(d), Some(p)) => d
            .param_by_key(&p)
            .map_or(1.0, |p| (p.max - p.min).max(f32::EPSILON)),
        _ => 1.0,
    }
}
