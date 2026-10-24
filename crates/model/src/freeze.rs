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
//! effects after it stay live, processing the frozen audio. The channel strip itself (fader, pan,
//! width, sends, mute, solo, output) is never frozen, and neither are send effects: both keep
//! working on the frozen audio. What the strip does in front of its insert point (input gain,
//! filters, and dynamics and EQ unless they follow the insert) is part of the sound at the tap, so
//! it is frozen with an insert chain of one or more effects.
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
    AUDIO_TRACK_TYPE, AudioClipContent, Clip, ClipContent, ClipId, Lane, LaneKind, SongCommand,
    Track,
};
use crate::song_cmd::{
    add_audio_track, audio_lane, bad, check_audio, check_geometry, find_clip, insert_new, range,
    track_index,
};
use crate::workflow_cmd::WorkflowCommand;
use crate::{
    Cable, Command, ConsoleCommand, DeviceKey, InsertEffect, ModelError, RackDoc, SocketRef,
    insert_chain,
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
            controllers: Vec::new(),
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
            frozen: None,
            lanes: vec![Lane {
                kind: LaneKind::Audio,
                clips: vec![frozen.clip()],
                take_groups: Vec::new(),
            }],
        })
    }
}

/// What a track's chain is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackChain {
    /// The console channel the track plays through.
    pub channel: DeviceKey,
    /// The effects of the channel's insert loop a freeze can include, in signal order (a
    /// container effect ends the list: a container is not frozen).
    pub effects: Vec<DeviceKey>,
}

/// The chain of the track on `device`.
///
/// # Errors
/// The reason a track cannot be frozen: its device is a container or has no console channel, or
/// is not cabled to it.
pub fn track_chain(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
) -> Result<TrackChain, String> {
    let dev = doc
        .device(device)
        .ok_or_else(|| format!("{device} has no device"))?;
    if dev.inner.is_some() {
        return Err(format!("{} is a container: it cannot be frozen", dev.name));
    }
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
        .take_while(|e| doc.device(e).is_some_and(|d| d.inner.is_none()))
        .take(INSERT_SLOTS)
        .collect();
    Ok(TrackChain { channel, effects })
}

/// The outputs of the chain through `effects` in the flat rack `x`: the last effect's
/// `out_l`/`out_r`, or, with no effect, the sockets of `device` that feed a console channel's
/// `in_l` and `in_r` (both the same one for a mono feed). `None` when `device` feeds no channel.
pub fn freeze_tap(
    x: &ExpandedRack,
    device: &DeviceKey,
    effects: &[DeviceKey],
) -> Option<[SocketRef; 2]> {
    if let Some(last) = effects.last() {
        let out = |socket: &str| SocketRef {
            device: last.clone(),
            socket: socket.to_owned(),
        };
        return Some([out("out_l"), out("out_r")]);
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
                &c.from.device == device && channels.contains(&c.to.device) && c.to.socket == input
            })
            .map(|c| c.from.clone())
    };
    match (feed("in_l"), feed("in_r")) {
        (Some(l), Some(r)) => Some([l, r]),
        (Some(m), None) | (None, Some(m)) => Some([m.clone(), m]),
        (None, None) => None,
    }
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
/// effects, as flat keys (they are all top-level devices).
pub fn frozen_devices(doc: &RackDoc) -> BTreeSet<DeviceKey> {
    let mut out = BTreeSet::new();
    for t in &doc.song.tracks {
        if let Some(f) = &t.frozen {
            out.insert(t.device.clone());
            out.extend(f.effects.iter().cloned());
        }
    }
    out
}

/// Applies the frozen tracks of `doc` to its expansion `x` (see the module docs): flags the
/// frozen devices, drops their cables, re-sources what the last one fed to the track's `~freeze`
/// player and appends the players. A document without a frozen track is untouched.
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
    // (last frozen device, its tap, the player)
    let mut plans: Vec<(DeviceKey, Option<[SocketRef; 2]>, DeviceKey)> = Vec::new();
    let mut omitted: BTreeSet<DeviceKey> = BTreeSet::new();
    for (t, f) in &frozen {
        omitted.insert(t.device.clone());
        omitted.extend(f.effects.iter().cloned());
        let last = f.effects.last().unwrap_or(&t.device).clone();
        plans.push((
            last,
            freeze_tap(x, &t.device, &f.effects),
            freeze_key(&t.device),
        ));
    }
    let mut cables: Vec<Cable> = Vec::with_capacity(x.cables.len());
    for mut c in std::mem::take(&mut x.cables) {
        if omitted.contains(&c.to.device) {
            continue;
        }
        if omitted.contains(&c.from.device) {
            let Some((_, Some([l, r]), player)) = plans.iter().find(|p| p.0 == c.from.device)
            else {
                continue;
            };
            let socket = if c.from.socket == l.socket {
                "out_l"
            } else if c.from.socket == r.socket {
                "out_r"
            } else {
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
        d.frozen = omitted.contains(&d.key);
    }
    for (_, _, player) in plans {
        x.devices.push(ExpandedDevice {
            key: player,
            type_id: AUDIO_TRACK_TYPE.to_owned(),
            // Monitor off: the player has no input and plays the render only.
            params: BTreeMap::from([("monitor".to_owned(), 0.0)]),
            zones: Vec::new(),
            steps: None,
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
/// Commands that only look at a frozen track (selection, colour, mute, a move in the list) pass.
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
            .clip(*id)
            .and_then(|(t, _, _)| track_frozen(&t.device))
    };
    let omitted = frozen_devices(doc);
    let device_frozen =
        |device: &DeviceKey| omitted.contains(device).then(|| frozen_error(doc, device));
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
            .and_then(|t| t.frozen.as_ref().map(|f| (t, f.effects.len())))
    };
    let channel_error =
        |channel: &DeviceKey| frozen_channel(channel).map(|(t, _)| frozen_error(doc, &t.device));
    let refused = match command {
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
            | SongCommand::AddAutomationLane { device, .. } => track_frozen(device),
            SongCommand::RemoveClip { id }
            | SongCommand::SetClipGeometry { id, .. }
            | SongCommand::SetClipContent { id, .. }
            | SongCommand::SplitClip { id, .. }
            | SongCommand::DuplicateClip { id, .. }
            | SongCommand::SetAudioClip { id, .. }
            | SongCommand::StripSilence { id, .. } => clip_frozen(id),
            SongCommand::SetClipGrid { clip, .. }
            | SongCommand::SetClipControllers { clip, .. }
            | SongCommand::SetClipScale { clip, .. }
            | SongCommand::QuantizeNotes { clip, .. } => clip_frozen(clip),
            SongCommand::ApplyQuantize { clips, .. } => clips.iter().find_map(clip_frozen),
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
            | WorkflowCommand::JoinClips { ids } => ids.iter().find_map(clip_frozen),
            WorkflowCommand::MoveClips { moves, .. } => moves
                .iter()
                .find_map(|m| clip_frozen(&m.id).or_else(|| track_frozen(&m.track))),
            WorkflowCommand::PasteClips { track, .. } => track.as_ref().and_then(track_frozen),
            WorkflowCommand::PasteNotes { clip, .. }
            | WorkflowCommand::DuplicateNotes { clip, .. }
            | WorkflowCommand::PasteAutomationPoints { clip, .. }
            | WorkflowCommand::DuplicatePoints { clip, .. } => clip_frozen(clip),
            WorkflowCommand::DuplicateTrack { device } => track_frozen(device),
            WorkflowCommand::RemoveDevices { keys } | WorkflowCommand::DeleteDevices { keys } => {
                keys.iter().find_map(|k| {
                    if doc.song.track(k).is_some_and(|t| t.frozen.is_some()) {
                        None
                    } else {
                        device_frozen(k).or_else(|| channel_error(k))
                    }
                })
            }
            WorkflowCommand::DisconnectDevice { key } => device_frozen(key),
            WorkflowCommand::DisconnectDevices { keys } => keys.iter().find_map(device_frozen),
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
    if track.frozen.is_some() {
        return Err(bad(format!("{device} is already frozen")));
    }
    let chain = track_chain(doc, registry, device).map_err(bad)?;
    let n = frozen.effects.len();
    if n > chain.effects.len() || chain.effects[..n] != frozen.effects[..] {
        return Err(bad(format!(
            "the frozen effects must be the first effects of the insert chain of {}",
            chain.channel
        )));
    }
    check_geometry(frozen.start, frozen.length, 0, None).map_err(range)?;
    check_audio(doc, &frozen.audio)?;
    if frozen.audio.reverse || frozen.audio.warp.active() {
        return Err(bad("a frozen render is plain audio: no reverse, no warp"));
    }
    Ok(Edit::Frozen {
        device: device.clone(),
        old: None,
        new: Some(frozen),
    })
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

/// Plans a new audio track with one clip of rendered audio `(start, length, audio)`, and the
/// originals in `mute` muted: the new track's edits, the clip's, then one per muted clip.
/// Returns the edit and the new clip's id.
pub(crate) fn plan_rendered_track(
    doc: &RackDoc,
    registry: &Registry,
    name: Option<String>,
    (start, length, audio): (i64, i64, AudioClipContent),
    mute: &[ClipId],
) -> Result<(Edit, ClipId), ModelError> {
    check_geometry(start, length, 0, None).map_err(range)?;
    check_audio(doc, &audio)?;
    if audio.reverse || audio.warp.active() {
        return Err(bad("a rendered clip is plain audio: no reverse, no warp"));
    }
    let track = add_audio_track(doc, registry, name)?;
    let mut scratch = doc.clone();
    track.perform(&mut scratch);
    let device = scratch
        .song
        .tracks
        .last()
        .map(|t| t.device.clone())
        .ok_or_else(|| bad("the new audio track is missing"))?;
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
        controllers: Vec::new(),
        content: ClipContent::Audio(audio),
    };
    let (insert, id) = insert_new(&scratch, device, lane, clip)?;
    let mut edits = vec![track, insert];
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
    let base = song.frame_at(start_tick, sample_rate);
    let covers = |len: i64| song.frame_at(start_tick + len, sample_rate) - base >= frames;
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
