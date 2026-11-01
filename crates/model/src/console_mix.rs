//! Mix channels, parallel channels and the follow links of the sends.
//!
//! **Mix channel** (`AddMixChannel`): an `atomik.channel` with no source, wired to the master and
//! its sends like any channel. Its `in_l`/`in_r` are the Direct In: whatever is cabled there is
//! what the strip plays.
//!
//! **Parallel channel** (`CreateParallel`): a source-less channel fed from a strip's
//! `parallel_out_l/r` (the strip's signal at the end of its first stage, before the fader and the
//! pan), named `P<n>: <strip name>` and routed where the strip is routed (the master, or the bus
//! the strip plays into). [`crate::console::Console::parallels`] records the pair so the name
//! follows renames and the child goes with its parent.
//!
//! **Send follow** (`Console::send_follow`): every strip that plays into a bus gets a hidden
//! console link `bus:follow_out -> strip:follow_in` ([`crate::console::LinkKind::FollowLink`]), so
//! the strip's post-fader sends carry the bus fader and mute (and the buses above it). The
//! links are kept by the routing commands; a project saved before the flag keeps none.

use crate::console::{
    CHANNEL_TYPE, ConsoleChannel, FOLLOW_IN, FOLLOW_OUT, OutputDest, PARALLEL_OUT, is_strip,
    output_dest,
};
use crate::console_cmd::{
    Planner, at, bad, ensure_master, wire, wire_channel_to_master, wire_master_to_interface,
};
use crate::history::Edit;
use crate::{DeviceKey, ModelError, rules};

/// The name the parent's parallel channel gets: `P<n>: <parent name>`.
fn parallel_name(n: usize, parent_name: &str) -> String {
    format!("P{n}: {parent_name}")
}

/// The part of a parallel channel's name before the parent's name (`"P2: "`), when `name` is
/// `P<digits>: ` followed by `parent_name`.
fn parallel_prefix<'a>(name: &'a str, parent_name: &str) -> Option<&'a str> {
    let rest = name.strip_suffix(parent_name)?;
    let digits = rest.strip_prefix('P')?.strip_suffix(": ")?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then_some(&name[..rest.len()])
}

/// The rack index a new strip goes to: after the last console channel, else the end of the rack.
fn channel_insert_index(p: &Planner<'_>) -> usize {
    let doc = p.doc();
    doc.console
        .channels
        .iter()
        .filter_map(|c| rules::index_of(doc, &c.channel).ok())
        .max()
        .map_or(doc.devices.len(), |i| i + 1)
}

/// Inserts a source-less console channel named `name` (or the default name when `None`) wired to
/// the master, and returns its key.
fn new_sourceless_channel(
    p: &mut Planner<'_>,
    index: Option<usize>,
    name: Option<String>,
) -> Result<DeviceKey, ModelError> {
    rules::descriptor(p.registry, CHANNEL_TYPE)?;
    rules::descriptor(p.registry, crate::console::MASTER_TYPE)?;
    let had_master = p.doc().console.master.is_some();
    let master = ensure_master(p)?;
    if !had_master {
        wire_master_to_interface(p, &master)?;
    }
    let index = index.unwrap_or_else(|| channel_insert_index(p));
    let channel = p.insert_device(index, CHANNEL_TYPE)?;
    if let Some(new) = name {
        let old = rules::device(p.doc(), &channel)?.name.clone();
        if old != new {
            p.push(Edit::Name {
                key: channel.clone(),
                old,
                new,
            });
        }
    }
    // Like a bus, the strip starts folded in the rack.
    p.push(Edit::Folded {
        key: channel.clone(),
        old: false,
        new: true,
    });
    wire_channel_to_master(p, &channel, &master)?;
    let at_index = p.doc().console.channels.len();
    p.push(Edit::ConsoleChannel {
        index: at_index,
        old: None,
        new: Some(ConsoleChannel {
            channel: channel.clone(),
            source: None,
        }),
    });
    Ok(channel)
}

/// Plans `AddMixChannel { name }` and returns the new channel key.
pub(crate) fn add_mix_channel(p: &mut Planner<'_>, name: &str) -> Result<DeviceKey, ModelError> {
    let name = name.trim();
    let name = if name.is_empty() {
        rules::unique_name(p.doc(), "Mix Channel")
    } else {
        name.to_owned()
    };
    new_sourceless_channel(p, None, Some(name))
}

/// Plans `CreateParallel { strip }` and returns the new channel key.
pub(crate) fn create_parallel(
    p: &mut Planner<'_>,
    strip: &DeviceKey,
) -> Result<DeviceKey, ModelError> {
    let console = &p.doc().console;
    if console.channel_index(strip).is_none() && console.bus_index(strip).is_none() {
        return Err(bad(format!("{strip} is not a console channel or bus")));
    }
    let desc = rules::descriptor(p.registry, p.type_of(strip)?)?;
    if desc.socket_by_key(PARALLEL_OUT[0]).is_none() {
        return Err(bad(format!("{strip} has no parallel output")));
    }
    let n = console.parallels_of(strip).count() + 1;
    let name = parallel_name(n, &rules::device(p.doc(), strip)?.name);
    let index = rules::index_of(p.doc(), strip)? + 1;
    let child = new_sourceless_channel(p, Some(index), Some(name))?;
    for (side, out) in ["l", "r"].into_iter().zip(PARALLEL_OUT) {
        p.connect(wire(at(strip, out), at(&child, format!("in_{side}"))))?;
    }
    // The copy plays where its parent plays.
    if let OutputDest::Bus(bus) = output_dest(p.doc(), strip) {
        crate::console_bus::set_output(p, &child, &OutputDest::Bus(bus))?;
    }
    let old = p.doc().console.parallels.clone();
    let mut new = old.clone();
    new.push((strip.clone(), child.clone()));
    p.push(Edit::ConsoleParallels { old, new });
    sync_follow(p, &child)?;
    Ok(child)
}

/// Renames the parallel channels of `parent` (and theirs) whose name still is the one the old
/// parent name gave them. A copy the user renamed keeps its name.
pub(crate) fn follow_names(
    p: &mut Planner<'_>,
    parent: &DeviceKey,
    old_name: &str,
    new_name: &str,
) -> Result<(), ModelError> {
    let kids: Vec<DeviceKey> = p.doc().console.parallels_of(parent).cloned().collect();
    for kid in kids {
        let kid_old = rules::device(p.doc(), &kid)?.name.clone();
        let Some(prefix) = parallel_prefix(&kid_old, old_name) else {
            continue;
        };
        let kid_new = format!("{prefix}{new_name}");
        p.push(Edit::Name {
            key: kid.clone(),
            old: kid_old.clone(),
            new: kid_new.clone(),
        });
        follow_names(p, &kid, &kid_old, &kid_new)?;
    }
    Ok(())
}

/// Removes the entries of `key` as a parent (and removes its parallel channels, theirs too) and
/// as a child, before `key` itself goes. `remove_channel` removes one console channel with its
/// device; the planner state after the call has no parallel entry naming `key`.
pub(crate) fn detach(
    p: &mut Planner<'_>,
    key: &DeviceKey,
    remove_channel: fn(&mut Planner<'_>, usize) -> Result<(), ModelError>,
) -> Result<(), ModelError> {
    let kids: Vec<DeviceKey> = p.doc().console.parallels_of(key).cloned().collect();
    for kid in kids {
        // The kid's own removal runs `detach` for it, which also drops its entry.
        let index = p
            .doc()
            .console
            .channel_index(&kid)
            .ok_or_else(|| bad(format!("{kid} is not a console channel")))?;
        remove_channel(p, index)?;
    }
    let old = p.doc().console.parallels.clone();
    if old
        .iter()
        .any(|(parent, child)| parent == key || child == key)
    {
        let new = old
            .iter()
            .filter(|(parent, child)| parent != key && child != key)
            .cloned()
            .collect();
        p.push(Edit::ConsoleParallels { old, new });
    }
    Ok(())
}

/// Makes the follow link of `strip` match its routing (see the module docs): a strip that plays
/// into a bus follows it, any other strip follows nothing. Does nothing while
/// `Console::send_follow` is off.
pub(crate) fn sync_follow(p: &mut Planner<'_>, strip: &DeviceKey) -> Result<(), ModelError> {
    if !p.doc().console.send_follow || !is_strip(p.doc(), strip) {
        return Ok(());
    }
    let has_input = rules::descriptor(p.registry, p.type_of(strip)?)?
        .socket_by_key(FOLLOW_IN)
        .is_some();
    if !has_input {
        return Ok(());
    }
    let wanted = match output_dest(p.doc(), strip) {
        OutputDest::Bus(bus) => Some(wire(at(&bus, FOLLOW_OUT), at(strip, FOLLOW_IN))),
        _ => None,
    };
    let target = at(strip, FOLLOW_IN);
    p.disconnect_where(|c| c.to == target && wanted.as_ref() != Some(c));
    if let Some(cable) = wanted {
        p.connect(cable)?;
    }
    Ok(())
}

/// Plans `SetSendFollow(on)`: the flag, and the links of every strip (made on, removed off).
pub(crate) fn set_send_follow(p: &mut Planner<'_>, on: bool) -> Result<(), ModelError> {
    let old = p.doc().console.send_follow;
    if old == on {
        return Ok(());
    }
    if !on {
        let doc = p.doc();
        let links: Vec<crate::Cable> = doc
            .cables
            .iter()
            .filter(|c| {
                crate::console::link_kind(doc, c) == Some(crate::console::LinkKind::FollowLink)
            })
            .cloned()
            .collect();
        for cable in links {
            p.push(Edit::Disconnect(cable));
        }
    }
    p.push(Edit::SendFollow { old, new: on });
    if on {
        let strips: Vec<DeviceKey> = p
            .doc()
            .console
            .channels
            .iter()
            .map(|c| c.channel.clone())
            .chain(p.doc().console.buses.iter().map(|b| b.bus.clone()))
            .collect();
        for strip in strips {
            sync_follow(p, &strip)?;
        }
    }
    Ok(())
}
