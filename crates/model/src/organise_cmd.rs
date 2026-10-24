//! Planning of the track organisation commands (`GroupTracks`, `Ungroup`, `RenameGroup`,
//! `SetGroupCollapsed`, `SetGroupColor`, `MoveTrack`, `MoveGroup`, `SetTrackColor`,
//! `SetTrackComment`, `SetLaneFolded`, `SetTakesOpen`); the data and its invariant are in
//! [`crate::organise`].

use crate::history::Edit;
use crate::organise::{
    MAX_GROUP_DEPTH, MAX_GROUPS, Organisation, OutlineKind, TrackGroup, TrackLayout, comment_ok,
    group_name_ok, normalise,
};
use crate::song::{Song, SongCommand};
use crate::song_cmd::{bad, track_index};
use crate::{DeviceKey, ModelError, RackDoc};

/// The track devices in order.
fn order_of(song: &Song) -> Vec<DeviceKey> {
    song.tracks.iter().map(|t| t.device.clone()).collect()
}

/// The edit from the document's layout to `order` and `org` (cleaned first).
fn layout(song: &Song, order: Vec<DeviceKey>, org: &Organisation) -> Edit {
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
            Ok(layout(song, new_order, &org))
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
            Ok(layout(song, order, &org))
        }
        SongCommand::RenameGroup { group, name } => {
            if !group_name_ok(&name) {
                return Err(bad(
                    "a folder name must be trimmed, non-empty and short, without control characters",
                ));
            }
            group_mut(&mut org, group)?.name = name;
            Ok(layout(song, order, &org))
        }
        SongCommand::SetGroupCollapsed { group, collapsed } => {
            group_mut(&mut org, group)?.collapsed = collapsed;
            Ok(layout(song, order, &org))
        }
        SongCommand::SetGroupColor { group, color } => {
            group_mut(&mut org, group)?.color = color;
            Ok(layout(song, order, &org))
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
                    } else {
                        set(&mut members, Some(theirs[mine.len()]));
                    }
                }
            }
            org.members = members;
            Ok(layout(song, new_order, &org))
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
            Ok(layout(song, new_order, &org))
        }
        SongCommand::SetTrackColor { device, color } => {
            track_index(doc, &device)?;
            match color {
                Some(c) => org.colors.insert(device, c),
                None => org.colors.remove(&device),
            };
            Ok(layout(song, order, &org))
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
            Ok(layout(song, order, &org))
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
            Ok(layout(song, order, &org))
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
            Ok(layout(song, order, &org))
        }
        _ => unreachable!("only the organisation commands are planned here"),
    }
}
