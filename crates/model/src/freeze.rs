//! Freeze and bounce in place: the model side.
//!
//! A track is **frozen** when its instrument and the first effects of its channel's insert chain
//! have been rendered to audio. The render lives in the track as [`Frozen`] (an audio clip's worth
//! of content, where it starts and how long it is, and the effects it includes); the track's own
//! clips, its instrument and those effects stay in the document exactly as they were, so an
//! unfreeze restores the state bit for bit. Nothing the song or the rack stores changes: the
//! frozen state is *applied* when the engine graph is built.
//!
//! # What a freeze covers
//!
//! The **chain** of a track is its instrument (or an audio track's player) followed by the
//! effects on its console channel's insert loop ([`crate::insert_chain`], at most four). A freeze
//! takes the track *through* the first `n` of them (`effects`, `0..=len`): the render is the
//! signal at the output of the last of them (the instrument's output when `n` is 0), and the
//! effects after it stay live, processing the frozen audio. Unless the freeze bakes the strip in,
//! the channel strip itself (fader, pan, width, sends, mute, solo, output) is not frozen, and
//! neither are send effects: both keep working on the frozen audio. What the strip does in front
//! of its insert point (input gain, filters, and dynamics and EQ unless they follow the insert)
//! is part of the sound at the tap, so it is frozen with an insert chain of one or more effects.
//!
//! A **container** instrument or effect freezes as one device: every device inside it is flagged
//! and unplugged, and the tap is the inner output that feeds the container's output
//! ([`ExpandedRack::exits`](crate::ExpandedRack::exits)).
//!
//! A freeze can also **bake the strip**: the tap is the channel's own output, the whole insert
//! chain is included and the channel is flagged and unplugged like the instrument, so the
//! consumers of the channel (the master or a bus) are fed by the render directly. A strip with a
//! send on or a parallel channel is refused ([`strip_blocker`]).
//!
//! # How it is applied
//!
//! [`RackDoc::expand`](crate::RackDoc::expand) calls [`apply`]: the devices of a frozen track
//! (its instrument and its frozen effects) stay in the flat list, flagged
//! [`ExpandedDevice::frozen`](crate::ExpandedDevice::frozen), so every other device keeps its
//! engine id (and so its prepare seed: freezing a track does not change the sound of another
//! one), but every cable that touches them is dropped and the consumers of the last frozen
//! device's output are re-sourced to a synthetic `atomik.audio-track` device keyed
//! `<track>/~freeze`, appended at the end of the flat list. The flagged devices are not built, not
//! inserted into the engine and never processed; the song feeds the `~freeze` device the frozen
//! clip as the playback of an ordinary audio track.
//!
//! Commands that would change a frozen track or the devices it freezes are refused
//! ([`guard`]), with a message that says to unfreeze first.

use crate::console::{CHANNEL_TYPE, INSERT_SLOTS};
use crate::expand::{ExpandedDevice, ExpandedRack};
use crate::history::Edit;
use crate::song::{
    AUDIO_TRACK_TYPE, AudioClipContent, Clip, ClipContent, ClipId, Lane, SongCommand, Track,
};
use crate::song_cmd::{
    add_audio_track, audio_lane, bad, check_audio, check_geometry, find_clip, insert_new, range,
    track_index,
};
use crate::workflow_cmd::WorkflowCommand;
use crate::{
    Cable, Command, ConsoleCommand, ContainerCommand, CreateTarget, DeviceKey, InsertEffect,
    ModelError, RackDoc, SocketRef, insert_chain,
};
use atomik_device_sdk::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// The suffix of the synthetic audio player a frozen track plays through (`<track>/~freeze`),
/// like `~split` for a container.
pub const FREEZE_SUFFIX: &str = "~freeze";

/// The key of the synthetic player of frozen track `device`.
pub fn freeze_key(device: &DeviceKey) -> DeviceKey {
    DeviceKey(format!("{}/{FREEZE_SUFFIX}", device.0))
}

/// A frozen track: the render that plays in place of the track's chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Frozen {
    /// Song tick where the render starts.
    pub start: i64,
    /// Length of the render in ticks (the clip it plays as).
    pub length: i64,
    /// The rendered audio: an asset of the pool, from `source_offset`, with its gain and fades
    /// (a plain clip: no reverse, no warp).
    pub audio: AudioClipContent,
    /// The effects of the channel's insert chain that the render includes, in chain order (the
    /// first `effects.len()` of [`insert_chain`]). Empty: the instrument alone.
    pub effects: Vec<DeviceKey>,
    /// The render is taken at the output of the console channel, so the whole strip (input
    /// stage, EQ, dynamics, inserts, fader and pan) is baked into it and the channel is not
    /// built while the track is frozen. `effects` then lists the whole insert chain. `false`:
    /// the render stops at the insert chain and the strip stays live.
    pub strip: bool,
}

impl Frozen {
    /// The render as the audio clip it plays as (id 0: it is not a clip of any lane).
    pub fn clip(&self) -> Clip {
        Clip {
            id: ClipId(0),
            start: self.start,
            length: self.length,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: None,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            content: ClipContent::Audio(self.audio.clone()),
        }
    }
}

impl Track {
    /// The track as the engine plays it while frozen: a single audio lane holding the render, on
    /// the same device and with the same mute and solo state, so it flattens like any audio
    /// track. `None` when the track is not frozen.
    pub fn frozen_playback(&self) -> Option<Track> {
        let frozen = self.frozen.as_ref()?;
        Some(Track {
            device: self.device.clone(),
            muted: self.muted,
            solo: self.solo,
            armed: false,
            chase: false,
            notes_to: None,
            direct_record: false,
            controllers: Vec::new(),
            groove: None,
            frozen: None,
            lanes: vec![Lane::audio().with_clips(vec![frozen.clip()])],
        })
    }
}

/// What a track's chain is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackChain {
    /// The console channel the track plays through.
    pub channel: DeviceKey,
    /// The effects of the channel's insert loop a freeze can include, in signal order (a
    /// container effect is one of them: it freezes with everything inside it).
    pub effects: Vec<DeviceKey>,
}

/// The chain of the track on `device`.
///
/// # Errors
/// The reason a track cannot be frozen: its device has no console channel, or is not cabled to it.
/// A container instrument or effect is fine: it is frozen with every device inside it.
pub fn track_chain(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
) -> Result<TrackChain, String> {
    let dev = doc
        .device(device)
        .ok_or_else(|| format!("{device} has no device"))?;
    let channel = doc
        .console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(device))
        .ok_or_else(|| format!("{} has no console channel to freeze through", dev.name))?
        .channel
        .clone();
    let cabled = doc
        .cables
        .iter()
        .any(|c| &c.from.device == device && c.to.device == channel && c.to.socket == "in_l");
    if !cabled {
        return Err(format!("{} is not cabled to its channel", dev.name));
    }
    let effects = insert_chain(doc, registry, &channel)
        .into_iter()
        .take(INSERT_SLOTS)
        .collect();
    Ok(TrackChain { channel, effects })
}

/// Whether the flat device `flat` is the top-level device `key` or one of the devices inside it
/// (a container's inner devices are keyed by their path under the container's key).
pub fn within(key: &DeviceKey, flat: &DeviceKey) -> bool {
    flat == key
        || flat
            .0
            .strip_prefix(key.0.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The outputs of the chain through `effects` in the flat rack `x`: the last effect's
/// `out_l`/`out_r` (for a container effect, the inner outputs that feed them), or, with no
/// effect, the sockets of `device` (or of a device inside it, when it is a container) that feed a
/// console channel's `in_l` and `in_r` (both the same one for a mono feed). `None` when `device`
/// feeds no channel.
pub fn freeze_tap(
    x: &ExpandedRack,
    device: &DeviceKey,
    effects: &[DeviceKey],
) -> Option<[SocketRef; 2]> {
    if let Some(last) = effects.last() {
        let out = |socket: &str| {
            let own = SocketRef {
                device: last.clone(),
                socket: socket.to_owned(),
            };
            match x.exits.get(&own) {
                Some(inner) => Some(inner.clone()),
                // A container with nothing feeding its output has nothing to tap.
                None if x.alias.contains_key(last) => None,
                None => Some(own),
            }
        };
        return Some([out("out_l")?, out("out_r")?]);
    }
    let channels: BTreeSet<&DeviceKey> = x
        .devices
        .iter()
        .filter(|d| d.type_id == CHANNEL_TYPE)
        .map(|d| &d.key)
        .collect();
    let feed = |input: &str| {
        x.cables
            .iter()
            .find(|c| {
                within(device, &c.from.device)
                    && channels.contains(&c.to.device)
                    && c.to.socket == input
            })
            .map(|c| c.from.clone())
    };
    match (feed("in_l"), feed("in_r")) {
        (Some(l), Some(r)) => Some([l, r]),
        (Some(m), None) | (None, Some(m)) => Some([m.clone(), m]),
        (None, None) => None,
    }
}

/// The tap of a freeze of `device` through `effects`: the output of the console `strip` channel
/// when the strip is baked in (see [`Frozen::strip`]), else [`freeze_tap`].
pub fn frozen_tap(
    x: &ExpandedRack,
    device: &DeviceKey,
    effects: &[DeviceKey],
    strip: Option<&DeviceKey>,
) -> Option<[SocketRef; 2]> {
    match strip {
        Some(channel) => {
            let out = |socket: &str| SocketRef {
                device: channel.clone(),
                socket: socket.to_owned(),
            };
            x.devices
                .iter()
                .any(|d| &d.key == channel)
                .then(|| [out("out_l"), out("out_r")])
        }
        None => freeze_tap(x, device, effects),
    }
}

/// Why the channel strip of the track on `device` cannot be baked into a render: a send that is
/// on into a return slot that holds an effect, or a parallel channel fed from it, would go silent (their signal leaves the strip, which is
/// not built while it is frozen). `None` when the strip can be baked.
pub fn strip_blocker(doc: &RackDoc, registry: &Registry, device: &DeviceKey) -> Option<String> {
    let channel = doc
        .console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(device))?;
    let dev = doc.device(&channel.channel)?;
    let desc = registry.get(&dev.type_id)?.descriptor;
    let value = |key: &str| {
        desc.param_by_key(key)
            .map(|p| p.clamp(dev.params.get(key).copied().unwrap_or(p.default)))
    };
    let name = doc
        .device(device)
        .map_or(device.0.as_str(), |d| d.name.as_str());
    for k in 1..=crate::console::FX_SLOTS {
        let on = value(&format!("send{k}.on")).is_some_and(|v| v >= 0.5);
        // The bottom of the level range is off.
        let audible = desc
            .param_by_key(&format!("send{k}.level"))
            .zip(value(&format!("send{k}.level")))
            .is_some_and(|(p, v)| v > p.min);
        // A send into a return slot with no effect plays nothing.
        let returned = doc.console.returns[k - 1].is_some();
        if on && audible && returned {
            return Some(format!(
                "{name} has a send on: switch it off, or freeze without the strip"
            ));
        }
    }
    doc.console
        .parallels
        .iter()
        .any(|(parent, _)| parent == &channel.channel)
        .then(|| format!("{name} feeds a parallel channel: freeze without the strip"))
}

/// The span `[start, end)` in ticks of everything on a track: the clips of every lane (muted
/// ones too) and, with note clips, nothing beyond the clip. `None` for a track without clips.
pub fn clips_span(track: &Track) -> Option<(i64, i64)> {
    let clips = track.lanes.iter().flat_map(|l| &l.clips);
    let start = clips.clone().map(|c| c.start).min()?;
    let end = clips.map(Clip::end).max()?;
    Some((start, end))
}

/// The devices of the frozen tracks that are not built: each frozen instrument and its frozen
/// effects, and the channel of a track whose strip is baked in, as top-level keys (a container
/// stands for every device inside it).
pub fn frozen_devices(doc: &RackDoc) -> BTreeSet<DeviceKey> {
    let mut out = BTreeSet::new();
    for t in &doc.song.tracks {
        if let Some(f) = &t.frozen {
            out.insert(t.device.clone());
            out.extend(f.effects.iter().cloned());
            if f.strip {
                out.extend(strip_channel(doc, &t.device));
            }
        }
    }
    out
}

/// The console channel the track on `device` plays through.
fn strip_channel(doc: &RackDoc, device: &DeviceKey) -> Option<DeviceKey> {
    doc.console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(device))
        .map(|c| c.channel.clone())
}

/// Applies the frozen tracks of `doc` to its expansion `x` (see the module docs): flags the
/// frozen devices (and the devices inside a frozen container), drops their cables, re-sources what
/// the last one fed to the track's `~freeze` player and appends the players. A document without a
/// frozen track is untouched.
pub(crate) fn apply(doc: &RackDoc, x: &mut ExpandedRack) {
    let frozen: Vec<(&Track, &Frozen)> = doc
        .song
        .tracks
        .iter()
        .filter_map(|t| t.frozen.as_ref().map(|f| (t, f)))
        .collect();
    if frozen.is_empty() {
        return;
    }
    // (the top-level devices the render replaces, its tap, the player)
    type Plan = (Vec<DeviceKey>, Option<[SocketRef; 2]>, DeviceKey);
    let mut plans: Vec<Plan> = Vec::new();
    for (t, f) in &frozen {
        let mut keys = vec![t.device.clone()];
        keys.extend(f.effects.iter().cloned());
        let strip = f.strip.then(|| strip_channel(doc, &t.device)).flatten();
        keys.extend(strip.clone());
        plans.push((
            keys,
            frozen_tap(x, &t.device, &f.effects, strip.as_ref()),
            freeze_key(&t.device),
        ));
    }
    let omitted = |flat: &DeviceKey| plans.iter().any(|p| p.0.iter().any(|k| within(k, flat)));
    let mut cables: Vec<Cable> = Vec::with_capacity(x.cables.len());
    for mut c in std::mem::take(&mut x.cables) {
        if omitted(&c.to.device) {
            continue;
        }
        if omitted(&c.from.device) {
            let found = plans.iter().find_map(|(_, tap, player)| {
                let [l, r] = tap.as_ref()?;
                if c.from == *l {
                    Some((player, "out_l"))
                } else if c.from == *r {
                    Some((player, "out_r"))
                } else {
                    None
                }
            });
            let Some((player, socket)) = found else {
                continue;
            };
            c.from = SocketRef {
                device: player.clone(),
                socket: socket.to_owned(),
            };
        }
        cables.push(c);
    }
    cables.sort();
    x.cables = cables;
    for d in &mut x.devices {
        d.frozen = omitted(&d.key);
    }
    for (_, _, player) in plans {
        x.devices.push(ExpandedDevice {
            key: player,
            type_id: AUDIO_TRACK_TYPE.to_owned(),
            // Monitor off: the player has no input and plays the render only.
            params: BTreeMap::from([("monitor".to_owned(), 0.0)]),
            zones: Vec::new(),
            steps: None,
            curves: None,
            drums: None,
            plugin: None,
            frozen: false,
        });
    }
}

// ------------------------------------------------------------------------------ the guard

fn frozen_error(doc: &RackDoc, device: &DeviceKey) -> ModelError {
    let name = doc
        .device(device)
        .map_or(device.0.as_str(), |d| d.name.as_str());
    ModelError::Frozen(format!(
        "\"{name}\" is frozen: unfreeze the track to edit it"
    ))
}

/// Refuses a command that edits a frozen track (its clips, notes and automation), or a device a
/// frozen track includes (its parameters, cables, removal), or the frozen part of an insert
/// chain, or the tempo while any track is frozen (the render is tied to the tempo map).
///
/// Commands that only look at a frozen track (selection, colour, mute, a one-step move in the
/// list) pass; moving tracks to a place ([`SongCommand::MoveTracks`]) refuses a frozen one.
/// Undo and redo never come here: they restore states that were consistent.
pub(crate) fn guard(doc: &RackDoc, command: &Command) -> Result<(), ModelError> {
    if !doc.song.tracks.iter().any(|t| t.frozen.is_some()) {
        return Ok(());
    }
    let track_frozen = |device: &DeviceKey| {
        doc.song
            .track(device)
            .is_some_and(|t| t.frozen.is_some())
            .then(|| frozen_error(doc, device))
    };
    let clip_frozen = |id: &ClipId| {
        doc.song
            .clip_or_take(*id)
            .and_then(|(t, _)| track_frozen(&t.device))
    };
    let omitted = frozen_devices(doc);
    // A device inside a frozen container is as frozen as the container.
    let device_frozen = |device: &DeviceKey| {
        omitted
            .iter()
            .any(|o| within(o, device))
            .then(|| frozen_error(doc, device))
    };
    let tempo = || {
        ModelError::Frozen(
            "a track is frozen: unfreeze it to change the tempo or the meter".to_owned(),
        )
    };
    // The track a console channel plays, when it is frozen, with how many effects it froze.
    let frozen_channel = |channel: &DeviceKey| {
        doc.console
            .channels
            .iter()
            .find(|c| &c.channel == channel)
            .and_then(|c| c.source.as_ref())
            .and_then(|s| doc.song.track(s))
            .and_then(|t| {
                // With the strip baked in, no insert slot is free to change.
                t.frozen.as_ref().map(|f| {
                    let held = if f.strip { usize::MAX } else { f.effects.len() };
                    (t, held)
                })
            })
    };
    let channel_error =
        |channel: &DeviceKey| frozen_channel(channel).map(|(t, _)| frozen_error(doc, &t.device));
    let refused =
        match command {
            Command::RemoveDevice { key } => {
                // The instrument goes with its track; a frozen effect has to be unfrozen first.
                let is_instrument = doc.song.track(key).is_some_and(|t| t.frozen.is_some());
                if is_instrument {
                    None
                } else {
                    // Nor can the channel the freeze plays through go.
                    device_frozen(key).or_else(|| channel_error(key))
                }
            }
            Command::SetParam { key, .. } => device_frozen(key),
            Command::SetZones { device, .. }
            | Command::SetPattern { device, .. }
            | Command::SetCurves { device, .. }
            | Command::SetDrumStep { device, .. }
            | Command::SetDrumStepFx { device, .. }
            | Command::SetDrumLane { device, .. }
            | Command::SetDrumPattern { device, .. }
            | Command::AddDrumPattern { device }
            | Command::RemoveDrumPattern { device, .. }
            | Command::DuplicateDrumPattern { device, .. }
            | Command::AddPattern { device }
            | Command::RemovePattern { device, .. }
            | Command::DuplicatePattern { device, .. } => device_frozen(device),
            Command::Connect(c) => {
                device_frozen(&c.from.device).or_else(|| device_frozen(&c.to.device))
            }
            Command::Disconnect { to } => device_frozen(&to.device).or_else(|| {
                doc.cable_into(to)
                    .and_then(|c| device_frozen(&c.from.device))
            }),
            Command::SetTempo(_) => Some(tempo()),
            Command::Song(s) => match s {
                SongCommand::AddClip { device, .. }
                | SongCommand::AddAudioClip { device, .. }
                | SongCommand::NewTakeGroup { device, .. }
                | SongCommand::GroupAsTakes { device, .. }
                | SongCommand::AddAutomationLane { device, .. }
                | SongCommand::AddNoteLane { device, .. }
                | SongCommand::RemoveNoteLane { device, .. }
                | SongCommand::RemoveAutomationLane { device, .. }
                | SongCommand::SetLaneStatic { device, .. }
                | SongCommand::RenameLane { device, .. }
                | SongCommand::SetLaneMuted { device, .. }
                | SongCommand::SetLaneSolo { device, .. }
                | SongCommand::SetTrackControllers { device, .. }
                | SongCommand::SetLaneColor { device, .. } => track_frozen(device),
                SongCommand::MoveTracks { devices, .. }
                | SongCommand::MergeNoteLanes { devices } => devices.iter().find_map(track_frozen),
                SongCommand::MoveNoteLane {
                    device, to_device, ..
                } => track_frozen(device).or_else(|| track_frozen(to_device)),
                SongCommand::SetLinkedComp { groups, .. }
                | SongCommand::LinkTakeGroups { groups } => groups.iter().find_map(|g| {
                    doc.song
                        .take_group(*g)
                        .and_then(|(t, _, _)| track_frozen(&t.device))
                }),
                SongCommand::AddTake { group, .. }
                | SongCommand::DuplicateTake { group, .. }
                | SongCommand::MoveTake { group, .. }
                | SongCommand::DeleteUnusedTakes { group }
                | SongCommand::SetCompXfade { group, .. }
                | SongCommand::UnlinkTakeGroup { group }
                | SongCommand::SetCompSilence { group, .. }
                | SongCommand::SetTake { group, .. }
                | SongCommand::SetTakeLoop { group, .. } => doc
                    .song
                    .take_group(*group)
                    .and_then(|(t, _, _)| track_frozen(&t.device)),
                SongCommand::RemoveClip { id }
                | SongCommand::SetClipGeometry { id, .. }
                | SongCommand::SetClipContent { id, .. }
                | SongCommand::SplitClip { id, .. }
                | SongCommand::DuplicateClip { id, .. }
                | SongCommand::SetAudioClip { id, .. }
                | SongCommand::SetWarpMarkers { id, .. }
                | SongCommand::SetPitchEdits { id, .. }
                | SongCommand::StripSilence { id, .. } => clip_frozen(id),
                SongCommand::SetClipGrid { clip, .. }
                | SongCommand::SetClipControllers { clip, .. }
                | SongCommand::SetClipMidi { clip, .. }
                | SongCommand::SetClipParamLanes { clip, .. }
                | SongCommand::SetClipScale { clip, .. }
                | SongCommand::QuantizeNotes { clip, .. } => clip_frozen(clip),
                SongCommand::ApplyQuantize { clips, .. }
                | SongCommand::SetClipGroove { clips, .. }
                | SongCommand::ApplyNoteTool { clips, .. } => clips.iter().find_map(clip_frozen),
                SongCommand::SetClipMeta { ids, .. } => ids.iter().find_map(clip_frozen),
                SongCommand::SetTempoMap(_)
                | SongCommand::DragTempoMap { .. }
                | SongCommand::SetMeter { .. }
                | SongCommand::SetMeterMap(_)
                | SongCommand::DragMeterMap { .. } => Some(tempo()),
                _ => None,
            },
            Command::Workflow(w) => match w {
                WorkflowCommand::RemoveClips { ids }
                | WorkflowCommand::DuplicateClips { ids }
                | WorkflowCommand::SetClipsMuted { ids, .. }
                | WorkflowCommand::JoinClips { ids }
                | WorkflowCommand::RenderJoin { ids, .. }
                | WorkflowCommand::ConvertLoop { ids, .. }
                | WorkflowCommand::TrimClips { ids, .. }
                | WorkflowCommand::ExtendClips { ids, .. }
                | WorkflowCommand::DeleteOutsideEvents { ids }
                | WorkflowCommand::SplitClips { ids, .. }
                | WorkflowCommand::ReverseClips { ids, .. }
                | WorkflowCommand::ScaleTempo { ids, .. }
                | WorkflowCommand::StretchClips { ids, .. }
                | WorkflowCommand::ChopClips { ids, .. }
                | WorkflowCommand::CleanupAutomation { ids, .. }
                | WorkflowCommand::CutOut { ids, .. } => ids.iter().find_map(clip_frozen),
                WorkflowCommand::DuplicateLoopInstance { id, .. }
                | WorkflowCommand::RenderLoop { id, .. } => clip_frozen(id),
                WorkflowCommand::MoveCrossfade { later, .. }
                | WorkflowCommand::ResizeCrossfade { later, .. } => clip_frozen(later),
                WorkflowCommand::AutomationIntoClip { automation, into } => {
                    clip_frozen(automation).or_else(|| clip_frozen(into))
                }
                WorkflowCommand::ExtractNotes { clip, .. }
                | WorkflowCommand::ExplodeNotes { clip } => clip_frozen(clip),
                WorkflowCommand::MoveClips { moves, .. } => moves
                    .iter()
                    .find_map(|m| clip_frozen(&m.id).or_else(|| track_frozen(&m.track))),
                WorkflowCommand::PasteClips { track, .. } => track.as_ref().and_then(track_frozen),
                WorkflowCommand::PasteTakes { target, .. } => match target {
                    crate::TakeTarget::Group(group) => doc
                        .song
                        .take_group(*group)
                        .and_then(|(t, _, _)| track_frozen(&t.device)),
                    crate::TakeTarget::Lane { device, .. } => track_frozen(device),
                },
                WorkflowCommand::LayerRecording { clip, .. } => clip_frozen(clip),
                WorkflowCommand::EngulfRecording { device, .. } => track_frozen(device),
                WorkflowCommand::PasteNotes { clip, .. }
                | WorkflowCommand::DuplicateNotes { clip, .. }
                | WorkflowCommand::PasteAutomationPoints { clip, .. }
                | WorkflowCommand::DuplicatePoints { clip, .. } => clip_frozen(clip),
                WorkflowCommand::DuplicateTrack { device } => track_frozen(device),
                WorkflowCommand::DuplicateTracks { devices, .. } => {
                    devices.iter().find_map(track_frozen)
                }
                WorkflowCommand::ApplyPatch { device, .. }
                | WorkflowCommand::ResetDevice { device } => device_frozen(device),
                WorkflowCommand::ReplaceDevice { target, .. } => device_frozen(target),
                WorkflowCommand::SplitSub { device, .. } => {
                    device_frozen(device).or_else(|| track_frozen(device))
                }
                WorkflowCommand::AddReverbSend { track, .. }
                | WorkflowCommand::DuckFrom { target: track, .. }
                | WorkflowCommand::GentlePump { target: track }
                | WorkflowCommand::RemoveDucking { target: track }
                | WorkflowCommand::AddRumble { kick: track, .. } => {
                    device_frozen(track).or_else(|| track_frozen(track))
                }
                WorkflowCommand::AddTransition {
                    edit: crate::transition::TransitionEdit::Fill { track },
                    ..
                } => device_frozen(track).or_else(|| track_frozen(track)),
                WorkflowCommand::RemoveDevices { keys }
                | WorkflowCommand::DeleteDevices { keys } => keys.iter().find_map(|k| {
                    if doc.song.track(k).is_some_and(|t| t.frozen.is_some()) {
                        None
                    } else {
                        device_frozen(k).or_else(|| channel_error(k))
                    }
                }),
                WorkflowCommand::CreatePlayer {
                    target: CreateTarget::Selected(key),
                    ..
                }
                | WorkflowCommand::DisconnectDevice { key } => device_frozen(key),
                WorkflowCommand::DisconnectDevices { keys } => keys.iter().find_map(device_frozen),
                _ => None,
            },
            // A frozen track's devices (and the track itself) stay where they are.
            Command::Container(c) => match c {
                ContainerCommand::AddInner { container, .. }
                | ContainerCommand::SetSplits { container, .. }
                | ContainerCommand::SetMacro { container, .. }
                | ContainerCommand::RemoveMacroTarget { container, .. }
                | ContainerCommand::SetMacroKind { container, .. } => device_frozen(container),
                ContainerCommand::Combine { keys, .. } => keys
                    .iter()
                    .find_map(|k| device_frozen(k).or_else(|| track_frozen(k))),
                ContainerCommand::MoveIntoContainer { key, container, .. } => device_frozen(key)
                    .or_else(|| track_frozen(key))
                    .or_else(|| device_frozen(container)),
                ContainerCommand::Uncombine { container } => {
                    device_frozen(container).or_else(|| track_frozen(container))
                }
                ContainerCommand::MoveOutOfContainer { key, .. } => {
                    crate::container::split_path(key).0.and_then(|container| {
                        device_frozen(&container).or_else(|| track_frozen(&container))
                    })
                }
                _ => None,
            },
            Command::Console(c) => match c {
                ConsoleCommand::RemoveChannel { channel } => channel_error(channel),
                ConsoleCommand::SetInsert {
                    channel,
                    slot,
                    effect,
                } => frozen_channel(channel)
                    .filter(|(_, frozen)| *slot < *frozen)
                    .map(|(t, _)| frozen_error(doc, &t.device))
                    .or_else(|| match effect {
                        // A frozen effect cannot be put on another insert loop.
                        Some(InsertEffect::Existing(key)) => device_frozen(key),
                        _ => None,
                    }),
                // The inserts a freeze rendered stay as they are.
                ConsoleCommand::ClearInserts { strip }
                | ConsoleCommand::ResetStrip { strip, .. } => frozen_channel(strip)
                    .filter(|(_, frozen)| *frozen > 0)
                    .map(|(t, _)| frozen_error(doc, &t.device)),
                ConsoleCommand::PasteSettings { strips, clip }
                    if clip.sections.covers_inserts() =>
                {
                    strips
                        .iter()
                        .filter_map(&frozen_channel)
                        .find(|(_, frozen)| *frozen > 0)
                        .map(|(t, _)| frozen_error(doc, &t.device))
                }
                ConsoleCommand::AddInsert {
                    channel,
                    slot,
                    effect,
                } => frozen_channel(channel)
                    .filter(|(_, frozen)| *slot < *frozen)
                    .map(|(t, _)| frozen_error(doc, &t.device))
                    .or_else(|| match effect {
                        InsertEffect::Existing(key) => device_frozen(key),
                        InsertEffect::New { .. } => None,
                    }),
                ConsoleCommand::ExtractInsert { channel, slot, .. } => frozen_channel(channel)
                    .filter(|(_, frozen)| *slot < *frozen)
                    .map(|(t, _)| frozen_error(doc, &t.device)),
                ConsoleCommand::MoveInsert {
                    channel, from, to, ..
                } => frozen_channel(channel)
                    .filter(|(_, frozen)| (*from).min(*to) < *frozen)
                    .map(|(t, _)| frozen_error(doc, &t.device)),
                _ => None,
            },
            _ => None,
        };
    refused.map_or(Ok(()), Err)
}

// ------------------------------------------------------------------------------ the commands

/// Plans `FreezeTrack`.
pub(crate) fn plan_freeze(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    frozen: Frozen,
) -> Result<Edit, ModelError> {
    let index = track_index(doc, device)?;
    let track = &doc.song.tracks[index];
    if track.is_automation_only() {
        return Err(bad(format!(
            "{device} is an automation track: there is nothing to freeze"
        )));
    }
    if track.frozen.is_some() {
        return Err(bad(format!("{device} is already frozen")));
    }
    check_frozen(doc, registry, device, &frozen)?;
    Ok(Edit::Frozen {
        device: device.clone(),
        old: None,
        new: Some(frozen),
    })
}

/// Plans `RefreezeTrack`: the frozen render of `device` is replaced by `frozen` in one step (the
/// track stays frozen, so its devices keep their places in the engine).
pub(crate) fn plan_refreeze(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    frozen: Frozen,
) -> Result<Edit, ModelError> {
    let index = track_index(doc, device)?;
    let old = doc.song.tracks[index]
        .frozen
        .clone()
        .ok_or_else(|| bad(format!("{device} is not frozen")))?;
    check_frozen(doc, registry, device, &frozen)?;
    Ok(Edit::Frozen {
        device: device.clone(),
        old: Some(old),
        new: Some(frozen),
    })
}

/// What a render must satisfy to freeze `device`: its effects are the first of the channel's
/// insert chain, its place is valid and its audio is plain audio of the pool.
fn check_frozen(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    frozen: &Frozen,
) -> Result<(), ModelError> {
    let chain = track_chain(doc, registry, device).map_err(bad)?;
    let n = frozen.effects.len();
    if n > chain.effects.len() || chain.effects[..n] != frozen.effects[..] {
        return Err(bad(format!(
            "the frozen effects must be the first effects of the insert chain of {}",
            chain.channel
        )));
    }
    if frozen.strip && n != chain.effects.len() {
        return Err(bad(format!(
            "a render with the strip of {} baked in includes the whole insert chain",
            chain.channel
        )));
    }
    check_geometry(frozen.start, frozen.length, 0, None).map_err(range)?;
    check_audio(doc, &frozen.audio)?;
    if frozen.audio.reverse || frozen.audio.warp.active() {
        return Err(bad("a frozen render is plain audio: no reverse, no warp"));
    }
    Ok(())
}

/// Plans `UnfreezeTrack` (without keeping the audio; see `SongCommand::UnfreezeTrack`).
pub(crate) fn plan_unfreeze(doc: &RackDoc, device: &DeviceKey) -> Result<Edit, ModelError> {
    let index = track_index(doc, device)?;
    let old = doc.song.tracks[index]
        .frozen
        .clone()
        .ok_or_else(|| bad(format!("{device} is not frozen")))?;
    Ok(Edit::Frozen {
        device: device.clone(),
        old: Some(old),
        new: None,
    })
}

/// Plans a new audio track (right after the track of `after`, else last) with one clip of
/// rendered audio `(start, length, audio)`, and the
/// originals in `mute` muted: the new track's edits, the clip's, then one per muted clip.
/// Returns the edit and the new clip's id.
pub(crate) fn plan_rendered_track(
    doc: &RackDoc,
    registry: &Registry,
    name: Option<String>,
    after: Option<&DeviceKey>,
    (start, length, audio): (i64, i64, AudioClipContent),
    mute: &[ClipId],
) -> Result<(Edit, ClipId), ModelError> {
    check_geometry(start, length, 0, None).map_err(range)?;
    check_audio(doc, &audio)?;
    if audio.reverse || audio.warp.active() {
        return Err(bad("a rendered clip is plain audio: no reverse, no warp"));
    }
    let track = add_audio_track(doc, registry, name, after)?;
    let mut scratch = doc.clone();
    track.perform(&mut scratch)?;
    let device = scratch
        .song
        .tracks
        .iter()
        .find(|t| doc.song.track(&t.device).is_none())
        .map(|t| t.device.clone())
        .ok_or_else(|| bad("the new audio track is missing"))?;
    // Next to its source the new track is in the same folder and shows the same colour.
    let join = after.and_then(|source| crate::organise_cmd::join_track(&scratch, &device, source));
    let lane = audio_lane(&scratch, &device)?;
    let clip = Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        content: ClipContent::Audio(audio),
    };
    let (insert, id) = insert_new(&scratch, device, lane, clip)?;
    let mut edits = vec![track];
    edits.extend(join);
    edits.push(insert);
    for &original in mute {
        let (device, lane, c) = find_clip(doc, original)?;
        if !c.muted {
            edits.push(Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip {
                    muted: true,
                    ..c.clone()
                }),
            });
        }
    }
    Ok((Edit::Compound(edits), id))
}

/// The length in ticks of a clip that plays `frames` frames starting at `start_tick`: the
/// smallest whole number of ticks whose span, through the song's tempo map
/// ([`crate::song::Song::frame_at`]), is at least `frames` (audio is not stretched). At least 1,
/// at most [`crate::song::MAX_TICK`]. A render of that many frames fits its clip.
pub fn ticks_for_frames(
    song: &crate::song::Song,
    start_tick: i64,
    frames: u64,
    sample_rate: u32,
) -> i64 {
    use crate::song::MAX_TICK;
    let clock = song.frame_clock(sample_rate);
    let base = clock.at(start_tick);
    let covers = |len: i64| clock.at(start_tick + len) - base >= frames;
    // Grow until it covers, then bisect: `frame_at` never decreases with the tick.
    let (mut lo, mut hi) = (0_i64, 1_i64);
    while !covers(hi) {
        if hi >= MAX_TICK {
            return MAX_TICK;
        }
        lo = hi;
        hi = (hi * 2).min(MAX_TICK);
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if covers(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}
