//! Routing a track folder to an output bus.
//!
//! A folder can play into a bus ([`FolderRoute`], `Console::folder_routes`): the console channels
//! of its tracks (sub-folders included) have their output on the bus, so the bus fader is the
//! folder's level and the bus carries the folder's sum through its own inserts. The routing
//! itself is ordinary cables (see `console_bus.rs`); the route only remembers which folder feeds
//! which bus and which strips it has put there.
//!
//! **Following.** After every command the model brings the routes in line with the song in the
//! same undo step ([`sync`]): a track that joins the folder gets its output on the bus; a track
//! that leaves the folder goes back to the master if it still plays into the bus; a route whose
//! folder is dissolved (or empty) or whose bus is deleted is forgotten, and the strips stay as
//! they are. A strip the route already owns is never moved again, so an output a user chose by
//! hand for one channel of the folder stays.
//!
//! **Nesting.** Where a routed folder holds another routed folder, the tracks of the inner folder
//! belong to the inner route; the outer route takes the rest.
//!
//! One route per folder; several folders may share a bus. The commands are
//! `RouteFolderToBus`, `RouteFolderToNewBus` and `UnrouteFolder`, each one undo step on one
//! [`Edit::ConsoleFolderRoutes`] plus the cable edits.
//!
//! **Stop routing.** `UnrouteFolder` also removes the bus Route Folder to New Bus made for the
//! folder (see [`RackDoc::folder_bus_name`]) when nothing else uses it any more: no strip plays
//! into it, no other route or group points at it, and it has no inserts, no parallel channels,
//! no setting of its own and still plays into the master.

use crate::console::{ConsoleCommand, OutputDest, output_dest};
use crate::console_bus::{check_dest, create_bus, set_output};
use crate::console_cmd::{Planner, bad};
use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// The most folder routes a console keeps.
pub const MAX_FOLDER_ROUTES: usize = 64;

/// One track folder that plays into a bus.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderRoute {
    /// The id of the track folder (from 1).
    pub folder: u32,
    /// The bus (`atomik.bus` device key) the folder's channels play into.
    pub bus: DeviceKey,
    /// The channels the route has put on the bus, in console order. A channel of the folder that
    /// is not listed is routed at the next command; one listed keeps whatever output it has.
    pub members: Vec<DeviceKey>,
}

impl crate::Console {
    /// The route of the folder `folder`, if it has one.
    pub fn folder_route(&self, folder: u32) -> Option<&FolderRoute> {
        self.folder_routes.iter().find(|r| r.folder == folder)
    }
}

impl RackDoc {
    /// The console channels `route` owns now: the channels of the folder's tracks, without those
    /// of a routed folder nested inside it, in console order.
    fn route_strips(&self, routes: &[FolderRoute], folder: u32) -> Vec<DeviceKey> {
        let mine = self.song.group_tracks(folder);
        let inner: Vec<DeviceKey> = routes
            .iter()
            .filter(|r| r.folder != folder)
            .filter(|r| {
                let theirs = self.song.group_tracks(r.folder);
                !theirs.is_empty() && theirs.iter().all(|t| mine.contains(t))
            })
            .flat_map(|r| self.folder_strips(r.folder))
            .collect();
        self.folder_strips(folder)
            .into_iter()
            .filter(|k| !inner.contains(k))
            .collect()
    }

    /// The strips a folder route would own for `folder` (its route, or the folder alone when it
    /// has none): what Route Folder to Bus puts on the bus.
    pub fn folder_route_strips(&self, folder: u32) -> Vec<DeviceKey> {
        self.route_strips(&self.console.folder_routes, folder)
    }
}

/// Longest stem of the name of a folder's bus: the bus name limit less room for a ` 99` suffix.
const BUS_STEM_CHARS: usize = crate::console::MAX_BUS_NAME_CHARS - 3;

/// `<folder name> Bus`, cut to [`BUS_STEM_CHARS`] characters.
fn bus_stem(folder_name: &str) -> String {
    format!("{} Bus", folder_name.trim())
        .chars()
        .take(BUS_STEM_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

impl RackDoc {
    /// The name Route Folder to New Bus gives the bus of `folder`: `<folder name> Bus`, with
    /// ` 2`, ` 3`, ... when a bus has that name already; `Bus n` for a folder with no name.
    pub fn folder_bus_name(&self, folder: u32) -> String {
        let name = self
            .song
            .org
            .group(folder)
            .map(|g| g.name.trim().to_owned())
            .filter(|n| !n.is_empty());
        let Some(name) = name else {
            return self.console.default_bus_name();
        };
        let stem = bus_stem(&name);
        std::iter::once(stem.clone())
            .chain((2usize..).map(|n| format!("{stem} {n}")))
            .find(|n| self.console.bus_by_name(n).is_none())
            .expect("a free name exists")
    }

    /// True when the bus called `bus_name` is one Route Folder to New Bus could have made for
    /// `folder` (its name, with or without the number, or just the folder's name as older
    /// projects have it).
    fn is_folder_bus_name(&self, folder: u32, bus_name: &str) -> bool {
        let Some(group) = self.song.org.group(folder) else {
            return false;
        };
        let name = group.name.trim();
        let stem = bus_stem(name);
        // Bus names are unique ignoring case, so they are compared that way.
        let (bus_name, stem, name) = (
            bus_name.to_lowercase(),
            stem.to_lowercase(),
            name.to_lowercase(),
        );
        let numbered = bus_name == stem
            || bus_name
                .strip_prefix(&stem)
                .and_then(|r| r.strip_prefix(' '))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        !name.is_empty() && (numbered || bus_name == name)
    }

    /// True when nothing but the folder routes in `routes` could miss the bus `bus`: no strip
    /// plays into it, no group is routed to it, and it holds no insert, no parallel channel, no
    /// setting of its own and plays into the master.
    fn bus_is_idle(&self, bus: &DeviceKey, routes: &[FolderRoute], registry: &Registry) -> bool {
        let console = &self.console;
        let fed = console
            .strip_keys()
            .any(|k| k != bus && output_dest(self, k) == OutputDest::Bus(bus.clone()));
        !fed && routes.iter().all(|r| r.bus != *bus)
            && console.vcas.iter().all(|v| v.bus.as_ref() != Some(bus))
            && console.parallels_of(bus).next().is_none()
            && crate::console_insert::insert_chain(self, registry, bus).is_empty()
            && output_dest(self, bus) == OutputDest::Master
            && self.device(bus).is_some_and(|d| d.params.is_empty())
    }
}

fn edit_routes(p: &mut Planner<'_>, new: Vec<FolderRoute>) {
    let old = p.doc().console.folder_routes.clone();
    if old != new {
        p.push(Edit::ConsoleFolderRoutes { old, new });
    }
}

/// Plans one of the three folder commands.
pub(crate) fn plan(p: &mut Planner<'_>, command: ConsoleCommand) -> Result<(), ModelError> {
    match command {
        ConsoleCommand::RouteFolderToBus { folder, bus } => route(p, folder, &bus),
        ConsoleCommand::RouteFolderToNewBus { folder, name } => {
            if p.doc().folder_route_strips(folder).is_empty() {
                return Err(bad(format!(
                    "folder {folder} has no track with a channel to route"
                )));
            }
            let bus = create_bus(p, &name, &OutputDest::Master)?;
            route(p, folder, &bus)
        }
        ConsoleCommand::UnrouteFolder { folder } => unroute(p, folder),
        _ => Err(bad("not a folder routing command")),
    }
}

/// Puts the strips of `folder` on `bus` and records the route (replacing the folder's route).
fn route(p: &mut Planner<'_>, folder: u32, bus: &DeviceKey) -> Result<(), ModelError> {
    if p.doc().song.group_tracks(folder).is_empty() {
        return Err(bad(format!("{folder} is not a track folder with tracks")));
    }
    if p.doc().console.bus_index(bus).is_none() {
        return Err(bad(format!("{bus} is not a console bus")));
    }
    let routes = p.doc().console.folder_routes.clone();
    let existing = routes.iter().position(|r| r.folder == folder);
    if existing.is_none() && routes.len() >= MAX_FOLDER_ROUTES {
        return Err(bad(format!(
            "at most {MAX_FOLDER_ROUTES} folders can be routed to a bus"
        )));
    }
    let strips = p.doc().route_strips(&routes, folder);
    if strips.is_empty() {
        return Err(bad(format!(
            "folder {folder} has no track with a channel to route"
        )));
    }
    if let Some(i) = existing
        && routes[i].bus == *bus
        && routes[i].members == strips
        && strips
            .iter()
            .all(|s| output_dest(p.doc(), s) == OutputDest::Bus(bus.clone()))
    {
        return Err(bad(format!("folder {folder} already plays into {bus}")));
    }
    for s in &strips {
        set_output(p, s, &OutputDest::Bus(bus.clone()))?;
    }
    let mut new = routes;
    let entry = FolderRoute {
        folder,
        bus: bus.clone(),
        members: strips,
    };
    match existing {
        Some(i) => new[i] = entry,
        None => new.push(entry),
    }
    edit_routes(p, new);
    Ok(())
}

/// Takes `folder` off its bus: the strips that still play into the bus go back to the master and
/// the route is forgotten; the bus goes too when it is the folder's own and idle.
fn unroute(p: &mut Planner<'_>, folder: u32) -> Result<(), ModelError> {
    let Some(r) = p.doc().console.folder_route(folder).cloned() else {
        return Err(bad(format!("folder {folder} is not routed to a bus")));
    };
    back_to_master(p, &r.bus, &r.members)?;
    let new: Vec<FolderRoute> = p
        .doc()
        .console
        .folder_routes
        .iter()
        .filter(|x| x.folder != folder)
        .cloned()
        .collect();
    edit_routes(p, new.clone());
    // The bus Route Folder to New Bus made goes with the route when nothing else uses it.
    let name = {
        let console = &p.doc().console;
        console
            .bus_index(&r.bus)
            .map(|i| console.buses[i].name.clone())
    };
    if name.is_some_and(|n| p.doc().is_folder_bus_name(folder, &n))
        && p.doc().bus_is_idle(&r.bus, &new, p.registry)
    {
        crate::console_bus::delete_bus(p, &r.bus)?;
    }
    Ok(())
}

/// Routes the `strips` that play into `bus` to the master.
fn back_to_master(
    p: &mut Planner<'_>,
    bus: &DeviceKey,
    strips: &[DeviceKey],
) -> Result<(), ModelError> {
    if p.doc().console.master.is_none() {
        return Ok(());
    }
    for s in strips {
        if output_dest(p.doc(), s) == OutputDest::Bus(bus.clone()) {
            set_output(p, s, &OutputDest::Master)?;
        }
    }
    Ok(())
}

/// The edit that brings every folder route in line with the song and the console, or none when
/// they are (the usual case: no route, or nothing moved). Applied by the model right after any
/// command, in the same undo step. A failure to route a strip leaves everything as it was.
pub(crate) fn sync(doc: &RackDoc, registry: &Registry) -> Option<Edit> {
    if doc.console.folder_routes.is_empty() {
        return None;
    }
    let old = doc.console.folder_routes.clone();
    let live: Vec<FolderRoute> = old
        .iter()
        .filter(|r| {
            doc.console.bus_index(&r.bus).is_some() && !doc.song.group_tracks(r.folder).is_empty()
        })
        .cloned()
        .collect();
    // Cheap pass first: does any route differ from the song?
    let wanted: Vec<Vec<DeviceKey>> = live
        .iter()
        .map(|r| doc.route_strips(&live, r.folder))
        .collect();
    // The members are kept in console order (what the loader restores), so a channel moved within
    // the console re-sorts them in the same step.
    let steady = live.len() == old.len()
        && live
            .iter()
            .zip(&wanted)
            .all(|(r, w)| r.members.iter().eq(w.iter()));
    if steady {
        return None;
    }
    let mut p = Planner::new(doc, registry);
    let mut new = live.clone();
    for (r, want) in new.iter_mut().zip(&wanted) {
        let gone: Vec<DeviceKey> = r
            .members
            .iter()
            .filter(|m| !want.contains(m))
            .cloned()
            .collect();
        if back_to_master(&mut p, &r.bus, &gone).is_err() {
            return None;
        }
        let mut members: Vec<DeviceKey> = r
            .members
            .iter()
            .filter(|m| want.contains(m))
            .cloned()
            .collect();
        for s in want {
            if members.contains(s) {
                continue;
            }
            let dest = OutputDest::Bus(r.bus.clone());
            if check_dest(p.doc(), s, &dest).is_err() || set_output(&mut p, s, &dest).is_err() {
                return None;
            }
            members.push(s.clone());
        }
        let order: Vec<&DeviceKey> = doc.console.strip_keys().collect();
        members.sort_by_key(|m| order.iter().position(|k| *k == m).unwrap_or(usize::MAX));
        r.members = members;
    }
    if new != old {
        p.push(Edit::ConsoleFolderRoutes { old, new });
    }
    Some(p.finish())
}
