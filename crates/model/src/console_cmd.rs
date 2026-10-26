//! Console commands: validation, auto-wiring and console bookkeeping of
//! device removal, as reversible edits.
//!
//! Multi-part commands are planned on a scratch copy of the document: every edit is
//! performed on the scratch as it is planned, so each cable is validated against the state
//! it will actually join, and the result is one compound undo step.
//!
//! **Auto-wiring of a new channel**, in this order:
//! 1. If the console has no master, an `atomik.master` device is appended to the rack and
//!    every existing channel is wired to it (rules 3 and 4).
//! 2. The `atomik.channel` device is inserted right after its source in the rack.
//! 3. The source's cables into the interface are removed.
//! 4. Source → channel `in_l`/`in_r`: the source's Audio outputs `out_l`/`out_r`, or else
//!    its first two Audio outputs in descriptor order; a single Audio output feeds both.
//! 5. Channel `out_l/r` → master `bus_l/r`; channel `send{k}_l/r` → master
//!    `send_bus{k}_l/r` (k = 1..8).
//! 6. Master `main_l/r` → interface `out_1`/`out_2` (only the outputs the interface has),
//!    replacing whatever was plugged into them.
//! 7. The channel is appended to the console.
//!
//! `AddChannelFor` follows rules 1, 2 and 5-7 with the given outputs in rule 4 (no rule 3: the
//! outputs must be uncabled).

use crate::console::{
    CHANNEL_TYPE, ConsoleChannel, ConsoleCommand, FX_SLOTS, MASTER_TYPE, is_console_type, sockets,
};
use crate::history::Edit;
use crate::{Cable, DeviceKey, INTERFACE_KEY, ModelError, RackDevice, RackDoc, SocketRef, rules};
use atomik_device_sdk::{DeviceKind, Direction, Registry, SocketKind};
use std::collections::BTreeMap;

/// The name of the master in the starter document. A master created with the first
/// channel keeps the name of the registry's descriptor, so existing documents and examples do
/// not change.
pub(crate) const STARTER_MASTER_NAME: &str = "Master";

pub(crate) fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadConsole(message.into())
}

pub(crate) fn at(device: &DeviceKey, socket: impl Into<String>) -> SocketRef {
    SocketRef {
        device: device.clone(),
        socket: socket.into(),
    }
}

pub(crate) fn wire(from: SocketRef, to: SocketRef) -> Cable {
    Cable { from, to }
}

/// Builds one undo step on a scratch copy of the document.
pub(crate) struct Planner<'r> {
    scratch: RackDoc,
    edits: Vec<Edit>,
    pub(crate) registry: &'r Registry,
}

impl<'r> Planner<'r> {
    pub(crate) fn new(doc: &RackDoc, registry: &'r Registry) -> Self {
        Self {
            scratch: doc.clone(),
            edits: Vec::new(),
            registry,
        }
    }

    /// The document as planned so far.
    pub(crate) fn doc(&self) -> &RackDoc {
        &self.scratch
    }

    /// Performs `edit` on the scratch and records it (compounds are flattened).
    pub(crate) fn push(&mut self, edit: Edit) {
        match edit {
            Edit::Compound(edits) => edits.into_iter().for_each(|e| self.push(e)),
            edit => {
                // A strip that goes takes its VCA membership with it, first, so the undo of the
                // removal brings the strip back and then its membership.
                let leaving = match &edit {
                    Edit::ConsoleChannel {
                        old: Some(c),
                        new: None,
                        ..
                    } => Some(&c.channel),
                    Edit::ConsoleBus {
                        old: Some(b),
                        new: None,
                        ..
                    } => Some(&b.bus),
                    _ => None,
                };
                if let Some(leave) =
                    leaving.and_then(|k| crate::console_vca::leave_edit(&self.scratch.console, k))
                {
                    leave.perform(&mut self.scratch);
                    self.edits.push(leave);
                }
                edit.perform(&mut self.scratch);
                self.edits.push(edit);
            }
        }
    }

    /// The planned step.
    pub(crate) fn finish(mut self) -> Edit {
        if self.edits.len() == 1 {
            self.edits.pop().expect("one edit")
        } else {
            Edit::Compound(self.edits)
        }
    }

    /// Validates and adds `cable`; nothing to do if it already exists.
    pub(crate) fn connect(&mut self, cable: Cable) -> Result<(), ModelError> {
        if !self.scratch.cables.contains(&cable) {
            rules::check_cable(&self.scratch, self.registry, &cable)?;
            self.push(Edit::Connect(cable));
        }
        Ok(())
    }

    /// Removes every cable matching `pred`.
    pub(crate) fn disconnect_where(&mut self, pred: impl Fn(&Cable) -> bool) {
        let doomed: Vec<Cable> = self
            .scratch
            .cables
            .iter()
            .filter(|c| pred(c))
            .cloned()
            .collect();
        for c in doomed {
            self.push(Edit::Disconnect(c));
        }
    }

    /// Inserts a new device of `type_id` at rack `index` with a generated key and name.
    pub(crate) fn insert_device(
        &mut self,
        index: usize,
        type_id: &str,
    ) -> Result<DeviceKey, ModelError> {
        let desc = rules::descriptor(self.registry, type_id)?;
        crate::plugin_cmd::refuse_generic_add(type_id)?;
        let key = rules::generate_key(&self.scratch, type_id);
        let mut device = crate::container::new_device(
            self.registry,
            desc,
            key.clone(),
            rules::unique_name(&self.scratch, desc.name),
        );
        // Console and insert devices never carry patterns of their own.
        device.steps = None;
        self.push(Edit::Insert {
            index: rules::top_index(&self.scratch, index),
            device,
            cables: Vec::new(),
        });
        Ok(key)
    }

    /// Removes device `key`, its song track (first) and its cables.
    pub(crate) fn remove_device(&mut self, key: &DeviceKey) -> Result<(), ModelError> {
        // A track whose notes entered at this device enters at the next one down the note
        // cables now (at its own device when that is the next, or when there is none).
        let next = crate::workflow_create::notes_successor(&self.scratch, self.registry, key);
        let entering: Vec<DeviceKey> = self
            .scratch
            .song
            .tracks
            .iter()
            .filter(|t| t.notes_to.as_ref() == Some(key))
            .map(|t| t.device.clone())
            .collect();
        for device in entering {
            let new = next.clone().filter(|n| *n != device);
            self.push(Edit::TrackEntry {
                device,
                old: Some(key.clone()),
                new,
            });
        }
        if let Some(t) = self
            .scratch
            .song
            .tracks
            .iter()
            .position(|t| &t.device == key)
        {
            self.push(Edit::Track {
                index: t,
                old: Some(self.scratch.song.tracks[t].clone()),
                new: None,
            });
        }
        let index = rules::index_of(&self.scratch, key)?;
        let cables = self
            .scratch
            .cables
            .iter()
            .filter(|c| &c.from.device == key || &c.to.device == key)
            .cloned()
            .collect();
        self.push(Edit::Remove {
            index,
            device: self.scratch.devices[index].clone(),
            cables,
        });
        Ok(())
    }

    pub(crate) fn type_of(&self, key: &DeviceKey) -> Result<&str, ModelError> {
        Ok(&rules::device(&self.scratch, key)?.type_id)
    }
}

/// Plans the insertion of `device` at the top level (index clamped) as `AddDevice` does: with
/// the console devices in the registry an `instrument` also gets its console channel, and the
/// master when missing, in the same undo step. An instrument without audio outputs gets
/// none.
pub(crate) fn insert_with_channel(
    doc: &RackDoc,
    registry: &Registry,
    index: usize,
    device: RackDevice,
    instrument: bool,
) -> Edit {
    let key = device.key.clone();
    // Nothing goes above the master while it is the first device.
    let insert = Edit::Insert {
        index: rules::top_index(doc, index),
        device,
        cables: Vec::new(),
    };
    if instrument && console_available(registry) {
        let mut p = Planner::new(doc, registry);
        p.push(insert.clone());
        match add_channel(&mut p, &key) {
            Ok(()) => p.finish(),
            Err(_) => insert,
        }
    } else {
        insert
    }
}

/// True if the registry holds both console device types (automatic channels are on).
pub(crate) fn console_available(registry: &Registry) -> bool {
    registry.get(CHANNEL_TYPE).is_some() && registry.get(MASTER_TYPE).is_some()
}

/// True if `type_id` is an instrument in `registry` (it gets a channel automatically).
pub(crate) fn is_instrument(registry: &Registry, type_id: &str) -> bool {
    registry
        .get(type_id)
        .is_some_and(|f| f.descriptor.kind == DeviceKind::Instrument)
}

/// The two source outputs feeding a channel's `in_l`/`in_r` (rule 4).
pub(crate) fn source_outputs(
    doc: &RackDoc,
    registry: &Registry,
    source: &DeviceKey,
) -> Result<(String, String), ModelError> {
    let desc = rules::descriptor(registry, &rules::device(doc, source)?.type_id)?;
    let audio_out = |key: &str| {
        desc.socket_by_key(key)
            .is_some_and(|s| s.kind == SocketKind::Audio && s.direction == Direction::Output)
    };
    if audio_out("out_l") && audio_out("out_r") {
        return Ok(("out_l".into(), "out_r".into()));
    }
    let outs: Vec<&str> = desc
        .sockets
        .iter()
        .filter(|s| s.kind == SocketKind::Audio && s.direction == Direction::Output)
        .map(|s| s.key)
        .take(2)
        .collect();
    match outs[..] {
        [l, r] => Ok((l.into(), r.into())),
        [mono] => Ok((mono.into(), mono.into())),
        _ => Err(bad(format!(
            "{source} has no audio output to feed a channel"
        ))),
    }
}

/// Wires a channel's outputs and sends into the master's buses (rule 5).
fn wire_channel_to_master(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    master: &DeviceKey,
) -> Result<(), ModelError> {
    for side in ["l", "r"] {
        p.connect(wire(
            at(channel, format!("out_{side}")),
            at(master, format!("bus_{side}")),
        ))?;
    }
    for k in sockets::sends() {
        for side in ["l", "r"] {
            p.connect(wire(
                at(channel, sockets::channel_send(k, side)),
                at(master, sockets::master_send_bus(k, side)),
            ))?;
        }
    }
    Ok(())
}

/// The cables `main_l → interface out_1` and `main_r → out_2` of `master`, only for the outputs
/// the interface has (rule 6 and the starter document).
fn main_cables(master: &DeviceKey, output_channels: u16) -> Vec<Cable> {
    let interface = DeviceKey(INTERFACE_KEY.to_owned());
    [(1u16, "l"), (2, "r")]
        .into_iter()
        .filter(|(n, _)| *n <= output_channels)
        .map(|(n, side)| {
            wire(
                at(master, format!("main_{side}")),
                at(&interface, format!("out_{n}")),
            )
        })
        .collect()
}

/// Master `main_l/r` → interface `out_1/2`, replacing other cables there (rule 6).
pub(crate) fn wire_master_to_interface(
    p: &mut Planner<'_>,
    master: &DeviceKey,
) -> Result<(), ModelError> {
    for cable in main_cables(master, p.doc().output_channels) {
        let (out, main) = (cable.to.clone(), cable.from.clone());
        p.disconnect_where(|c| c.to == out && c.from != main);
        p.connect(cable)?;
    }
    Ok(())
}

/// A console master device: the params stay at their defaults, unfolded, no patterns.
fn master_device(key: DeviceKey, name: String) -> RackDevice {
    RackDevice {
        key,
        type_id: MASTER_TYPE.to_owned(),
        name,
        params: BTreeMap::new(),
        folded: false,
        color: None,
        zones: Vec::new(),
        steps: None,
        curves: None,
        drums: None,
        inner: None,
        plugin: None,
        column: 0,
    }
}

/// The edit that inserts the master at rack `index` and registers it in the console. The one
/// definition of "a new master" (key, defaults, console entry): [`ensure_master`] (rule 1, the
/// descriptor's name) and the starter document use it.
fn new_master_edits(doc: &RackDoc, index: usize, name: &str) -> (DeviceKey, [Edit; 2]) {
    let key = rules::generate_key(doc, MASTER_TYPE);
    let device = master_device(key.clone(), rules::unique_name(doc, name));
    let edits = [
        Edit::Insert {
            index,
            device,
            cables: Vec::new(),
        },
        Edit::ConsoleMaster {
            old: None,
            new: Some(key.clone()),
        },
    ];
    (key, edits)
}

/// The console master, created (rule 1) when missing.
pub(crate) fn ensure_master(p: &mut Planner<'_>) -> Result<DeviceKey, ModelError> {
    if let Some(m) = &p.doc().console.master {
        return Ok(m.clone());
    }
    let desc = rules::descriptor(p.registry, MASTER_TYPE)?;
    let index = p.doc().devices.len();
    let (master, edits) = new_master_edits(p.doc(), index, desc.name);
    edits.into_iter().for_each(|e| p.push(e));
    let channels: Vec<DeviceKey> = p
        .doc()
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .collect();
    for ch in &channels {
        wire_channel_to_master(p, ch, &master)?;
    }
    Ok(master)
}

/// Makes `doc` (an empty document) the starter of File, New: the master first in the rack,
/// registered in the console, `main` cabled to the interface outputs (the same master, the same
/// cables as the first channel creates, see [`ensure_master`] and rule 6).
pub(crate) fn install_starter_master(doc: &mut RackDoc) {
    let (master, edits) = new_master_edits(doc, 0, STARTER_MASTER_NAME);
    for edit in edits {
        edit.perform(doc);
    }
    for cable in main_cables(&master, doc.output_channels) {
        Edit::Connect(cable).perform(doc);
    }
}

/// Plans `AddChannel { source }` (the module docs list the rules).
pub(crate) fn add_channel(p: &mut Planner<'_>, source: &DeviceKey) -> Result<(), ModelError> {
    rules::descriptor(p.registry, CHANNEL_TYPE)?;
    rules::descriptor(p.registry, MASTER_TYPE)?;
    if is_console_type(p.type_of(source)?) {
        return Err(bad(format!(
            "{source} is a console device; it cannot feed a channel"
        )));
    }
    if let Some(c) = p.doc().console.channel_of(source) {
        return Err(bad(format!("{source} already feeds channel {}", c.channel)));
    }
    if let Some(slot) = p.doc().console.return_slot(source) {
        return Err(bad(format!(
            "{source} is on return {}; it cannot also feed a channel",
            slot + 1
        )));
    }
    let (out_l, out_r) = source_outputs(p.doc(), p.registry, source)?;
    let master = ensure_master(p)?;
    let index = rules::index_of(p.doc(), source)? + 1;
    let channel = p.insert_device(index, CHANNEL_TYPE)?;
    p.disconnect_where(|c| &c.from.device == source && c.to.device.0 == INTERFACE_KEY);
    p.connect(wire(at(source, out_l), at(&channel, "in_l")))?;
    p.connect(wire(at(source, out_r), at(&channel, "in_r")))?;
    wire_channel_to_master(p, &channel, &master)?;
    wire_master_to_interface(p, &master)?;
    let index = p.doc().console.channels.len();
    p.push(Edit::ConsoleChannel {
        index,
        old: None,
        new: Some(ConsoleChannel {
            channel,
            source: Some(source.clone()),
        }),
    });
    Ok(())
}

/// The default name of a channel for output `out_l` of `source_name`: `"<name> <stem>"`, the
/// stem being the socket key without its `_l` and `_out` ends, spaced (`pad2_out_l` → `Pad 2`).
fn default_channel_name(source_name: &str, out_l: &str) -> String {
    let stem = out_l.strip_suffix("_l").unwrap_or(out_l);
    let stem = stem.strip_suffix("_out").unwrap_or(stem);
    let mut text = String::new();
    let mut prev: Option<char> = None;
    for c in stem.chars() {
        let c = if c == '_' { ' ' } else { c };
        if c.is_ascii_digit() && prev.is_some_and(|p| p.is_alphabetic()) {
            text.push(' ');
        }
        if prev.is_none_or(|p| p == ' ') {
            text.extend(c.to_uppercase());
        } else {
            text.push(c);
        }
        prev = Some(c);
    }
    format!("{source_name} {text}")
}

/// Plans `AddChannelFor` (see [`ConsoleCommand::AddChannelFor`]).
fn add_channel_for(
    p: &mut Planner<'_>,
    source: &DeviceKey,
    out_l: &str,
    out_r: &str,
    name: Option<String>,
) -> Result<(), ModelError> {
    rules::descriptor(p.registry, CHANNEL_TYPE)?;
    rules::descriptor(p.registry, MASTER_TYPE)?;
    let type_id = p.type_of(source)?.to_owned();
    if is_console_type(&type_id) {
        return Err(bad(format!(
            "{source} is a console device; it cannot feed a channel"
        )));
    }
    if let Some(slot) = p.doc().console.return_slot(source) {
        return Err(bad(format!(
            "{source} is on return {}; it cannot also feed a channel",
            slot + 1
        )));
    }
    let desc = rules::descriptor(p.registry, &type_id)?;
    for out in [out_l, out_r] {
        let ok = desc
            .socket_by_key(out)
            .is_some_and(|s| s.kind == SocketKind::Audio && s.direction == Direction::Output);
        if !ok {
            return Err(bad(format!("{source}:{out} is not an audio output")));
        }
        if p.doc()
            .cables
            .iter()
            .any(|c| &c.from.device == source && c.from.socket == out)
        {
            return Err(bad(format!("{source}:{out} is already cabled")));
        }
    }
    let master = ensure_master(p)?;
    let source_name = rules::device(p.doc(), source)?.name.clone();
    let index = rules::index_of(p.doc(), source)? + 1;
    let channel = p.insert_device(index, CHANNEL_TYPE)?;
    let new = name.unwrap_or_else(|| default_channel_name(&source_name, out_l));
    let old = rules::device(p.doc(), &channel)?.name.clone();
    p.push(Edit::Name {
        key: channel.clone(),
        old,
        new,
    });
    p.connect(wire(at(source, out_l), at(&channel, "in_l")))?;
    p.connect(wire(at(source, out_r), at(&channel, "in_r")))?;
    wire_channel_to_master(p, &channel, &master)?;
    wire_master_to_interface(p, &master)?;
    let index = p.doc().console.channels.len();
    p.push(Edit::ConsoleChannel {
        index,
        old: None,
        new: Some(ConsoleChannel {
            channel,
            source: Some(source.clone()),
        }),
    });
    Ok(())
}

/// Removes console channel `index` and its device.
fn remove_channel_at(p: &mut Planner<'_>, index: usize) -> Result<(), ModelError> {
    let ch = p.doc().console.channels[index].clone();
    p.push(Edit::ConsoleChannel {
        index,
        old: Some(ch.clone()),
        new: None,
    });
    p.remove_device(&ch.channel)
}

/// Plans `RemoveDevice { key }`: the console bookkeeping first, then the device (with its
/// track and cables).
///
/// - A console channel leaves the console.
/// - An instrument's channels go with it; other sources leave their channel source-less.
/// - The master is fixed: removing it is rejected ([`ModelError::Fixed`]).
/// - A return effect leaves its slot.
pub(crate) fn remove_device(p: &mut Planner<'_>, key: &DeviceKey) -> Result<(), ModelError> {
    let type_id = p.type_of(key)?.to_owned();
    rules::check_deletable(p.doc(), key)?;
    // Deleting a bus device is `DeleteBus`: its feeders move to its own destination.
    if p.doc().console.bus_index(key).is_some() {
        return crate::console_bus::delete_bus(p, key);
    }
    if let Some(index) = p.doc().console.channel_index(key) {
        p.push(Edit::ConsoleChannel {
            index,
            old: Some(p.doc().console.channels[index].clone()),
            new: None,
        });
    }
    // A container that feeds a channel is its source like an instrument is: the channel goes
    // with it.
    let instrument = is_instrument(p.registry, &type_id)
        || p.registry
            .get(&type_id)
            .is_some_and(|f| f.descriptor.kind == DeviceKind::Container);
    while let Some(index) = p
        .doc()
        .console
        .channels
        .iter()
        .position(|c| c.source.as_ref() == Some(key))
    {
        if instrument {
            remove_channel_at(p, index)?;
        } else {
            let old = p.doc().console.channels[index].clone();
            let new = ConsoleChannel {
                source: None,
                ..old.clone()
            };
            p.push(Edit::ConsoleChannel {
                index,
                old: Some(old),
                new: Some(new),
            });
        }
    }
    if let Some(slot) = p.doc().console.return_slot(key) {
        p.push(Edit::ConsoleReturn {
            slot,
            old: Some(key.clone()),
            new: None,
        });
    }
    p.remove_device(key)
}

/// Unwires FX return `slot` (master send → effect → master return).
fn unwire_return(p: &mut Planner<'_>, master: &DeviceKey, slot: usize, effect: Option<&DeviceKey>) {
    let k = slot + 1;
    let fx: Vec<SocketRef> = ["l", "r"]
        .iter()
        .map(|s| at(master, sockets::master_fx_send(k, s)))
        .collect();
    let ret: Vec<SocketRef> = ["l", "r"]
        .iter()
        .map(|s| at(master, sockets::master_return(k, s)))
        .collect();
    p.disconnect_where(|c| {
        ret.contains(&c.to) || (fx.contains(&c.from) && effect.is_some_and(|e| &c.to.device == e))
    });
}

/// Plans a console command.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: ConsoleCommand,
) -> Result<Edit, ModelError> {
    let console = &doc.console;
    let not_channel = |k: &DeviceKey| bad(format!("{k} is not a console channel"));
    let mut p = Planner::new(doc, registry);
    match command {
        ConsoleCommand::AddChannel { source } => add_channel(&mut p, &source)?,
        ConsoleCommand::AddChannelFor {
            source,
            out_l,
            out_r,
            name,
        } => add_channel_for(&mut p, &source, &out_l, &out_r, name)?,
        ConsoleCommand::RemoveChannel { channel } => {
            rules::check_deletable(doc, &channel)?;
            let index = console
                .channel_index(&channel)
                .ok_or_else(|| not_channel(&channel))?;
            remove_channel_at(&mut p, index)?;
        }
        ConsoleCommand::MoveChannel { channel, to } => {
            let from = console
                .channel_index(&channel)
                .ok_or_else(|| not_channel(&channel))?;
            p.push(Edit::ConsoleMove {
                from,
                to: to.min(console.channels.len() - 1),
            });
        }
        ConsoleCommand::SetReturn { slot, effect } => {
            if slot >= FX_SLOTS {
                return Err(ModelError::OutOfRange(format!(
                    "return slot {slot} is outside 0..{FX_SLOTS}"
                )));
            }
            let master = console
                .master
                .clone()
                .ok_or_else(|| bad("the console has no master"))?;
            if let Some(e) = &effect {
                if is_console_type(&rules::device(doc, e)?.type_id) {
                    return Err(bad(format!("{e} is a console device, not a return effect")));
                }
                if let Some(other) = console.return_slot(e).filter(|&s| s != slot) {
                    return Err(bad(format!("{e} is already on return {}", other + 1)));
                }
                if let Some(c) = console.channel_of(e) {
                    return Err(bad(format!(
                        "{e} feeds channel {}; it cannot also be a return effect",
                        c.channel
                    )));
                }
            }
            let old = console.returns[slot].clone();
            unwire_return(&mut p, &master, slot, old.as_ref());
            if let Some(e) = &effect {
                // The effect leaves wherever it was cabled before.
                let ins = [at(e, "in_l"), at(e, "in_r")];
                let outs = [at(e, "out_l"), at(e, "out_r")];
                p.disconnect_where(|c| ins.contains(&c.to) || outs.contains(&c.from));
                let k = slot + 1;
                for side in ["l", "r"] {
                    p.connect(wire(
                        at(&master, sockets::master_fx_send(k, side)),
                        at(e, format!("in_{side}")),
                    ))?;
                }
                for side in ["l", "r"] {
                    p.connect(wire(
                        at(e, format!("out_{side}")),
                        at(&master, sockets::master_return(k, side)),
                    ))?;
                }
            }
            p.push(Edit::ConsoleReturn {
                slot,
                old,
                new: effect,
            });
        }
        ConsoleCommand::SetInsert {
            channel,
            slot,
            effect,
        } => crate::console_insert::set_insert(&mut p, &channel, slot, effect)?,
        ConsoleCommand::MoveInsert { channel, from, to } => {
            crate::console_insert::move_insert(&mut p, &channel, from, to)?
        }
        ConsoleCommand::SetCompensation(on) => p.push(Edit::Compensation {
            old: console.compensation,
            new: on,
        }),
        ConsoleCommand::CreateBus { name, dest } => {
            crate::console_bus::create_bus(&mut p, &name, &dest)?;
        }
        ConsoleCommand::RenameBus { bus, name } => {
            crate::console_bus::rename_bus(&mut p, &bus, &name)?
        }
        ConsoleCommand::DeleteBus { bus } => crate::console_bus::delete_bus(&mut p, &bus)?,
        ConsoleCommand::ReorderBus { bus, to } => {
            crate::console_bus::reorder_bus(&mut p, &bus, to)?
        }
        ConsoleCommand::SetOutput { strip, dest } => {
            crate::console_bus::set_output(&mut p, &strip, &dest)?
        }
        ConsoleCommand::RouteToNewBus { strips, name } => {
            crate::console_bus::route_to_new_bus(&mut p, &strips, &name)?
        }
        ConsoleCommand::ClearSolos => {
            clear(&mut p, "solo")?;
            crate::console_vca::clear_flags(&mut p, "solo")?;
        }
        ConsoleCommand::ClearMutes => {
            clear(&mut p, "mute")?;
            crate::console_vca::clear_flags(&mut p, "mute")?;
        }
        vca_command @ (ConsoleCommand::CreateVca { .. }
        | ConsoleCommand::RenameVca { .. }
        | ConsoleCommand::SetVcaColor { .. }
        | ConsoleCommand::SetVcaLevel { .. }
        | ConsoleCommand::SetVcaMute { .. }
        | ConsoleCommand::SetVcaSolo { .. }
        | ConsoleCommand::AssignToVca { .. }
        | ConsoleCommand::DeleteVca { .. }) => crate::console_vca::plan(&mut p, vca_command)?,
        snapshot_command @ (ConsoleCommand::StoreSnapshot { .. }
        | ConsoleCommand::UpdateSnapshot { .. }
        | ConsoleCommand::RecallSnapshot { .. }
        | ConsoleCommand::RenameSnapshot { .. }
        | ConsoleCommand::DeleteSnapshot { .. }
        | ConsoleCommand::MoveSnapshot { .. }
        | ConsoleCommand::Compare { .. }
        | ConsoleCommand::EndCompare) => crate::console_snapshot::plan(&mut p, snapshot_command)?,
    }
    // `ClearSolos` with nothing to clear is an empty (but valid) step.
    Ok(p.finish())
}

/// Resets `param` to "off" on every console channel that has it on.
fn clear(p: &mut Planner<'_>, param: &str) -> Result<(), ModelError> {
    let channels: Vec<DeviceKey> = p
        .doc()
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .collect();
    for ch in channels {
        let device = rules::device(p.doc(), &ch)?;
        let desc = rules::descriptor(p.registry, &device.type_id)?;
        let Some(pd) = desc.param_by_key(param) else {
            continue;
        };
        let off = pd.clamp(0.0);
        let new = (pd.default.to_bits() != off.to_bits()).then_some(off);
        let old = device.params.get(param).copied();
        if old.map(f32::to_bits) != new.map(f32::to_bits) {
            p.push(Edit::Param {
                key: ch,
                param: param.to_owned(),
                old,
                new,
                default: pd.default,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starter(output_channels: u16) -> RackDoc {
        let mut doc = crate::empty_doc();
        doc.output_channels = output_channels;
        install_starter_master(&mut doc);
        doc
    }

    fn main_to(side: &str, out: u16) -> Cable {
        wire(
            at(&DeviceKey("master".into()), format!("main_{side}")),
            at(&DeviceKey(INTERFACE_KEY.into()), format!("out_{out}")),
        )
    }

    #[test]
    fn the_starter_skips_out_2_when_the_interface_has_one_output() {
        assert_eq!(starter(1).cables, [main_to("l", 1)]);
    }

    #[test]
    fn the_starter_has_no_cables_without_outputs() {
        let doc = starter(0);
        assert!(doc.cables.is_empty());
        assert_eq!(doc.console.master, Some(DeviceKey("master".into())));
    }

    #[test]
    fn the_starter_cables_both_outputs_from_two_up() {
        for n in [2, 6] {
            assert_eq!(starter(n).cables, [main_to("l", 1), main_to("r", 2)]);
        }
    }

    #[test]
    fn the_starter_equals_what_the_first_channel_would_create() {
        // The shared master code: the master the starter installs is the one `ensure_master`
        // plans (same key, name, params), so the two paths cannot drift.
        let doc = starter(2);
        let (key, edits) = new_master_edits(&crate::empty_doc(), 0, STARTER_MASTER_NAME);
        let mut again = crate::empty_doc();
        edits.iter().for_each(|e| {
            e.perform(&mut again);
        });
        assert_eq!(doc.devices, again.devices);
        assert_eq!(doc.console.master, Some(key));
    }
}
