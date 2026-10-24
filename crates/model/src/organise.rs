//! Track organisation: folders of tracks, track colours and comments, and folded lanes.
//!
//! A folder ([`TrackGroup`]) is not a track and has no device: it is a named, coloured range of
//! the track list that can be collapsed. Which folder a track is in, a track's colour and
//! comment, the automation lanes that are folded and the take groups whose take rows are open
//! all live in [`Organisation`] (`Song::org`), keyed by device key like the track heights. None
//! of it reaches the audio: folders do not change routing, and mute, solo and arm on a folder
//! are the same flags on its tracks (a client sets them on every track of the folder in one
//! batch; see [`Song::group_tracks`]).
//!
//! **Invariant.** The tracks of a folder, with those of its sub-folders, are one block in
//! `Song::tracks`, a sub-folder's block lies inside its parent's, and a folder has at least one
//! track. The commands keep it. Removing a track (or a device with one) leaves its entries
//! behind, and a loaded or hand-built document may break the invariant, so every reader goes
//! through [`Song::outline`], which repairs what it finds (a track whose folder block was
//! left is shown outside that folder), and [`Song::normalised_org`] (what is saved, and what the
//! commands start from) drops what no longer applies.

use crate::DeviceKey;
use crate::song::Song;
use std::collections::{BTreeMap, BTreeSet};

/// Longest folder name, in characters.
pub const MAX_GROUP_NAME: usize = 64;
/// Longest track comment, in characters.
pub const MAX_TRACK_COMMENT: usize = 1000;
/// Deepest nesting of folders (a folder in a folder in a ... ).
pub const MAX_GROUP_DEPTH: usize = 8;
/// Most folders in a song.
pub const MAX_GROUPS: usize = 256;

/// A folder of tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackGroup {
    /// Unique among the folders, at least 1.
    pub id: u32,
    /// Trimmed, non-empty, at most [`MAX_GROUP_NAME`] characters.
    pub name: String,
    /// The folder this one is in.
    pub parent: Option<u32>,
    /// The tracks of the folder are hidden in the arrangement (its own row stays).
    pub collapsed: bool,
    /// Colour of the folder row; the tracks keep their own.
    pub color: Option<[u8; 3]>,
}

/// Folders, colours, comments and fold state of a song's tracks (see the module documentation).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Organisation {
    /// The folders, sorted by id.
    pub groups: Vec<TrackGroup>,
    /// The innermost folder of a track; a track without an entry is not in a folder.
    pub members: BTreeMap<DeviceKey, u32>,
    /// Track colours. A track without one shows its device's colour; its clips follow it.
    pub colors: BTreeMap<DeviceKey, [u8; 3]>,
    /// Track comments: trimmed, non-empty, at most [`MAX_TRACK_COMMENT`] characters.
    pub comments: BTreeMap<DeviceKey, String>,
    /// The automation lanes that are folded to a thin strip, by `(device, param)`.
    pub folded_lanes: BTreeSet<(DeviceKey, String)>,
    /// The take groups whose take rows are shown (a group without an entry is closed).
    pub open_takes: BTreeSet<u32>,
}

/// One row of [`Song::outline`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineKind {
    /// The header of the folder with this id.
    Folder(u32),
    /// The track at this index of `Song::tracks`.
    Track(usize),
}

/// A folder header or a track, in display order, with its nesting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutlineEntry {
    /// What the row is.
    pub kind: OutlineKind,
    /// How many folders it is inside (0 at the top level).
    pub depth: usize,
    /// The innermost folder it is inside (for a header, its parent).
    pub parent: Option<u32>,
    /// A folder above it is collapsed, so the arrangement does not show the row.
    pub hidden: bool,
}

impl Organisation {
    /// The folder with this id.
    pub fn group(&self, id: u32) -> Option<&TrackGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// The first free folder id (above every id in use).
    pub fn next_group_id(&self) -> u32 {
        self.groups.iter().map(|g| g.id).max().map_or(1, |m| m + 1)
    }

    /// The folders from the outermost down to `leaf` itself, following the parents. A cycle, a
    /// missing parent or more than [`MAX_GROUP_DEPTH`] levels end the chain there.
    pub fn chain(&self, leaf: Option<u32>) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut at = leaf;
        while let Some(id) = at {
            if chain.contains(&id) || chain.len() >= MAX_GROUP_DEPTH {
                break;
            }
            let Some(g) = self.group(id) else { break };
            chain.push(id);
            at = g.parent;
        }
        chain.reverse();
        chain
    }

    /// Whether the folder is collapsed (false for an unknown id).
    pub fn collapsed(&self, id: u32) -> bool {
        self.group(id).is_some_and(|g| g.collapsed)
    }
}

/// The outline of tracks listed by device, in order: [`OutlineKind::Track`] indexes `devices`.
pub(crate) fn outline_of<'a>(
    org: &Organisation,
    devices: impl IntoIterator<Item = &'a DeviceKey>,
) -> Vec<OutlineEntry> {
    let mut out = Vec::new();
    let mut open: Vec<u32> = Vec::new();
    let mut closed: BTreeSet<u32> = BTreeSet::new();
    for (i, device) in devices.into_iter().enumerate() {
        let chain = org.chain(org.members.get(device).copied());
        let common = open.iter().zip(&chain).take_while(|(a, b)| a == b).count();
        while open.len() > common {
            closed.extend(open.pop());
        }
        for &g in &chain[common..] {
            if closed.contains(&g) {
                break;
            }
            out.push(OutlineEntry {
                kind: OutlineKind::Folder(g),
                depth: open.len(),
                parent: open.last().copied(),
                hidden: open.iter().any(|a| org.collapsed(*a)),
            });
            open.push(g);
        }
        out.push(OutlineEntry {
            kind: OutlineKind::Track(i),
            depth: open.len(),
            parent: open.last().copied(),
            hidden: open.iter().any(|a| org.collapsed(*a)),
        });
    }
    out
}

/// The organisation of `song` with its tracks in `order`, and `org`'s entries, cleaned (see
/// [`Song::normalised_org`]).
pub(crate) fn normalise(song: &Song, order: &[DeviceKey], org: &Organisation) -> Organisation {
    let mut out = Organisation::default();
    let mut stack: Vec<u32> = Vec::new();
    for e in outline_of(org, order) {
        stack.truncate(e.depth);
        match e.kind {
            OutlineKind::Folder(id) => {
                if let Some(g) = org.group(id) {
                    out.groups.push(TrackGroup {
                        parent: stack.last().copied(),
                        ..g.clone()
                    });
                }
                stack.push(id);
            }
            OutlineKind::Track(i) => {
                if let Some(g) = stack.last() {
                    out.members.insert(order[i].clone(), *g);
                }
            }
        }
    }
    out.groups.sort_by_key(|g| g.id);
    let has_track = |d: &DeviceKey| order.contains(d);
    out.colors = org
        .colors
        .iter()
        .filter(|(d, _)| has_track(d))
        .map(|(d, c)| (d.clone(), *c))
        .collect();
    out.comments = org
        .comments
        .iter()
        .filter(|(d, c)| has_track(d) && !c.is_empty())
        .map(|(d, c)| (d.clone(), c.clone()))
        .collect();
    out.folded_lanes = org
        .folded_lanes
        .iter()
        .filter(|(d, p)| {
            has_track(d)
                && song
                    .track(d)
                    .is_some_and(|t| t.automation_lane(p).is_some())
        })
        .cloned()
        .collect();
    out.open_takes = org
        .open_takes
        .iter()
        .copied()
        .filter(|id| song.take_group(*id).is_some())
        .collect();
    out
}

impl Song {
    /// The folder headers and tracks in display order, with nesting and collapsed state. A
    /// folder's header comes right before its first track. See the module documentation for
    /// how a document that breaks the invariant is read.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        outline_of(&self.org, self.tracks.iter().map(|t| &t.device))
    }

    /// The indices into `tracks` of every track in a folder, sub-folders included, in order
    /// (empty for an unknown folder).
    pub fn group_tracks(&self, group: u32) -> Vec<usize> {
        let outline = self.outline();
        let Some(at) = outline
            .iter()
            .position(|e| e.kind == OutlineKind::Folder(group))
        else {
            return Vec::new();
        };
        let depth = outline[at].depth;
        outline[at + 1..]
            .iter()
            .take_while(|e| e.depth > depth)
            .filter_map(|e| match e.kind {
                OutlineKind::Track(i) => Some(i),
                OutlineKind::Folder(_) => None,
            })
            .collect()
    }

    /// The innermost folder a track is shown in (as [`Song::outline`] reads the document).
    pub fn track_group(&self, device: &DeviceKey) -> Option<u32> {
        self.outline().iter().find_map(|e| match e.kind {
            OutlineKind::Track(i) if &self.tracks[i].device == device => e.parent,
            _ => None,
        })
    }

    /// Whether [`SongCommand::MoveTrack`] would move the track: it can unless it is at the first
    /// or last place of the top level.
    pub fn can_move_track(&self, device: &DeviceKey, down: bool) -> bool {
        let Some(i) = self.tracks.iter().position(|t| &t.device == device) else {
            return false;
        };
        let at_edge = if down {
            i + 1 == self.tracks.len()
        } else {
            i == 0
        };
        !at_edge || self.track_group(device).is_some()
    }

    /// Whether [`SongCommand::MoveGroup`] would move the folder: it can unless it is at the first
    /// or last place of the top level.
    pub fn can_move_group(&self, group: u32, down: bool) -> bool {
        let tracks = self.group_tracks(group);
        let (Some(first), Some(last)) = (tracks.first(), tracks.last()) else {
            return false;
        };
        let at_edge = if down {
            last + 1 == self.tracks.len()
        } else {
            *first == 0
        };
        let nested = self
            .outline()
            .iter()
            .any(|e| e.kind == OutlineKind::Folder(group) && e.parent.is_some());
        !at_edge || nested
    }

    /// The colour of a track, if it has one.
    pub fn track_color(&self, device: &DeviceKey) -> Option<[u8; 3]> {
        self.org.colors.get(device).copied()
    }

    /// The comment of a track (empty when it has none).
    pub fn track_comment(&self, device: &DeviceKey) -> &str {
        self.org.comments.get(device).map_or("", String::as_str)
    }

    /// Whether the automation lane of `param` on `device`'s track is folded.
    pub fn lane_folded(&self, device: &DeviceKey, param: &str) -> bool {
        self.org
            .folded_lanes
            .contains(&(device.clone(), param.to_owned()))
    }

    /// The organisation with what no longer applies dropped: entries of tracks, lanes and take
    /// groups that do not exist, folders without a track, and the folder links of tracks
    /// outside their folder's block; the parents are the folders the outline shows. This is
    /// what is saved and what the commands change.
    pub fn normalised_org(&self) -> Organisation {
        let order: Vec<DeviceKey> = self.tracks.iter().map(|t| t.device.clone()).collect();
        normalise(self, &order, &self.org)
    }
}

/// True for a folder name the model accepts: trimmed, non-empty, at most [`MAX_GROUP_NAME`]
/// characters, no control characters.
pub(crate) fn group_name_ok(name: &str) -> bool {
    name == name.trim()
        && !name.is_empty()
        && name.chars().count() <= MAX_GROUP_NAME
        && !name.chars().any(char::is_control)
}

/// True for a comment the model accepts: trimmed, non-empty, at most [`MAX_TRACK_COMMENT`]
/// characters, no control characters but the newline.
pub(crate) fn comment_ok(text: &str) -> bool {
    text == text.trim()
        && !text.is_empty()
        && text.chars().count() <= MAX_TRACK_COMMENT
        && !text.chars().any(|c| c.is_control() && c != '\n')
}

/// The track order and the organisation as one unit: what a layout edit swaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackLayout {
    /// The devices of the tracks, in order.
    pub order: Vec<DeviceKey>,
    /// The organisation.
    pub org: Organisation,
}
