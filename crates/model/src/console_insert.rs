//! Insert chains: up to [`INSERT_SLOTS`] effects between a console
//! channel's (or the master's) `insert_send_*` and `insert_return_*`.
//!
//! The chain is **derived from cables**, never stored: starting at the device's
//! `insert_send_l/r`, an effect follows when its `in_l` and `in_r` are fed by the previous
//! stage's left and right outputs (`insert_send_*` for the first, `out_*` after that). The
//! last effect's `out_l/r` normally feed `insert_return_l/r`. Hand-cabled loops of the same
//! shape therefore show up as chains, and the project format does not change.
//!
//! Commands re-cable the whole chain in order (`send → fx1 → … → return`) in one undo step
//! on a scratch copy of the document, so a rejection changes nothing.
//!
//! **Bypass** is the `bypass` param of each effect, not a switch of the strip: a bypassed effect
//! keeps its own delay compensation (a built-in effect delays its dry signal by its latency, a
//! plugin device does the same), so the strip stays time aligned with the other channels.
//! [`inserts_bypassed`] reads the state of a chain and [`bypass_inserts_commands`] builds the
//! commands of the strip's "bypass all inserts" switch. The switch covers the strip's own
//! dynamics and EQ sections too: a section that is in use (its `comp.on`, `gate.on` or `eq.on`
//! is set) is taken out by its `dyn.bypass` or `eq.bypass` param, which keeps its settings and
//! its place in the signal path (the compressor look-ahead delay stays).

use crate::console::{FX_SLOTS, INSERT_SLOTS, InsertEffect, is_console_type, sockets};
use crate::console_cmd::{Planner, at, bad, wire};
use crate::history::Edit;
use crate::rack_columns::Placement;
use crate::{Cable, Command, DeviceKey, ModelError, RackDoc, RackSlot, rules};
use atomik_device_sdk::{DeviceKind, Direction, Registry, SocketKind};

/// Longest chain read back from cables (a guard; edits allow the same).
const MAX_DERIVED: usize = INSERT_SLOTS;

const SEND: [&str; 2] = ["insert_send_l", "insert_send_r"];
const RETURN: [&str; 2] = ["insert_return_l", "insert_return_r"];
const IN: [&str; 2] = ["in_l", "in_r"];
const OUT: [&str; 2] = ["out_l", "out_r"];

/// The effects on `device`'s insert loop in signal order (see the module documentation).
/// Empty when nothing is cabled from the insert send to a stereo effect.
pub fn insert_chain(doc: &RackDoc, registry: &Registry, device: &DeviceKey) -> Vec<DeviceKey> {
    chain_from(
        doc,
        registry,
        device,
        SEND.map(str::to_owned),
        MAX_DERIVED,
        &[],
    )
}

/// The effects cabled in series from the pair of output sockets `first` of `device`: the
/// stereo effect whose `in_l` and `in_r` are fed by them, then the one fed by its `out_l/r`,
/// and so on, at most `max` of them. Effects in `skip` end the walk (they belong elsewhere).
fn chain_from(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    first: [String; 2],
    max: usize,
    skip: &[DeviceKey],
) -> Vec<DeviceKey> {
    let mut chain: Vec<DeviceKey> = Vec::new();
    let mut from = device.clone();
    let mut sockets = first;
    while chain.len() < max {
        let next = doc
            .cables
            .iter()
            .filter(|c| c.from.device == from && c.from.socket == sockets[0])
            .filter(|c| c.to.socket == IN[0])
            .map(|c| &c.to.device)
            .find(|d| {
                *d != device
                    && !chain.contains(d)
                    && !skip.contains(d)
                    && is_stereo_effect(doc, registry, d)
                    && doc.cables.iter().any(|c2| {
                        c2.from.device == from
                            && c2.from.socket == sockets[1]
                            && &c2.to.device == *d
                            && c2.to.socket == IN[1]
                    })
            })
            .cloned();
        let Some(next) = next else { break };
        chain.push(next.clone());
        from = next;
        sockets = OUT.map(str::to_owned);
    }
    chain
}

/// Longest send effect chain read back from cables (a guard).
const MAX_RETURN_CHAIN: usize = 16;

/// The effects of shared send `slot` (0-based) in signal order: the effects cabled in series
/// from the master's `fx_send{k}` pair (their last one normally feeds `return{k}`, and is the
/// console's `returns[slot]`). Empty without a master or without a cabled effect.
pub fn send_chain(doc: &RackDoc, registry: &Registry, slot: usize) -> Vec<DeviceKey> {
    let Some(master) = doc.console.master.as_ref() else {
        return Vec::new();
    };
    if slot >= FX_SLOTS {
        return Vec::new();
    }
    let k = slot + 1;
    let first = ["l", "r"].map(|side| sockets::master_fx_send(k, side));
    chain_from(doc, registry, master, first, MAX_RETURN_CHAIN, &[])
}

/// The shared send whose chain holds `effect` (0-based), the send effects the rack groups under
/// the master section.
pub fn send_slot_of(doc: &RackDoc, registry: &Registry, effect: &DeviceKey) -> Option<usize> {
    (0..FX_SLOTS).find(|slot| send_chain(doc, registry, *slot).contains(effect))
}

/// The master section's stack in the rack, in order: its insert chain, then the effects of the
/// shared sends 1 to 8 (each send's effects in signal order), the ones that sit in the master's
/// column. Everything the rack draws under the master section and moves with it.
pub fn master_stack(doc: &RackDoc, registry: &Registry) -> Vec<DeviceKey> {
    let Some(master) = doc.console.master.as_ref() else {
        return Vec::new();
    };
    let column = doc.device(master).map(|d| d.column);
    let mut out = insert_chain(doc, registry, master);
    for slot in 0..FX_SLOTS {
        // A send's effects count up to the first one in another column.
        let chain = send_chain(doc, registry, slot);
        let here = chain
            .into_iter()
            .take_while(|k| doc.device(k).map(|d| d.column) == column);
        for k in here {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

/// The console channel, output bus or master whose insert chain holds `effect`: the strip it is
/// stacked under in the rack.
pub fn insert_owner(doc: &RackDoc, registry: &Registry, effect: &DeviceKey) -> Option<DeviceKey> {
    let console = &doc.console;
    console
        .channels
        .iter()
        .map(|c| &c.channel)
        .chain(console.buses.iter().map(|b| &b.bus))
        .chain(console.master.iter())
        .find(|c| insert_chain(doc, registry, c).contains(effect))
        .cloned()
}

/// The top-level order that stacks `strip`'s insert chain right under it, in chain order and in
/// its column; `None` when it is stacked so already (or has no chain).
fn stacked_layout(
    doc: &RackDoc,
    registry: &Registry,
    strip: &DeviceKey,
) -> Option<Vec<(DeviceKey, u16)>> {
    let chain = if doc.console.master.as_ref() == Some(strip) {
        // The master section's stack is its inserts and then its shared send effects.
        master_stack(doc, registry)
    } else {
        insert_chain(doc, registry, strip)
    };
    if chain.is_empty() {
        return None;
    }
    let old: Vec<(DeviceKey, u16)> = doc
        .devices
        .iter()
        .map(|d| (d.key.clone(), d.column))
        .collect();
    let column = old.iter().find(|(k, _)| k == strip)?.1;
    let mut new: Vec<(DeviceKey, u16)> = old
        .iter()
        .filter(|(k, _)| !chain.contains(k))
        .cloned()
        .collect();
    let at = new.iter().position(|(k, _)| k == strip)? + 1;
    new.splice(at..at, chain.into_iter().map(|k| (k, column)));
    (new != old).then_some(new)
}

/// Plans the layout that stacks `strip`'s insert chain right under it, in chain order (nothing
/// when it is stacked already). Every insert command ends with it, so a plain move of the
/// strip, which carries its inserts, never leaves them elsewhere.
pub(crate) fn stack(p: &mut Planner<'_>, strip: &DeviceKey) {
    if let Some(new) = stacked_layout(p.doc(), p.registry, strip) {
        let old = p
            .doc()
            .devices
            .iter()
            .map(|d| (d.key.clone(), d.column))
            .collect();
        p.push(Edit::Layout { old, new });
    }
}

/// The key of the bypass param of an effect (host-style, on every built-in effect and plugin).
const BYPASS: &str = "bypass";

/// The strip's own, legacy `insert.bypass` param (not delay compensated; see `atomik.channel`).
const STRIP_BYPASS: &str = "insert.bypass";

/// The strip's own sections that bypass-all switches: the params that put the section to work
/// and the param that bypasses it. A section is part of the switch while one of the first is on
/// or its bypass is already set.
const SECTIONS: [(&[&str], &str); 2] = [
    (&["comp.on", "gate.on"], "dyn.bypass"),
    (&["eq.on"], "eq.bypass"),
];

/// Whether `param` of `key` is on (set to 0.5 or more), the descriptor default when unset.
fn param_on(doc: &RackDoc, registry: &Registry, key: &DeviceKey, param: &str) -> Option<bool> {
    let device = doc.device(key)?;
    let desc = registry.get(&device.type_id)?.descriptor;
    let p = desc.param_by_key(param)?;
    Some(p.clamp(device.params.get(param).copied().unwrap_or(p.default)) >= 0.5)
}

/// The effects of `device`'s insert chain that can be bypassed, in signal order, each with
/// whether it is bypassed now.
pub fn insert_bypass_states(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
) -> Vec<(DeviceKey, bool)> {
    insert_chain(doc, registry, device)
        .into_iter()
        .filter_map(|k| param_on(doc, registry, &k, BYPASS).map(|on| (k, on)))
        .collect()
}

/// The sections of `device` (dynamics, EQ) that bypass-all switches, each as its bypass param
/// and whether it is bypassed now. Empty for a device without the bypass params (the master)
/// and for a section that is off and not bypassed.
pub fn section_bypass_states(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
) -> Vec<(&'static str, bool)> {
    SECTIONS
        .iter()
        .filter_map(|(works, bypass)| {
            let bypassed = param_on(doc, registry, device, bypass)?;
            let in_use = works
                .iter()
                .any(|p| param_on(doc, registry, device, p) == Some(true));
            (in_use || bypassed).then_some((*bypass, bypassed))
        })
        .collect()
}

/// Whether `device`'s inserts are bypassed: `Some(true)` when every effect that can be
/// bypassed, and every dynamics or EQ section in use, is (or the strip's legacy `insert.bypass`
/// is on), `Some(false)` when one plays, and `None` when there is nothing to bypass (no chain
/// or only effects without a bypass, and no section in use).
pub fn inserts_bypassed(doc: &RackDoc, registry: &Registry, device: &DeviceKey) -> Option<bool> {
    let legacy = param_on(doc, registry, device, STRIP_BYPASS).unwrap_or(false);
    let states = insert_bypass_states(doc, registry, device);
    let sections = section_bypass_states(doc, registry, device);
    if legacy {
        return Some(true);
    }
    if states.is_empty() && sections.is_empty() {
        return None;
    }
    Some(states.iter().all(|(_, on)| *on) && sections.iter().all(|(_, on)| *on))
}

/// The commands that bypass (`bypass` true) or restore every effect of `device`'s insert chain
/// that can be bypassed and its dynamics and EQ sections, for one `Command::Batch` (one undo
/// step). Restoring also clears the strip's legacy `insert.bypass`. Only what changes is
/// included; empty when nothing does.
pub fn bypass_inserts_commands(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    bypass: bool,
) -> Vec<Command> {
    let set = |key: &DeviceKey, param: &str, on: bool| Command::SetParam {
        key: key.clone(),
        param: param.to_owned(),
        value: if on { 1.0 } else { 0.0 },
        merge: None,
    };
    let mut commands: Vec<Command> = insert_bypass_states(doc, registry, device)
        .into_iter()
        .filter(|(_, on)| *on != bypass)
        .map(|(k, _)| set(&k, BYPASS, bypass))
        .collect();
    commands.extend(
        section_bypass_states(doc, registry, device)
            .into_iter()
            .filter(|(_, on)| *on != bypass)
            .map(|(param, _)| set(device, param, bypass)),
    );
    if !bypass && param_on(doc, registry, device, STRIP_BYPASS) == Some(true) {
        commands.push(set(device, STRIP_BYPASS, false));
    }
    commands
}

/// True for an effect device with stereo audio `in_l/in_r` and `out_l/out_r`.
fn is_stereo_effect(doc: &RackDoc, registry: &Registry, key: &DeviceKey) -> bool {
    doc.device(key)
        .and_then(|d| registry.get(&d.type_id))
        .is_some_and(|f| stereo_effect_type(f.descriptor))
}

pub(crate) fn stereo_effect_type(desc: &atomik_device_sdk::DeviceDescriptor) -> bool {
    let has = |key: &str, dir: Direction| {
        desc.socket_by_key(key).is_some_and(|s| {
            s.kind == SocketKind::Audio
                && if dir == Direction::Output {
                    s.direction == Direction::Output
                } else {
                    s.direction.is_input()
                }
        })
    };
    // A container with the four stereo sockets is an effect too.
    (desc.kind == DeviceKind::Effect || desc.kind == DeviceKind::Container)
        && IN.iter().all(|k| has(k, Direction::Input))
        && OUT.iter().all(|k| has(k, Direction::Output))
}

/// The cables of the current path `send → chain… → return` that exist (both sides).
fn path_cables(doc: &RackDoc, device: &DeviceKey, chain: &[DeviceKey]) -> Vec<Cable> {
    let mut nodes: Vec<(&DeviceKey, [&str; 2], [&str; 2])> = Vec::new();
    for e in chain {
        nodes.push((e, IN, OUT));
    }
    let mut out = Vec::new();
    let mut from = (device, SEND);
    for (dev, ins, outs) in &nodes {
        for (o, input) in from.1.iter().copied().zip(*ins) {
            out.push(wire(at(from.0, o), at(dev, input)));
        }
        from = (dev, *outs);
    }
    for (o, ret) in from.1.iter().copied().zip(RETURN) {
        out.push(wire(at(from.0, o), at(device, ret)));
    }
    out.retain(|c| doc.cables.contains(c));
    out
}

/// `channel` must be a console channel, a bus or the master with an insert loop.
fn check_target(p: &Planner<'_>, channel: &DeviceKey) -> Result<(), ModelError> {
    let console = &p.doc().console;
    if console.channel_index(channel).is_none()
        && console.bus_index(channel).is_none()
        && console.master.as_ref() != Some(channel)
    {
        return Err(bad(format!(
            "{channel} is not a console channel, bus or the master"
        )));
    }
    let desc = rules::descriptor(p.registry, &rules::device(p.doc(), channel)?.type_id)?;
    if SEND
        .iter()
        .chain(&RETURN)
        .any(|k| desc.socket_by_key(k).is_none())
    {
        return Err(bad(format!("{channel} has no insert loop")));
    }
    Ok(())
}

/// Removes the cables of `device`'s current path `send → old… → return`.
pub(crate) fn unwire(p: &mut Planner<'_>, device: &DeviceKey, old: &[DeviceKey]) {
    let doomed = path_cables(p.doc(), device, old);
    p.disconnect_where(|c| doomed.contains(c));
}

/// Cables `send → new[0] → … → return`; nothing for an empty chain (an unused insert).
pub(crate) fn wire_chain(
    p: &mut Planner<'_>,
    device: &DeviceKey,
    new: &[DeviceKey],
) -> Result<(), ModelError> {
    if new.is_empty() {
        return Ok(());
    }
    let mut from = (device.clone(), SEND);
    for e in new {
        for (out, input) in from.1.iter().copied().zip(IN) {
            p.connect(wire(at(&from.0, out), at(e, input)))?;
        }
        from = (e.clone(), OUT);
    }
    for (out, ret) in from.1.iter().copied().zip(RETURN) {
        p.connect(wire(at(&from.0, out), at(device, ret)))?;
    }
    Ok(())
}

/// Re-cables `device`'s loop from `old` to `new` (devices already exist on the scratch).
fn rewire(
    p: &mut Planner<'_>,
    device: &DeviceKey,
    old: &[DeviceKey],
    new: &[DeviceKey],
) -> Result<(), ModelError> {
    unwire(p, device, old);
    wire_chain(p, device, new)
}

pub(crate) fn set_insert(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    slot: usize,
    effect: Option<InsertEffect>,
) -> Result<(), ModelError> {
    set_insert_in(p, channel, slot, effect, false, None)
}

/// [`set_insert`] with a new container effect that comes with the inside `inner` (a pasted copy).
pub(crate) fn set_insert_container(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    slot: usize,
    inner: Box<crate::container::ContainerDoc>,
) -> Result<(), ModelError> {
    let effect = InsertEffect::New {
        type_id: crate::container::CONTAINER_TYPE.to_owned(),
    };
    set_insert_in(p, channel, slot, Some(effect), false, Some(inner))
}

/// `AddInsert`: like [`set_insert`] with an effect, but the effects from `slot` on shift down.
/// An existing effect that sits in the chain of another channel or bus leaves that chain (which
/// closes behind it) and joins this one in the same step.
pub(crate) fn add_insert(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    slot: usize,
    effect: InsertEffect,
) -> Result<(), ModelError> {
    check_target(p, channel)?;
    if let InsertEffect::Existing(e) = &effect
        && let Some(other) = insert_owner(p.doc(), p.registry, e)
        && other != *channel
    {
        let old = insert_chain(p.doc(), p.registry, &other);
        let new: Vec<DeviceKey> = old.iter().filter(|k| *k != e).cloned().collect();
        rewire(p, &other, &old, &new)?;
    }
    set_insert_in(p, channel, slot, Some(effect), true, None)
}

fn set_insert_in(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    slot: usize,
    effect: Option<InsertEffect>,
    shift: bool,
    inner: Option<Box<crate::container::ContainerDoc>>,
) -> Result<(), ModelError> {
    check_target(p, channel)?;
    if slot >= INSERT_SLOTS {
        return Err(bad(format!(
            "insert slot {slot} is outside 0..{INSERT_SLOTS}"
        )));
    }
    let old = insert_chain(p.doc(), p.registry, channel);
    if slot > old.len() {
        return Err(bad(format!(
            "insert slot {slot} is past the end of the chain ({} used)",
            old.len()
        )));
    }
    let Some(effect) = effect else {
        if slot >= old.len() {
            return Err(bad(format!("insert slot {slot} is empty")));
        }
        let mut new = old.clone();
        new.remove(slot);
        return rewire(p, channel, &old, &new);
    };
    let existing = match &effect {
        InsertEffect::New { type_id } => {
            let desc = rules::descriptor(p.registry, type_id)?;
            if !stereo_effect_type(desc) {
                return Err(bad(format!("{type_id} is not a stereo effect")));
            }
            None
        }
        InsertEffect::Existing(e) => {
            validate_existing(p, channel, e, slot, &old)?;
            if old.get(slot) == Some(e) {
                return Ok(()); // already there
            }
            Some(e.clone())
        }
    };
    if shift && old.len() >= INSERT_SLOTS {
        return Err(bad(format!(
            "the insert chain of {channel} is full ({INSERT_SLOTS} effects)"
        )));
    }
    // Take the old chain off the loop first, then place the new member.
    let mut new = old.clone();
    let key = match existing {
        Some(e) => {
            unwire(p, channel, &old);
            let ins = [at(&e, IN[0]), at(&e, IN[1])];
            let outs = [at(&e, OUT[0]), at(&e, OUT[1])];
            p.disconnect_where(|c| ins.contains(&c.to) || outs.contains(&c.from));
            e
        }
        None => {
            unwire(p, channel, &old);
            let InsertEffect::New { type_id } = &effect else {
                unreachable!("only New has no existing key")
            };
            let after = if slot == 0 { channel } else { &old[slot - 1] };
            let index = rules::index_of(p.doc(), after)? + 1;
            p.insert_device_with(index, type_id, inner)?
        }
    };
    if slot < new.len() && !shift {
        new[slot] = key;
    } else {
        new.insert(slot, key);
    }
    wire_chain(p, channel, &new)?;
    stack(p, channel);
    Ok(())
}

fn validate_existing(
    p: &Planner<'_>,
    channel: &DeviceKey,
    e: &DeviceKey,
    slot: usize,
    old: &[DeviceKey],
) -> Result<(), ModelError> {
    let doc = p.doc();
    let device = rules::device(doc, e)?;
    if is_console_type(&device.type_id) {
        return Err(bad(format!(
            "{e} is a console device, not an insert effect"
        )));
    }
    if !is_stereo_effect(doc, p.registry, e) {
        return Err(bad(format!("{e} is not a stereo effect")));
    }
    if let Some(s) = doc.console.return_slot(e) {
        return Err(bad(format!("{e} is on return {}", s + 1)));
    }
    if let Some(c) = doc.console.channel_of(e) {
        return Err(bad(format!(
            "{e} feeds channel {}; it cannot also be an insert",
            c.channel
        )));
    }
    if let Some(at_slot) = old.iter().position(|k| k == e)
        && at_slot != slot
    {
        return Err(bad(format!(
            "{e} is already insert {} of {channel} (use MoveInsert)",
            at_slot + 1
        )));
    }
    let others = doc
        .console
        .channels
        .iter()
        .map(|c| &c.channel)
        .chain(doc.console.buses.iter().map(|b| &b.bus))
        .chain(doc.console.master.iter())
        .filter(|k| *k != channel);
    for other in others {
        if insert_chain(doc, p.registry, other).contains(e) {
            return Err(bad(format!("{e} is already an insert of {other}")));
        }
    }
    Ok(())
}

pub(crate) fn move_insert(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    from: usize,
    to: usize,
) -> Result<(), ModelError> {
    check_target(p, channel)?;
    let old = insert_chain(p.doc(), p.registry, channel);
    if from >= old.len() || to >= old.len() {
        return Err(bad(format!(
            "insert move {from} -> {to} is outside the chain ({} used)",
            old.len()
        )));
    }
    if from == to {
        return Ok(());
    }
    let mut new = old.clone();
    let e = new.remove(from);
    new.insert(to, e);
    rewire(p, channel, &old, &new)?;
    stack(p, channel);
    Ok(())
}

/// `ExtractInsert`: the effect at `slot` leaves the chain (which closes behind it) and stays in
/// the rack uncabled, at `to` when given.
pub(crate) fn extract_insert(
    p: &mut Planner<'_>,
    channel: &DeviceKey,
    slot: usize,
    to: Option<RackSlot>,
) -> Result<(), ModelError> {
    check_target(p, channel)?;
    let old = insert_chain(p.doc(), p.registry, channel);
    if slot >= old.len() {
        return Err(bad(format!("insert slot {slot} is empty")));
    }
    let mut new = old.clone();
    let effect = new.remove(slot);
    rewire(p, channel, &old, &new)?;
    match to {
        Some(to) => {
            let moved = crate::rack_columns::plan_move(
                p.doc(),
                std::slice::from_ref(&effect),
                Placement::Slot(to),
            )?;
            p.push(moved);
            stack(p, channel);
        }
        // Left in place: right under the stack, which closes up above it.
        None => {
            stack(p, channel);
            let anchor = new.last().unwrap_or(channel).clone();
            let old_order: Vec<(DeviceKey, u16)> = p
                .doc()
                .devices
                .iter()
                .map(|d| (d.key.clone(), d.column))
                .collect();
            let column = old_order
                .iter()
                .find(|(k, _)| *k == anchor)
                .map(|(_, c)| *c);
            let mut order: Vec<(DeviceKey, u16)> = old_order
                .iter()
                .filter(|(k, _)| *k != effect)
                .cloned()
                .collect();
            if let (Some(column), Some(at)) = (column, order.iter().position(|(k, _)| *k == anchor))
            {
                order.insert(at + 1, (effect, column));
                if order != old_order {
                    p.push(Edit::Layout {
                        old: old_order,
                        new: order,
                    });
                }
            }
        }
    }
    Ok(())
}
