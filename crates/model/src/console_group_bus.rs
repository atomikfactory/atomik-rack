//! Routing a mixer group to an output bus.
//!
//! A VCA group can play its members into a bus ([`VcaGroup::bus`]): the outputs of its member
//! strips (channels and buses) are on the bus, so the bus fader is the sum's level and the bus
//! carries the group's sum through its own inserts. The group itself stays what it was (a master
//! fader, mute and solo over the members, with no audio of its own); the routing is ordinary cables
//! (see `console_bus.rs`), and the group only remembers which bus it feeds and which strips it has
//! put there ([`VcaGroup::routed`]).
//!
//! **Following.** After every command the model brings the route in line with the group in the
//! same undo step ([`sync`]): a strip that joins the group gets its output on the bus (unless that
//! would make a loop, or it is the bus itself, and then it stays where it is); a strip that leaves
//! the group, or is removed, goes back to the master if it still plays into the bus; a route whose
//! bus is deleted is forgotten, and the strips stay as they are. A strip the route already owns is
//! never moved again, so an output a user chose by hand for one member stays.
//!
//! One bus per group; several groups may share a bus. The commands are `RouteVcaToBus`,
//! `RouteVcaToNewBus` and `UnrouteVca`, each one undo step on one [`Edit::ConsoleVcas`] plus the
//! cable edits.

use crate::console::{ConsoleCommand, OutputDest, output_dest};
use crate::console_bus::{check_dest, create_bus, set_output};
use crate::console_cmd::{Planner, bad};
use crate::console_vca::VcaGroup;
use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// The members of `group` a route to `bus` would own now: those it owns already and those that can
/// go on the bus (not the bus itself, no loop), in console order.
fn wanted_members(doc: &RackDoc, group: &VcaGroup, bus: &DeviceKey) -> Vec<DeviceKey> {
    group
        .members
        .iter()
        .filter(|m| {
            group.routed.contains(m) || check_dest(doc, m, &OutputDest::Bus(bus.clone())).is_ok()
        })
        .cloned()
        .collect()
}

impl RackDoc {
    /// The strips a route of group `vca` to `bus` would own: its members that can play into the
    /// bus (what Route Group to Bus puts there). Empty for a group that does not exist.
    pub fn group_route_strips(&self, vca: u32, bus: &DeviceKey) -> Vec<DeviceKey> {
        self.console
            .vca(vca)
            .map(|g| wanted_members(self, g, bus))
            .unwrap_or_default()
    }
}

fn edit_groups(p: &mut Planner<'_>, change: impl FnOnce(&mut Vec<VcaGroup>)) {
    let old = p.doc().console.vcas.clone();
    let mut new = old.clone();
    change(&mut new);
    if new != old {
        p.push(Edit::ConsoleVcas { old, new });
    }
}

/// Plans one of the three group routing commands.
pub(crate) fn plan(p: &mut Planner<'_>, command: ConsoleCommand) -> Result<(), ModelError> {
    match command {
        ConsoleCommand::RouteVcaToBus { vca, bus } => route(p, vca, &bus),
        ConsoleCommand::RouteVcaToNewBus { vca, name } => {
            let Some(group) = p.doc().console.vca(vca) else {
                return Err(bad(format!("there is no VCA group {vca}")));
            };
            if group.members.is_empty() {
                return Err(bad(format!(
                    "the VCA \"{}\" has no member to route",
                    group.name
                )));
            }
            let bus = create_bus(p, &name, &OutputDest::Master)?;
            route(p, vca, &bus)
        }
        ConsoleCommand::UnrouteVca { vca } => unroute(p, vca),
        _ => Err(bad("not a group routing command")),
    }
}

/// Puts the members of group `vca` on `bus` and records the route (replacing the group's route).
fn route(p: &mut Planner<'_>, vca: u32, bus: &DeviceKey) -> Result<(), ModelError> {
    let Some(group) = p.doc().console.vca(vca).cloned() else {
        return Err(bad(format!("there is no VCA group {vca}")));
    };
    if p.doc().console.bus_index(bus).is_none() {
        return Err(bad(format!("{bus} is not a console bus")));
    }
    let strips = wanted_members(p.doc(), &group, bus);
    if strips.is_empty() {
        return Err(bad(format!(
            "the VCA \"{}\" has no member that can play into {bus}",
            group.name
        )));
    }
    if group.bus.as_ref() == Some(bus)
        && group.routed == strips
        && strips
            .iter()
            .all(|s| output_dest(p.doc(), s) == OutputDest::Bus(bus.clone()))
    {
        return Err(bad(format!(
            "the VCA \"{}\" already plays into {bus}",
            group.name
        )));
    }
    // A move to another bus lets go of the strips of the old one first.
    if let Some(old) = group.bus.as_ref().filter(|b| *b != bus) {
        back_to_master(p, old, &group.routed)?;
    }
    for s in &strips {
        set_output(p, s, &OutputDest::Bus(bus.clone()))?;
    }
    edit_groups(p, |groups| {
        if let Some(g) = groups.iter_mut().find(|g| g.id == vca) {
            g.bus = Some(bus.clone());
            g.routed = strips;
        }
    });
    Ok(())
}

/// Takes group `vca` off its bus: the strips that still play into the bus go back to the master and
/// the route is forgotten (the bus stays).
fn unroute(p: &mut Planner<'_>, vca: u32) -> Result<(), ModelError> {
    let Some(group) = p.doc().console.vca(vca).cloned() else {
        return Err(bad(format!("there is no VCA group {vca}")));
    };
    let Some(bus) = group.bus else {
        return Err(bad(format!(
            "the VCA \"{}\" is not routed to a bus",
            group.name
        )));
    };
    back_to_master(p, &bus, &group.routed)?;
    edit_groups(p, |groups| {
        if let Some(g) = groups.iter_mut().find(|g| g.id == vca) {
            g.bus = None;
            g.routed.clear();
        }
    });
    Ok(())
}

/// Where a strip goes when its group lets go of it: the bus of the routed folder that owns it,
/// else the master (what the strip played into before the route is not kept in the project).
fn home_dest(doc: &RackDoc, strip: &DeviceKey, leaving: &DeviceKey) -> OutputDest {
    doc.console
        .folder_routes
        .iter()
        .find(|r| r.members.contains(strip) && r.bus != *leaving)
        .map(|r| OutputDest::Bus(r.bus.clone()))
        .filter(|d| check_dest(doc, strip, d).is_ok())
        .unwrap_or(OutputDest::Master)
}

/// Routes the `strips` that play into `bus` back where they belong, the bus of their folder or
/// the master (a strip that is gone is skipped).
fn back_to_master(
    p: &mut Planner<'_>,
    bus: &DeviceKey,
    strips: &[DeviceKey],
) -> Result<(), ModelError> {
    if p.doc().console.master.is_none() {
        return Ok(());
    }
    for s in strips {
        let console = &p.doc().console;
        if console.channel_index(s).is_none() && console.bus_index(s).is_none() {
            continue;
        }
        if output_dest(p.doc(), s) == OutputDest::Bus(bus.clone()) {
            let home = home_dest(p.doc(), s, bus);
            set_output(p, s, &home)?;
        }
    }
    Ok(())
}

/// The edit that brings every group route in line with the groups and the console, or none when
/// they are (the usual case: no route, or nothing moved). Applied by the model right after any
/// command, in the same undo step. A failure to route a strip leaves everything as it was.
pub(crate) fn sync(doc: &RackDoc, registry: &Registry) -> Option<Edit> {
    if doc.console.vcas.iter().all(|g| g.bus.is_none()) {
        return None;
    }
    // Cheap pass first: does any route differ from its group?
    let steady = doc.console.vcas.iter().all(|g| {
        let Some(bus) = &g.bus else {
            return g.routed.is_empty();
        };
        if doc.console.bus_index(bus).is_none() {
            return false;
        }
        let want = wanted_members(doc, g, bus);
        want == g.routed && g.routed.iter().all(|r| g.members.contains(r))
    });
    if steady {
        return None;
    }
    let mut p = Planner::new(doc, registry);
    let old = doc.console.vcas.clone();
    let mut new = old.clone();
    for g in &mut new {
        let Some(bus) = g.bus.clone() else {
            g.routed.clear();
            continue;
        };
        if doc.console.bus_index(&bus).is_none() {
            // The bus is gone: the route is forgotten, the strips stay as they are.
            g.bus = None;
            g.routed.clear();
            continue;
        }
        let gone: Vec<DeviceKey> = g
            .routed
            .iter()
            .filter(|r| !g.members.contains(r))
            .cloned()
            .collect();
        if back_to_master(&mut p, &bus, &gone).is_err() {
            return None;
        }
        let mut routed: Vec<DeviceKey> = g
            .routed
            .iter()
            .filter(|r| g.members.contains(r))
            .cloned()
            .collect();
        for m in &g.members {
            if routed.contains(m) {
                continue;
            }
            let dest = OutputDest::Bus(bus.clone());
            // A strip that cannot go on the bus (it is the bus, or it would loop) stays where it is.
            if *m == bus || check_dest(p.doc(), m, &dest).is_err() {
                continue;
            }
            if set_output(&mut p, m, &dest).is_err() {
                return None;
            }
            routed.push(m.clone());
        }
        let order: Vec<&DeviceKey> = doc.console.strip_keys().collect();
        routed.sort_by_key(|m| order.iter().position(|k| k == &m).unwrap_or(usize::MAX));
        g.routed = routed;
    }
    if new != old {
        // Pushed after the cable edits so the undo order is the plan's.
        p.push(Edit::ConsoleVcas { old, new });
    }
    Some(p.finish())
}
