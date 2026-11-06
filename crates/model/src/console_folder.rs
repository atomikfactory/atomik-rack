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
/// the route is forgotten (the bus stays).
fn unroute(p: &mut Planner<'_>, folder: u32) -> Result<(), ModelError> {
    let Some(r) = p.doc().console.folder_route(folder).cloned() else {
        return Err(bad(format!("folder {folder} is not routed to a bus")));
    };
    back_to_master(p, &r.bus, &r.members)?;
    let new = p
        .doc()
        .console
        .folder_routes
        .iter()
        .filter(|x| x.folder != folder)
        .cloned()
        .collect();
    edit_routes(p, new);
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
