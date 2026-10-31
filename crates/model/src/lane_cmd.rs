//! Note-lane workflows: moving and copying lanes ([`move_note_lane`]), merging a track's lanes
//! onto its top lane ([`merge_note_lanes`]) and pulling notes out of a clip into lanes of their
//! own ([`extract_notes`], [`explode_notes`]).
//!
//! What a track plays is the merge of its unmuted note lanes (see [`crate::song::flatten_notes`]),
//! and every plan here keeps it:
//!
//! * **Merge** reads the audible notes and controller points of the track and writes them back
//!   as one clip per connected stretch of sound on lane 0. Two notes of one pitch that start on
//!   the same tick in different lanes become one (the later lane's, as a clip holds one note per
//!   tick and key); notes keep the length the track plays them with, so a note that was cut by a
//!   later clip stays cut. A muted lane, a muted clip and a muted note are not audible, so
//!   nothing of them is kept.
//! * **Move** keeps every clip, id and take group of the lane. A copy takes new clip ids and
//!   leaves its take groups behind (the comp clips stay as plain clips). The notes of a track
//!   do not change; the order of its lanes only decides ties (the earlier lane's note first at
//!   one tick and key, the later lane's controller point at one tick).
//! * **Extract** and **explode** put notes on new lanes under clips of the same place, length,
//!   offset, loop and grid as the clip they came from. A clip another clip of its lane partly
//!   hides is refused: the moved notes would sound where the hidden ones did not.

use crate::controller::{ControllerLane, canonical};
use crate::history::Edit;
use crate::song::{
    AutomationPoint, Clip, ClipContent, ClipId, Lane, MAX_NOTE_LANES, Note, Song, Track,
    audible_spans, flatten_controllers, flatten_notes_with,
};
use crate::song_cmd::{bad, check_geometry, note_lane_of, range};
use crate::{DeviceKey, ModelError, RackDoc};
use std::collections::{BTreeMap, BTreeSet};

/// What happens to the notes [`crate::WorkflowCommand::ExtractNotes`] pulls out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractMode {
    /// The notes leave their clip.
    Move,
    /// The clip keeps its notes; the lane gets copies.
    Copy,
}

/// The next free clip id, advancing `next`.
fn fresh(next: &mut u64) -> Result<ClipId, ModelError> {
    let id = ClipId(*next);
    *next = next
        .checked_add(1)
        .ok_or_else(|| bad("clip ids are exhausted"))?;
    Ok(id)
}

/// The edit that sets the song's next clip id once ids were taken from `first` to `next`.
fn next_id_edit(first: u64, next: u64) -> Option<Edit> {
    (next > first).then_some(Edit::NextClipId {
        old: first,
        new: next,
    })
}

fn song_note_track<'d>(song: &'d Song, device: &DeviceKey) -> Result<&'d Track, ModelError> {
    let t = song
        .track(device)
        .ok_or_else(|| ModelError::UnknownKey(format!("track {device}")))?;
    if t.is_audio() {
        return Err(bad(format!(
            "{device} is an audio track, so it has no note lanes"
        )));
    }
    if t.is_automation_only() {
        return Err(bad(format!(
            "{device} is an automation track, so it has no note lanes"
        )));
    }
    Ok(t)
}

fn lane_edit(device: &DeviceKey, index: usize, old: Option<Lane>, new: Option<Lane>) -> Edit {
    Edit::Lane {
        device: device.clone(),
        index,
        old,
        new,
    }
}

impl Song {
    /// Whether [`crate::SongCommand::MoveNoteLane`] with these arguments would be accepted:
    /// both tracks are unfrozen note tracks, the place exists and the move changes something.
    /// The arrangement uses it to draw only the drops the model takes.
    pub fn moves_note_lane(
        &self,
        device: &DeviceKey,
        lane: usize,
        to: (&DeviceKey, usize),
        copy: bool,
    ) -> bool {
        let frozen = |d: &DeviceKey| self.track(d).is_none_or(|t| t.frozen.is_some());
        !frozen(device) && !frozen(to.0) && check_move(self, device, lane, to, copy).is_ok()
    }
}

/// The checks of a lane move: the lane, the places and the limits.
fn check_move<'s>(
    song: &'s Song,
    device: &DeviceKey,
    lane: usize,
    to: (&DeviceKey, usize),
    copy: bool,
) -> Result<&'s Lane, ModelError> {
    let (to_device, to_index) = to;
    let from = song_note_track(song, device)?;
    let source = note_lane_of(from, lane)?;
    let dest = song_note_track(song, to_device)?;
    let same = device == to_device;
    // Where the lane ends up counts the lanes of the target without it, if it leaves.
    let others = if copy || !same {
        dest.note_lane_count()
    } else {
        dest.note_lane_count() - 1
    };
    if to_index > others {
        return Err(range(format!(
            "{to_device} has {others} other note lanes, no place {to_index}"
        )));
    }
    if (copy || !same) && dest.note_lane_count() >= MAX_NOTE_LANES {
        return Err(range(format!(
            "a track has at most {MAX_NOTE_LANES} note lanes"
        )));
    }
    if !copy && !same && from.note_lane_count() == 1 {
        return Err(bad("a track needs at least one note lane"));
    }
    if !copy && same && to_index == lane {
        return Err(bad("the lane is already there"));
    }
    Ok(source)
}

/// Plans `SongCommand::MoveNoteLane`: see the command docs.
pub(crate) fn move_note_lane(
    doc: &RackDoc,
    device: &DeviceKey,
    lane: usize,
    to: (&DeviceKey, usize),
    copy: bool,
) -> Result<Edit, ModelError> {
    let (to_device, to_index) = to;
    let source = check_move(&doc.song, device, lane, to, copy)?;
    let mut moved = source.clone();
    let mut edits = Vec::new();
    let first = doc.song.next_clip_id;
    let mut next = first;
    if copy {
        moved.take_groups.clear();
        for c in &mut moved.clips {
            c.id = fresh(&mut next)?;
        }
    } else {
        edits.push(lane_edit(device, lane, Some(source.clone()), None));
    }
    edits.push(lane_edit(to_device, to_index, None, Some(moved)));
    edits.extend(next_id_edit(first, next));
    Ok(Edit::Compound(edits))
}

/// A track's connected stretches of audible sound: the union of the audible spans of its unmuted
/// note lanes, touching spans joined, in song order.
fn sounding_intervals(track: &Track) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = track
        .note_lanes()
        .filter(|l| !l.muted)
        .flat_map(audible_spans)
        .map(|(_, a, b)| (a, b))
        .collect();
    spans.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (a, b) in spans {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The lane that holds what `track` plays on its note lanes, as clips on one lane.
fn merged_lane(doc: &RackDoc, track: &Track, next: &mut u64) -> Result<Lane, ModelError> {
    let intervals = sounding_intervals(track);
    let notes = flatten_notes_with(track, doc.song.tails);
    let controllers = flatten_controllers(track);
    let mut lane = Lane::notes();
    lane.name = track.lanes[0].name.clone();
    for (a, b) in intervals {
        // One note per tick and key: of two, the later lane's (the last of the flattened order).
        let mut content: Vec<Note> = Vec::new();
        for &(t, length, key, velocity) in notes.iter().filter(|n| n.0 >= a && n.0 < b) {
            let note = Note::new(t - a, length, key, velocity);
            match content.last_mut() {
                Some(last) if (last.tick, last.key) == (note.tick, note.key) => *last = note,
                _ => content.push(note),
            }
        }
        let lanes = canonical(
            controllers
                .iter()
                .map(|(kind, points)| {
                    ControllerLane::new(
                        *kind,
                        points
                            .iter()
                            .filter(|p| p.0 >= a && p.0 < b)
                            .map(|&(tick, value, curve)| AutomationPoint {
                                tick: tick - a,
                                value,
                                curve,
                            })
                            .collect(),
                    )
                })
                .filter(|l| !l.points.is_empty())
                .collect(),
        );
        if content.is_empty() && lanes.is_empty() {
            continue;
        }
        check_geometry(a, b - a, 0, None).map_err(range)?;
        lane.clips.push(Clip {
            id: fresh(next)?,
            start: a,
            length: b - a,
            offset: 0,
            loop_length: None,
            muted: false,
            name: None,
            color: None,
            grid: None,
            scale: None,
            controllers: lanes,
            content: ClipContent::Notes(content.into()),
        });
    }
    Ok(lane)
}

/// Plans `SongCommand::MergeNoteLanes`: see the command docs.
pub(crate) fn merge_note_lanes(doc: &RackDoc, devices: &[DeviceKey]) -> Result<Edit, ModelError> {
    if devices.is_empty() {
        return Err(bad("name at least one track to merge"));
    }
    let mut seen = BTreeSet::new();
    let first = doc.song.next_clip_id;
    let mut next = first;
    let mut edits = Vec::new();
    for device in devices {
        let t = doc
            .song
            .track(device)
            .ok_or_else(|| ModelError::UnknownKey(format!("track {device}")))?;
        let count = t.note_lane_count();
        if count < 2 || !seen.insert(device.clone()) {
            continue;
        }
        let merged = merged_lane(doc, t, &mut next)?;
        edits.push(lane_edit(device, 0, Some(t.lanes[0].clone()), Some(merged)));
        for i in (1..count).rev() {
            edits.push(lane_edit(device, i, Some(t.lanes[i].clone()), None));
        }
    }
    if edits.is_empty() {
        return Err(bad("no track has more than one note lane to merge"));
    }
    edits.extend(next_id_edit(first, next));
    Ok(Edit::Compound(edits))
}

/// The note clip `id`, its track and lane, checked for pulling notes out of it.
fn source_clip(song: &Song, id: ClipId) -> Result<(DeviceKey, usize, &Track, &Clip), ModelError> {
    let (t, lane, c) = song
        .clip(id)
        .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))?;
    let device = t.device.clone();
    let track = song_note_track(song, &device)?;
    note_lane_of(track, lane)?;
    if !matches!(c.content, ClipContent::Notes(_)) {
        return Err(bad(format!("clip {} is not a note clip", id.0)));
    }
    // The notes of a take group's compiled clip are rebuilt from the takes.
    if track.lanes[lane]
        .take_groups
        .iter()
        .any(|g| g.clips.contains(&id))
    {
        return Err(bad(format!("clip {} belongs to a take group", id.0)));
    }
    Ok((device, lane, track, c))
}

/// Refuses a clip that another clip of its lane partly hides (or that is muted and so hides
/// nothing, but whose notes would sound when moved to a lane where it is not hidden).
fn check_whole(track: &Track, lane: usize, c: &Clip) -> Result<(), ModelError> {
    if c.muted {
        return Ok(());
    }
    let mine: i64 = audible_spans(&track.lanes[lane])
        .iter()
        .filter(|s| s.0 == c.id)
        .map(|s| s.2 - s.1)
        .sum();
    if mine != c.length {
        return Err(bad(format!(
            "clip {} is partly hidden by another clip: move the other clip first",
            c.id.0
        )));
    }
    Ok(())
}

/// A clip of `source`'s place, length, offset, loop, grid and scale holding `notes`.
fn sibling(source: &Clip, id: ClipId, notes: Vec<Note>) -> Clip {
    Clip {
        id,
        name: None,
        controllers: Vec::new(),
        content: ClipContent::Notes(notes.into()),
        ..source.clone()
    }
}

/// A lane name after a key.
fn key_name(key: u8) -> String {
    crate::midi_map::note_name(key)
}

/// Plans `WorkflowCommand::ExtractNotes`: see the command docs.
pub(crate) fn extract_notes(
    song: &Song,
    clip: ClipId,
    selection: &[usize],
    mode: ExtractMode,
    to: Option<usize>,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let (device, lane, track, c) = source_clip(song, clip)?;
    let ClipContent::Notes(all) = &c.content else {
        unreachable!("source_clip checked the content");
    };
    let picked: BTreeSet<usize> = selection.iter().copied().collect();
    if picked.is_empty() {
        return Err(bad("select the notes to extract"));
    }
    if let Some(&i) = picked.iter().find(|&&i| i >= all.len()) {
        return Err(range(format!(
            "clip {} has {} notes, not note {i}",
            clip.0,
            all.len()
        )));
    }
    check_whole(track, lane, c)?;
    let moved: Vec<Note> = picked.iter().map(|&i| all[i]).collect();
    let rest: Vec<Note> = all
        .iter()
        .enumerate()
        .filter(|(i, _)| !picked.contains(i))
        .map(|(_, n)| *n)
        .collect();
    let first = song.next_clip_id;
    let mut next = first;
    let id = fresh(&mut next)?;
    let new_clip = sibling(c, id, moved.clone());
    let mut edits = Vec::new();
    let target = match to {
        None => {
            let count = track.note_lane_count();
            if count >= MAX_NOTE_LANES {
                return Err(range(format!(
                    "a track has at most {MAX_NOTE_LANES} note lanes"
                )));
            }
            let mut l = Lane::notes();
            let key = moved[0].key;
            l.name = moved.iter().all(|n| n.key == key).then(|| key_name(key));
            l.muted = track.lanes[lane].muted;
            edits.push(lane_edit(&device, count, None, Some(l)));
            count
        }
        Some(i) => {
            let l = note_lane_of(track, i)?;
            if i == lane {
                return Err(bad("the notes are already on that lane"));
            }
            if l.clips
                .iter()
                .any(|o| o.start < c.end() && c.start < o.end())
            {
                return Err(bad(format!(
                    "lane {i} has a clip where clip {} is: pick an empty place",
                    clip.0
                )));
            }
            i
        }
    };
    edits.push(Edit::Clip {
        device: device.clone(),
        lane: target,
        old: None,
        new: Some(new_clip),
    });
    if mode == ExtractMode::Move {
        let mut kept = c.clone();
        kept.content = ClipContent::Notes(rest.into());
        edits.push(Edit::Clip {
            device,
            lane,
            old: Some(c.clone()),
            new: Some(kept),
        });
    }
    edits.extend(next_id_edit(first, next));
    Ok((Edit::Compound(edits), Some(id)))
}

/// Plans `WorkflowCommand::ExplodeNotes`: see the command docs.
pub(crate) fn explode_notes(
    song: &Song,
    clip: ClipId,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let (device, lane, track, c) = source_clip(song, clip)?;
    let ClipContent::Notes(all) = &c.content else {
        unreachable!("source_clip checked the content");
    };
    let mut by_key: BTreeMap<u8, Vec<Note>> = BTreeMap::new();
    for n in all.iter() {
        by_key.entry(n.key).or_default().push(*n);
    }
    if by_key.len() < 2 {
        return Err(bad(format!(
            "clip {} has {} pitches: nothing to explode",
            clip.0,
            by_key.len()
        )));
    }
    let count = track.note_lane_count();
    if count + by_key.len() > MAX_NOTE_LANES {
        return Err(range(format!(
            "{} pitches need {} more lanes but a track has at most {MAX_NOTE_LANES}",
            by_key.len(),
            by_key.len()
        )));
    }
    check_whole(track, lane, c)?;
    let first = song.next_clip_id;
    let mut next = first;
    let mut edits = Vec::new();
    let mut last = None;
    for (i, (key, notes)) in by_key.into_iter().enumerate() {
        let mut l = Lane::notes();
        l.name = Some(key_name(key));
        l.muted = track.lanes[lane].muted;
        edits.push(lane_edit(&device, count + i, None, Some(l)));
        let id = fresh(&mut next)?;
        last = Some(id);
        edits.push(Edit::Clip {
            device: device.clone(),
            lane: count + i,
            old: None,
            new: Some(sibling(c, id, notes)),
        });
    }
    // The clip stays, without notes (its controllers and its place on the lane are unchanged).
    let mut kept = c.clone();
    kept.content = ClipContent::Notes(Vec::new().into());
    edits.push(Edit::Clip {
        device,
        lane,
        old: Some(c.clone()),
        new: Some(kept),
    });
    edits.extend(next_id_edit(first, next));
    Ok((Edit::Compound(edits), last))
}

impl Song {
    /// Why [`crate::WorkflowCommand::ExplodeNotes`] on `clip` would be refused, if it would.
    pub fn explode_refusal(&self, clip: ClipId) -> Option<String> {
        explode_notes(self, clip).err().map(|e| e.to_string())
    }

    /// Why [`crate::WorkflowCommand::ExtractNotes`] with these arguments would be refused, if it
    /// would.
    pub fn extract_refusal(
        &self,
        clip: ClipId,
        selection: &[usize],
        to: Option<usize>,
    ) -> Option<String> {
        extract_notes(self, clip, selection, ExtractMode::Move, to)
            .err()
            .map(|e| e.to_string())
    }
}
