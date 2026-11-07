//! `WorkflowCommand::AddRumble`: the track a printed rumble layer plays on, in one undo step.
//!
//! The rumble itself is audio made offline from a rendered kick (see `atomik_dsp::rumble`) and
//! registered in the project's pool before this command runs. The command puts it on a new audio
//! track right after the kick's, and gives that track the two inserts that keep it under the kick:
//!
//! ```text
//! kick channel out ------------------------------> Duck key (sc_in_l/r)
//! rumble clip -> channel -> [Sidechain Tool, key from the kick] -> [Auto-Filter, low-pass]
//!                                                                -> [EQ, high-pass]
//! ```
//!
//! * **Duck.** The Sidechain Tool runs in `trigger` mode: a hit on the kick's channel output
//!   restarts the dip, so the rumble is down by [`RumbleTrackOptions::depth`] at every kick and
//!   comes back over [`RumbleTrackOptions::release_ms`]. The kick's channel output is cabled to
//!   the key sockets, so the duck follows the kick as it is mixed.
//! * **Low-pass.** An Auto-Filter with no sweep (depth, follow and CV all zero), a low-pass at
//!   [`RumbleTrackOptions::low_pass`], 24 dB per octave and no resonance, keeps the rumble in the
//!   sub band. (The EQ's low-pass starts at 1 kHz, too high for a rumble.)
//! * **High-pass.** The EQ's low cut ([`RumbleTrackOptions::high_pass`], 24 dB per octave) keeps
//!   what no speaker plays out of it.
//!
//! The helpers [`kick_key`] and [`kick_hits`] say which notes of a track are the kick: all of them
//! on a kick track, the notes on the kick pad on a drum machine.
//!
//! The track, its clip, its devices and the cable are one planned edit built on a scratch copy of
//! the document: a refusal changes nothing, and undo takes it all back in one step.

use crate::console_cmd::{self, Planner};
use crate::console_insert::{insert_chain, set_insert};
use crate::history::Edit;
use crate::reverb_send::{median_gap_of, set};
use crate::song::{AudioClipContent, ClipId, Song, Track, flatten_track_notes};
use crate::{DeviceKey, InsertEffect, ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// The device type of the duck.
pub const DUCK_TYPE: &str = "atomik.duck";
/// The device type of the low-pass.
pub const FILTER_TYPE: &str = "atomik.auto-filter";
/// The device type of the EQ.
pub const EQ_TYPE: &str = "atomik.eq";
/// The drum machine's type id: its kick pad is the kick.
pub const DRUM_MACHINE_TYPE: &str = "atomik.drum-machine";
/// The `padN.module` value of a synthesised kick.
const KICK_MODULE: f32 = 1.0;
/// The range of the low-pass the command accepts, in Hz.
pub const LOW_PASS_RANGE: (f32, f32) = (40.0, 400.0);
/// The range of the EQ's high-pass the command accepts, in Hz.
pub const HIGH_PASS_RANGE: (f32, f32) = (20.0, 60.0);
/// The range of the duck's depth the command accepts.
pub const DEPTH_RANGE: (f32, f32) = (0.1, 1.0);
/// The range of the duck's release the command accepts, in ms.
pub const RELEASE_RANGE: (f32, f32) = (20.0, 2_000.0);
/// The release when the kick has no rhythm to read, in ms.
pub const DEFAULT_RELEASE_MS: f32 = 200.0;
/// The range of the reverb rumble's decay the suggestion keeps, in seconds.
pub const DECAY_RANGE: (f32, f32) = (1.0, 6.0);
/// The decay of the reverb rumble when the kick has no rhythm to read, in seconds.
pub const DEFAULT_DECAY: f32 = 3.0;
/// The share of the gap between kick hits the duck's release takes.
const RELEASE_SHARE: f32 = 0.35;
/// The least distinct hits a rhythm is read from.
const MIN_HITS: usize = 2;

/// The choices of the rumble's track.
#[derive(Debug, Clone, PartialEq)]
pub struct RumbleTrackOptions {
    /// The corner of the low-pass, in Hz.
    pub low_pass: f32,
    /// The corner of the EQ's high-pass (its low cut), in Hz.
    pub high_pass: f32,
    /// How deep the duck goes at a kick hit, `0.1..=1`.
    pub depth: f32,
    /// How long the duck takes to come back, in ms.
    pub release_ms: f32,
}

impl Default for RumbleTrackOptions {
    fn default() -> Self {
        Self {
            low_pass: 160.0,
            high_pass: 28.0,
            depth: 0.9,
            release_ms: DEFAULT_RELEASE_MS,
        }
    }
}

/// The key of the notes that are the kick on `device`: `Some(key)` on a drum machine (the first
/// pad that plays a synthesised kick, else the first pad), `None` on any other instrument, whose
/// notes are all the kick.
pub fn kick_key(doc: &RackDoc, device: &DeviceKey) -> Option<u8> {
    let d = doc.device(device)?;
    if d.type_id != DRUM_MACHINE_TYPE {
        return None;
    }
    let module = |pad: u8| {
        d.params
            .get(&format!("pad{pad}.module"))
            .copied()
            .unwrap_or(0.0)
    };
    let pad = (1..=16_u8)
        .find(|&p| module(p).to_bits() == KICK_MODULE.to_bits())
        .unwrap_or(1);
    Some(36 + pad - 1)
}

/// The start ticks of the kick hits `track` plays, ascending and distinct: every played note, or
/// with `key` only the notes on that key.
pub fn kick_hits(song: &Song, track: &Track, key: Option<u8>) -> Vec<i64> {
    let mut ticks: Vec<i64> = flatten_track_notes(song, track)
        .into_iter()
        .filter(|&(_, _, k, _)| key.is_none_or(|want| want == k))
        .map(|(tick, ..)| tick)
        .collect();
    ticks.sort_unstable();
    ticks.dedup();
    ticks
}

/// The release the duck gets for kick hits at `ticks`: a third of the median gap between them,
/// kept within [`RELEASE_RANGE`]; [`DEFAULT_RELEASE_MS`] with fewer than two hits.
pub fn suggested_release(song: &Song, ticks: &[i64]) -> f32 {
    if ticks.len() < MIN_HITS {
        return DEFAULT_RELEASE_MS;
    }
    median_gap_of(song, ticks).map_or(DEFAULT_RELEASE_MS, |gap| {
        (gap * 1000.0 * RELEASE_SHARE).clamp(RELEASE_RANGE.0, RELEASE_RANGE.1)
    })
}

/// The decay of the reverb rumble for kick hits at `ticks`, in seconds: twice the median gap
/// between them (the tail has mostly rung out when the next hit comes), kept within
/// [`DECAY_RANGE`]; [`DEFAULT_DECAY`] with fewer than two hits.
pub fn suggested_decay(song: &Song, ticks: &[i64]) -> f32 {
    if ticks.len() < MIN_HITS {
        return DEFAULT_DECAY;
    }
    median_gap_of(song, ticks).map_or(DEFAULT_DECAY, |gap| {
        (2.0 * gap).clamp(DECAY_RANGE.0, DECAY_RANGE.1)
    })
}

fn refuse(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// Plans the rumble track after the track of `kick`: `place` is the clip `(start, length, audio)`
/// and `name` the track's name (default "<kick name> Rumble"). Returns the edit and the new clip's
/// id.
///
/// # Errors
/// The kick is inside a container or has no track or console channel, an option is outside its
/// range, or the clip, a device or a cable is refused.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    kick: &DeviceKey,
    place: (i64, i64, AudioClipContent),
    name: Option<String>,
    options: &RumbleTrackOptions,
) -> Result<(Edit, ClipId), ModelError> {
    if kick.0.contains('/') {
        return Err(refuse(format!(
            "{kick} is inside a container: a rumble works on a track of the rack"
        )));
    }
    if doc.song.track(kick).is_none() {
        return Err(refuse(format!("{kick} has no track")));
    }
    let kick_channel = doc
        .console
        .channel_of(kick)
        .ok_or_else(|| {
            refuse(format!(
                "{kick} has no console channel to key the duck from"
            ))
        })?
        .channel
        .clone();
    let in_range = |v: f32, (lo, hi): (f32, f32)| v.is_finite() && (lo..=hi).contains(&v);
    if !in_range(options.low_pass, LOW_PASS_RANGE)
        || !in_range(options.high_pass, HIGH_PASS_RANGE)
        || !in_range(options.depth, DEPTH_RANGE)
        || !in_range(options.release_ms, RELEASE_RANGE)
    {
        return Err(ModelError::OutOfRange(
            "a rumble option is outside its range".to_owned(),
        ));
    }
    let name = name.unwrap_or_else(|| {
        let source = doc
            .device(kick)
            .map_or_else(|| kick.0.clone(), |d| d.name.clone());
        format!("{source} Rumble")
    });
    let (track_edit, clip) =
        crate::freeze::plan_rendered_track(doc, registry, Some(name), Some(kick), place, &[])?;
    let mut p = Planner::new(doc, registry);
    p.push(track_edit);
    let device = p
        .doc()
        .song
        .tracks
        .iter()
        .find(|t| doc.song.track(&t.device).is_none())
        .map(|t| t.device.clone())
        .ok_or_else(|| refuse("the new audio track is missing"))?;
    let channel = p
        .doc()
        .console
        .channel_of(&device)
        .ok_or_else(|| refuse("the new audio track has no console channel"))?
        .channel
        .clone();
    for (slot, type_id) in [DUCK_TYPE, FILTER_TYPE, EQ_TYPE].into_iter().enumerate() {
        set_insert(
            &mut p,
            &channel,
            slot,
            Some(InsertEffect::New {
                type_id: type_id.to_owned(),
            }),
        )?;
    }
    let chain = insert_chain(p.doc(), registry, &channel);
    let [duck, filter, eq] = chain.as_slice() else {
        return Err(refuse("the rumble track's inserts could not be made"));
    };
    set(&mut p, duck, "mode", 1.0)?;
    set(&mut p, duck, "depth", options.depth)?;
    set(&mut p, duck, "attack", 1.0)?;
    set(&mut p, duck, "time", options.release_ms)?;
    for side in ["l", "r"] {
        p.connect(console_cmd::wire(
            console_cmd::at(&kick_channel, format!("out_{side}")),
            console_cmd::at(duck, format!("sc_in_{side}")),
        ))?;
    }
    // A fixed low-pass: nothing sweeps it.
    set(&mut p, filter, "mode", 0.0)?;
    set(&mut p, filter, "slope", 1.0)?;
    set(&mut p, filter, "cutoff", options.low_pass)?;
    set(&mut p, filter, "resonance", 0.0)?;
    set(&mut p, filter, "depth", 0.0)?;
    set(&mut p, filter, "follow", 0.0)?;
    set(&mut p, eq, "band1.on", 1.0)?;
    set(&mut p, eq, "band1.freq", options.high_pass)?;
    set(&mut p, eq, "lowcut.slope", 1.0)?;
    Ok((p.finish(), clip))
}
