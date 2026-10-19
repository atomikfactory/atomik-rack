//! The auto-routing creation chain: creating an instrument or an
//! effect that wires itself, the Auto-route command, Disconnect for several devices and delete
//! bridging. Every command plans on a [`Planner`] scratch copy and is one undo step; a rejected
//! command changes nothing.
//!
//! **Instrument** (`CreateInstrument`): the device (with its console channel and the master,,
//! exactly as `AddDevice` does), then, when the device has a Notes input and `Alt` did not
//! skip the track, its song track, armed; the tracks the caller names in
//! [`CreateOptions::disarm`] (the ones an earlier auto-route armed) are disarmed in the same
//! step. The model cannot know who armed a track (the project format has no such flag), so the
//! app passes the list.
//!
//! **Effect** (`CreateEffect`), the branch order (the master send rule
//! comes before the channel rule and before the generic device rule, the master being a device
//! with audio outputs):
//!
//! 1. `route == false`: the effect is added after the selection (or at the end) and nothing else;
//! 2. the selection is the console master: the lowest free send/return slot (`SetReturn`, the effect right after the last effect on a return, or after the master when
//!    none); all eight used leaves it added only (the caller reports
//!    [`RouteNote::SendSlotsFull`]); master inserts are never an auto-route target;
//! 3. the selection is a console channel, or the instrument a channel was made for: the first
//!    free insert slot of that channel (`SetInsert`), the effect right after the chain;
//! 4. the selection is the effect on an FX return: the new effect follows it in series on the
//!    same return;
//! 5. the selection is another top-level device with audio outputs: the effect right after it,
//!    cabled stereo output to input when the outputs are not cabled yet;
//! 6. otherwise (nothing selected, or a step above does not apply: full chain, not a stereo
//!    effect, cabled outputs, a selection inside a container): the effect is only added.
//!
//! The effect is always added; a routing step that cannot be done (every one is tried on a
//! scratch copy) leaves it unrouted instead of failing the command.

use crate::console::{FX_SLOTS, INSERT_SLOTS, InsertEffect, is_console_type, sockets};
use crate::console_cmd::{Planner, add_channel, console_available, source_outputs};
use crate::console_insert::{insert_chain, set_insert, stereo_effect_type};
use crate::container::split_path;
use crate::history::Edit;
use crate::plugin::{PluginCommand, key_base_of_name};
use crate::song::Track;
use crate::song_cmd::{has_notes_input, new_track};
use crate::{
    Cable, Command, ConsoleCommand, DeviceKey, ModelError, RackDoc, SocketRef, console_cmd, rules,
};
use atomik_device_sdk::{
    DeviceDescriptor, DeviceKind, PluginLayout, PluginRef, Registry, SocketKind,
};
use std::collections::BTreeMap;

/// Options of the creation commands.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateOptions {
    /// Route the new device (default `true`, the Workflow setting `auto_route`): false means
    /// "add the device only", the earlier behaviour (Shift at the create action inverts it for
    /// one device). Ignored by `AutoRoute`.
    pub route: bool,
    /// Do not create a song track (Alt at the create action).
    pub skip_track: bool,
    /// Tracks to disarm when the new track is armed: the ones an earlier auto-route armed.
    /// Tracks that no longer exist or are not armed are ignored.
    pub disarm: Vec<DeviceKey>,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            route: true,
            skip_track: false,
            disarm: Vec::new(),
        }
    }
}

/// What a creation command adds.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateSource {
    /// A built-in device of the registry.
    Type(String),
    /// A plugin (the scanned reference and layout, as `PluginCommand::Add` takes them).
    Plugin {
        /// The plugin.
        plugin: PluginRef,
        /// Its layout, as the scan reports it.
        layout: PluginLayout,
        /// Where it was found.
        path_hint: Option<String>,
    },
}

/// The selection a new effect is routed from.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum CreateTarget {
    /// Nothing is selected: the effect is only added, at the end.
    #[default]
    Nothing,
    /// The selected top-level device (a console channel, the master, the effect on an FX
    /// return, an instrument or any other device); the model classifies it. A key that does not
    /// exist or lives inside a container counts as nothing selected.
    Selected(DeviceKey),
    /// The document's console master: the same as `Selected` with the master's key,
    /// for callers that do not know the key. A document without a master counts as nothing
    /// selected.
    Master,
}

/// What a creation command did not do that the user should hear about. The model
/// returns a document edit only; the caller asks [`effect_route_note`] on the document from
/// before the command and shows the text as a status message (not an error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteNote {
    /// The master was selected but all eight send/return slots are in use: the effect was added
    /// unrouted.
    SendSlotsFull,
}

impl RouteNote {
    /// The status text.
    pub fn text(self) -> &'static str {
        match self {
            RouteNote::SendSlotsFull => SEND_SLOTS_FULL,
        }
    }
}

/// Status text of [`RouteNote::SendSlotsFull`] and of the `AssignSend` rejection.
pub const SEND_SLOTS_FULL: &str = "All 8 send and return slots are in use";

/// The lowest free send/return slot (index 0..8), `None` when all eight are in use or the
/// document has no console master.
pub fn first_free_send_slot(doc: &RackDoc) -> Option<usize> {
    doc.console.master.as_ref()?;
    doc.console.returns.iter().position(Option::is_none)
}

/// The note `CreateEffect { source, target, options }` deserves on `doc` (the document before
/// the command): [`RouteNote::SendSlotsFull`] when the effect would have been routed to the
/// master's send/return slots and none is free. `None` when nothing is worth saying (including
/// every case where the command routes by another rule or is not given the option to route).
pub fn effect_route_note(
    doc: &RackDoc,
    registry: &Registry,
    source: &CreateSource,
    target: &CreateTarget,
    options: &CreateOptions,
) -> Option<RouteNote> {
    if !options.route || classify(doc, target) != Dest::Master {
        return None;
    }
    let (desc, _) = prepare(doc, registry, source, None).ok()?;
    let applies = desc.kind != DeviceKind::Instrument
        && !is_console_type(desc.type_id)
        && stereo_effect_type(desc);
    (applies && first_free_send_slot(doc).is_none()).then_some(RouteNote::SendSlotsFull)
}

fn bad_scope(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// The descriptor and the key a new device of `source` gets (the key an `AddDevice` or
/// `PluginCommand::Add` without a key would generate).
fn prepare(
    doc: &RackDoc,
    registry: &Registry,
    source: &CreateSource,
    key: Option<DeviceKey>,
) -> Result<(&'static DeviceDescriptor, DeviceKey), ModelError> {
    match source {
        CreateSource::Type(type_id) => {
            let desc = rules::descriptor(registry, type_id)?;
            let key = key.unwrap_or_else(|| rules::generate_key(doc, type_id));
            Ok((desc, key))
        }
        CreateSource::Plugin { plugin, layout, .. } => {
            let type_id = plugin.type_id();
            let desc = registry
                .descriptor(&type_id)
                .filter(|_| registry.is_plugin(&type_id))
                .ok_or(ModelError::UnknownType(type_id))?;
            let key = key.unwrap_or_else(|| {
                rules::generate_key_with_base_in(&doc.devices, &key_base_of_name(&layout.name))
            });
            Ok((desc, key))
        }
    }
}

/// The plain add of `source` at `index` under `key` (gives an instrument its channel).
fn add_edit(
    p: &Planner<'_>,
    source: &CreateSource,
    index: usize,
    key: DeviceKey,
) -> Result<Edit, ModelError> {
    let command = match source {
        CreateSource::Type(type_id) => Command::AddDevice {
            index,
            type_id: type_id.clone(),
            key: Some(key),
        },
        CreateSource::Plugin {
            plugin,
            layout,
            path_hint,
        } => Command::Plugin(PluginCommand::Add {
            index,
            container: None,
            plugin: plugin.clone(),
            layout: layout.clone(),
            path_hint: path_hint.clone(),
            key: Some(key),
            auto_split: false,
        }),
    };
    Ok(crate::Document::plan(p.doc(), command, p.registry)?.0)
}

/// Disarms the tracks of `names` that exist and are armed.
fn disarm(p: &mut Planner<'_>, names: &[DeviceKey], except: &DeviceKey) {
    for name in names {
        if name == except {
            continue;
        }
        let Some(t) = p.doc().song.track(name) else {
            continue;
        };
        if t.armed {
            let old = [t.muted, t.solo, t.armed];
            p.push(Edit::TrackFlags {
                device: name.clone(),
                old,
                new: [old[0], old[1], false],
            });
        }
    }
}

/// Adds the armed song track of `key` and disarms `names`.
fn arm_new_track(p: &mut Planner<'_>, key: &DeviceKey, names: &[DeviceKey]) {
    disarm(p, names, key);
    let index = p.doc().song.tracks.len();
    p.push(Edit::Track {
        index,
        old: None,
        new: Some(Track {
            armed: true,
            ..new_track(key.clone())
        }),
    });
}

/// `CreateInstrument`.
pub(crate) fn create_instrument(
    doc: &RackDoc,
    registry: &Registry,
    index: usize,
    source: &CreateSource,
    key: Option<DeviceKey>,
    options: &CreateOptions,
) -> Result<Edit, ModelError> {
    let (desc, key) = prepare(doc, registry, source, key)?;
    if desc.kind != DeviceKind::Instrument {
        return Err(bad_scope(format!("{} is not an instrument", desc.name)));
    }
    let mut p = Planner::new(doc, registry);
    let add = add_edit(&p, source, index, key.clone())?;
    p.push(add);
    // The strip made for the new instrument starts folded, so the rack grows
    // by one slim line per instrument (the existing `folded` flag, in the same undo step). A
    // channel made by a plain `AddDevice`, and with `route` off (exactly `AddDevice`), is left
    // as it was.
    if options.route
        && let Some(channel) = p.doc().console.channel_of(&key).map(|c| c.channel.clone())
    {
        p.push(Edit::Folded {
            key: channel,
            old: false,
            new: true,
        });
    }
    if options.route && !options.skip_track && has_notes_input(desc) {
        arm_new_track(&mut p, &key, &options.disarm);
    }
    Ok(p.finish())
}

/// What the selection of a `CreateEffect` is (in the order the rules look at it).
#[derive(Debug, Clone, PartialEq)]
enum Dest {
    /// The console master (puts the send/return slot rule here, before every other).
    Master,
    /// A console channel (selected, or the one made for the selected instrument).
    Channel(DeviceKey),
    /// The effect on FX return `slot`.
    Return(DeviceKey, usize),
    /// Any other top-level device.
    Device(DeviceKey),
    /// Nothing usable.
    Nothing,
}

fn classify(doc: &RackDoc, target: &CreateTarget) -> Dest {
    let k = match target {
        CreateTarget::Selected(k) => k,
        CreateTarget::Master => {
            return match &doc.console.master {
                Some(m) if doc.device(m).is_some() => Dest::Master,
                _ => Dest::Nothing,
            };
        }
        CreateTarget::Nothing => return Dest::Nothing,
    };
    if k.0.contains('/') || doc.device(k).is_none() {
        return Dest::Nothing;
    }
    let console = &doc.console;
    if console.master.as_ref() == Some(k) {
        Dest::Master
    } else if console.channel_index(k).is_some() {
        Dest::Channel(k.clone())
    } else if let Some(c) = console.channel_of(k) {
        Dest::Channel(c.channel.clone())
    } else if let Some(slot) = console.return_slot(k) {
        Dest::Return(k.clone(), slot)
    } else {
        Dest::Device(k.clone())
    }
}

/// Rack index right after the selected top-level device, else the end.
fn after_selection(doc: &RackDoc, target: &CreateTarget) -> usize {
    let selected = match target {
        CreateTarget::Selected(k) if !k.0.contains('/') => Some(k),
        CreateTarget::Master => doc.console.master.as_ref(),
        _ => None,
    };
    selected
        .and_then(|k| rules::index_of(doc, k).ok())
        .map(|i| i + 1)
        .unwrap_or(doc.devices.len())
}

/// `CreateEffect`.
pub(crate) fn create_effect(
    doc: &RackDoc,
    registry: &Registry,
    source: &CreateSource,
    target: &CreateTarget,
    key: Option<DeviceKey>,
    options: &CreateOptions,
) -> Result<Edit, ModelError> {
    let (desc, key) = prepare(doc, registry, source, key)?;
    if desc.kind == DeviceKind::Instrument {
        return Err(bad_scope(format!(
            "{} is an instrument, not an effect",
            desc.name
        )));
    }
    if is_console_type(desc.type_id) {
        return Err(bad_scope(format!(
            "{} is a console device; the console creates it",
            desc.name
        )));
    }
    let plain = |index: usize| -> Result<Edit, ModelError> {
        let mut p = Planner::new(doc, registry);
        let add = add_edit(&p, source, index, key.clone())?;
        p.push(add);
        Ok(p.finish())
    };
    let fallback = after_selection(doc, target);
    if !options.route || !stereo_effect_type(desc) {
        return plain(fallback);
    }
    // Each rule is tried on its own scratch copy; one that cannot be done leaves the effect
    // added and unrouted.
    let routed = match classify(doc, target) {
        // The send/return slot rule comes first: the master is a device with audio outputs, so
        // the generic rule below must never see it.
        Dest::Master => route_to_send(doc, registry, source, &key),
        Dest::Nothing => None,
        Dest::Channel(channel) => route_to_insert(doc, registry, source, &key, &channel),
        Dest::Return(effect, slot) => route_on_return(doc, registry, source, &key, &effect, slot),
        Dest::Device(device) => route_after_device(doc, registry, source, &key, &device),
    };
    match routed {
        Some(edit) => Ok(edit),
        None => plain(fallback),
    }
}

/// The rack index a new send effect goes to: right after the last effect on a return, or after
/// the master when no return is used.
fn send_effect_index(doc: &RackDoc, master: &DeviceKey) -> Option<usize> {
    let after = doc
        .console
        .returns
        .iter()
        .flatten()
        .filter_map(|e| rules::index_of(doc, e).ok())
        .max()
        .or(rules::index_of(doc, master).ok())?;
    Some(after + 1)
}

/// The new effect on the lowest free send/return slot: `fx_send{k} -> effect -> return{k}`.
fn route_to_send(
    doc: &RackDoc,
    registry: &Registry,
    source: &CreateSource,
    key: &DeviceKey,
) -> Option<Edit> {
    let slot = first_free_send_slot(doc)?;
    let index = send_effect_index(doc, doc.console.master.as_ref()?)?;
    let mut p = Planner::new(doc, registry);
    let add = add_edit(&p, source, index, key.clone()).ok()?;
    p.push(add);
    let set = console_cmd::plan(
        p.doc(),
        registry,
        ConsoleCommand::SetReturn {
            slot,
            effect: Some(key.clone()),
        },
    )
    .ok()?;
    p.push(set);
    Some(p.finish())
}

/// `AssignSend` ("Use as Send Effect"): an existing, unrouted, stereo effect goes to the lowest
/// free send/return slot. Rejected with the reason, and no change, for a device that is not a
/// top-level stereo effect (a console device, an instrument, a mixer), one that is on a return
/// or in an insert chain already or has cables, when the console has no master, and when all
/// eight slots are in use.
pub(crate) fn assign_send(
    doc: &RackDoc,
    registry: &Registry,
    effect: &DeviceKey,
) -> Result<Edit, ModelError> {
    let device = rules::resolve(doc, effect)?;
    let name = device.name.clone();
    if effect.0.contains('/') {
        return Err(bad_scope(format!(
            "{name} is inside a container; only top-level effects can be send effects"
        )));
    }
    let desc = rules::descriptor(registry, &device.type_id)?;
    if is_console_type(desc.type_id) {
        return Err(bad_scope(format!("{name} is part of the console")));
    }
    if desc.kind == DeviceKind::Instrument {
        return Err(bad_scope(format!("{name} is an instrument, not an effect")));
    }
    if !stereo_effect_type(desc) {
        return Err(bad_scope(format!("{name} is not a stereo effect")));
    }
    let console = &doc.console;
    if let Some(s) = console.return_slot(effect) {
        return Err(bad_scope(format!(
            "{name} is already the effect of send/return {}",
            s + 1
        )));
    }
    let in_chain = console
        .channels
        .iter()
        .map(|c| &c.channel)
        .chain(console.master.iter())
        .any(|c| insert_chain(doc, registry, c).contains(effect));
    if in_chain {
        return Err(bad_scope(format!("{name} is already an insert effect")));
    }
    if has_audio_cables(doc, effect) {
        return Err(bad_scope(format!("{name} is already connected")));
    }
    if console.master.is_none() {
        return Err(bad_scope("the console has no master"));
    }
    let slot = first_free_send_slot(doc).ok_or_else(|| bad_scope(SEND_SLOTS_FULL))?;
    debug_assert!(slot < FX_SLOTS);
    console_cmd::plan(
        doc,
        registry,
        ConsoleCommand::SetReturn {
            slot,
            effect: Some(effect.clone()),
        },
    )
}

/// The new effect in the first free insert slot of `channel`.
fn route_to_insert(
    doc: &RackDoc,
    registry: &Registry,
    source: &CreateSource,
    key: &DeviceKey,
    channel: &DeviceKey,
) -> Option<Edit> {
    let chain = insert_chain(doc, registry, channel);
    if chain.len() >= INSERT_SLOTS {
        return None;
    }
    let after = chain.last().unwrap_or(channel);
    let index = rules::index_of(doc, after).ok()? + 1;
    let mut p = Planner::new(doc, registry);
    let add = add_edit(&p, source, index, key.clone()).ok()?;
    p.push(add);
    set_insert(
        &mut p,
        channel,
        chain.len(),
        Some(InsertEffect::Existing(key.clone())),
    )
    .ok()?;
    Some(p.finish())
}

/// The new effect right after `effect`, on the same return: `effect -> new -> return k`.
fn route_on_return(
    doc: &RackDoc,
    registry: &Registry,
    source: &CreateSource,
    key: &DeviceKey,
    effect: &DeviceKey,
    slot: usize,
) -> Option<Edit> {
    let master = doc.console.master.clone()?;
    let k = slot + 1;
    let index = rules::index_of(doc, effect).ok()? + 1;
    let mut p = Planner::new(doc, registry);
    let add = add_edit(&p, source, index, key.clone()).ok()?;
    p.push(add);
    for side in ["l", "r"] {
        let out = console_cmd::at(effect, format!("out_{side}"));
        let ret = console_cmd::at(&master, sockets::master_return(k, side));
        let old = console_cmd::wire(out.clone(), ret.clone());
        if !p.doc().cables.contains(&old) {
            return None;
        }
        p.push(Edit::Disconnect(old));
        p.connect(console_cmd::wire(
            out,
            console_cmd::at(key, format!("in_{side}")),
        ))
        .ok()?;
        p.connect(console_cmd::wire(
            console_cmd::at(key, format!("out_{side}")),
            ret,
        ))
        .ok()?;
    }
    Some(p.finish())
}

/// True if any cable leaves `socket` of `device`.
fn output_cabled(doc: &RackDoc, device: &DeviceKey, socket: &str) -> bool {
    doc.cables
        .iter()
        .any(|c| &c.from.device == device && c.from.socket == socket)
}

/// The new effect right after `device`, cabled from its audio outputs when they are free.
fn route_after_device(
    doc: &RackDoc,
    registry: &Registry,
    source: &CreateSource,
    key: &DeviceKey,
    device: &DeviceKey,
) -> Option<Edit> {
    let (out_l, out_r) = source_outputs(doc, registry, device).ok()?;
    if output_cabled(doc, device, &out_l) || output_cabled(doc, device, &out_r) {
        return None;
    }
    let index = rules::index_of(doc, device).ok()? + 1;
    let mut p = Planner::new(doc, registry);
    let add = add_edit(&p, source, index, key.clone()).ok()?;
    p.push(add);
    p.connect(console_cmd::wire(
        console_cmd::at(device, out_l),
        console_cmd::at(key, "in_l"),
    ))
    .ok()?;
    p.connect(console_cmd::wire(
        console_cmd::at(device, out_r),
        console_cmd::at(key, "in_r"),
    ))
    .ok()?;
    Some(p.finish())
}

// ------------------------------------------------------------------------------ auto-route

/// `AutoRoute`: applies the creation rules to devices that lack them, in rack order, as one
/// step. Rejected, with the reason of the first device, when nothing changes.
pub(crate) fn auto_route(
    doc: &RackDoc,
    registry: &Registry,
    devices: &[DeviceKey],
    options: &CreateOptions,
) -> Result<Edit, ModelError> {
    if devices.is_empty() {
        return Err(bad_scope("nothing is selected"));
    }
    for k in devices {
        rules::resolve(doc, k)?;
        if k.0.contains('/') {
            return Err(bad_scope(format!(
                "{k} is inside a container; auto-route works on the top level"
            )));
        }
    }
    let mut ordered: Vec<DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| devices.contains(&d.key))
        .map(|d| d.key.clone())
        .collect();
    ordered.dedup();
    let mut p = Planner::new(doc, registry);
    let mut reasons = Vec::new();
    let mut changed = false;
    for k in &ordered {
        match route_one(&mut p, k, options) {
            Ok(()) => changed = true,
            Err(why) => reasons.push(why),
        }
    }
    if !changed {
        return Err(bad_scope(
            reasons
                .into_iter()
                .next()
                .unwrap_or_else(|| "nothing to route".into()),
        ));
    }
    Ok(p.finish())
}

fn route_one(p: &mut Planner<'_>, key: &DeviceKey, options: &CreateOptions) -> Result<(), String> {
    let registry = p.registry;
    let device = rules::device(p.doc(), key).map_err(|e| e.to_string())?;
    let name = device.name.clone();
    let desc = rules::descriptor(registry, &device.type_id).map_err(|e| e.to_string())?;
    if is_console_type(desc.type_id) {
        return Err(format!("{name} is part of the console"));
    }
    if desc.kind == DeviceKind::Instrument {
        let mut changed = false;
        let console = &p.doc().console;
        if console_available(registry)
            && console.channel_of(key).is_none()
            && console.return_slot(key).is_none()
            && source_outputs(p.doc(), registry, key).is_ok()
        {
            let mut sub = Planner::new(p.doc(), registry);
            if add_channel(&mut sub, key).is_ok() {
                p.push(sub.finish());
                changed = true;
            }
        }
        if !options.skip_track && has_notes_input(desc) && p.doc().song.track(key).is_none() {
            arm_new_track(p, key, &options.disarm);
            changed = true;
        }
        return if changed {
            Ok(())
        } else {
            Err(format!("{name} is already routed"))
        };
    }
    if !stereo_effect_type(desc) {
        return Err(format!("{name} has nothing to route"));
    }
    route_loose_effect(p, key, &name)
}

/// True if `effect` has a cable on any of its audio sockets.
fn has_audio_cables(doc: &RackDoc, effect: &DeviceKey) -> bool {
    doc.cables
        .iter()
        .any(|c| &c.from.device == effect || &c.to.device == effect)
}

/// An effect without cables, insert or return: into the insert chain of the nearest console
/// channel before it (or the channel of the nearest instrument before it), else cabled after the
/// nearest device before it that has free stereo outputs (an instrument that has no channel
/// counts, as it does for `CreateEffect` with it selected).
fn route_loose_effect(p: &mut Planner<'_>, key: &DeviceKey, name: &str) -> Result<(), String> {
    let registry = p.registry;
    let doc = p.doc();
    let on_chain = |k: &DeviceKey| {
        doc.console
            .channels
            .iter()
            .map(|c| &c.channel)
            .chain(doc.console.master.iter())
            .any(|c| insert_chain(doc, registry, c).contains(k))
    };
    if has_audio_cables(doc, key) || doc.console.return_slot(key).is_some() || on_chain(key) {
        return Err(format!("{name} is already connected"));
    }
    let at = rules::index_of(doc, key).map_err(|e| e.to_string())?;
    let before: Vec<DeviceKey> = doc.devices[..at]
        .iter()
        .rev()
        .map(|d| d.key.clone())
        .collect();
    // The nearest channel (or instrument that has one).
    let channel = before.iter().find_map(|d| {
        if doc.console.channel_index(d).is_some() {
            Some(d.clone())
        } else {
            doc.console.channel_of(d).map(|c| c.channel.clone())
        }
    });
    if let Some(channel) = channel {
        let chain = insert_chain(doc, registry, &channel);
        if chain.len() < INSERT_SLOTS {
            let mut sub = Planner::new(doc, registry);
            if set_insert(
                &mut sub,
                &channel,
                chain.len(),
                Some(InsertEffect::Existing(key.clone())),
            )
            .is_ok()
            {
                p.push(sub.finish());
                return Ok(());
            }
        }
    }
    for d in &before {
        let Ok(device) = rules::device(doc, d) else {
            continue;
        };
        // An instrument without a channel is a source like any other device here, as
        // `CreateEffect` with it selected treats it (decision: the two agree). One that
        // has a channel never gets here: its channel was found above.
        if is_console_type(&device.type_id) {
            continue;
        }
        let Ok((out_l, out_r)) = source_outputs(doc, registry, d) else {
            continue;
        };
        if output_cabled(doc, d, &out_l) || output_cabled(doc, d, &out_r) {
            continue;
        }
        let mut sub = Planner::new(doc, registry);
        let ok = sub
            .connect(console_cmd::wire(
                console_cmd::at(d, out_l),
                console_cmd::at(key, "in_l"),
            ))
            .and_then(|()| {
                sub.connect(console_cmd::wire(
                    console_cmd::at(d, out_r),
                    console_cmd::at(key, "in_r"),
                ))
            })
            .is_ok();
        if ok {
            p.push(sub.finish());
            return Ok(());
        }
    }
    Err(format!("{name} has nowhere to be routed to"))
}

// ------------------------------------------------------------------ disconnect and bridging

/// `DisconnectDevices`: every cable of every device, one step. The devices must be in one
/// scope; devices without cables are fine as long as something is cut.
pub(crate) fn disconnect_devices(doc: &RackDoc, keys: &[DeviceKey]) -> Result<Edit, ModelError> {
    let first = keys
        .first()
        .ok_or_else(|| bad_scope("nothing is selected"))?;
    let (parent, _) = split_path(first);
    let mut locals = Vec::new();
    for k in keys {
        let (p, local) = split_path(k);
        if p != parent {
            return Err(bad_scope(format!(
                "{first} and {k} are not in the same scope"
            )));
        }
        rules::resolve(doc, k)?;
        locals.push(local);
    }
    let cables: Vec<Cable> = doc
        .scope_cables(parent.as_ref())
        .unwrap_or(&[])
        .iter()
        .filter(|c| locals.contains(&c.from.device) || locals.contains(&c.to.device))
        .cloned()
        .collect();
    if cables.is_empty() {
        return Err(ModelError::BadCable("the devices have no cables".into()));
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

/// The side of an audio socket key: `l` or `r` for `in_l`, `out_r`, `ch1_in_l`, else none.
fn side(socket: &str) -> Option<&str> {
    match socket.rsplit_once('_') {
        Some((_, s @ ("l" | "r"))) => Some(s),
        _ => None,
    }
}

/// The cables that keep the signal path of top-level device `key` when it goes away: each audio
/// input socket with exactly one cable is joined to the output socket of the same side (a single
/// connected input and a single connected output pair whatever their names), and the cable's
/// source is cabled to every destination of that output. Empty when the path is ambiguous (an
/// input fed by several sources, outputs whose sides match no input), when there is no input or
/// no output cable, for console devices, and for a path that leads back into one device (the
/// insert loop of a channel, an FX return effect: `send -> effect -> return` of one device).
pub(crate) fn bridge_cables(doc: &RackDoc, registry: &Registry, key: &DeviceKey) -> Vec<Cable> {
    let Some(device) = doc.device(key) else {
        return Vec::new();
    };
    if is_console_type(&device.type_id) {
        return Vec::new();
    }
    let Ok(desc) = rules::descriptor(registry, &device.type_id) else {
        return Vec::new();
    };
    // Only a device that passes audio through (an effect, a utility, a container) has a path to
    // keep; a mixer or an instrument is removed plainly.
    if !matches!(
        desc.kind,
        DeviceKind::Effect | DeviceKind::Utility | DeviceKind::Container
    ) {
        return Vec::new();
    }
    let audio = |socket: &str| {
        desc.socket_by_key(socket)
            .is_some_and(|s| s.kind == SocketKind::Audio)
    };
    let mut ins: BTreeMap<&str, Vec<&Cable>> = BTreeMap::new();
    let mut outs: BTreeMap<&str, Vec<&Cable>> = BTreeMap::new();
    for c in &doc.cables {
        if &c.to.device == key && audio(&c.to.socket) {
            ins.entry(&c.to.socket).or_default().push(c);
        }
        if &c.from.device == key && audio(&c.from.socket) {
            outs.entry(&c.from.socket).or_default().push(c);
        }
    }
    if ins.is_empty() || outs.is_empty() || ins.values().any(|v| v.len() != 1) {
        return Vec::new();
    }
    let pairs: Vec<(&str, &str)> = if ins.len() == 1 && outs.len() == 1 {
        vec![(
            *ins.keys().next().expect("one input"),
            *outs.keys().next().expect("one output"),
        )]
    } else {
        let mut pairs = Vec::new();
        for out in outs.keys() {
            let Some(s) = side(out) else {
                return Vec::new();
            };
            let matching: Vec<&str> = ins.keys().copied().filter(|i| side(i) == Some(s)).collect();
            match matching[..] {
                [one] => pairs.push((one, *out)),
                [] => {}
                _ => return Vec::new(),
            }
        }
        pairs
    };
    let mut bridge = Vec::new();
    for (input, output) in pairs {
        let from: SocketRef = ins[input][0].from.clone();
        for c in &outs[output] {
            if from.device == c.to.device {
                continue;
            }
            bridge.push(Cable {
                from: from.clone(),
                to: c.to.clone(),
            });
        }
    }
    bridge.sort();
    bridge.dedup();
    bridge
}

/// Joins `cables` on `p` all or none: a bridge that the rules refuse leaves the removal plain.
pub(crate) fn connect_bridge(p: &mut Planner<'_>, cables: &[Cable]) {
    if cables.is_empty() {
        return;
    }
    let mut sub = Planner::new(p.doc(), p.registry);
    if cables.iter().all(|c| sub.connect(c.clone()).is_ok()) {
        p.push(sub.finish());
    }
}
