//! Planning of the track organisation commands (`GroupTracks`, `Ungroup`, `RenameGroup`,
//! `SetGroupCollapsed`, `SetGroupColor`, `MoveTrack`, `MoveGroup`, `SetTrackColor`,
//! `SetTrackComment`, `SetLaneFolded`, `SetTakesOpen`); the data and its invariant are in
//! [`crate::organise`].

use crate::history::Edit;
use crate::organise::{
    MAX_GROUP_DEPTH, MAX_GROUPS, Organisation, OutlineKind, TrackGroup, TrackLayout, TrackPlace,
    comment_ok, group_name_ok, normalise,
};
use crate::song::{Song, SongCommand};
use crate::song_cmd::{bad, track_index};
use crate::{DeviceKey, ModelError, RackDoc};

/// The track devices in order.
fn order_of(song: &Song) -> Vec<DeviceKey> {
    song.tracks.iter().map(|t| t.device.clone()).collect()
}

/// The edit from the document's layout to `order` and `org` (cleaned first).
pub(crate) fn layout_edit(song: &Song, order: Vec<DeviceKey>, org: &Organisation) -> Edit {
    let new = TrackLayout {
        org: normalise(song, &order, org),
        order,
    };
    Edit::Organise {
        old: Box::new(TrackLayout {
            order: order_of(song),
            org: song.org.clone(),
        }),
        new: Box::new(new),
    }
}

/// The layout edit that puts the new track `new` in the folder of `source` and gives it the
/// colour `source` shows (its own, else its device's); `None` when there is nothing to copy.
/// `doc` already holds `new`, next to `source`.
pub(crate) fn join_track(doc: &RackDoc, new: &DeviceKey, source: &DeviceKey) -> Option<Edit> {
    let song = &doc.song;
    let mut org = song.normalised_org();
    let mut changed = false;
    if let Some(&group) = org.members.get(source) {
        org.members.insert(new.clone(), group);
        changed = true;
    }
    let color = org
        .colors
        .get(source)
        .copied()
        .or_else(|| doc.device(source).and_then(|d| d.color));
    if let Some(color) = color {
        org.colors.insert(new.clone(), color);
        changed = true;
    }
    changed.then(|| layout_edit(song, order_of(song), &org))
}

/// The edit that gives the document the organisation `org` (cleaned first), the track order
/// unchanged.
pub(crate) fn org_edit(song: &Song, org: Organisation) -> Edit {
    layout_edit(song, order_of(song), &org)
}

/// The track order and organisation after moving `devices` to `place` (see
/// [`SongCommand::MoveTracks`]).
pub(crate) fn moved_layout(
    song: &Song,
    devices: &[DeviceKey],
    place: TrackPlace,
) -> Result<MovedLayout, ModelError> {
    if devices.is_empty() {
        return Err(bad("there are no tracks to move"));
    }
    let order = order_of(song);
    let org = song.normalised_org();
    for (i, d) in devices.iter().enumerate() {
        if song.track(d).is_none() {
            return Err(ModelError::UnknownKey(d.0.clone()));
        }
        if devices[..i].contains(d) {
            return Err(bad(format!("{d} is listed twice")));
        }
    }
    let (moved, stay): (Vec<DeviceKey>, Vec<DeviceKey>) =
        order.iter().cloned().partition(|d| devices.contains(d));
    place_layout(song, &order, &org, moved, stay, place)
}

/// Puts `moved` among `stay` at `place` (the common part of moving and copying tracks).
fn place_layout(
    song: &Song,
    order: &[DeviceKey],
    org: &Organisation,
    moved: Vec<DeviceKey>,
    stay: Vec<DeviceKey>,
    place: TrackPlace,
) -> Result<MovedLayout, ModelError> {
    let TrackPlace { to_index, into } = place;
    if let Some(g) = into
        && org.group(g).is_none()
    {
        return Err(ModelError::UnknownKey(format!("folder {g}")));
    }
    if to_index > stay.len() {
        return Err(crate::song_cmd::range(format!(
            "place {to_index} is past the {} tracks that stay",
            stay.len()
        )));
    }
    let mut new_order = stay;
    new_order.splice(to_index..to_index, moved.iter().cloned());
    let mut asked = org.clone();
    for d in &moved {
        match into {
            Some(g) => asked.members.insert(d.clone(), g),
            None => asked.members.remove(d),
        };
    }
    // The outline repairs a track that is out of its folder's block: when the repair changes
    // where any track is shown, the place is in the middle of a folder it does not belong to.
    let cleaned = normalise(song, &new_order, &asked);
    let shown = |d: &DeviceKey| cleaned.members.get(d).copied();
    let wanted = |d: &DeviceKey| {
        if moved.contains(d) {
            into
        } else {
            org.members.get(d).copied()
        }
    };
    if new_order.iter().any(|d| shown(d) != wanted(d)) {
        return Err(bad(
            "that place is inside a folder the tracks do not belong to",
        ));
    }
    let changed = new_order != order || asked.members != org.members;
    Ok(MovedLayout {
        order: new_order,
        org: asked,
        changed,
    })
}

/// The result of [`moved_layout`].
pub(crate) struct MovedLayout {
    /// The new track order.
    pub order: Vec<DeviceKey>,
    /// The new organisation (not yet cleaned).
    pub org: Organisation,
    /// Whether the order or a folder link differs from the song's.
    pub changed: bool,
}

impl Song {
    /// Whether [`SongCommand::MoveTracks`] with these arguments would move something (it would
    /// not be refused for the place, the tracks or because it changes nothing). Frozen tracks
    /// are checked by the command, not here.
    pub fn moves_tracks(&self, devices: &[DeviceKey], place: TrackPlace) -> bool {
        moved_layout(self, devices, place).is_ok_and(|m| m.changed)
    }

    /// Whether copies of tracks (as [`crate::WorkflowCommand::DuplicateTracks`] with a place)
    /// could go to `place`, where `to_index` counts all the tracks the song has now.
    pub fn accepts_copies_at(&self, place: TrackPlace) -> bool {
        let order = order_of(self);
        let org = self.normalised_org();
        // A track that does not exist yet stands in for the copies.
        let copy = DeviceKey("\u{0}copy".to_owned());
        place_layout(self, &order, &org, vec![copy], order.clone(), place).is_ok()
    }
}

/// The folder `id` of a cleaned organisation.
fn group_mut(org: &mut Organisation, id: u32) -> Result<&mut TrackGroup, ModelError> {
    org.groups
        .iter_mut()
        .find(|g| g.id == id)
        .ok_or_else(|| ModelError::UnknownKey(format!("folder {id}")))
}

/// How many leading entries two chains share.
fn common(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// The folder a track or folder with this chain moves to when it leaves its innermost folder.
fn outer(chain: &[u32]) -> Option<u32> {
    chain.len().checked_sub(2).map(|i| chain[i])
}

/// Plans one organisation command.
pub(crate) fn plan(doc: &RackDoc, command: SongCommand) -> Result<Edit, ModelError> {
    let song = &doc.song;
    let order = order_of(song);
    let mut org = song.normalised_org();
    match command {
        SongCommand::GroupTracks { devices, name } => {
            if !group_name_ok(&name) {
                return Err(bad(
                    "a folder name must be trimmed, non-empty and short, without control characters",
                ));
            }
            if devices.is_empty() {
                return Err(bad("a folder needs at least one track"));
            }
            if org.groups.len() >= MAX_GROUPS {
                return Err(bad(format!("a song has at most {MAX_GROUPS} folders")));
            }
            let mut picked = Vec::new();
            for d in &devices {
                let i = track_index(doc, d)?;
                if picked.contains(&i) {
                    return Err(bad(format!("{d} is listed twice")));
                }
                picked.push(i);
            }
            picked.sort_unstable();
            let parent = org.members.get(&order[picked[0]]).copied();
            if picked
                .iter()
                .any(|i| org.members.get(&order[*i]).copied() != parent)
            {
                return Err(bad("the tracks are not all in the same folder"));
            }
            if org.chain(parent).len() + 1 > MAX_GROUP_DEPTH {
                return Err(bad(format!(
                    "folders nest at most {MAX_GROUP_DEPTH} levels deep"
                )));
            }
            // The members come together at the place of the first one.
            let moved: Vec<DeviceKey> = picked.iter().map(|i| order[*i].clone()).collect();
            let mut new_order = Vec::with_capacity(order.len());
            for (i, d) in order.iter().enumerate() {
                if i == picked[0] {
                    new_order.extend(moved.iter().cloned());
                } else if !picked.contains(&i) {
                    new_order.push(d.clone());
                }
            }
            let id = org.next_group_id();
            org.groups.push(TrackGroup {
                id,
                name,
                parent,
                collapsed: false,
                color: None,
            });
            for d in moved {
                org.members.insert(d, id);
            }
            Ok(layout_edit(song, new_order, &org))
        }
        SongCommand::Ungroup { group } => {
            let parent = group_mut(&mut org, group)?.parent;
            org.groups.retain(|g| g.id != group);
            for g in &mut org.groups {
                if g.parent == Some(group) {
                    g.parent = parent;
                }
            }
            let inside: Vec<DeviceKey> = org
                .members
                .iter()
                .filter(|(_, m)| **m == group)
                .map(|(d, _)| d.clone())
                .collect();
            for d in inside {
                match parent {
                    Some(p) => org.members.insert(d, p),
                    None => org.members.remove(&d),
                };
            }
            Ok(layout_edit(song, order, &org))
        }
        SongCommand::RenameGroup { group, name } => {
            if !group_name_ok(&name) {
                return Err(bad(
                    "a folder name must be trimmed, non-empty and short, without control characters",
                ));
            }
            group_mut(&mut org, group)?.name = name;
            Ok(layout_edit(song, order, &org))
        }
        SongCommand::SetGroupCollapsed { group, collapsed } => {
            group_mut(&mut org, group)?.collapsed = collapsed;
            Ok(layout_edit(song, order, &org))
        }
        SongCommand::SetGroupColor { group, color } => {
            group_mut(&mut org, group)?.color = color;
            Ok(layout_edit(song, order, &org))
        }
        SongCommand::MoveTrack { device, down } => {
            let i = track_index(doc, &device)?;
            let chain_of = |j: usize| org.chain(org.members.get(&order[j]).copied());
            let mine = chain_of(i);
            let next = if down {
                (i + 1 < order.len()).then_some(i + 1)
            } else {
                i.checked_sub(1)
            };
            let mut new_order = order.clone();
            let mut members = org.members.clone();
            let set = |members: &mut std::collections::BTreeMap<DeviceKey, u32>, g: Option<u32>| {
                match g {
                    Some(g) => members.insert(device.clone(), g),
                    None => members.remove(&device),
                };
            };
            match next {
                None if mine.is_empty() => {
                    return Err(bad(format!(
                        "{device} is already the {} track",
                        if down { "last" } else { "first" }
                    )));
                }
                // The edge of the list: out of the folder.
                None => set(&mut members, outer(&mine)),
                Some(j) => {
                    let theirs = chain_of(j);
                    let shared = common(&mine, &theirs);
                    if mine.len() > shared {
                        set(&mut members, outer(&mine));
                    } else if theirs.len() == mine.len() {
                        new_order.swap(i, j);
                    } else if org.collapsed(theirs[mine.len()]) {
                        // A collapsed folder shows none of its tracks, so entering it would
                        // look like nothing happened: the track hops over the whole folder.
                        let folder = theirs[mine.len()];
                        let mut end = j;
                        loop {
                            let k = if down { end + 1 } else { end.wrapping_sub(1) };
                            if k >= order.len() || !chain_of(k).contains(&folder) {
                                break;
                            }
                            end = k;
                        }
                        // Moving down, taking the track out shifts the far end one place
                        // earlier and it lands right after it; moving up, it lands before it.
                        let moved = new_order.remove(i);
                        new_order.insert(end, moved);
                    } else {
                        set(&mut members, Some(theirs[mine.len()]));
                    }
                }
            }
            org.members = members;
            Ok(layout_edit(song, new_order, &org))
        }
        SongCommand::MoveTracks { devices, place } => {
            let moved = moved_layout(song, &devices, place)?;
            if !moved.changed {
                return Err(bad("the tracks are already there"));
            }
            Ok(layout_edit(song, moved.order, &moved.org))
        }
        SongCommand::MoveGroup { group, down } => {
            group_mut(&mut org, group)?;
            let outline = crate::organise::outline_of(&org, &order);
            // The track range of a folder in the outline.
            let block = |g: u32| {
                let at = outline
                    .iter()
                    .position(|e| e.kind == OutlineKind::Folder(g))?;
                let depth = outline[at].depth;
                let tracks: Vec<usize> = outline[at + 1..]
                    .iter()
                    .take_while(|e| e.depth > depth)
                    .filter_map(|e| match e.kind {
                        OutlineKind::Track(t) => Some(t),
                        OutlineKind::Folder(_) => None,
                    })
                    .collect();
                Some((*tracks.first()?, *tracks.last()?))
            };
            let (a, b) = block(group).ok_or_else(|| bad("the folder has no tracks"))?;
            let parent_chain = org.chain(org.group(group).and_then(|g| g.parent));
            let chain_of = |j: usize| org.chain(org.members.get(&order[j]).copied());
            let neighbour = if down {
                (b + 1 < order.len()).then_some(b + 1)
            } else {
                a.checked_sub(1)
            };
            let mut new_order = order.clone();
            let mut new_parent = None;
            match neighbour {
                None if parent_chain.is_empty() => {
                    return Err(bad(format!(
                        "the folder is already the {} item",
                        if down { "last" } else { "first" }
                    )));
                }
                None => new_parent = Some(outer(&parent_chain)),
                Some(j) => {
                    let theirs = chain_of(j);
                    let shared = common(&parent_chain, &theirs);
                    if parent_chain.len() > shared {
                        new_parent = Some(outer(&parent_chain));
                    } else {
                        // The neighbour: a track at this level, or the whole sibling folder.
                        let (na, nb) = if theirs.len() == parent_chain.len() {
                            (j, j)
                        } else {
                            block(theirs[parent_chain.len()])
                                .ok_or_else(|| bad("the neighbouring folder has no tracks"))?
                        };
                        if down {
                            let len = b - a + 1;
                            new_order[a..=nb].rotate_left(len);
                        } else {
                            let len = nb - na + 1;
                            new_order[na..=b].rotate_left(len);
                        }
                    }
                }
            }
            if let Some(p) = new_parent {
                group_mut(&mut org, group)?.parent = p;
            }
            Ok(layout_edit(song, new_order, &org))
        }
        SongCommand::SetTrackColor { device, color } => {
            track_index(doc, &device)?;
            match color {
                Some(c) => org.colors.insert(device, c),
                None => org.colors.remove(&device),
            };
            Ok(layout_edit(song, order, &org))
        }
        SongCommand::SetTrackComment { device, comment } => {
            track_index(doc, &device)?;
            if comment.trim().is_empty() {
                org.comments.remove(&device);
            } else if comment_ok(&comment) {
                org.comments.insert(device, comment);
            } else {
                return Err(bad(
                    "a comment must be trimmed and short, without control characters but newlines",
                ));
            }
            Ok(layout_edit(song, order, &org))
        }
        SongCommand::SetLaneFolded {
            device,
            param,
            folded,
        } => {
            let t = &song.tracks[track_index(doc, &device)?];
            if t.automation_lane(&param).is_none() {
                return Err(bad(format!("{device} has no automation lane for {param}")));
            }
            let key = (device, param);
            if folded {
                org.folded_lanes.insert(key);
            } else {
                org.folded_lanes.remove(&key);
            }
            Ok(layout_edit(song, order, &org))
        }
        SongCommand::SetTakesOpen { take_group, open } => {
            if song.take_group(take_group).is_none() {
                return Err(ModelError::UnknownKey(format!("take group {take_group}")));
            }
            if open {
                org.open_takes.insert(take_group);
            } else {
                org.open_takes.remove(&take_group);
            }
            Ok(layout_edit(song, order, &org))
        }
        _ => unreachable!("only the organisation commands are planned here"),
    }
}
