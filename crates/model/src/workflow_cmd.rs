//! Workflow commands: multi-device edits and the clipboard pastes, each
//! planned on a scratch copy of the document and committed as one undo step.
//!
//! Every command builds its edits on a [`Planner`], which performs each edit on a scratch copy
//! as it is planned, so each cable, key and limit is validated against the state it joins.
//! A command that fails anywhere is rejected whole and the document is untouched. Keys
//! of pasted devices come from the existing generators (`generate_key_in`, the plugin key base)
//! in rack order, clip ids from the song's next free id upward in snapshot order, and the
//! console channel of a pasted instrument from the rule (`add_channel`), all inside the
//! same step, so the same inputs always give the same document.

use crate::console::{MASTER_TYPE, is_console_type};
use crate::console_cmd::{Planner, add_channel, console_available, is_instrument};
use crate::container::{SPLIT_TYPE, split_path};
use crate::history::Edit;
use crate::plugin::{MAX_PROJECT_PLUGIN_STATE_BYTES, key_base_of_name};
use crate::rack_columns::{self, Placement};
use crate::song::{AUDIO_TRACK_TYPE, AutomationPoint, Clip, ClipContent, ClipId, LaneKind, Note};
use crate::song_cmd::{
    check_audio, check_content, check_geometry, has_notes_input, is_audio_track_type,
};
use crate::workflow::{ClipSnapshot, DeviceSnapshot, NoteSnapshot, PointSnapshot};
use crate::workflow_create::{self, CreateOptions, CreateSource, CreateTarget};
use crate::{
    Cable, DeviceKey, INTERFACE_KEY, ModelError, RackDevice, RackDoc, RackSlot, SocketRef,
    console_cmd, container_cmd, rules, zones,
};
use atomik_device_sdk::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// A workflow edit (wrapped in [`crate::Command::Workflow`]). Every command is one undo step; a
/// rejected command changes nothing. Keys are path keys (`container/device` inside a
/// container); a command that takes several keys needs them all in one scope.
///
/// Sync ops are those of the compound steps: `DeviceAdded` for each pasted device (with its
/// stored params), `RoutingChanged`, `ViewChanged` for a move, `ConsoleChanged` and
/// `SongChanged` when channels and tracks come along, and `DeviceRemoved` after
/// `RoutingChanged` for a removal. The commands open no drag-merge group and use no merge id.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowCommand {
    /// Removes the devices, each as [`crate::Command::RemoveDevice`] does it: its cables, its
    /// song track, its console channel and the rest of the console bookkeeping. Devices inside
    /// a removed container are not removed twice; a key that goes away with another (a channel
    /// with its instrument) is fine. One unknown key rejects the command.
    RemoveDevices {
        /// The devices.
        keys: Vec<DeviceKey>,
    },
    /// Copies the devices (with the cables between them) and pastes the copies right after the
    /// last selected one (after its console channel).
    DuplicateDevices {
        /// The devices, in one scope.
        keys: Vec<DeviceKey>,
    },
    /// Pastes a snapshot into the top level (or into `container`, a path key) at rack index `at`
    /// (clamped), keeping the snapshot's order. The copies get fresh keys and names, keep their
    /// params, zones, patterns, inner containers and plugin documents, keep the cables between
    /// them and drop every other cable. A copied instrument that feeds no other copied device
    /// gets its console channel and the master when missing; the tracks of the snapshot
    /// come back with fresh clip ids and are not armed; a copied audio track is wired to the
    /// interface inputs like a new one. The container, plugin and inner-device limits apply and
    /// every asset the snapshot needs must be in the document's pool.
    PasteDevices {
        /// What to paste.
        snapshot: DeviceSnapshot,
        /// The container to paste into, or `None` for the top level.
        container: Option<DeviceKey>,
        /// Rack index of the first pasted device.
        at: usize,
    },
    /// Moves the devices together: they keep their relative order and end up contiguous with the
    /// first at index `to` among the other devices of the scope (clamped so the group fits). A
    /// move that changes nothing is rejected.
    MoveDevices {
        /// The devices, in one scope.
        keys: Vec<DeviceKey>,
        /// Target index.
        to: usize,
    },
    /// Moves the devices together to a rack column slot ([`RackSlot`]): they keep their
    /// relative rack order and end up contiguous at `slot.index` among the devices that stay
    /// in `slot.column` (all take that column). `slot.column == columns` appends a new column in
    /// the same undo step; column 0 never goes above the Master Section. Inside a container only
    /// `slot.index` counts (like [`WorkflowCommand::MoveDevices`]). A move that changes nothing
    /// is rejected. One undo step; reports `ViewChanged` only.
    MoveDevicesToSlot {
        /// The devices, in one scope.
        keys: Vec<DeviceKey>,
        /// Target slot.
        slot: RackSlot,
    },
    /// [`WorkflowCommand::PasteDevices`] at a rack column slot: the copies land at
    /// `slot.index` of `slot.column` (a column past the last appends one). Inside a
    /// `container` only `slot.index` counts.
    PasteDevicesAt {
        /// What to paste.
        snapshot: DeviceSnapshot,
        /// The container to paste into, or `None` for the top level.
        container: Option<DeviceKey>,
        /// Where the first copy goes.
        slot: RackSlot,
    },
    /// [`WorkflowCommand::CreateInstrument`] at a rack column slot (see [`crate::Command::AddDeviceAt`]
    /// for how the slot resolves). The instrument's console channel lands by the console rules.
    CreateInstrumentAt {
        /// Where the instrument goes.
        slot: RackSlot,
        /// What to add.
        source: CreateSource,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options.
        options: CreateOptions,
    },
    /// [`WorkflowCommand::CreateEffect`] at a rack column slot: the effect is inserted exactly
    /// there and the routing rules run as if the device directly above the slot (the previous
    /// device of that column) were selected. A slot at the top of its column has no such
    /// device: the effect is only added, without auto-route. Instruments and console devices
    /// are rejected.
    CreateEffectAt {
        /// Where the effect goes.
        slot: RackSlot,
        /// What to add.
        source: CreateSource,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options (`skip_track` and `disarm` do not apply to effects).
        options: CreateOptions,
    },
    /// Duplicates a device together with its song track, lanes and clips (fresh clip ids, not
    /// armed, a console channel by the instrument rule), right after the device and its
    /// channel.
    DuplicateTrack {
        /// The device that has the track.
        device: DeviceKey,
    },
    /// Removes every cable of the device (a device without cables is rejected).
    DisconnectDevice {
        /// The device.
        key: DeviceKey,
    },
    /// Pastes clips with their track-relative layout: the earliest clip starts at `at_tick`.
    /// Each clip goes to the lane of its kind on its source track, or on `track` when given
    /// (a lane of the same kind must exist there). New ids are allocated from the song's next
    /// free id in snapshot order (the last is [`crate::Document::last_clip_id`]).
    PasteClips {
        /// The track that takes every clip, or `None` for the source tracks.
        track: Option<DeviceKey>,
        /// Song tick of the earliest pasted clip.
        at_tick: i64,
        /// What to paste.
        clips: ClipSnapshot,
    },
    /// Copies the clips right after the selection, keeping their layout: the earliest copy
    /// starts where the selection ends.
    DuplicateClips {
        /// The clips.
        ids: Vec<ClipId>,
    },
    /// Removes the clips (one undo step; `SongCommand::RemoveClip` cannot be batched, so a
    /// selection of several clips goes through this command). Unknown clips reject it.
    RemoveClips {
        /// The clips.
        ids: Vec<ClipId>,
    },
    /// Sets `muted` on every listed clip (note, automation and audio clips) in one undo step.
    /// An unknown clip rejects the command, and so does one that would change nothing (every
    /// clip already has that state) or an empty list.
    SetClipsMuted {
        /// The clips.
        ids: Vec<ClipId>,
        /// The new mute state.
        muted: bool,
    },
    /// Joins unlooped note or automation clips of one lane into the first of them (it keeps
    /// its id), which then spans all of them. Each clip contributes what it shows (its content
    /// window, notes cut at the window end); where two notes or points coincide the later
    /// clip wins. Audio and looped clips and clips of different lanes are rejected.
    JoinClips {
        /// At least two clips.
        ids: Vec<ClipId>,
    },
    /// Pastes notes into a note clip so the earliest starts at song tick `at_tick`. A pasted
    /// note replaces a note with the same start and key. The clip keeps its length.
    PasteNotes {
        /// The note clip.
        clip: ClipId,
        /// Song tick of the earliest pasted note.
        at_tick: i64,
        /// What to paste.
        notes: NoteSnapshot,
    },
    /// Copies the notes at `indices` (positions in the clip's note list) right after the
    /// selection.
    DuplicateNotes {
        /// The note clip.
        clip: ClipId,
        /// Note positions.
        indices: Vec<usize>,
    },
    /// Pastes automation points into an automation clip so the earliest is at song tick
    /// `at_tick`; a pasted point replaces a point at the same tick.
    PasteAutomationPoints {
        /// The automation clip.
        clip: ClipId,
        /// Song tick of the earliest pasted point.
        at_tick: i64,
        /// What to paste.
        points: PointSnapshot,
    },
    /// Copies the points at `indices` right after the selection (by the selection's span).
    DuplicatePoints {
        /// The automation clip.
        clip: ClipId,
        /// Point positions.
        indices: Vec<usize>,
    },
    /// Creates an instrument that wires itself, in one undo step: the
    /// device (with its console channel and the master), its song track when it has a
    /// Notes input, the track armed, and the tracks in `options.disarm` (the ones an earlier
    /// auto-route armed) disarmed. `options.route == false` is exactly `AddDevice` (or
    /// `PluginCommand::Add`); `options.skip_track` leaves the track out. `index` is clamped and
    /// `key` generated when `None`. Anything that is not an instrument is rejected.
    CreateInstrument {
        /// Rack position of the device (clamped).
        index: usize,
        /// What to add.
        source: CreateSource,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options.
        options: CreateOptions,
    },
    /// Creates an effect and routes it by the rules of
    /// the `workflow_create` module (first free insert slot of the selected channel, on the
    /// selected FX return, after the selected device, else only added), in one undo step.
    /// Instruments and console devices are rejected.
    CreateEffect {
        /// What to add.
        source: CreateSource,
        /// The selection the effect is routed from.
        target: CreateTarget,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Routing options (`skip_track` and `disarm` do not apply to effects).
        options: CreateOptions,
    },
    /// Applies the creation rules to existing devices that lack them, in
    /// one step: an instrument gets its console channel and its armed song track, a loose
    /// stereo effect goes to the next free insert of the nearest channel before it (or is cabled
    /// after the nearest device with free outputs). Devices that are routed already are skipped;
    /// the command is rejected, with the reason, when nothing changes. `options.route` is
    /// ignored.
    AutoRoute {
        /// The devices (top level).
        devices: Vec<DeviceKey>,
        /// `skip_track` and `disarm` apply.
        options: CreateOptions,
    },
    /// "Use as Send Effect": puts an existing, unrouted stereo effect on the
    /// lowest free send/return slot (`SetReturn`), in one undo step. Rejected, with the reason and
    /// no change, for an effect that is on a return, in an insert chain or cabled already, for a
    /// console device, an instrument or a device that is not a stereo effect, and when all eight
    /// slots are in use.
    AssignSend {
        /// The effect (top level).
        effect: DeviceKey,
    },
    /// Removes every cable of every device, in one step. Devices in one scope;
    /// devices without cables are fine when some other has any.
    DisconnectDevices {
        /// The devices.
        keys: Vec<DeviceKey>,
    },
    /// Removes the devices like [`WorkflowCommand::RemoveDevices`] and keeps
    /// the audio path of each removed top-level device where it is unambiguous: its single
    /// source is cabled to its destinations (an effect in a chain, a middle insert). Console
    /// devices, instruments, paths that fan in from several sources, outputs that match no
    /// input, the insert loop of a channel and devices inside containers are removed plainly.
    DeleteDevices {
        /// The devices.
        keys: Vec<DeviceKey>,
    },
}

fn bad_song(message: impl Into<String>) -> ModelError {
    ModelError::BadSong(message.into())
}

fn bad_scope(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// Plans a workflow command: the edit and the last clip id it allocated or reports, if any.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: WorkflowCommand,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let plain = |edit: Result<Edit, ModelError>| edit.map(|e| (e, None));
    match command {
        WorkflowCommand::RemoveDevices { keys } => {
            plain(remove_devices(doc, registry, &keys, false))
        }
        WorkflowCommand::DeleteDevices { keys } => {
            plain(remove_devices(doc, registry, &keys, true))
        }
        WorkflowCommand::CreateInstrument {
            index,
            source,
            key,
            options,
        } => plain(workflow_create::create_instrument(
            doc, registry, index, &source, key, &options,
        )),
        WorkflowCommand::CreateEffect {
            source,
            target,
            key,
            options,
        } => plain(workflow_create::create_effect(
            doc, registry, &source, &target, key, &options,
        )),
        WorkflowCommand::CreateInstrumentAt {
            slot,
            source,
            key,
            options,
        } => {
            let at = doc.resolve_slot(slot)?;
            let edit = workflow_create::create_instrument(
                doc, registry, at.index, &source, key, &options,
            )?;
            Ok((rack_columns::place_new_device(doc, at, edit), None))
        }
        WorkflowCommand::CreateEffectAt {
            slot,
            source,
            key,
            options,
        } => plain(workflow_create::create_effect_at(
            doc, registry, slot, &source, key, &options,
        )),
        WorkflowCommand::MoveDevicesToSlot { keys, slot } => {
            plain(move_devices(doc, &keys, Placement::Slot(slot)))
        }
        WorkflowCommand::PasteDevicesAt {
            snapshot,
            container,
            slot,
        } => {
            let mut p = Planner::new(doc, registry);
            let (at, column) = match &container {
                None => {
                    let r = doc.resolve_slot(slot)?;
                    (r.index, Some(r.column))
                }
                Some(_) => (slot.index, None),
            };
            paste_devices(&mut p, &snapshot, container.as_ref(), at, column)?;
            Ok((p.finish(), None))
        }
        WorkflowCommand::AutoRoute { devices, options } => plain(workflow_create::auto_route(
            doc, registry, &devices, &options,
        )),
        WorkflowCommand::AssignSend { effect } => {
            plain(workflow_create::assign_send(doc, registry, &effect))
        }
        WorkflowCommand::DisconnectDevices { keys } => {
            plain(workflow_create::disconnect_devices(doc, &keys))
        }
        WorkflowCommand::DuplicateDevices { keys } => {
            plain(duplicate_devices(doc, registry, &keys))
        }
        WorkflowCommand::PasteDevices {
            snapshot,
            container,
            at,
        } => {
            let mut p = Planner::new(doc, registry);
            paste_devices(&mut p, &snapshot, container.as_ref(), at, None)?;
            Ok((p.finish(), None))
        }
        WorkflowCommand::MoveDevices { keys, to } => {
            plain(move_devices(doc, &keys, Placement::Flat(to)))
        }
        WorkflowCommand::DuplicateTrack { device } => {
            plain(duplicate_track(doc, registry, &device))
        }
        WorkflowCommand::DisconnectDevice { key } => plain(disconnect_device(doc, &key)),
        WorkflowCommand::PasteClips {
            track,
            at_tick,
            clips,
        } => paste_clips(doc, registry, track.as_ref(), at_tick, &clips),
        WorkflowCommand::DuplicateClips { ids } => {
            let s = ClipSnapshot::capture(doc, &ids)?;
            paste_clips(doc, registry, None, s.origin.saturating_add(s.span), &s)
        }
        WorkflowCommand::JoinClips { ids } => join_clips(doc, &ids),
        WorkflowCommand::RemoveClips { ids } => plain(remove_clips(doc, &ids)),
        WorkflowCommand::SetClipsMuted { ids, muted } => plain(set_clips_muted(doc, &ids, muted)),
        WorkflowCommand::PasteNotes {
            clip,
            at_tick,
            notes,
        } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let base = at_tick.saturating_sub(c.start).saturating_add(c.offset);
            plain(notes_edit(doc, registry, clip, base, &notes))
        }
        WorkflowCommand::DuplicateNotes { clip, indices } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let s = NoteSnapshot::capture(c, &indices)?;
            let ClipContent::Notes(all) = &c.content else {
                unreachable!("capture checked the kind")
            };
            let first = all[indices.iter().copied().min().unwrap_or(0)].tick;
            plain(notes_edit(doc, registry, clip, first + s.span, &s))
        }
        WorkflowCommand::PasteAutomationPoints {
            clip,
            at_tick,
            points,
        } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let base = at_tick.saturating_sub(c.start).saturating_add(c.offset);
            plain(points_edit(doc, registry, clip, base, &points))
        }
        WorkflowCommand::DuplicatePoints { clip, indices } => {
            let (_, _, c) = clip_of(doc, clip)?;
            let s = PointSnapshot::capture(c, &indices)?;
            let ClipContent::Automation(all) = &c.content else {
                unreachable!("capture checked the kind")
            };
            let first = all[indices.iter().copied().min().unwrap_or(0)].tick;
            plain(points_edit(doc, registry, clip, first + s.span, &s))
        }
    }
}

// ------------------------------------------------------------------------------ devices

/// The ancestors of a path key: `a/b/c` has `a` and `a/b`.
fn ancestors(key: &DeviceKey) -> impl Iterator<Item = DeviceKey> + '_ {
    key.0
        .match_indices('/')
        .map(|(i, _)| DeviceKey(key.0[..i].to_owned()))
}

fn remove_devices(
    doc: &RackDoc,
    registry: &Registry,
    keys: &[DeviceKey],
    bridge: bool,
) -> Result<Edit, ModelError> {
    if keys.is_empty() {
        return Err(bad_scope("nothing is selected"));
    }
    let set: BTreeSet<&DeviceKey> = keys.iter().collect();
    for k in &set {
        rules::resolve(doc, k)?;
    }
    // A device inside a removed container goes with it.
    let tops: Vec<DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| set.contains(&d.key))
        .map(|d| d.key.clone())
        .collect();
    let inners: Vec<DeviceKey> = set
        .iter()
        .filter(|k| k.0.contains('/') && !ancestors(k).any(|a| set.contains(&a)))
        .map(|k| (*k).clone())
        .collect();
    let mut p = Planner::new(doc, registry);
    for k in tops {
        // An instrument's channel can be in the set too and is gone by the time its turn comes.
        if rules::index_of(p.doc(), &k).is_ok() {
            let joins = if bridge {
                workflow_create::bridge_cables(p.doc(), registry, &k)
            } else {
                Vec::new()
            };
            console_cmd::remove_device(&mut p, &k)?;
            workflow_create::connect_bridge(&mut p, &joins);
        }
    }
    for k in inners {
        if p.doc().resolve(&k).is_some() {
            let edit = container_cmd::remove_inner(p.doc(), &k)?;
            p.push(edit);
        }
    }
    Ok(p.finish())
}

/// The rack index right after `source` (a local key of `parent`'s scope) and, at the top level,
/// after the console channels it feeds.
fn after_with_channels(doc: &RackDoc, parent: Option<&DeviceKey>, source: &DeviceKey) -> usize {
    let scope = doc.scope_devices(parent).unwrap_or(&[]);
    let mut at = scope
        .iter()
        .position(|d| &d.key == source)
        .map_or(scope.len(), |i| i + 1);
    if parent.is_none() {
        while at < scope.len()
            && doc
                .console
                .channels
                .iter()
                .any(|c| c.channel == scope[at].key && c.source.as_ref() == Some(source))
        {
            at += 1;
        }
    }
    at
}

fn duplicate_devices(
    doc: &RackDoc,
    registry: &Registry,
    keys: &[DeviceKey],
) -> Result<Edit, ModelError> {
    let snapshot = DeviceSnapshot::capture(doc, keys, false)?;
    let parent = keys.first().and_then(|k| split_path(k).0);
    let last = &snapshot.devices[snapshot.devices.len() - 1].key;
    let at = after_with_channels(doc, parent.as_ref(), last);
    let mut p = Planner::new(doc, registry);
    paste_devices(&mut p, &snapshot, parent.as_ref(), at, None)?;
    Ok(p.finish())
}

fn duplicate_track(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
) -> Result<Edit, ModelError> {
    rules::device(doc, device)?;
    if doc.song.track(device).is_none() {
        return Err(bad_song(format!("{device} has no track")));
    }
    let snapshot = DeviceSnapshot::capture(doc, std::slice::from_ref(device), true)?;
    let at = after_with_channels(doc, None, device);
    let mut p = Planner::new(doc, registry);
    paste_devices(&mut p, &snapshot, None, at, None)?;
    Ok(p.finish())
}

/// `base` without a trailing ` <digits>`: `Poly Synth 2` -> `Poly Synth`.
fn name_base(name: &str) -> &str {
    match name.rsplit_once(' ') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => name,
    }
}

/// A name for a pasted device: its own when free in the scope, else the next free numbered one.
fn pasted_name(scope: &[RackDevice], name: &str) -> String {
    if scope.iter().all(|d| d.name != name) {
        name.to_owned()
    } else {
        rules::unique_name_in(scope, name_base(name))
    }
}

/// The zone ranges of `device` and of everything inside it, against the pool of `doc`.
fn check_device_zones(doc: &RackDoc, device: &RackDevice) -> Result<(), ModelError> {
    zones::check_zones(&doc.assets, &device.zones)
        .map_err(|(i, m)| ModelError::BadZones(format!("{} zone {i}: {m}", device.name)))?;
    if let Some(inner) = &device.inner {
        for d in &inner.devices {
            check_device_zones(doc, d)?;
        }
    }
    Ok(())
}

/// Adds the channel of an instrument as `AddDevice` does; an instrument that cannot be wired
/// (no audio output) stays a plain device.
fn try_channel(p: &mut Planner<'_>, source: &DeviceKey) {
    let mut sub = Planner::new(p.doc(), p.registry);
    if add_channel(&mut sub, source).is_ok() {
        p.push(sub.finish());
    }
}

/// Plans the paste of `snapshot` (see [`WorkflowCommand::PasteDevices`]) on `p`; returns the new
/// keys in snapshot order. At the top level the copies all go to `column` (a column equal to
/// the rack's column count is appended first), by default the column of the device before the
/// first copy; inside a container they are in column 0.
fn paste_devices(
    p: &mut Planner<'_>,
    snapshot: &DeviceSnapshot,
    container: Option<&DeviceKey>,
    at: usize,
    column: Option<u16>,
) -> Result<Vec<DeviceKey>, ModelError> {
    if snapshot.devices.is_empty() {
        return Err(bad_scope("the clipboard holds no devices"));
    }
    if let Some(c) = container {
        rules::resolve(p.doc(), c)?;
        if p.doc().inner(c).is_none() {
            return Err(bad_scope(format!("{c} is not a container")));
        }
        if !snapshot.tracks.is_empty() {
            return Err(bad_scope("a track cannot live inside a container"));
        }
    }
    if let Some(h) = snapshot
        .required_assets()
        .into_iter()
        .find(|h| !p.doc().assets.contains_key(h))
    {
        return Err(ModelError::UnknownKey(format!(
            "asset {h} is not in this project's sample pool"
        )));
    }
    let first = match container {
        // Nothing goes above the master: a paste at the top lands below it.
        None => rules::top_index(p.doc(), at),
        Some(_) => at.min(p.doc().scope_devices(container).map_or(0, <[_]>::len)),
    };
    let column = match container {
        None => column.unwrap_or_else(|| p.doc().column_at(first)),
        Some(_) => 0,
    };
    if container.is_none() && column >= p.doc().columns {
        let old = p.doc().columns;
        p.push(Edit::Columns {
            old,
            new: column + 1,
        });
    }
    let mut map: BTreeMap<DeviceKey, DeviceKey> = BTreeMap::new();
    let mut new_keys = Vec::with_capacity(snapshot.devices.len());
    for (i, d) in snapshot.devices.iter().enumerate() {
        rules::descriptor(p.registry, &d.type_id)?;
        if d.type_id == MASTER_TYPE {
            return Err(rules::fixed_master(rules::MASTER_COPY));
        }
        if is_console_type(&d.type_id) {
            return Err(ModelError::BadConsole(format!(
                "{} is a console device; it cannot be pasted",
                d.name
            )));
        }
        if d.type_id == SPLIT_TYPE {
            return Err(ModelError::NotAllowedInContainer(format!(
                "{SPLIT_TYPE} is internal and cannot be pasted"
            )));
        }
        check_device_zones(p.doc(), d)?;
        let scope = p.doc().scope_devices(container).unwrap_or(&[]);
        let new_key = match &d.plugin {
            Some(plugin) => {
                rules::generate_key_with_base_in(scope, &key_base_of_name(&plugin.layout.name))
            }
            None => rules::generate_key_in(scope, &d.type_id),
        };
        let mut copy = d.clone();
        copy.key = new_key.clone();
        copy.name = pasted_name(scope, &d.name);
        copy.column = column;
        let index = (first + i).min(scope.len());
        p.push(match container {
            None => Edit::Insert {
                index,
                device: copy,
                cables: Vec::new(),
            },
            Some(c) => Edit::InnerInsert {
                container: c.clone(),
                index,
                device: copy,
                cables: Vec::new(),
            },
        });
        map.insert(d.key.clone(), new_key.clone());
        new_keys.push(new_key);
    }
    for c in &snapshot.cables {
        let (Some(from), Some(to)) = (map.get(&c.from.device), map.get(&c.to.device)) else {
            continue;
        };
        let cable = Cable {
            from: SocketRef {
                device: from.clone(),
                socket: c.from.socket.clone(),
            },
            to: SocketRef {
                device: to.clone(),
                socket: c.to.socket.clone(),
            },
        };
        match container {
            None => p.connect(cable)?,
            Some(cont) => {
                rules::check_inner_cable(p.doc(), p.registry, cont, &cable)?;
                p.push(Edit::InnerConnect {
                    container: cont.clone(),
                    cable,
                });
            }
        }
    }
    if container.is_none() {
        paste_tracks(p, snapshot, &map)?;
        let feeds: BTreeSet<&DeviceKey> = snapshot.cables.iter().map(|c| &c.from.device).collect();
        for d in &snapshot.devices {
            if is_instrument(p.registry, &d.type_id)
                && console_available(p.registry)
                && !feeds.contains(&d.key)
            {
                try_channel(p, &map[&d.key]);
            }
        }
        for t in &snapshot.tracks {
            let Some(new) = map.get(&t.device) else {
                continue;
            };
            if p.type_of(new)? == AUDIO_TRACK_TYPE {
                wire_inputs(p, new)?;
            }
        }
    }
    p.doc().check_containers(p.registry)?;
    if p.doc().plugin_state_bytes(None) > MAX_PROJECT_PLUGIN_STATE_BYTES {
        return Err(ModelError::BadPlugin(format!(
            "the plugin states of the project would total more than {MAX_PROJECT_PLUGIN_STATE_BYTES} bytes"
        )));
    }
    Ok(new_keys)
}

/// The song tracks of a snapshot for the pasted devices: fresh clip ids, not armed.
fn paste_tracks(
    p: &mut Planner<'_>,
    snapshot: &DeviceSnapshot,
    map: &BTreeMap<DeviceKey, DeviceKey>,
) -> Result<(), ModelError> {
    let start = p.doc().song.next_clip_id;
    let mut next = start;
    for t in &snapshot.tracks {
        let Some(new) = map.get(&t.device) else {
            continue;
        };
        let desc = rules::descriptor(p.registry, p.type_of(new)?)?;
        if !is_audio_track_type(desc) && !has_notes_input(desc) {
            return Err(bad_song(format!("{new} has no Notes input")));
        }
        let mut track = t.clone();
        track.device = new.clone();
        track.armed = false;
        for lane in &mut track.lanes {
            for c in &mut lane.clips {
                c.id = ClipId(next);
                next = next
                    .checked_add(1)
                    .ok_or_else(|| bad_song("clip ids are exhausted"))?;
                if let ClipContent::Audio(a) = &c.content {
                    check_audio(p.doc(), a)?;
                }
            }
        }
        let index = p.doc().song.tracks.len();
        p.push(Edit::Track {
            index,
            old: None,
            new: Some(track),
        });
    }
    if next != start {
        p.push(Edit::NextClipId {
            old: start,
            new: next,
        });
    }
    Ok(())
}

/// The interface inputs of a new audio track, as `AddAudioTrack` wires them.
fn wire_inputs(p: &mut Planner<'_>, device: &DeviceKey) -> Result<(), ModelError> {
    let inputs = p.doc().input_channels;
    for (input, socket) in [(1u16, "in_l"), (2, "in_r")] {
        if inputs == 0 {
            break;
        }
        p.connect(Cable {
            from: SocketRef {
                device: DeviceKey(INTERFACE_KEY.to_owned()),
                socket: format!("in_{}", input.min(inputs)),
            },
            to: SocketRef {
                device: device.clone(),
                socket: socket.to_owned(),
            },
        })?;
    }
    Ok(())
}

fn move_devices(doc: &RackDoc, keys: &[DeviceKey], place: Placement) -> Result<Edit, ModelError> {
    let first = keys
        .first()
        .ok_or_else(|| bad_scope("nothing is selected"))?;
    let parent = split_path(first).0;
    let mut wanted = BTreeSet::new();
    for k in keys {
        let (p, local) = split_path(k);
        if p != parent {
            return Err(bad_scope(format!(
                "{first} and {k} are not in the same scope"
            )));
        }
        rules::resolve(doc, k)?;
        wanted.insert(local);
    }
    if parent.is_none() {
        let edit = rack_columns::plan_move(doc, keys, place)?;
        if rack_columns::is_noop(&edit) {
            return Err(ModelError::OutOfRange(
                "the devices are already there".into(),
            ));
        }
        return Ok(edit);
    }
    // Inside a container: one column, only the index counts.
    let to = match place {
        Placement::Flat(to) => to,
        Placement::Slot(slot) => slot.index,
    };
    let order: Vec<DeviceKey> = doc
        .scope_devices(parent.as_ref())
        .ok_or_else(|| ModelError::UnknownDevice(first.0.clone()))?
        .iter()
        .map(|d| d.key.clone())
        .collect();
    let group: Vec<DeviceKey> = order
        .iter()
        .filter(|k| wanted.contains(k))
        .cloned()
        .collect();
    let rest: Vec<DeviceKey> = order
        .iter()
        .filter(|k| !wanted.contains(k))
        .cloned()
        .collect();
    let at = to.min(rest.len());
    let mut target = rest[..at].to_vec();
    target.extend(group.iter().cloned());
    target.extend_from_slice(&rest[at..]);
    if target == order {
        return Err(ModelError::OutOfRange(
            "the devices are already there".into(),
        ));
    }
    let mut current = order;
    let mut edits = Vec::new();
    for (i, k) in target.iter().enumerate() {
        let from = current
            .iter()
            .position(|c| c == k)
            .expect("the target is a permutation");
        if from != i {
            let k = current.remove(from);
            current.insert(i, k);
            edits.push(Edit::InnerMove {
                container: parent.clone().expect("a container scope"),
                from,
                to: i,
            });
        }
    }
    Ok(Edit::Compound(edits))
}

fn disconnect_device(doc: &RackDoc, key: &DeviceKey) -> Result<Edit, ModelError> {
    rules::resolve(doc, key)?;
    let (parent, local) = split_path(key);
    let cables: Vec<Cable> = doc
        .scope_cables(parent.as_ref())
        .unwrap_or(&[])
        .iter()
        .filter(|c| c.from.device == local || c.to.device == local)
        .cloned()
        .collect();
    if cables.is_empty() {
        return Err(ModelError::BadCable(format!("{key} has no cables")));
    }
    Ok(Edit::Compound(
        cables
            .into_iter()
            .map(|cable| match &parent {
                None => Edit::Disconnect(cable),
                Some(c) => Edit::InnerDisconnect {
                    container: c.clone(),
                    cable,
                },
            })
            .collect(),
    ))
}

// ------------------------------------------------------------------------------ clips

fn clip_of(doc: &RackDoc, id: ClipId) -> Result<(DeviceKey, usize, &Clip), ModelError> {
    doc.song
        .clip(id)
        .map(|(t, lane, c)| (t.device.clone(), lane, c))
        .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))
}

fn lane_name(kind: &LaneKind) -> String {
    match kind {
        LaneKind::Notes => "note".into(),
        LaneKind::Audio => "audio".into(),
        LaneKind::Automation(p) => format!("{p} automation"),
    }
}

fn paste_clips(
    doc: &RackDoc,
    registry: &Registry,
    track: Option<&DeviceKey>,
    at_tick: i64,
    snapshot: &ClipSnapshot,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    if snapshot.entries.is_empty() {
        return Err(bad_song("the clipboard holds no clips"));
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
    let mut p = Planner::new(doc, registry);
    let start = doc.song.next_clip_id;
    let mut next = start;
    for e in &snapshot.entries {
        let device = track.unwrap_or(&e.device);
        let t = doc
            .song
            .track(device)
            .ok_or_else(|| bad_song(format!("{device} has no track")))?;
        let lane = t
            .lanes
            .iter()
            .position(|l| l.kind == e.lane)
            .ok_or_else(|| bad_song(format!("{device} has no {} lane", lane_name(&e.lane))))?;
        let desc = rules::descriptor(registry, &rules::device(doc, device)?.type_id)?;
        let mut clip = e.clip.clone();
        clip.id = ClipId(next);
        clip.start = at_tick.saturating_add(e.clip.start);
        next = next
            .checked_add(1)
            .ok_or_else(|| bad_song("clip ids are exhausted"))?;
        check_geometry(clip.start, clip.length, clip.offset, clip.loop_length)
            .map_err(ModelError::OutOfRange)?;
        clip.content = check_content(doc, clip.content, &t.lanes[lane].kind, desc)?;
        p.push(Edit::Clip {
            device: device.clone(),
            lane,
            old: None,
            new: Some(clip),
        });
    }
    p.push(Edit::NextClipId {
        old: start,
        new: next,
    });
    Ok((p.finish(), Some(ClipId(next - 1))))
}

/// Removes the clips in one step (`SongCommand::RemoveClip` cannot be batched, so a selection
/// of several clips needs this one). An unknown clip rejects the command.
fn remove_clips(doc: &RackDoc, ids: &[ClipId]) -> Result<Edit, ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    if wanted.is_empty() {
        return Err(bad_song("no clips are selected"));
    }
    let mut edits = Vec::new();
    for id in &wanted {
        let (device, lane, c) = clip_of(doc, *id)?;
        edits.push(Edit::Clip {
            device,
            lane,
            old: Some(c.clone()),
            new: None,
        });
    }
    Ok(Edit::Compound(edits))
}

fn set_clips_muted(doc: &RackDoc, ids: &[ClipId], muted: bool) -> Result<Edit, ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    if wanted.is_empty() {
        return Err(bad_song("no clips are selected"));
    }
    let mut edits = Vec::new();
    for id in &wanted {
        let (device, lane, c) = clip_of(doc, *id)?;
        if c.muted != muted {
            edits.push(Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip { muted, ..c.clone() }),
            });
        }
    }
    if edits.is_empty() {
        return Err(bad_song(if muted {
            "the clips are already muted"
        } else {
            "the clips are not muted"
        }));
    }
    Ok(Edit::Compound(edits))
}

fn join_clips(doc: &RackDoc, ids: &[ClipId]) -> Result<(Edit, Option<ClipId>), ModelError> {
    let wanted: BTreeSet<ClipId> = ids.iter().copied().collect();
    let mut clips = Vec::new();
    for id in &wanted {
        clips.push(clip_of(doc, *id)?);
    }
    if clips.len() < 2 {
        return Err(bad_song("joining needs at least two clips"));
    }
    clips.sort_by_key(|(_, _, c)| (c.start, c.id));
    let (device, lane, _) = clips[0].clone();
    let kind = &doc.song.track(&device).expect("the clip has a track").lanes[lane].kind;
    if clips.iter().any(|(d, l, _)| *d != device || *l != lane) {
        return Err(bad_song("clips of different lanes cannot be joined"));
    }
    if kind.is_audio() {
        return Err(bad_song("audio clips cannot be joined"));
    }
    if let Some((_, _, c)) = clips.iter().find(|(_, _, c)| c.loop_length.is_some()) {
        return Err(bad_song(format!(
            "clip {} loops; unloop it before joining",
            c.id.0
        )));
    }
    let start = clips[0].2.start;
    let end = clips.iter().map(|(_, _, c)| c.end()).max().unwrap_or(start);
    let content = match kind {
        LaneKind::Notes => {
            let mut notes: BTreeMap<(i64, u8), Note> = BTreeMap::new();
            for (_, _, c) in &clips {
                let ClipContent::Notes(all) = &c.content else {
                    return Err(bad_song("note lane without notes"));
                };
                let window = c.offset..c.offset + c.length;
                for n in all.iter().filter(|n| window.contains(&n.tick)) {
                    let tick = c.start + (n.tick - c.offset) - start;
                    notes.insert(
                        (tick, n.key),
                        Note {
                            tick,
                            length: n.length.min(window.end - n.tick),
                            ..*n
                        },
                    );
                }
            }
            ClipContent::Notes(notes.into_values().collect())
        }
        _ => {
            let mut points: BTreeMap<i64, AutomationPoint> = BTreeMap::new();
            for (_, _, c) in &clips {
                let ClipContent::Automation(all) = &c.content else {
                    return Err(bad_song("automation lane without points"));
                };
                let window = c.offset..c.offset + c.length;
                for pt in all.iter().filter(|pt| window.contains(&pt.tick)) {
                    let tick = c.start + (pt.tick - c.offset) - start;
                    points.insert(tick, AutomationPoint { tick, ..*pt });
                }
            }
            ClipContent::Automation(points.into_values().collect())
        }
    };
    check_geometry(start, end - start, 0, None).map_err(ModelError::OutOfRange)?;
    let first = clips[0].2;
    let joined = Clip {
        id: first.id,
        start,
        length: end - start,
        offset: 0,
        loop_length: None,
        muted: clips.iter().all(|(_, _, c)| c.muted),
        grid: first.grid,
        scale: first.scale,
        content,
    };
    let mut edits: Vec<Edit> = clips[1..]
        .iter()
        .map(|(device, lane, c)| Edit::Clip {
            device: device.clone(),
            lane: *lane,
            old: Some((*c).clone()),
            new: None,
        })
        .collect();
    edits.push(Edit::Clip {
        device,
        lane,
        old: Some(first.clone()),
        new: Some(joined),
    });
    Ok((Edit::Compound(edits), Some(first.id)))
}

/// Replaces the content of clip `id` with the result of `merge`, checked as `SetClipContent`
/// checks it.
fn content_edit(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    merge: impl FnOnce(&Clip) -> Result<ClipContent, ModelError>,
) -> Result<Edit, ModelError> {
    let (device, lane, c) = clip_of(doc, id)?;
    let desc = rules::descriptor(registry, &rules::device(doc, &device)?.type_id)?;
    let kind = &doc.song.track(&device).expect("the clip has a track").lanes[lane].kind;
    let content = check_content(doc, merge(c)?, kind, desc)?;
    Ok(Edit::Clip {
        device,
        lane,
        old: Some(c.clone()),
        new: Some(Clip {
            content,
            ..c.clone()
        }),
    })
}

fn notes_edit(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    base: i64,
    snapshot: &NoteSnapshot,
) -> Result<Edit, ModelError> {
    content_edit(doc, registry, id, |c| {
        let ClipContent::Notes(existing) = &c.content else {
            return Err(bad_song(format!("clip {} is not a note clip", id.0)));
        };
        let mut notes: BTreeMap<(i64, u8), Note> =
            existing.iter().map(|n| ((n.tick, n.key), *n)).collect();
        for n in &snapshot.notes {
            let tick = base.saturating_add(n.tick);
            notes.insert((tick, n.key), Note { tick, ..*n });
        }
        Ok(ClipContent::Notes(notes.into_values().collect()))
    })
}

fn points_edit(
    doc: &RackDoc,
    registry: &Registry,
    id: ClipId,
    base: i64,
    snapshot: &PointSnapshot,
) -> Result<Edit, ModelError> {
    content_edit(doc, registry, id, |c| {
        let ClipContent::Automation(existing) = &c.content else {
            return Err(bad_song(format!("clip {} is not an automation clip", id.0)));
        };
        let mut points: BTreeMap<i64, AutomationPoint> =
            existing.iter().map(|p| (p.tick, *p)).collect();
        for pt in &snapshot.points {
            let tick = base.saturating_add(pt.tick);
            points.insert(tick, AutomationPoint { tick, ..*pt });
        }
        Ok(ClipContent::Automation(points.into_values().collect()))
    })
}
