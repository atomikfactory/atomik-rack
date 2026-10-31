//! Loops of clips turned into clips: [`convert_loop`] (the `ConvertLoop` command) and
//! [`loop_instance`] (`DuplicateLoopInstance`).
//!
//! A looped clip shows its content window `[0, loop)` again and again from `offset mod loop`;
//! each repetition is one *piece* of the clip. The conversions keep what the lane plays:
//!
//! * **Separate** turns each piece into a clip of its own (the first keeps the clip's id) with
//!   the same content and the offset its piece started at, so the flattened notes, points and
//!   audio regions are the ones the loop gave.
//! * **Single** unrolls a note or automation clip into one clip without loop or offset that
//!   holds every repetition's content; it emits exactly the events the loop emitted. A note
//!   clip with swing cannot (the swing counts steps from the content start, which unrolling
//!   moves); an audio clip cannot (it has no content to copy).
//! * **Loop instance** makes one repetition a clip of its own and leaves the repetitions before
//!   and after it as looped clips.
//!
//! Clips that overlap the looped clip on its lane are refused: a later clip hides an earlier one
//! by `(start, id)`, and the new clips would take other places in that order.

use crate::controller::{ControllerLane, canonical};
use crate::history::Edit;
use crate::song::{
    AutomationPoint, Clip, ClipContent, ClipId, Curve, Note, automation_piece, controller_piece,
};
use crate::song_cmd::{bad, check_content, check_geometry, find_clip, range};
use crate::{DeviceKey, ModelError, RackDoc, rules};
use atomik_device_sdk::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// What a loop becomes (see [`crate::WorkflowCommand::ConvertLoop`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopConvert {
    /// One clip that holds every repetition.
    Single,
    /// One clip per repetition.
    Separate,
}

/// What a clip shows, as pieces `(song start, song end, content start)`: one piece, or with a
/// loop one per repetition (the content range `[0, loop)` repeats from `offset mod loop`).
pub(crate) fn content_pieces(c: &Clip) -> Vec<(i64, i64, i64)> {
    let end = c.end();
    match c.loop_length {
        Some(n) if n > 0 => {
            let mut out = Vec::new();
            let mut t = c.start;
            let mut cs = c.offset.rem_euclid(n);
            while t < end {
                let e = (t + n - cs).min(end);
                out.push((t, e, cs));
                t = e;
                cs = 0;
            }
            out
        }
        _ => vec![(c.start, end, c.offset)],
    }
}

type Found<'d> = (DeviceKey, usize, &'d Clip);

/// The looped clips of `ids` in song order, and an error naming the first unknown clip.
fn looped_clips<'d>(doc: &'d RackDoc, ids: &[ClipId]) -> Result<Vec<Found<'d>>, ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    if wanted.is_empty() {
        return Err(bad("no clips are selected"));
    }
    let mut clips = Vec::new();
    for id in &wanted {
        clips.push(find_clip(doc, *id)?);
    }
    clips.retain(|(_, _, c)| c.loop_length.is_some());
    if clips.is_empty() {
        return Err(bad("none of the clips loops"));
    }
    clips.sort_by_key(|(_, _, c)| (c.start, c.id));
    Ok(clips)
}

/// Refuses a clip another clip of its lane overlaps (see the module docs). A muted clip hides
/// nothing, so it is never refused.
fn check_uncrowded(doc: &RackDoc, (device, lane, c): &Found<'_>) -> Result<(), ModelError> {
    if c.muted {
        return Ok(());
    }
    let crowded = doc
        .song
        .track(device)
        .and_then(|t| t.lanes.get(*lane))
        .is_some_and(|l| {
            l.clips
                .iter()
                .any(|o| o.id != c.id && !o.muted && o.start < c.end() && c.start < o.end())
        });
    if crowded {
        return Err(bad(format!(
            "another clip overlaps clip {}: move it away before converting the loop",
            c.id.0
        )));
    }
    Ok(())
}

/// The next free clip id, advancing `next`.
fn fresh(next: &mut u64) -> Result<ClipId, ModelError> {
    let id = ClipId(*next);
    *next = next
        .checked_add(1)
        .ok_or_else(|| bad("clip ids are exhausted"))?;
    Ok(id)
}

/// The clip of piece `(from, to, cs)` of `c`: no loop, the content window that piece showed.
fn piece_clip(c: &Clip, id: ClipId, (from, to, cs): (i64, i64, i64)) -> Result<Clip, ModelError> {
    let clip = Clip {
        id,
        start: from,
        length: to - from,
        offset: if c.content.is_audio() { 0 } else { cs },
        loop_length: None,
        ..c.clone()
    };
    check_geometry(clip.start, clip.length, clip.offset, None).map_err(range)?;
    Ok(clip)
}

fn replace(device: &DeviceKey, lane: usize, old: &Clip, new: Clip) -> Edit {
    Edit::Clip {
        device: device.clone(),
        lane,
        old: Some(old.clone()),
        new: Some(new),
    }
}

fn insert(device: &DeviceKey, lane: usize, clip: Clip) -> Edit {
    Edit::Clip {
        device: device.clone(),
        lane,
        old: None,
        new: Some(clip),
    }
}

/// Plans `ConvertLoop`: see the command docs. The clips that do not loop are left alone.
pub(crate) fn convert_loop(
    doc: &RackDoc,
    registry: &Registry,
    ids: &[ClipId],
    mode: LoopConvert,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let clips = looped_clips(doc, ids)?;
    let first = doc.song.next_clip_id;
    let mut next = first;
    let mut edits = Vec::new();
    for found in &clips {
        let (device, lane, c) = found;
        match mode {
            LoopConvert::Separate => {
                check_uncrowded(doc, found)?;
                for (k, piece) in content_pieces(c).into_iter().enumerate() {
                    if k == 0 {
                        edits.push(replace(device, *lane, c, piece_clip(c, c.id, piece)?));
                    } else {
                        let id = fresh(&mut next)?;
                        edits.push(insert(device, *lane, piece_clip(c, id, piece)?));
                    }
                }
            }
            LoopConvert::Single => {
                let single = unrolled(doc, registry, found)?;
                edits.push(replace(device, *lane, c, single));
            }
        }
    }
    let last = (next > first).then(|| ClipId(next - 1));
    if next > first {
        edits.push(Edit::NextClipId {
            old: first,
            new: next,
        });
    }
    Ok((Edit::Compound(edits), last.or(Some(clips[0].2.id))))
}

/// The single clip a looped note or automation clip unrolls into (see the module docs).
fn unrolled(
    doc: &RackDoc,
    registry: &Registry,
    (device, lane, c): &Found<'_>,
) -> Result<Clip, ModelError> {
    let lane_kind = &doc
        .song
        .track(device)
        .and_then(|t| t.lanes.get(*lane))
        .ok_or_else(|| bad(format!("{device} has no lane {lane}")))?
        .kind;
    let pieces = content_pieces(c);
    let content = match &c.content {
        ClipContent::Audio(_) => {
            return Err(bad(
                "an audio loop cannot become one clip: convert it to separate clips instead",
            ));
        }
        ClipContent::Notes(all) => {
            if c.grid.is_some_and(|g| g.swing > 0) {
                return Err(bad(format!(
                    "clip {} has swing, which counts steps from the content start: remove the swing or convert the loop to separate clips",
                    c.id.0
                )));
            }
            let tails = doc.song.tails;
            let mut notes: Vec<Note> = Vec::new();
            for &(from, to, cs) in &pieces {
                let window = cs..cs + (to - from);
                for n in all.iter().filter(|n| window.contains(&n.tick)) {
                    let length = if tails {
                        n.length
                    } else {
                        n.length.min(window.end - n.tick)
                    };
                    notes.push(Note {
                        tick: from + (n.tick - cs) - c.start,
                        length,
                        ..*n
                    });
                }
            }
            ClipContent::Notes(notes.into())
        }
        ClipContent::Automation(points) => {
            let mut out: Vec<AutomationPoint> = Vec::new();
            if !points.is_empty() {
                for &(from, to, cs) in &pieces {
                    out.extend(
                        automation_piece(points, (from - c.start, to - c.start), cs)
                            .into_iter()
                            .map(|(tick, value, curve)| AutomationPoint { tick, value, curve }),
                    );
                }
            }
            ClipContent::Automation(out)
        }
    };
    let mut by_kind: BTreeMap<_, Vec<AutomationPoint>> = BTreeMap::new();
    for lane in &c.controllers {
        let all = by_kind.entry(lane.kind).or_default();
        for &(from, to, cs) in &pieces {
            all.extend(
                controller_piece(&lane.points, (from - c.start, to - c.start), cs)
                    .into_iter()
                    .map(|(tick, value, curve): (i64, f32, Curve)| AutomationPoint {
                        tick,
                        value,
                        curve,
                    }),
            );
        }
    }
    let controllers = canonical(
        by_kind
            .into_iter()
            .map(|(kind, points)| ControllerLane::new(kind, points))
            .collect(),
    );
    let desc = rules::descriptor(registry, &rules::device(doc, device)?.type_id)?;
    let content = check_content(doc, content, lane_kind, desc)?;
    let clip = Clip {
        offset: 0,
        loop_length: None,
        content,
        controllers,
        ..(*c).clone()
    };
    check_geometry(clip.start, clip.length, 0, None).map_err(range)?;
    Ok(clip)
}

/// Plans `DuplicateLoopInstance`: see the command docs.
pub(crate) fn loop_instance(
    doc: &RackDoc,
    id: ClipId,
    index: usize,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let clips = looped_clips(doc, &[id])?;
    let found = &clips[0];
    check_uncrowded(doc, found)?;
    let (device, lane, c) = found;
    let pieces = content_pieces(c);
    let Some(&piece) = pieces.get(index) else {
        return Err(range(format!(
            "clip {} has {} repetitions, none at index {index}",
            c.id.0,
            pieces.len()
        )));
    };
    let first = doc.song.next_clip_id;
    let mut next = first;
    let mut edits = Vec::new();
    // The repetitions before the instance stay a looped clip under the clip's id.
    let instance_id = if index > 0 {
        let left = Clip {
            length: piece.0 - c.start,
            ..(*c).clone()
        };
        check_geometry(left.start, left.length, left.offset, left.loop_length).map_err(range)?;
        edits.push(replace(device, *lane, c, left));
        None
    } else {
        Some(c.id)
    };
    // Those after it start on a repetition boundary, so they loop from content tick 0.
    if index + 1 < pieces.len() {
        let right = Clip {
            id: fresh(&mut next)?,
            start: piece.1,
            length: c.end() - piece.1,
            offset: 0,
            ..(*c).clone()
        };
        check_geometry(right.start, right.length, 0, right.loop_length).map_err(range)?;
        edits.push(insert(device, *lane, right));
    }
    let instance_id = match instance_id {
        Some(id) => id,
        None => fresh(&mut next)?,
    };
    let instance = piece_clip(c, instance_id, piece)?;
    if instance_id == c.id {
        edits.push(replace(device, *lane, c, instance));
    } else {
        edits.push(insert(device, *lane, instance));
    }
    if next > first {
        edits.push(Edit::NextClipId {
            old: first,
            new: next,
        });
    }
    Ok((Edit::Compound(edits), Some(instance_id)))
}
