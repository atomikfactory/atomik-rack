//! Planning of the take commands (`NewTakeGroup`, `PickTake`, `SetCompSegment`, `DeleteTake`,
//! `FlattenTakes`): each is one compound edit that changes the group list and swaps the group's
//! compiled lane clips (see [`crate::takes`]).

use crate::history::Edit;
use crate::song::{Clip, ClipContent, ClipId, MAX_TICK, SongCommand};
use crate::song_cmd::{bad, check_audio, check_geometry, check_new_clip, range, track_index};
use crate::takes::{
    CompSegment, TakeGroup, check_comp, clear_segment, comp_clips, comp_with_take_inserted,
    comp_with_take_moved, comp_within, comp_without_take, layered_comp, set_segment,
};
use crate::workflow::TakeSnapshot;
use crate::workflow_cmd::{TakeTarget, WorkflowCommand};
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// Most takes one group may hold.
pub const MAX_TAKES: usize = 256;

/// Most groups one linked comp may name.
pub const MAX_LINKED_GROUPS: usize = 64;

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

/// The edit that gives take groups `groups` the link tag `tag` (`None`: no link), one
/// `TakeGroups` edit per lane that holds any of them.
fn set_links(doc: &RackDoc, groups: &[u32], tag: Option<u32>) -> Result<Edit, ModelError> {
    let mut lanes: Vec<(DeviceKey, usize)> = Vec::new();
    for id in groups {
        let (device, lane, _) = find(doc, *id)?;
        if !lanes.contains(&(device.clone(), lane)) {
            lanes.push((device, lane));
        }
    }
    let mut edits = Vec::with_capacity(lanes.len());
    for (device, lane) in lanes {
        let old = doc.song.tracks[track_index(doc, &device)?].lanes[lane]
            .take_groups
            .clone();
        let mut new = old.clone();
        for g in &mut new {
            if groups.contains(&g.id) {
                g.link = tag;
            }
        }
        edits.push(Edit::TakeGroups {
            device,
            lane,
            old,
            new,
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
            if t.lanes.get(lane).is_none() {
                return Err(bad(format!("{device} has no lane {lane}")));
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
                link: None,
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
                link: None,
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
        SongCommand::SetLinkedComp {
            groups,
            take,
            start,
            end,
        } => {
            if start >= end {
                return Err(range(format!("comp segment [{start}, {end}) is empty")));
            }
            if groups.is_empty() || groups.len() > MAX_LINKED_GROUPS {
                return Err(range(format!(
                    "a linked comp holds 1 to {MAX_LINKED_GROUPS} groups, not {}",
                    groups.len()
                )));
            }
            // Each group is planned on the document the previous ones left, so the clip ids the
            // compiled comps take do not collide.
            let mut scratch = doc.clone();
            let mut edits = Vec::new();
            for (i, id) in groups.iter().enumerate() {
                if groups[..i].contains(id) {
                    return Err(bad(format!("take group {id} is listed twice")));
                }
                let (device, lane, g) = find(&scratch, *id)?;
                let (rs, re) = g.range();
                let (s, e) = (start.max(rs), end.min(re));
                if take >= g.takes.len() || s >= e {
                    continue;
                }
                let mut new = g.clone();
                set_segment(&mut new.comp, take, s, e);
                if new.comp == g.comp {
                    continue;
                }
                check_comp(&new.comp, g.takes.len(), (rs, re)).map_err(range)?;
                let edit = build(
                    &scratch,
                    &device,
                    lane,
                    Some(g),
                    Some(new),
                    Clips::Recompile,
                )?;
                edit.perform(&mut scratch)?;
                edits.push(edit);
            }
            if edits.is_empty() {
                return Err(bad("no linked group changes"));
            }
            Ok(Edit::Compound(edits))
        }
        SongCommand::LinkTakeGroups { groups } => {
            if groups.len() < 2 || groups.len() > MAX_LINKED_GROUPS {
                return Err(range(format!(
                    "a link holds 2 to {MAX_LINKED_GROUPS} groups, not {}",
                    groups.len()
                )));
            }
            let mut tags = Vec::with_capacity(groups.len());
            for (i, id) in groups.iter().enumerate() {
                if groups[..i].contains(id) {
                    return Err(bad(format!("take group {id} is listed twice")));
                }
                tags.push(find(doc, *id)?.2.link);
            }
            if tags[0].is_some() && tags.iter().all(|t| *t == tags[0]) {
                let alone = doc
                    .song
                    .take_groups()
                    .all(|(_, _, g)| g.link != tags[0] || groups.contains(&g.id));
                if alone {
                    return Err(bad("those take groups are linked already"));
                }
            }
            let tag = doc
                .song
                .take_groups()
                .filter_map(|(_, _, g)| g.link)
                .max()
                .map_or(1, |m| m.saturating_add(1));
            set_links(doc, &groups, Some(tag))
        }
        SongCommand::UnlinkTakeGroup { group } => {
            if find(doc, group)?.2.link.is_none() {
                return Err(bad(format!("take group {group} is not linked")));
            }
            set_links(doc, &[group], None)
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
        SongCommand::SetTakeLoop {
            group,
            take,
            repeats,
        } => {
            let (device, lane, g) = find(doc, group)?;
            check_take(g, take)?;
            if repeats == 0 {
                return Err(range("a take plays its loop at least once"));
            }
            let old = &g.takes[take];
            if matches!(&old.content, ClipContent::Audio(a) if a.reverse) {
                return Err(bad("a reversed audio take cannot loop"));
            }
            let n = old.loop_length.unwrap_or(old.length);
            let length = n
                .checked_mul(i64::from(repeats))
                .filter(|l| *l <= MAX_TICK)
                .ok_or_else(|| range("the looped take would run past the last tick"))?;
            let loop_length = (repeats > 1).then_some(n);
            // Repeating a take whole is not a loop of its own: one repetition is the take.
            let offset = if loop_length.is_none() && old.loop_length.is_some() {
                old.offset.rem_euclid(n)
            } else {
                old.offset
            };
            check_geometry(old.start, length, offset, loop_length).map_err(range)?;
            if length == old.length && loop_length == old.loop_length {
                return Err(bad("the take already plays that many repetitions"));
            }
            let mut new = g.clone();
            let t = &mut new.takes[take];
            let (old_end, new_end) = (old.end(), old.start + length);
            *t = Clip {
                length,
                loop_length,
                offset,
                ..t.clone()
            };
            if new_end > MAX_TICK {
                return Err(range("the looped take would run past the last tick"));
            }
            // A segment that ran to the end of the take runs on to the new end, up to the next
            // segment; one that ran past a shorter end is cut there.
            let starts: Vec<i64> = g.comp.iter().map(|s| s.start).collect();
            for (i, s) in new.comp.iter_mut().enumerate() {
                if s.take != take {
                    continue;
                }
                if s.end == old_end && new_end > old_end {
                    let wall = starts.get(i + 1).copied().unwrap_or(i64::MAX);
                    s.end = new_end.min(wall);
                } else if s.end > new_end {
                    s.end = new_end;
                }
            }
            new.comp.retain(|s| s.start < s.end);
            new.comp = comp_within(&new.comp, new.range());
            build(doc, &device, lane, Some(g), Some(new), Clips::Recompile)
        }
        SongCommand::FlattenTakes { group } => {
            let (device, lane, g) = find(doc, group)?;
            build(doc, &device, lane, Some(g), None, Clips::Keep)
        }
        _ => unreachable!("only take commands are routed here"),
    }
}

/// Plans [`crate::WorkflowCommand::PasteTakes`]: the snapshot's takes join a group as new takes
/// on top (each plays over its extent, as a new recording does), or start a group on a lane.
pub(crate) fn paste(
    doc: &RackDoc,
    registry: &Registry,
    target: &TakeTarget,
    at_tick: i64,
    snapshot: &TakeSnapshot,
) -> Result<Edit, ModelError> {
    if snapshot.takes.is_empty() {
        return Err(bad("the clipboard holds no takes"));
    }
    if let Some(h) = snapshot
        .required_assets()
        .into_iter()
        .find(|h| !doc.assets.contains_key(h))
    {
        return Err(ModelError::UnknownKey(format!(
            "asset {h} is not in this project's sample pool"
        )));
    }
    let moved = |c: &Clip| Clip {
        start: at_tick.saturating_add(c.start),
        ..c.clone()
    };
    match target {
        TakeTarget::Lane { device, lane } => plan(
            doc,
            registry,
            SongCommand::NewTakeGroup {
                device: device.clone(),
                lane: *lane,
                takes: snapshot.takes.iter().map(moved).collect(),
            },
        ),
        TakeTarget::Group(id) => {
            let (device, lane, g) = find(doc, *id)?;
            if g.takes.len() + snapshot.takes.len() > MAX_TAKES {
                return Err(range(format!(
                    "a take group holds at most {MAX_TAKES} takes"
                )));
            }
            let mut new = g.clone();
            let first = doc.song.next_clip_id;
            let mut next = first;
            for take in &snapshot.takes {
                let mut clip = check_new_clip(doc, registry, &device, lane, moved(take))?;
                clip.id = ClipId(next);
                next = next
                    .checked_add(1)
                    .ok_or_else(|| bad("clip ids are exhausted"))?;
                let (start, end) = (clip.start, clip.end());
                new.takes.push(clip);
                let top = new.takes.len() - 1;
                set_segment(&mut new.comp, top, start, end);
            }
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
                Edit::NextClipId {
                    old: first,
                    new: next,
                },
                edit,
            ]))
        }
    }
}

/// The take a clip-editing command (what the piano roll and the audio editor send) names, when
/// its clip id is a take and not a clip of a lane.
pub(crate) fn take_target(doc: &RackDoc, command: &SongCommand) -> Option<ClipId> {
    let id = match command {
        SongCommand::SetClipContent { id, .. }
        | SongCommand::SetAudioClip { id, .. }
        | SongCommand::SetWarpMarkers { id, .. }
        | SongCommand::SetPitchEdits { id, .. } => *id,
        SongCommand::SetClipGrid { clip, .. }
        | SongCommand::SetClipControllers { clip, .. }
        | SongCommand::SetClipMidi { clip, .. }
        | SongCommand::SetClipParamLanes { clip, .. }
        | SongCommand::SetClipScale { clip, .. } => *clip,
        _ => return None,
    };
    is_take(doc, id).then_some(id)
}

/// The take a note paste or duplicate (the piano roll's clipboard) names, when its clip id is a
/// take.
pub(crate) fn take_target_workflow(doc: &RackDoc, command: &WorkflowCommand) -> Option<ClipId> {
    match command {
        WorkflowCommand::PasteNotes { clip, .. } | WorkflowCommand::DuplicateNotes { clip, .. } => {
            is_take(doc, *clip).then_some(*clip)
        }
        _ => None,
    }
}

fn is_take(doc: &RackDoc, id: ClipId) -> bool {
    doc.song.clip(id).is_none() && doc.song.take_clip(id).is_some()
}

/// Plans a clip-editing command on take `id`: `plan` is the command's planner, run on a scratch
/// document where the take is a clip of its lane, and the clip it comes out as replaces the
/// take in its group (the comp is compiled again, so the lane plays the edit). One undo step,
/// the same as the command makes on a plain clip.
pub(crate) fn plan_take_edit(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    plan: impl FnOnce(&RackDoc, &Registry) -> Result<Edit, ModelError>,
) -> Result<Edit, ModelError> {
    let (track, lane, group, at) = doc
        .song
        .take_clip(id)
        .ok_or_else(|| ModelError::UnknownKey(format!("take clip {}", id.0)))?;
    let device = track.device.clone();
    let mut scratch = doc.clone();
    let ti = track_index(doc, &device)?;
    scratch.song_mut().tracks[ti].lanes[lane]
        .clips
        .push(group.takes[at].clone());
    let edit = plan(&scratch, registry)?;
    let Edit::Clip {
        new: Some(clip), ..
    } = edit
    else {
        return Err(bad("that edit cannot be made on a take"));
    };
    if clip.id != id {
        return Err(bad("that edit cannot be made on a take"));
    }
    let mut new = group.clone();
    new.takes[at] = clip;
    // The edit may have moved or resized the take: the comp stays inside the group's range.
    new.comp = comp_within(&new.comp, new.range());
    build(doc, &device, lane, Some(group), Some(new), Clips::Recompile)
}
