//! Planning of the take commands (`NewTakeGroup`, `PickTake`, `SetCompSegment`, `DeleteTake`,
//! `FlattenTakes`): each is one compound edit that changes the group list and swaps the group's
//! compiled lane clips (see [`crate::takes`]).

use crate::history::Edit;
use crate::song::{Clip, ClipContent, ClipId, LaneKind, MAX_TICK, SongCommand};
use crate::song_cmd::{bad, check_audio, check_new_clip, range, track_index};
use crate::takes::{
    CompSegment, TakeGroup, check_comp, clear_segment, comp_clips, comp_with_take_inserted,
    comp_with_take_moved, comp_within, comp_without_take, layered_comp, set_segment,
};
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// Most takes one group may hold.
pub const MAX_TAKES: usize = 256;

/// What happens to a group's compiled lane clips.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clips {
    /// Remove the old ones and compile the new comp.
    Recompile,
    /// Remove the old ones; the group goes.
    Remove,
    /// Keep them as plain clips; the group goes.
    Keep,
}

/// The group `id` with its device and lane index.
fn find(doc: &RackDoc, id: u32) -> Result<(DeviceKey, usize, &TakeGroup), ModelError> {
    doc.song
        .take_group(id)
        .map(|(t, lane, g)| (t.device.clone(), lane, g))
        .ok_or_else(|| ModelError::UnknownKey(format!("take group {id}")))
}

/// `take` must be a take of `g`.
fn check_take(g: &TakeGroup, take: usize) -> Result<(), ModelError> {
    if take >= g.takes.len() {
        return Err(range(format!(
            "group {} has {} takes, not take {take}",
            g.id,
            g.takes.len()
        )));
    }
    Ok(())
}

/// The edit that replaces group `old` (`None`: a new group) of lane `lane` by `new` (`None`: the
/// group goes), handling the compiled clips as `clips` says.
fn build(
    doc: &RackDoc,
    device: &DeviceKey,
    lane: usize,
    old: Option<&TakeGroup>,
    mut new: Option<TakeGroup>,
    clips: Clips,
) -> Result<Edit, ModelError> {
    let track = &doc.song.tracks[track_index(doc, device)?];
    let lane_ref = &track.lanes[lane];
    // A comp cuts a take anywhere, which a repetition of an audio loop cannot follow.
    if let Some(group) = &new
        && group
            .takes
            .iter()
            .any(|t| t.loop_length.is_some() && t.content.is_audio())
    {
        return Err(bad(
            "an audio take cannot loop: convert the loop to separate clips first",
        ));
    }
    let mut edits = Vec::new();
    if let Some(old) = old
        && clips != Clips::Keep
    {
        for id in &old.clips {
            if let Some(c) = lane_ref.clips.iter().find(|c| c.id == *id) {
                edits.push(Edit::Clip {
                    device: device.clone(),
                    lane,
                    old: Some(c.clone()),
                    new: None,
                });
            }
        }
    }
    let mut next = doc.song.next_clip_id;
    let first = next;
    if let Some(group) = &mut new
        && clips == Clips::Recompile
    {
        group.clips.clear();
        for mut clip in comp_clips(doc, group) {
            clip.id = ClipId(next);
            next = next
                .checked_add(1)
                .ok_or_else(|| bad("clip ids are exhausted"))?;
            group.clips.push(clip.id);
            edits.push(Edit::Clip {
                device: device.clone(),
                lane,
                old: None,
                new: Some(clip),
            });
        }
    }
    let mut groups = lane_ref.take_groups.clone();
    let at = old.and_then(|o| groups.iter().position(|g| g.id == o.id));
    match (at, new) {
        (Some(i), Some(g)) => groups[i] = g,
        (Some(i), None) => {
            groups.remove(i);
        }
        (None, Some(g)) => groups.push(g),
        (None, None) => {}
    }
    edits.push(Edit::TakeGroups {
        device: device.clone(),
        lane,
        old: lane_ref.take_groups.clone(),
        new: groups,
    });
    if next != first {
        edits.push(Edit::NextClipId {
            old: first,
            new: next,
        });
    }
    Ok(Edit::Compound(edits))
}

/// Plans one take command.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: SongCommand,
) -> Result<Edit, ModelError> {
    match command {
        SongCommand::NewTakeGroup {
            device,
            lane,
            takes,
        } => {
            let t = &doc.song.tracks[track_index(doc, &device)?];
            let kind = &t
                .lanes
                .get(lane)
                .ok_or_else(|| bad(format!("{device} has no lane {lane}")))?
                .kind;
            if matches!(kind, LaneKind::Automation(_)) {
                return Err(bad("takes are for notes and audio lanes"));
            }
            if takes.is_empty() || takes.len() > MAX_TAKES {
                return Err(range(format!(
                    "a take group needs 1..={MAX_TAKES} takes, found {}",
                    takes.len()
                )));
            }
            // Take clips take ids above the next one; the compiled clips come after them.
            let mut next = doc.song.next_clip_id;
            let first = next;
            let mut checked: Vec<Clip> = Vec::with_capacity(takes.len());
            for clip in takes {
                let mut clip = check_new_clip(doc, registry, &device, lane, clip)?;
                clip.id = ClipId(next);
                next = next
                    .checked_add(1)
                    .ok_or_else(|| bad("clip ids are exhausted"))?;
                checked.push(clip);
            }
            let group = TakeGroup {
                id: doc.song.next_take_group_id(),
                comp: layered_comp(&checked),
                takes: checked,
                clips: Vec::new(),
            };
            // Reserve the take ids first, then let `build` allocate the compiled clips after them.
            let mut scratch = doc.clone();
            scratch.song_mut().next_clip_id = next;
            let edit = build(&scratch, &device, lane, None, Some(group), Clips::Recompile)?;
            Ok(Edit::Compound(vec![
                Edit::NextClipId {
                    old: first,
                    new: next,
                },
                edit,
            ]))
        }
        SongCommand::GroupAsTakes {
            device,
            lane,
            clips,
            extra,
        } => {
            let t = &doc.song.tracks[track_index(doc, &device)?];
            let lane_ref = t
                .lanes
                .get(lane)
                .ok_or_else(|| bad(format!("{device} has no lane {lane}")))?;
            if matches!(lane_ref.kind, LaneKind::Automation(_)) {
                return Err(bad("takes are for notes and audio lanes"));
            }
            let total = clips.len() + extra.len();
            if total == 0 || total > MAX_TAKES {
                return Err(range(format!(
                    "a take group needs 1..={MAX_TAKES} takes, found {total}"
                )));
            }
            let mut moved: Vec<&Clip> = Vec::with_capacity(clips.len());
            for id in &clips {
                let c = lane_ref.clips.iter().find(|c| c.id == *id).ok_or_else(|| {
                    bad(format!("clip {} is not on lane {lane} of {device}", id.0))
                })?;
                if lane_ref.take_groups.iter().any(|g| g.clips.contains(id)) {
                    return Err(bad(format!(
                        "clip {} is already compiled from a take group",
                        id.0
                    )));
                }
                if moved.iter().any(|m| m.id == *id) {
                    return Err(bad(format!("clip {} is listed twice", id.0)));
                }
                moved.push(c);
            }
            moved.sort_by_key(|c| (c.start, c.id));
            let mut edits: Vec<Edit> = moved
                .iter()
                .map(|c| Edit::Clip {
                    device: device.clone(),
                    lane,
                    old: Some((*c).clone()),
                    new: None,
                })
                .collect();
            let mut takes: Vec<Clip> = moved.into_iter().cloned().collect();
            let mut next = doc.song.next_clip_id;
            let first = next;
            for clip in extra {
                let mut clip = check_new_clip(doc, registry, &device, lane, clip)?;
                clip.id = ClipId(next);
                next = next
                    .checked_add(1)
                    .ok_or_else(|| bad("clip ids are exhausted"))?;
                takes.push(clip);
            }
            let group = TakeGroup {
                id: doc.song.next_take_group_id(),
                comp: layered_comp(&takes),
                takes,
                clips: Vec::new(),
            };
            let mut scratch = doc.clone();
            scratch.song_mut().next_clip_id = next;
            edits.insert(
                0,
                Edit::NextClipId {
                    old: first,
                    new: next,
                },
            );
            edits.push(build(
                &scratch,
                &device,
                lane,
                None,
                Some(group),
                Clips::Recompile,
            )?);
            Ok(Edit::Compound(edits))
        }
        SongCommand::AddTake { group, clip } => {
            let (device, lane, g) = find(doc, group)?;
            if g.takes.len() >= MAX_TAKES {
                return Err(range(format!(
                    "a take group holds at most {MAX_TAKES} takes"
                )));
            }
            let mut clip = check_new_clip(doc, registry, &device, lane, clip)?;
            let id = doc.song.next_clip_id;
            let next = id
                .checked_add(1)
                .ok_or_else(|| bad("clip ids are exhausted"))?;
            clip.id = ClipId(id);
            let mut new = g.clone();
            let (start, end) = (clip.start, clip.end());
            new.takes.push(clip);
            let top = new.takes.len() - 1;
            set_segment(&mut new.comp, top, start, end);
            let mut scratch = doc.clone();
            scratch.song_mut().next_clip_id = next;
            let edit = build(
                &scratch,
                &device,
                lane,
                Some(g),
                Some(new),
                Clips::Recompile,
            )?;
            Ok(Edit::Compound(vec![
                Edit::NextClipId { old: id, new: next },
                edit,
            ]))
        }
        SongCommand::PickTake { group, take } => {
            let (device, lane, g) = find(doc, group)?;
            if take >= g.takes.len() {
                return Err(range(format!(
                    "group {group} has {} takes, not take {take}",
                    g.takes.len()
                )));
            }
            let (start, end) = g.range();
            let mut new = g.clone();
            new.comp = vec![CompSegment::new(take, start, end)];
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::SetCompSegment {
            group,
            take,
            start,
            end,
        } => {
            let (device, lane, g) = find(doc, group)?;
            let range_ = g.range();
            if start >= end {
                return Err(range(format!("comp segment [{start}, {end}) is empty")));
            }
            let mut new = g.clone();
            set_segment(&mut new.comp, take, start, end);
            check_comp(&new.comp, g.takes.len(), range_).map_err(range)?;
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::DeleteTake { group, take } => {
            let (device, lane, g) = find(doc, group)?;
            if take >= g.takes.len() {
                return Err(range(format!(
                    "group {group} has {} takes, not take {take}",
                    g.takes.len()
                )));
            }
            match comp_without_take(&g.comp, take, g.takes.len()) {
                None => build(doc, &device, lane, Some(g), None, Clips::Remove),
                Some(comp) => {
                    let mut new = g.clone();
                    new.takes.remove(take);
                    new.comp = comp;
                    // The range may have shrunk; segments outside it are cut back to it.
                    new.comp = comp_within(&new.comp, new.range());
                    build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
                }
            }
        }
        SongCommand::DuplicateTake { group, take } => {
            let (device, lane, g) = find(doc, group)?;
            check_take(g, take)?;
            if g.takes.len() >= MAX_TAKES {
                return Err(range(format!(
                    "a take group holds at most {MAX_TAKES} takes"
                )));
            }
            let id = doc.song.next_clip_id;
            let next = id
                .checked_add(1)
                .ok_or_else(|| bad("clip ids are exhausted"))?;
            let mut copy = g.takes[take].clone();
            copy.id = ClipId(id);
            let mut new = g.clone();
            new.takes.insert(take + 1, copy);
            new.comp = comp_with_take_inserted(&g.comp, take + 1);
            let mut scratch = doc.clone();
            scratch.song_mut().next_clip_id = next;
            let edit = build(
                &scratch,
                &device,
                lane,
                Some(g),
                Some(new),
                Clips::Recompile,
            )?;
            Ok(Edit::Compound(vec![
                Edit::NextClipId { old: id, new: next },
                edit,
            ]))
        }
        SongCommand::MoveTake { group, take, to } => {
            let (device, lane, g) = find(doc, group)?;
            check_take(g, take)?;
            check_take(g, to)?;
            if take == to {
                return Err(range(format!("take {take} is already at index {to}")));
            }
            let mut new = g.clone();
            let moved = new.takes.remove(take);
            new.takes.insert(to, moved);
            new.comp = comp_with_take_moved(&g.comp, take, to);
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::DeleteUnusedTakes { group } => {
            let (device, lane, g) = find(doc, group)?;
            let used: Vec<bool> = (0..g.takes.len())
                .map(|t| g.comp.iter().any(|s| s.take == t))
                .collect();
            let unused = used.iter().filter(|u| !**u).count();
            if unused == 0 {
                return Err(bad(format!("every take of group {group} is used")));
            }
            if unused == g.takes.len() {
                return Err(bad(format!("group {group} plays no take: nothing to keep")));
            }
            let mut new = g.clone();
            let renumber: Vec<usize> = used
                .iter()
                .scan(0usize, |n, u| {
                    let at = *n;
                    *n += usize::from(*u);
                    Some(at)
                })
                .collect();
            new.takes = g
                .takes
                .iter()
                .zip(&used)
                .filter(|(_, u)| **u)
                .map(|(t, _)| t.clone())
                .collect();
            new.comp = g
                .comp
                .iter()
                .map(|s| CompSegment {
                    take: renumber[s.take],
                    ..*s
                })
                .collect();
            new.comp = comp_within(&new.comp, new.range());
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::SetCompXfade { group, at, xfade } => {
            let (device, lane, g) = find(doc, group)?;
            if xfade > MAX_TICK as u64 {
                return Err(range(format!("crossfade of {xfade} ticks is too long")));
            }
            let Some(i) = g.comp.iter().position(|s| s.start == at) else {
                return Err(bad(format!(
                    "group {group} has no segment starting at {at}"
                )));
            };
            if !g.takes.iter().all(|t| t.content.is_audio()) {
                return Err(bad("only audio takes crossfade"));
            }
            if g.comp[i].xfade == xfade {
                return Err(bad(format!("the cut at {at} already has that crossfade")));
            }
            let mut new = g.clone();
            new.comp[i].xfade = xfade;
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::SetCompSilence { group, start, end } => {
            let (device, lane, g) = find(doc, group)?;
            if start >= end {
                return Err(range(format!("silent range [{start}, {end}) is empty")));
            }
            let (rs, re) = g.range();
            if start < rs || end > re {
                return Err(range(format!(
                    "silent range [{start}, {end}) is outside the group's range [{rs}, {re})"
                )));
            }
            let mut new = g.clone();
            clear_segment(&mut new.comp, start, end);
            if new.comp == g.comp {
                return Err(bad("that range is already silent"));
            }
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::SetTake {
            group,
            take,
            gain_db,
            source_offset_delta,
        } => {
            let (device, lane, g) = find(doc, group)?;
            check_take(g, take)?;
            let ClipContent::Audio(audio) = &g.takes[take].content else {
                return Err(bad("only an audio take has a level and an offset"));
            };
            let offset = audio
                .source_offset
                .checked_add_signed(source_offset_delta)
                .ok_or_else(|| range("the offset would move before the start of the asset"))?;
            let mut content = audio.clone();
            content.gain_db = gain_db;
            content.source_offset = offset;
            check_audio(doc, &content)?;
            if content == *audio {
                return Err(bad("the take already has that level and offset"));
            }
            let mut new = g.clone();
            new.takes[take].content = ClipContent::Audio(content);
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::FlattenTakes { group } => {
            let (device, lane, g) = find(doc, group)?;
            build(doc, &device, lane, Some(g), None, Clips::Keep)
        }
        _ => unreachable!("only take commands are routed here"),
    }
}
