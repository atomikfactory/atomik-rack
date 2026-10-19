//! Clipboard snapshots and neighbour helpers.
//!
//! A snapshot is plain data that owns what it copied (it reuses the document types, so zones,
//! step patterns, inner containers and plugin documents travel unchanged) and does not borrow
//! the document, so it survives a New or an Open: the app keeps one in its in-process
//! clipboard and pastes it into whatever document is open. The snapshot code is pure: no
//! registry, no clock, no hashing order (`BTreeMap` and sorted vectors only).
//!
//! Pasting is a [`crate::WorkflowCommand`]; capturing is a function here, so the app builds the
//! snapshot at copy time and the command at paste time.

use crate::console::is_console_type;
use crate::container::{collect_device_assets, join, split_path};
use crate::song::{AutomationPoint, Clip, ClipContent, ClipId, LaneKind, Note, Song, Track};
use crate::{AssetMeta, Cable, DeviceKey, ModelError, RackDevice, RackDoc, rules};
use std::collections::{BTreeMap, BTreeSet};

fn bad_song(message: impl Into<String>) -> ModelError {
    ModelError::BadSong(message.into())
}

/// The pool entries of `hashes` that `doc` knows (a hash the pool lacks is skipped: the paste
/// reports it).
fn pool_of(doc: &RackDoc, hashes: &BTreeSet<String>) -> BTreeMap<String, AssetMeta> {
    hashes
        .iter()
        .filter_map(|h| doc.assets.get(h).map(|m| (h.clone(), m.clone())))
        .collect()
}

fn track_assets(track: &Track, out: &mut BTreeSet<String>) {
    for lane in &track.lanes {
        for c in &lane.clips {
            if let ClipContent::Audio(a) = &c.content {
                out.insert(a.asset.clone());
            }
        }
    }
}

/// Devices copied from one scope (the top level or one container): the devices in rack order,
/// the cables whose **both** ends are in the set, and optionally their song tracks.
///
/// Device keys and cable ends are the keys of the source scope (local keys). A paste generates
/// fresh keys; the snapshot only uses the old ones to say which cable joins which devices.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSnapshot {
    /// The copied devices in rack order (a container comes with its whole inside, a plugin
    /// device with its document and state, a sampler with its zones, a sequencer with its
    /// patterns). Console devices are never copied.
    pub devices: Vec<RackDevice>,
    /// The cables between two copied devices, sorted. Cables to anything else are dropped.
    pub cables: Vec<Cable>,
    /// The song tracks of the copied devices, in song order (top-level snapshots made with
    /// tracks only). Clip ids are the source ids; a paste allocates new ones.
    pub tracks: Vec<Track>,
    /// The pool entries the snapshot needs (zone and audio-clip assets), so an app can
    /// register them in the target document (`Command::RegisterAsset`) before the paste: a
    /// paste whose assets are missing is rejected and changes nothing.
    pub assets: BTreeMap<String, AssetMeta>,
}

impl DeviceSnapshot {
    /// Copies the devices `keys` (path keys of one scope: all top level, or all inside the same
    /// container) in rack order, the cables between them and, with `with_tracks`, their song
    /// tracks (top level only). Console devices in the selection are left out (an instrument's
    /// channel is created again on paste); a selection of console devices only is an error.
    pub fn capture(
        doc: &RackDoc,
        keys: &[DeviceKey],
        with_tracks: bool,
    ) -> Result<Self, ModelError> {
        let first = keys
            .first()
            .ok_or_else(|| ModelError::BadScope("nothing is selected".into()))?;
        let parent = split_path(first).0;
        let mut locals = BTreeSet::new();
        for k in keys {
            let (p, local) = split_path(k);
            if p != parent {
                return Err(ModelError::BadScope(format!(
                    "{first} and {k} are not in the same scope"
                )));
            }
            locals.insert((local, k));
        }
        let scope = doc
            .scope_devices(parent.as_ref())
            .ok_or_else(|| ModelError::UnknownDevice(first.0.clone()))?;
        for (local, full) in &locals {
            if !scope.iter().any(|d| &d.key == local) {
                return Err(ModelError::UnknownDevice(full.0.clone()));
            }
        }
        let wanted: BTreeSet<&DeviceKey> = locals.iter().map(|(l, _)| l).collect();
        let devices: Vec<RackDevice> = scope
            .iter()
            .filter(|d| wanted.contains(&d.key) && !is_console_type(&d.type_id))
            .cloned()
            .collect();
        if devices.is_empty() && parent.is_none() && wanted.iter().any(|k| rules::is_master(doc, k))
        {
            return Err(rules::fixed_master(rules::MASTER_COPY));
        }
        if devices.is_empty() {
            return Err(ModelError::BadConsole(
                "console devices are not copied: copy the instrument and its channel follows"
                    .into(),
            ));
        }
        let copied: BTreeSet<&DeviceKey> = devices.iter().map(|d| &d.key).collect();
        let cables: Vec<Cable> = doc
            .scope_cables(parent.as_ref())
            .unwrap_or(&[])
            .iter()
            .filter(|c| copied.contains(&c.from.device) && copied.contains(&c.to.device))
            .cloned()
            .collect();
        let tracks: Vec<Track> = if with_tracks {
            if parent.is_some() {
                return Err(ModelError::BadScope(
                    "tracks belong to top-level devices".into(),
                ));
            }
            doc.song
                .tracks
                .iter()
                .filter(|t| copied.contains(&t.device))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let mut hashes = BTreeSet::new();
        for d in &devices {
            collect_device_assets(d, &mut hashes);
        }
        for t in &tracks {
            track_assets(t, &mut hashes);
        }
        Ok(Self {
            assets: pool_of(doc, &hashes),
            devices,
            cables,
            tracks,
        })
    }

    /// Every pool asset the snapshot references: those of the devices' zones (inside
    /// containers too) and of the audio clips of its tracks.
    pub fn required_assets(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for d in &self.devices {
            collect_device_assets(d, &mut out);
        }
        for t in &self.tracks {
            track_assets(t, &mut out);
        }
        out
    }
}

/// One clip of a [`ClipSnapshot`].
#[derive(Debug, Clone, PartialEq)]
pub struct ClipEntry {
    /// The track (device) the clip was on: where it pastes when no track is chosen.
    pub device: DeviceKey,
    /// The kind of lane it was on; it pastes on the lane of the same kind.
    pub lane: LaneKind,
    /// The clip. `start` is relative to the earliest copied clip (0 for that one); `id` is 0.
    pub clip: Clip,
}

/// Clips copied from the arrangement, with track-relative layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipSnapshot {
    /// The clips in time order (by source start, then id): a paste allocates ids in this order.
    pub entries: Vec<ClipEntry>,
    /// The song tick of the earliest clip when it was copied.
    pub origin: i64,
    /// From the earliest start to the latest end: how far a repeated paste advances so copies
    /// lie end to end.
    pub span: i64,
    /// The pool entries the audio clips need (see [`DeviceSnapshot::assets`]).
    pub assets: BTreeMap<String, AssetMeta>,
}

impl ClipSnapshot {
    /// Copies the clips `ids`.
    pub fn capture(doc: &RackDoc, ids: &[ClipId]) -> Result<Self, ModelError> {
        let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
        if wanted.is_empty() {
            return Err(bad_song("no clips are selected"));
        }
        let mut found: Vec<(&Track, usize, &Clip)> = Vec::new();
        for id in &wanted {
            let (t, lane, c) = doc
                .song
                .clip(*id)
                .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))?;
            found.push((t, lane, c));
        }
        found.sort_by_key(|(_, _, c)| (c.start, c.id));
        let origin = found[0].2.start;
        let end = found
            .iter()
            .map(|(_, _, c)| c.end())
            .max()
            .unwrap_or(origin);
        let entries: Vec<ClipEntry> = found
            .iter()
            .map(|(t, lane, c)| ClipEntry {
                device: t.device.clone(),
                lane: t.lanes[*lane].kind.clone(),
                clip: Clip {
                    id: ClipId(0),
                    start: c.start - origin,
                    ..(*c).clone()
                },
            })
            .collect();
        let mut hashes = BTreeSet::new();
        for e in &entries {
            if let ClipContent::Audio(a) = &e.clip.content {
                hashes.insert(a.asset.clone());
            }
        }
        Ok(Self {
            assets: pool_of(doc, &hashes),
            entries,
            origin,
            span: end - origin,
        })
    }

    /// Every pool asset the audio clips reference.
    pub fn required_assets(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter_map(|e| match &e.clip.content {
                ClipContent::Audio(a) => Some(a.asset.clone()),
                _ => None,
            })
            .collect()
    }
}

/// The sorted, de-duplicated selection of `indices` in a list of `len` items.
fn picked(indices: &[usize], len: usize, what: &str) -> Result<Vec<usize>, ModelError> {
    let set: BTreeSet<usize> = indices.iter().copied().collect();
    if set.is_empty() {
        return Err(bad_song(format!("no {what} are selected")));
    }
    if let Some(i) = set.iter().find(|i| **i >= len) {
        return Err(bad_song(format!(
            "{what} {i} does not exist (the clip has {len})"
        )));
    }
    Ok(set.into_iter().collect())
}

/// Notes copied from a note clip.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteSnapshot {
    /// The notes sorted by (tick, key); `tick` is relative to the earliest copied note.
    pub notes: Vec<Note>,
    /// From the earliest start to the latest end: how far a repeated paste advances.
    pub span: i64,
}

impl NoteSnapshot {
    /// Copies the notes at `indices` (positions in the clip's note list) of `clip`.
    pub fn capture(clip: &Clip, indices: &[usize]) -> Result<Self, ModelError> {
        let ClipContent::Notes(all) = &clip.content else {
            return Err(bad_song(format!("clip {} is not a note clip", clip.id.0)));
        };
        let chosen = picked(indices, all.len(), "note")?;
        let first = chosen.iter().map(|i| all[*i].tick).min().unwrap_or(0);
        let end = chosen
            .iter()
            .map(|i| all[*i].tick + all[*i].length)
            .max()
            .unwrap_or(first);
        Ok(Self {
            notes: chosen
                .iter()
                .map(|i| Note {
                    tick: all[*i].tick - first,
                    ..all[*i]
                })
                .collect(),
            span: end - first,
        })
    }
}

/// Automation points copied from an automation clip.
#[derive(Debug, Clone, PartialEq)]
pub struct PointSnapshot {
    /// The points sorted by tick; `tick` is relative to the earliest copied point.
    pub points: Vec<AutomationPoint>,
    /// From the first to the last copied point (0 for a single point).
    pub span: i64,
}

impl PointSnapshot {
    /// Copies the points at `indices` of `clip`.
    pub fn capture(clip: &Clip, indices: &[usize]) -> Result<Self, ModelError> {
        let ClipContent::Automation(all) = &clip.content else {
            return Err(bad_song(format!(
                "clip {} is not an automation clip",
                clip.id.0
            )));
        };
        let chosen = picked(indices, all.len(), "point")?;
        let first = all[chosen[0]].tick;
        let last = all[chosen[chosen.len() - 1]].tick;
        Ok(Self {
            points: chosen
                .iter()
                .map(|i| AutomationPoint {
                    tick: all[*i].tick - first,
                    ..all[*i]
                })
                .collect(),
            span: last - first,
        })
    }
}

/// The device `delta` places after (positive) or before (negative) `from` in its scope's rack
/// order, as a path key, or `None` at the end of the list or for an unknown device. The rack
/// selection moves with Up and Down through this.
pub fn neighbour_device(doc: &RackDoc, from: &DeviceKey, delta: i32) -> Option<DeviceKey> {
    let (parent, local) = split_path(from);
    let scope = doc.scope_devices(parent.as_ref())?;
    let at = scope.iter().position(|d| d.key == local)?;
    let to = i64::try_from(at).ok()? + i64::from(delta);
    let to = usize::try_from(to).ok()?;
    scope.get(to).map(|d| join(parent.as_ref(), &d.key))
}

/// The clip after (`forward`) or before `id` in time order across all tracks (by start, then
/// id), or `None` at the ends or for an unknown clip.
pub fn neighbour_clip(song: &Song, id: ClipId, forward: bool) -> Option<ClipId> {
    let mut all: Vec<(i64, ClipId)> = song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter().map(|c| (c.start, c.id)))
        .collect();
    all.sort();
    let at = all.iter().position(|(_, i)| *i == id)?;
    if forward {
        all.get(at + 1).map(|(_, i)| *i)
    } else {
        at.checked_sub(1).map(|p| all[p].1)
    }
}

/// The track `delta` places after (positive) or before (negative) the track of `device` in song
/// order, as its device key, or `None` at the ends or when `device` has no track.
pub fn neighbour_track(song: &Song, device: &DeviceKey, delta: i32) -> Option<DeviceKey> {
    let at = song.tracks.iter().position(|t| &t.device == device)?;
    let to = usize::try_from(i64::try_from(at).ok()? + i64::from(delta)).ok()?;
    song.tracks.get(to).map(|t| t.device.clone())
}
