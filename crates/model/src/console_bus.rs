//! Output buses: the bus commands (`CreateBus`, `RenameBus`, `DeleteBus`, `ReorderBus`,
//! `SetOutput`, `RouteToNewBus`), the routing queries they share and the solo rule.
//!
//! A bus is a console strip (`atomik.bus`) whose summing input `bus_l`/`bus_r` is fed by the
//! `out_l`/`out_r` cables of channels and other buses. Everything is cables; the console only
//! records the bus list and names. A strip's `out_l`/`out_r` go to exactly one destination
//! ([`OutputDest`]): the master (`bus_l`/`bus_r`), a bus (`bus_l`/`bus_r`), or an Audio I/O
//! stereo pair (`interface:out_{2n-1}` and `out_{2n}`, which take one cable each, so a pair has
//! one source). Every strip's sends go to the master's send buses whatever its output.
//!
//! Every command is planned on a scratch copy, so each cable is validated against the state it
//! joins (the engine's cycle rule included), and the result is one undo step.

use crate::console::{
    BUS_TYPE, Console, ConsoleBus, LinkKind, MASTER_TYPE, MAX_BUS_NAME_CHARS, OutputDest, is_bus,
    link_kind, output_dest, sockets,
};
use crate::console_cmd::{Planner, at, bad, ensure_master, wire, wire_master_to_interface};
use crate::history::Edit;
use crate::{Cable, DeviceKey, INTERFACE_KEY, ModelError, RackDoc, SocketRef, rules};
use atomik_device_sdk::Registry;
use std::collections::BTreeSet;

/// Validates a bus name for the bus `except` (none for a new bus) and returns it trimmed.
fn checked_name(
    console: &Console,
    name: &str,
    except: Option<&DeviceKey>,
) -> Result<String, ModelError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad("a bus name cannot be empty"));
    }
    if name.chars().count() > MAX_BUS_NAME_CHARS {
        return Err(bad(format!(
            "a bus name has at most {MAX_BUS_NAME_CHARS} characters"
        )));
    }
    if let Some(other) = console.bus_by_name(name).filter(|b| except != Some(&b.bus)) {
        return Err(bad(format!(
            "a bus is already called \"{}\"; bus names are unique",
            other.name
        )));
    }
    Ok(name.to_owned())
}

/// The bus a strip's output goes to, if its output is routed to one.
fn parent_of(doc: &RackDoc, strip: &DeviceKey) -> Option<DeviceKey> {
    match output_dest(doc, strip) {
        OutputDest::Bus(b) => Some(b),
        _ => None,
    }
}

/// True if `from` reaches `to` by following bus outputs (`from == to` counts).
fn reaches(doc: &RackDoc, from: &DeviceKey, to: &DeviceKey) -> bool {
    let mut seen = BTreeSet::new();
    let mut at = Some(from.clone());
    while let Some(k) = at {
        if &k == to {
            return true;
        }
        if !seen.insert(k.clone()) {
            return false;
        }
        at = parent_of(doc, &k);
    }
    false
}

fn interface() -> DeviceKey {
    DeviceKey(INTERFACE_KEY.to_owned())
}

/// The two interface outputs of Audio I/O pair `n`.
fn pair_outputs(n: u16) -> [String; 2] {
    [format!("out_{}", 2 * n - 1), format!("out_{}", 2 * n)]
}

/// The two cables `strip:out_l/out_r -> dest` for a validated destination.
fn out_cables(
    doc: &RackDoc,
    strip: &DeviceKey,
    dest: &OutputDest,
) -> Result<[Cable; 2], ModelError> {
    match dest {
        OutputDest::Master => {
            let master = doc
                .console
                .master
                .clone()
                .ok_or_else(|| bad("the console has no master"))?;
            Ok(into_bus(strip, &master))
        }
        OutputDest::Bus(b) => Ok(into_bus(strip, b)),
        OutputDest::AudioIo(n) => {
            let [l, r] = pair_outputs(*n);
            Ok([
                wire(at(strip, "out_l"), at(&interface(), l)),
                wire(at(strip, "out_r"), at(&interface(), r)),
            ])
        }
        other => Err(bad(format!(
            "{other:?} is not a destination: choose the master, a bus or an Audio I/O pair"
        ))),
    }
}

fn into_bus(strip: &DeviceKey, target: &DeviceKey) -> [Cable; 2] {
    [
        wire(at(strip, "out_l"), at(target, "bus_l")),
        wire(at(strip, "out_r"), at(target, "bus_r")),
    ]
}

/// Rejects a destination `strip` cannot be routed to.
fn check_dest(doc: &RackDoc, strip: &DeviceKey, dest: &OutputDest) -> Result<(), ModelError> {
    match dest {
        OutputDest::Master => {
            if doc.console.master.is_none() {
                return Err(bad("the console has no master"));
            }
        }
        OutputDest::Bus(b) => {
            if doc.console.bus_index(b).is_none() {
                return Err(bad(format!("{b} is not a console bus")));
            }
            if b == strip {
                return Err(bad(format!("{strip} cannot be routed to itself")));
            }
            if reaches(doc, b, strip) {
                return Err(bad(format!(
                    "routing {strip} to {b} would make a loop: {b} already plays into {strip}"
                )));
            }
        }
        OutputDest::AudioIo(n) => {
            let channels = u32::from(doc.output_channels);
            if *n == 0 {
                return Err(ModelError::OutOfRange(
                    "Audio I/O pairs are numbered from 1 (1 is outputs 1+2)".into(),
                ));
            }
            if u32::from(*n) * 2 > channels {
                return Err(ModelError::OutOfRange(format!(
                    "Audio I/O pair {n} (outputs {}+{}) needs more than the {channels} output channels of this project",
                    u32::from(*n) * 2 - 1,
                    u32::from(*n) * 2
                )));
            }
            for socket in pair_outputs(*n) {
                let to = at(&interface(), socket);
                if let Some(c) = doc.cable_into(&to).filter(|c| &c.from.device != strip) {
                    return Err(bad(format!(
                        "Audio I/O {to} is already fed by {}; an output takes one cable",
                        c.from
                    )));
                }
            }
        }
        other => {
            return Err(bad(format!(
                "{other:?} is not a destination: choose the master, a bus or an Audio I/O pair"
            )));
        }
    }
    Ok(())
}

/// Why a destination is or is not offered to a strip's output selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputAvail {
    /// `SetOutput` accepts it.
    Available,
    /// A bus that plays into the strip (routing there would make a loop).
    Loop,
    /// An Audio I/O pair already fed by another source (pair 1 belongs to the master).
    InUse,
    /// An Audio I/O pair beyond the project's `output_channels`.
    BeyondOutputs,
}

/// One entry of a strip's output selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputOption {
    /// The destination (`Master`, `Bus` or `AudioIo`).
    pub dest: OutputDest,
    /// Whether `SetOutput` takes it, or why not.
    pub avail: OutputAvail,
}

/// Every destination `strip` could be routed to, in selector order: the master (when there is
/// one), each bus in console order except the strip itself, then all Audio I/O pairs up to the
/// format maximum. [`OutputAvail::Available`] holds exactly when `SetOutput` would accept the
/// destination, so a UI can grey out the rest with the reason. Pure.
pub fn output_options(doc: &RackDoc, strip: &DeviceKey) -> Vec<OutputOption> {
    let mut out = Vec::new();
    if doc.console.master.is_some() {
        out.push(OutputOption {
            dest: OutputDest::Master,
            avail: OutputAvail::Available,
        });
    }
    for b in &doc.console.buses {
        if &b.bus == strip {
            continue;
        }
        let avail = if reaches(doc, &b.bus, strip) {
            OutputAvail::Loop
        } else {
            OutputAvail::Available
        };
        out.push(OutputOption {
            dest: OutputDest::Bus(b.bus.clone()),
            avail,
        });
    }
    for n in 1..=crate::MAX_OUTPUT_CHANNELS / 2 {
        let dest = OutputDest::AudioIo(n);
        let avail = match check_dest(doc, strip, &dest) {
            Ok(()) => OutputAvail::Available,
            Err(ModelError::OutOfRange(_)) => OutputAvail::BeyondOutputs,
            Err(_) => OutputAvail::InUse,
        };
        out.push(OutputOption { dest, avail });
    }
    out
}

/// Plans `SetOutput { strip, dest }`.
pub(crate) fn set_output(
    p: &mut Planner<'_>,
    strip: &DeviceKey,
    dest: &OutputDest,
) -> Result<(), ModelError> {
    let console = &p.doc().console;
    if console.channel_index(strip).is_none() && console.bus_index(strip).is_none() {
        return Err(bad(format!("{strip} is not a console channel or bus")));
    }
    check_dest(p.doc(), strip, dest)?;
    let wanted = out_cables(p.doc(), strip, dest)?;
    let strip = strip.clone();
    p.disconnect_where(|c| {
        c.from.device == strip
            && matches!(c.from.socket.as_str(), "out_l" | "out_r")
            && !wanted.contains(c)
    });
    for cable in wanted {
        p.connect(cable)?;
    }
    Ok(())
}

/// The rack index a new bus goes to: after the last bus device, else the end of the rack.
fn bus_insert_index(doc: &RackDoc) -> usize {
    doc.console
        .buses
        .iter()
        .filter_map(|b| rules::index_of(doc, &b.bus).ok())
        .max()
        .map_or(doc.devices.len(), |i| i + 1)
}

/// Plans `CreateBus` and returns the new bus key.
pub(crate) fn create_bus(
    p: &mut Planner<'_>,
    name: &str,
    dest: &OutputDest,
) -> Result<DeviceKey, ModelError> {
    rules::descriptor(p.registry, BUS_TYPE)?;
    rules::descriptor(p.registry, MASTER_TYPE)?;
    let name = checked_name(&p.doc().console, name, None)?;
    let had_master = p.doc().console.master.is_some();
    let master = ensure_master(p)?;
    if !had_master {
        wire_master_to_interface(p, &master)?;
    }
    let index = bus_insert_index(p.doc());
    let bus = p.insert_device(index, BUS_TYPE)?;
    let old = rules::device(p.doc(), &bus)?.name.clone();
    p.push(Edit::Name {
        key: bus.clone(),
        old,
        new: name.clone(),
    });
    // Like a channel made by "Create instrument", the strip starts folded in the rack.
    p.push(Edit::Folded {
        key: bus.clone(),
        old: false,
        new: true,
    });
    for k in sockets::sends() {
        for side in ["l", "r"] {
            p.connect(wire(
                at(&bus, sockets::channel_send(k, side)),
                at(&master, sockets::master_send_bus(k, side)),
            ))?;
        }
    }
    let at_index = p.doc().console.buses.len();
    p.push(Edit::ConsoleBus {
        index: at_index,
        old: None,
        new: Some(ConsoleBus {
            bus: bus.clone(),
            name,
        }),
    });
    set_output(p, &bus, dest)?;
    Ok(bus)
}

/// Plans `RenameBus` (also the generic rename of a bus device).
pub(crate) fn rename_bus(
    p: &mut Planner<'_>,
    bus: &DeviceKey,
    name: &str,
) -> Result<(), ModelError> {
    let index = p
        .doc()
        .console
        .bus_index(bus)
        .ok_or_else(|| bad(format!("{bus} is not a console bus")))?;
    let name = checked_name(&p.doc().console, name, Some(bus))?;
    let old = p.doc().console.buses[index].clone();
    if old.name != name {
        p.push(Edit::ConsoleBus {
            index,
            old: Some(old),
            new: Some(ConsoleBus {
                bus: bus.clone(),
                name: name.clone(),
            }),
        });
    }
    let device_name = rules::device(p.doc(), bus)?.name.clone();
    if device_name != name {
        p.push(Edit::Name {
            key: bus.clone(),
            old: device_name,
            new: name,
        });
    }
    Ok(())
}

/// Plans `ReorderBus`.
pub(crate) fn reorder_bus(
    p: &mut Planner<'_>,
    bus: &DeviceKey,
    to: usize,
) -> Result<(), ModelError> {
    let from = p
        .doc()
        .console
        .bus_index(bus)
        .ok_or_else(|| bad(format!("{bus} is not a console bus")))?;
    let to = to.min(p.doc().console.buses.len() - 1);
    if from != to {
        p.push(Edit::ConsoleBusMove { from, to });
    }
    Ok(())
}

/// Plans `DeleteBus`, which is also what removing a bus device does.
pub(crate) fn delete_bus(p: &mut Planner<'_>, bus: &DeviceKey) -> Result<(), ModelError> {
    let index = p
        .doc()
        .console
        .bus_index(bus)
        .ok_or_else(|| bad(format!("{bus} is not a console bus")))?;
    let doc = p.doc();
    // The cables the feeders have into the bus, with the side they carry.
    let feeds: Vec<(Cable, &'static str)> = doc
        .cables
        .iter()
        .filter(|c| {
            &c.to.device == bus
                && matches!(
                    link_kind(doc, c),
                    Some(LinkKind::ChannelToBus | LinkKind::BusToBus)
                )
        })
        .map(|c| (c.clone(), if c.to.socket == "bus_l" { "l" } else { "r" }))
        .collect();
    let feeders: BTreeSet<&DeviceKey> = feeds.iter().map(|(c, _)| &c.from.device).collect();
    let target = match output_dest(doc, bus) {
        OutputDest::Bus(b) => OutputDest::Bus(b),
        // An Audio I/O pair takes one cable per output: only a single feeder can inherit it.
        OutputDest::AudioIo(n) if feeders.len() == 1 => OutputDest::AudioIo(n),
        _ => OutputDest::Master,
    };
    let old = doc.console.buses[index].clone();
    p.push(Edit::ConsoleBus {
        index,
        old: Some(old),
        new: None,
    });
    p.remove_device(bus)?;
    for (cable, side) in feeds {
        let to: SocketRef = match &target {
            OutputDest::Bus(b) => at(b, format!("bus_{side}")),
            OutputDest::AudioIo(n) => {
                let [l, r] = pair_outputs(*n);
                at(&interface(), if side == "l" { l } else { r })
            }
            _ => {
                let master = p
                    .doc()
                    .console
                    .master
                    .clone()
                    .ok_or_else(|| bad("the console has no master"))?;
                at(&master, format!("bus_{side}"))
            }
        };
        p.connect(wire(cable.from, to))?;
    }
    Ok(())
}

/// Plans `RouteToNewBus`.
pub(crate) fn route_to_new_bus(
    p: &mut Planner<'_>,
    strips: &[DeviceKey],
    name: &str,
) -> Result<(), ModelError> {
    if strips.is_empty() {
        return Err(bad("no strips to route to a new bus"));
    }
    let mut seen = BTreeSet::new();
    for s in strips {
        let console = &p.doc().console;
        if console.channel_index(s).is_none() && console.bus_index(s).is_none() {
            return Err(bad(format!("{s} is not a console channel or bus")));
        }
        if !seen.insert(s) {
            return Err(bad(format!("{s} is listed twice")));
        }
    }
    let bus = create_bus(p, name, &OutputDest::Master)?;
    for s in strips {
        set_output(p, s, &OutputDest::Bus(bus.clone()))?;
    }
    Ok(())
}

/// The derived `internal.solo_silence` state of every console strip: channels in console order,
/// then buses. Additive solo-in-place with these audibility rules: when no strip is soloed
/// nothing is silenced; otherwise a strip stays audible when it is soloed, when it is on the
/// path of a soloed strip to the master or an Audio I/O pair (the buses it plays into, directly
/// or not), or when it feeds a soloed bus (directly or not). Every other strip is silenced.
/// Sibling feeders of a bus that is only on the path of a soloed channel stay silenced.
pub(crate) fn solo_silence(doc: &RackDoc, registry: &Registry) -> Vec<(DeviceKey, bool)> {
    let strips: Vec<DeviceKey> = doc
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .chain(doc.console.buses.iter().map(|b| b.bus.clone()))
        .collect();
    let soloed: Vec<&DeviceKey> = strips
        .iter()
        .filter(|k| {
            doc.param_value(k, "solo", registry)
                .is_some_and(|v| v >= 0.5)
        })
        .collect();
    let mut audible: BTreeSet<DeviceKey> = BTreeSet::new();
    for s in &soloed {
        // Downstream: the buses on the path to the master / Audio I/O.
        let mut at = Some((*s).clone());
        let mut walked = BTreeSet::new();
        while let Some(k) = at {
            if !walked.insert(k.clone()) {
                break;
            }
            audible.insert(k.clone());
            at = parent_of(doc, &k);
        }
        // Upstream: everything that feeds a soloed bus.
        if is_bus(doc, s) {
            let mut stack = vec![(*s).clone()];
            let mut seen = BTreeSet::new();
            while let Some(b) = stack.pop() {
                if !seen.insert(b.clone()) {
                    continue;
                }
                for f in &strips {
                    if parent_of(doc, f).as_ref() == Some(&b) {
                        audible.insert(f.clone());
                        stack.push(f.clone());
                    }
                }
            }
        }
    }
    let any = !soloed.is_empty();
    strips
        .into_iter()
        .map(|k| {
            let silent = any && !audible.contains(&k);
            (k, silent)
        })
        .collect()
}
