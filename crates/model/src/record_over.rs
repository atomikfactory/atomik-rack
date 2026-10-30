//! Recording over existing clips: the planning of `WorkflowCommand::LayerRecording` and
//! `WorkflowCommand::EngulfRecording`, and the pure helpers the app uses to build one recorded
//! clip from the passes of a loop take.
//!
//! # Why a take can be hidden
//!
//! On a lane a later-starting clip hides every earlier one where they overlap, whether or not it
//! has notes there (see [`crate::song::audible_spans`]). A plain take that starts before an
//! older clip is therefore hidden by it from the older clip's start, and one that starts later
//! hides the older clip over the overlap. Two commands keep a recording audible without losing
//! what was there:
//!
//! - **Layer** merges the take into the note clip under its start: the clip grows to hold the
//!   take, every old note stays, a new note replaces an old one with the same start and key, and
//!   for each controller kind the take carries, its points replace the old points of that kind
//!   inside the take's span (a recorded value holds until the next point, so interleaving the
//!   two curves would corrupt both). One clip, one undo step.
//! - **Engulf** adds the take as a new clip and makes it the one that plays over its span: an
//!   unmuted clip that starts inside the new clip and ends within it is removed, one that starts
//!   inside it and ends after it has its start trimmed to the new clip's end (its notes before
//!   the new content offset no longer play, so the masked notes inside the span are gone from
//!   the sound), looped clips keep their loop and phase. Clips that start before the new clip
//!   are untouched (the new clip hides them over the overlap, as a later clip always did).

use crate::controller::{ControllerLane, canonical, check_lanes};
use crate::history::Edit;
use crate::song::{AutomationPoint, Clip, ClipContent, ClipId, Lane, LaneKind, MAX_TICK, Note};
use crate::song_cmd::{check_content, check_geometry, check_new_clip, insert_new};
use crate::{DeviceKey, ModelError, RackDoc, rules};
use atomik_device_sdk::Registry;
use std::collections::BTreeMap;

fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadSong(message.into())
}

/// The unmuted, non-looped note clip of `lane` that plays at song tick `tick` (the last-starting
/// one when several cover it), if any. This is the clip a `Layer` recording merges into.
pub fn covering_note_clip(lane: &Lane, tick: i64) -> Option<ClipId> {
    if !lane.kind.is_notes() {
        return None;
    }
    lane.clips
        .iter()
        .filter(|c| {
            !c.muted
                && c.loop_length.is_none()
                && c.content.is_notes()
                && c.start <= tick
                && tick < c.end()
        })
        .max_by_key(|c| (c.start, c.id))
        .map(|c| c.id)
}

/// Replaces the points of `old` inside `[lo, hi)` by `new` (both sorted by tick).
fn merge_points(
    old: &[AutomationPoint],
    new: &[AutomationPoint],
    lo: i64,
    hi: i64,
) -> Vec<AutomationPoint> {
    let mut out: Vec<AutomationPoint> = old
        .iter()
        .filter(|p| p.tick < lo || p.tick >= hi)
        .copied()
        .collect();
    out.extend_from_slice(new);
    out.sort_by_key(|p| p.tick);
    out.dedup_by(|later, earlier| {
        // Equal ticks cannot happen outside the span; keep the later (new) point if they do.
        let same = later.tick == earlier.tick;
        if same {
            *earlier = *later;
        }
        same
    });
    out
}

/// Merges the controller lanes of `take` (shifted by `shift` content ticks, spanning `[lo, hi)`)
/// into `lanes`, kind by kind.
fn merge_controllers(
    lanes: &[ControllerLane],
    take: &[ControllerLane],
    shift: i64,
    (lo, hi): (i64, i64),
) -> Vec<ControllerLane> {
    let mut by_kind: BTreeMap<_, Vec<AutomationPoint>> =
        lanes.iter().map(|l| (l.kind, l.points.clone())).collect();
    for lane in take {
        let shifted: Vec<AutomationPoint> = lane
            .points
            .iter()
            .map(|p| AutomationPoint {
                tick: p.tick + shift,
                ..*p
            })
            .collect();
        let merged = merge_points(
            by_kind.get(&lane.kind).map_or(&[], Vec::as_slice),
            &shifted,
            lo,
            hi,
        );
        by_kind.insert(lane.kind, merged);
    }
    canonical(
        by_kind
            .into_iter()
            .map(|(kind, points)| ControllerLane::new(kind, points))
            .collect(),
    )
}

/// `clip` with `take` merged in (module docs): the result keeps `clip`'s id, start, offset, grid
/// and scale and is long enough to hold the take. `take` must be a note clip that starts inside
/// `clip`; `clip` must be a non-looped note clip.
pub fn layered_clip(clip: &Clip, take: &Clip) -> Result<Clip, ModelError> {
    let (ClipContent::Notes(old), ClipContent::Notes(new)) = (&clip.content, &take.content) else {
        return Err(bad("only note clips can be layered"));
    };
    if clip.loop_length.is_some() {
        return Err(bad(format!(
            "clip {} loops; unloop it before recording into it",
            clip.id.0
        )));
    }
    if take.start < clip.start || take.start >= clip.end() {
        return Err(bad(format!(
            "the take does not start inside clip {}",
            clip.id.0
        )));
    }
    // Content tick of the take's content tick `t`: `clip.offset + (take.start + (t - take.offset) - clip.start)`.
    let shift = clip.offset + take.start - clip.start - take.offset;
    let mut notes: BTreeMap<(i64, u8), Note> = old.iter().map(|n| ((n.tick, n.key), *n)).collect();
    for n in new.iter().filter(|n| n.tick >= take.offset) {
        let tick = n.tick + shift;
        notes.insert((tick, n.key), Note { tick, ..*n });
    }
    let lo = take.offset + shift;
    let controllers = merge_controllers(
        &clip.controllers,
        &take.controllers,
        shift,
        (lo, lo + take.length),
    );
    Ok(Clip {
        length: clip.length.max(take.end() - clip.start),
        controllers,
        content: ClipContent::Notes(notes.into_values().collect()),
        ..clip.clone()
    })
}

/// One clip out of the passes of a loop take (in pass order): it spans from the earliest pass
/// start to the latest pass end, holds the notes of every pass (a later pass replaces a note of
/// an earlier one with the same position and key) and merges the controller lanes per kind (a
/// later pass replaces the points of an earlier one inside its own span). `None` for no passes
/// or for passes that are not note clips. The id, grid and scale come from the first pass.
pub fn merge_laps(laps: &[Clip]) -> Option<Clip> {
    let first = laps.first()?;
    if laps.iter().any(|l| !l.content.is_notes()) {
        return None;
    }
    let start = laps.iter().map(|l| l.start).min()?;
    let end = laps.iter().map(Clip::end).max()?;
    let mut notes: BTreeMap<(i64, u8), Note> = BTreeMap::new();
    let mut controllers: Vec<ControllerLane> = Vec::new();
    for lap in laps {
        let shift = lap.start - start - lap.offset;
        if let ClipContent::Notes(ns) = &lap.content {
            for n in ns {
                let tick = n.tick + shift;
                notes.insert((tick, n.key), Note { tick, ..*n });
            }
        }
        let lo = lap.offset + shift;
        controllers =
            merge_controllers(&controllers, &lap.controllers, shift, (lo, lo + lap.length));
    }
    Some(Clip {
        start,
        length: end - start,
        offset: 0,
        loop_length: None,
        muted: false,
        controllers,
        content: ClipContent::Notes(notes.into_values().collect()),
        ..first.clone()
    })
}

/// Plans `WorkflowCommand::LayerRecording`.
pub(crate) fn plan_layer(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    take: &Clip,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let (track, lane, clip) = doc
        .song
        .clip(id)
        .map(|(t, lane, c)| (t.device.clone(), lane, c))
        .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))?;
    let merged = layered_clip(clip, take)?;
    let desc = rules::descriptor(registry, &rules::device(doc, &track)?.type_id)?;
    let kind = &doc.song.tracks[doc
        .song
        .tracks
        .iter()
        .position(|t| t.device == track)
        .expect("the clip has a track")]
    .lanes[lane]
        .kind;
    check_geometry(
        merged.start,
        merged.length,
        merged.offset,
        merged.loop_length,
    )
    .map_err(ModelError::OutOfRange)?;
    check_lanes(&merged.controllers, MAX_TICK).map_err(ModelError::OutOfRange)?;
    let content = check_content(doc, merged.content.clone(), kind, desc)?;
    Ok((
        Edit::Clip {
            device: track,
            lane,
            old: Some(clip.clone()),
            new: Some(Clip { content, ..merged }),
        },
        Some(id),
    ))
}

/// Plans `WorkflowCommand::EngulfRecording`.
pub(crate) fn plan_engulf(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    lane: usize,
    clip: Clip,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let clip = check_new_clip(doc, registry, device, lane, clip)?;
    let track = doc
        .song
        .track(device)
        .ok_or_else(|| ModelError::UnknownKey(format!("track {device}")))?;
    let lane_ref = &track.lanes[lane];
    if !matches!(lane_ref.kind, LaneKind::Notes) {
        return Err(bad("only a note lane takes a recording over its clips"));
    }
    let (start, end) = (clip.start, clip.end());
    let mut edits: Vec<Edit> = Vec::new();
    for o in lane_ref
        .clips
        .iter()
        .filter(|o| !o.muted && o.start >= start && o.start < end)
    {
        if o.end() <= end {
            edits.push(Edit::Clip {
                device: device.clone(),
                lane,
                old: Some(o.clone()),
                new: None,
            });
            continue;
        }
        let cut = end - o.start;
        let offset = match o.loop_length {
            Some(n) => (o.offset + cut).rem_euclid(n),
            None => o.offset + cut,
        };
        check_geometry(end, o.length - cut, offset, o.loop_length)
            .map_err(ModelError::OutOfRange)?;
        edits.push(Edit::Clip {
            device: device.clone(),
            lane,
            old: Some(o.clone()),
            new: Some(Clip {
                start: end,
                length: o.length - cut,
                offset,
                ..o.clone()
            }),
        });
    }
    let (insert, id) = insert_new(doc, device.clone(), lane, clip)?;
    edits.insert(0, insert);
    Ok((Edit::Compound(edits), Some(id)))
}
