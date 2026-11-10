//! Quick sidechain: `WorkflowCommand::DuckFrom`, `GentlePump` and `RemoveDucking`.
//!
//! **Duck from.** One step makes a track duck under another, a bass under a kick or a pad under a
//! vocal. The target track's console channel gets a Compressor at the end of its insert chain,
//! named "Duck from <source>", and the source's signal is cabled to the compressor's key:
//!
//! ```text
//! source channel out (or parallel_out for pre-fader) ---> Compressor key (sc_in_l/r)
//! target channel -> ... -> [Compressor "Duck from <source>"] -> return
//! ```
//!
//! * **Key.** The source's post-fader output (`out_l/r`), or with `pre_fader` the tap before its
//!   fader and pan (`parallel_out_l/r`). The key switch (`sc_enable`) is on. The engine's usual
//!   delay compensation lines the key up with the audio.
//! * **Settings.** Ratio 4:1, attack [`ATTACK_MS`], release [`RELEASE_MS`], threshold
//!   [`THRESHOLD_DB`], no look-ahead, no make-up. The threshold is the knob to turn: a lower one
//!   ducks deeper. It is not measured from the key: a project that has not been played gives no
//!   level to read, so the same fixed value is used on every platform.
//! * **Why a compressor device.** The strip's own dynamics section is the user's compressor, with
//!   a fixed attack and a mono key; taking it over would change a sound the user set up. The
//!   device sits in the insert chain, shows its threshold on its panel and goes away with one
//!   command.
//! * **Naming.** The device's name is the marker: a Compressor in the target's insert chain named
//!   "Duck from ..." whose key is cabled from a console strip is a ducker ([`duckers_of`]). No
//!   saved field is involved. The source is read from the cable, so renaming a track never breaks
//!   the link.
//! * **Repeats.** Ducking from a source the target already ducks from is refused with a plain
//!   message ([`ducking_from`] tells the app first, so it can select the device instead).
//! * **Loops.** The key cable is judged by the same rules as any cable. A source that the target
//!   feeds (directly or through a bus) would loop back into it, and is refused with a message that
//!   says so; a bus the target plays into is such a source.
//!
//! **Gentle pump.** A Sidechain Tool in `pump` mode at quarter notes, shallow, with no key at all:
//! the track breathes with the tempo. Named "Gentle Pump".
//!
//! **Remove ducking.** Takes every ducker and pump of the target out of its insert chain, with
//! their cables, in one step.
//!
//! All three are one planned edit built on a scratch copy of the document: a refusal changes
//! nothing and one undo takes the whole step back.

use crate::console::INSERT_SLOTS;
use crate::console_cmd::{self, Planner};
use crate::console_insert::{insert_chain, set_insert, stack};
use crate::history::{Edit, Swap};
use crate::reverb_send::set;
use crate::{DeviceKey, InsertEffect, ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// The device type of a ducker.
pub const DUCKER_TYPE: &str = "atomik.compressor";
/// The device type of the gentle pump.
pub const PUMP_TYPE: &str = "atomik.duck";
/// What a ducker's name starts with; the source's name follows.
pub const NAME_PREFIX: &str = "Duck from ";
/// The name of the gentle pump.
pub const PUMP_NAME: &str = "Gentle Pump";
/// The ducker's ratio.
pub const RATIO: f32 = 4.0;
/// The ducker's attack in ms.
pub const ATTACK_MS: f32 = 5.0;
/// The ducker's release in ms.
pub const RELEASE_MS: f32 = 120.0;
/// The ducker's threshold in dB.
pub const THRESHOLD_DB: f32 = -24.0;
/// How deep the gentle pump goes (share of the level).
pub const PUMP_DEPTH: f32 = 0.35;
/// The `rate` choice of the gentle pump: quarter notes.
const PUMP_RATE_QUARTER: f32 = 1.0;
/// How long the gentle pump takes to come back, in ms.
const PUMP_TIME_MS: f32 = 320.0;
/// The most rows the quick sidechain menus list before "More...".
pub const MENU_ROWS: usize = 30;

/// A ducker or pump on a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Ducker {
    /// The device in the target's insert chain.
    pub device: DeviceKey,
    /// The track device (or bus) the key comes from; `None` for a gentle pump, and for a ducker
    /// whose key cable is gone (its source was removed).
    pub source: Option<DeviceKey>,
    /// The key is the source's pre-fader tap.
    pub pre_fader: bool,
    /// A gentle pump, not a ducker.
    pub pump: bool,
}

/// A track the quick sidechain menus offer as a source.
#[derive(Debug, Clone, PartialEq)]
pub struct DuckSource {
    /// The track's device: the argument of the command.
    pub device: DeviceKey,
    /// The track's name.
    pub name: String,
    /// The colour the track shows.
    pub color: Option<[u8; 3]>,
    /// The target already ducks from it.
    pub current: bool,
}

fn refuse(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// The name of `key` for a message.
fn name_of(doc: &RackDoc, key: &DeviceKey) -> String {
    doc.device(key)
        .map_or_else(|| key.0.clone(), |d| d.name.clone())
}

/// The console channel of the track `target`.
fn channel_of(doc: &RackDoc, target: &DeviceKey) -> Result<DeviceKey, ModelError> {
    if target.0.contains('/') {
        return Err(refuse(format!(
            "{} is inside a container: ducking works on a track of the rack",
            name_of(doc, target)
        )));
    }
    if doc.song.track(target).is_none() {
        return Err(refuse(format!("{} has no track", name_of(doc, target))));
    }
    doc.console
        .channel_of(target)
        .map(|c| c.channel.clone())
        .ok_or_else(|| {
            refuse(format!(
                "{} has no console channel to duck on",
                name_of(doc, target)
            ))
        })
}

/// The strip whose output the key is taken from: the console channel of the track `source`, or
/// `source` itself when it is a console bus.
fn strip_of(doc: &RackDoc, source: &DeviceKey) -> Result<DeviceKey, ModelError> {
    if let Some(c) = doc.console.channel_of(source) {
        return Ok(c.channel.clone());
    }
    if doc.console.bus_index(source).is_some() {
        return Ok(source.clone());
    }
    Err(refuse(format!(
        "{} is not a track with a console channel",
        name_of(doc, source)
    )))
}

/// The track device (or bus) of the console strip `strip`.
fn source_of_strip(doc: &RackDoc, strip: &DeviceKey) -> Option<DeviceKey> {
    if let Some(i) = doc.console.channel_index(strip) {
        return doc.console.channels[i].source.clone();
    }
    doc.console.bus_index(strip).map(|_| strip.clone())
}

/// The duckers and gentle pumps in the insert chain of `target`'s channel, in chain order.
pub fn duckers_of(doc: &RackDoc, registry: &Registry, target: &DeviceKey) -> Vec<Ducker> {
    let Some(channel) = doc.console.channel_of(target).map(|c| &c.channel) else {
        return Vec::new();
    };
    insert_chain(doc, registry, channel)
        .into_iter()
        .filter_map(|device| {
            let d = doc.device(&device)?;
            if d.type_id == PUMP_TYPE && d.name == PUMP_NAME {
                return Some(Ducker {
                    device,
                    source: None,
                    pre_fader: false,
                    pump: true,
                });
            }
            if d.type_id != DUCKER_TYPE || !d.name.starts_with(NAME_PREFIX) {
                return None;
            }
            let key = doc
                .cables
                .iter()
                .find(|c| c.to.device == device && c.to.socket == "sc_in_l");
            Some(Ducker {
                source: key.and_then(|c| source_of_strip(doc, &c.from.device)),
                pre_fader: key.is_some_and(|c| c.from.socket.starts_with("parallel_out")),
                pump: false,
                device,
            })
        })
        .collect()
}

/// The ducker of `target` that takes its key from `source`, if there is one.
pub fn ducking_from(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
    source: &DeviceKey,
) -> Option<Ducker> {
    duckers_of(doc, registry, target)
        .into_iter()
        .find(|d| d.source.as_ref() == Some(source))
}

/// The gentle pump of `target`, if it has one.
pub fn pump_of(doc: &RackDoc, registry: &Registry, target: &DeviceKey) -> Option<Ducker> {
    duckers_of(doc, registry, target)
        .into_iter()
        .find(|d| d.pump)
}

/// The tracks `target` can duck from, in song order: every other track with a console channel.
/// A track the target already ducks from is marked.
pub fn duck_sources(doc: &RackDoc, registry: &Registry, target: &DeviceKey) -> Vec<DuckSource> {
    let ducking = duckers_of(doc, registry, target);
    doc.song
        .tracks
        .iter()
        .filter(|t| t.device != *target && doc.console.channel_of(&t.device).is_some())
        .map(|t| DuckSource {
            name: name_of(doc, &t.device),
            color: doc.song.shown_color(&t.device),
            current: ducking.iter().any(|d| d.source.as_ref() == Some(&t.device)),
            device: t.device.clone(),
        })
        .collect()
}

/// Appends a new `type_id` effect to the end of `channel`'s insert chain in the plan and returns
/// its key.
fn append_insert(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    type_id: &str,
) -> Result<DeviceKey, ModelError> {
    let slot = insert_chain(p.doc(), p.registry, channel).len();
    if slot >= INSERT_SLOTS {
        return Err(refuse(format!(
            "the insert chain of {} is full ({INSERT_SLOTS} effects)",
            name_of(p.doc(), channel)
        )));
    }
    set_insert(
        p,
        channel,
        slot,
        Some(InsertEffect::New {
            type_id: type_id.to_owned(),
        }),
    )?;
    insert_chain(p.doc(), p.registry, channel)
        .get(slot)
        .cloned()
        .ok_or_else(|| refuse("the new effect could not be put in the insert chain"))
}

/// Gives the device `key` the name `name` in the plan.
fn rename(p: &mut Planner<'_>, key: &DeviceKey, name: String) {
    let old = name_of(p.doc(), key);
    p.push(Edit::Swap(Swap::Name {
        old,
        new: name,
        key: key.clone(),
    }));
}

/// Plans the ducker of the track `target` keyed from `source` (see the module docs).
///
/// # Errors
/// The target is inside a container or has no track or console channel, the source is not a track
/// with a channel (or a bus), it is the target itself, the target already ducks from it, the
/// insert chain is full, `pre_fader` is asked of a strip with no pre-fader tap, or the key cable
/// would make a loop.
pub(crate) fn plan_duck_from(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
    source: &DeviceKey,
    pre_fader: bool,
) -> Result<Edit, ModelError> {
    let channel = channel_of(doc, target)?;
    let target_name = name_of(doc, target);
    let source_name = name_of(doc, source);
    if target == source {
        return Err(refuse(format!(
            "{target_name} cannot duck from itself: pick another track"
        )));
    }
    let strip = strip_of(doc, source)?;
    if ducking_from(doc, registry, target, source).is_some() {
        return Err(refuse(format!("Already ducking from {source_name}")));
    }
    let tap = if pre_fader { "parallel_out" } else { "out" };
    let strip_desc =
        crate::rules::descriptor(registry, &crate::rules::device(doc, &strip)?.type_id)?;
    if strip_desc.socket_by_key(&format!("{tap}_l")).is_none() {
        return Err(refuse(format!(
            "{source_name} has no pre-fader tap to duck from"
        )));
    }
    let mut p = Planner::new(doc, registry);
    let ducker = append_insert(&mut p, &channel, DUCKER_TYPE)?;
    set(&mut p, &ducker, "sc_enable", 1.0)?;
    set(&mut p, &ducker, "ratio", RATIO)?;
    set(&mut p, &ducker, "attack", ATTACK_MS)?;
    set(&mut p, &ducker, "release", RELEASE_MS)?;
    set(&mut p, &ducker, "threshold", THRESHOLD_DB)?;
    set(&mut p, &ducker, "lookahead", 0.0)?;
    rename(&mut p, &ducker, format!("{NAME_PREFIX}{source_name}"));
    for side in ["l", "r"] {
        let cable = console_cmd::wire(
            console_cmd::at(&strip, format!("{tap}_{side}")),
            console_cmd::at(&ducker, format!("sc_in_{side}")),
        );
        p.connect(cable).map_err(|e| match e {
            ModelError::BadCable(_) => refuse(format!(
                "{target_name} cannot duck from {source_name}: {target_name} feeds it, so the \
                 key would loop back into {target_name}"
            )),
            other => other,
        })?;
    }
    stack(&mut p, &channel);
    Ok(p.finish())
}

/// Plans the gentle pump on the track `target` (see the module docs).
///
/// # Errors
/// The target is inside a container or has no track or console channel, it already has a gentle
/// pump, or its insert chain is full.
pub(crate) fn plan_gentle_pump(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
) -> Result<Edit, ModelError> {
    let channel = channel_of(doc, target)?;
    if pump_of(doc, registry, target).is_some() {
        return Err(refuse(format!(
            "{} already has a {PUMP_NAME}",
            name_of(doc, target)
        )));
    }
    let mut p = Planner::new(doc, registry);
    let pump = append_insert(&mut p, &channel, PUMP_TYPE)?;
    set(&mut p, &pump, "mode", 0.0)?;
    set(&mut p, &pump, "rate", PUMP_RATE_QUARTER)?;
    set(&mut p, &pump, "depth", PUMP_DEPTH)?;
    set(&mut p, &pump, "time", PUMP_TIME_MS)?;
    rename(&mut p, &pump, PUMP_NAME.to_owned());
    stack(&mut p, &channel);
    Ok(p.finish())
}

/// Plans the removal of every ducker and pump of the track `target` (see the module docs).
///
/// # Errors
/// The target is inside a container or has no track or console channel, or it has nothing to
/// remove.
pub(crate) fn plan_remove(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
) -> Result<Edit, ModelError> {
    let channel = channel_of(doc, target)?;
    let all = duckers_of(doc, registry, target);
    if all.is_empty() {
        return Err(refuse(format!(
            "{} has no ducking to remove",
            name_of(doc, target)
        )));
    }
    let mut p = Planner::new(doc, registry);
    for ducker in all {
        // The chain closes behind the device, then the device and its key cable go.
        let chain = insert_chain(p.doc(), registry, &channel);
        if let Some(slot) = chain.iter().position(|k| *k == ducker.device) {
            set_insert(&mut p, &channel, slot, None)?;
        }
        console_cmd::remove_device(&mut p, &ducker.device)?;
    }
    stack(&mut p, &channel);
    Ok(p.finish())
}
