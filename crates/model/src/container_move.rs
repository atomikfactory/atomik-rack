//! Moving devices between the rack and a container's inside: `Combine`, `Uncombine`,
//! `MoveIntoContainer` and `MoveOutOfContainer`.
//!
//! Every move is one planned [`Edit`] built on a scratch copy of the document (the console
//! [`Planner`]), then performed on a fresh copy and checked with
//! [`RackDoc::check_containers`], so a refused move changes nothing.
//!
//! **Cables.** A cable between two devices that move together stays a cable (inside the
//! container, or out of it again). A cable that joins a moving device to one that stays is spliced
//! through the container's fixed boundary: an outer cable into a moving input becomes a pair
//! (`source -> container:in_l` and `boundary:in_l -> input`), a moving output feeding the outside
//! becomes `output -> boundary:out_l` and `container:out_l -> sink`. The container has two audio
//! inputs, two audio outputs and one notes input, so every distinct source that enters (and every
//! distinct output that leaves) takes one of the two sockets, the left one first for a socket
//! named `..._l`; a third one has no socket to use and the move is refused, naming each cable
//! that needed it. CV cables, a notes output that leaves, and a second notes source are refused
//! the same way. A notes cable into a moving device becomes the container's notes input plus a
//! full-range split to that device, so the flat graph, and so the bounce, is the one the devices
//! had in the rack (the flat graph of the container only adds its `~split`).
//!
//! **Song and console.** A container has one song track and one console channel, so at most one
//! moving device may have a song track, a console channel or an FX return slot (its *owner*): the
//! track, the channels and the return slot name the container from then on, the track's automation
//! lanes (they name parameters of the owner) are dropped with a note in the result, and the
//! track's note entry becomes the container's splits. `Uncombine` hands all of this to the inner
//! device it finds to be the owner: the only instrument, else the end of the track's note chain,
//! else the only device (an FX return goes to the device its boundary input feeds).
//!
//! **Not moved.** Project macros of a macro bank that drive a moving device stop the move (the
//! bank would have nothing to drive), and so does a frozen track. MIDI mappings of a moving
//! device's params stay in the project and drive nothing while the device is inside.
//!
//! Combine and Uncombine work on top-level devices, and the moves on devices of a top-level
//! container.

use crate::console::ConsoleChannel;
use crate::console_cmd::Planner;
use crate::container::{
    BOUNDARY_KEY, CONTAINER_TYPE, ContainerCommand, ContainerDoc, FORBIDDEN_INSIDE,
    MAX_INNER_DEVICES, MAX_SPLITS, MacroDoc, NoteSplit, SPLIT_TYPE, new_device, notes_input,
    scope_socket,
};
use crate::history::Edit;
use crate::macros::MACRO_BANK_TYPE;
use crate::song::{Lane, LaneKind, Track};
use crate::{Cable, DeviceKey, ModelError, RackDevice, RackDoc, SocketRef, rules};
use atomik_device_sdk::{DeviceKind, Registry, SocketKind};
use std::collections::{BTreeMap, BTreeSet};

/// The container's audio inputs, as the boundary and the container both name them.
const IN_SOCKETS: [&str; 2] = ["in_l", "in_r"];
/// The container's audio outputs.
const OUT_SOCKETS: [&str; 2] = ["out_l", "out_r"];
/// The container's notes input.
const NOTES_IN: &str = "notes_in";

/// A planned move: the edit and what it had to leave behind, for the user to read.
pub(crate) struct Planned {
    pub(crate) edit: Edit,
    pub(crate) notes: Vec<String>,
}

fn bad_scope(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

fn at(device: &DeviceKey, socket: &str) -> SocketRef {
    SocketRef {
        device: device.clone(),
        socket: socket.to_owned(),
    }
}

fn boundary(socket: &str) -> SocketRef {
    at(&DeviceKey(BOUNDARY_KEY.to_owned()), socket)
}

fn wire(from: SocketRef, to: SocketRef) -> Cable {
    Cable { from, to }
}

/// `"name"` of the top-level device `key`, else the key.
fn label(doc: &RackDoc, key: &DeviceKey) -> String {
    doc.device(key)
        .map_or_else(|| format!("\"{key}\""), |d| format!("\"{}\"", d.name))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A key not in `taken`: `base`, else `base-2`, `base-3`, ...; added to `taken`.
fn free_key(taken: &mut BTreeSet<String>, base: &str) -> DeviceKey {
    let key = std::iter::once(base.to_owned())
        .chain((2u64..).map(|n| format!("{base}-{n}")))
        .find(|k| !taken.contains(k))
        .expect("a free key exists");
    taken.insert(key.clone());
    DeviceKey(key)
}

/// Which of the two stereo sides a socket name says (`out_l`, `ch1_in_r`), if it says one.
fn side(socket: &str) -> Option<usize> {
    if socket.ends_with("_l") {
        Some(0)
    } else if socket.ends_with("_r") {
        Some(1)
    } else {
        None
    }
}

/// Gives each signal in `wanted` (with the side it would like) one of the two boundary sockets.
/// A socket that `held` says already carries the signal is reused; otherwise a socket must be
/// `free`. Returns the socket of each signal, `None` for one that found none.
fn assign(
    wanted: &[(SocketRef, Option<usize>)],
    held: &[Option<SocketRef>; 2],
    free: [bool; 2],
) -> Vec<Option<usize>> {
    let mut taken = [false; 2];
    let mut slots: Vec<Option<usize>> = vec![None; wanted.len()];
    for (i, (signal, _)) in wanted.iter().enumerate() {
        if let Some(s) = (0..2).find(|&s| held[s].as_ref() == Some(signal)) {
            slots[i] = Some(s);
            taken[s] = true;
        }
    }
    for pass in 0..2 {
        for (i, (_, preferred)) in wanted.iter().enumerate() {
            if slots[i].is_some() {
                continue;
            }
            let usable = |s: usize| free[s] && !taken[s];
            let pick = if pass == 0 {
                preferred.filter(|&s| usable(s))
            } else {
                (0..2).find(|&s| usable(s))
            };
            if let Some(s) = pick {
                slots[i] = Some(s);
                taken[s] = true;
            }
        }
    }
    slots
}

/// The distinct values of `items`, first appearance first.
fn distinct<T: Clone + PartialEq>(items: impl Iterator<Item = T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// True when the device has a song track, a console channel or an FX return slot.
fn has_role(doc: &RackDoc, key: &DeviceKey) -> bool {
    doc.song.track(key).is_some()
        || doc.console.channel_of(key).is_some()
        || doc.console.return_slot(key).is_some()
}

/// Performs the planned step on a fresh copy of `doc` and checks every container; the plan when
/// all of it holds.
fn finish(
    doc: &RackDoc,
    registry: &Registry,
    planner: Planner<'_>,
    notes: Vec<String>,
) -> Result<Planned, ModelError> {
    let edit = planner.finish();
    let mut scratch = doc.clone();
    edit.perform(&mut scratch)?;
    scratch.check_containers(registry)?;
    Ok(Planned { edit, notes })
}

/// The refusal that lists every cable (or reason) that stopped a move.
fn refuse_cables(verb: &str, offenders: Vec<String>) -> Result<(), ModelError> {
    if offenders.is_empty() {
        return Ok(());
    }
    Err(ModelError::BadCable(format!(
        "cannot {verb}: {}",
        offenders.join("; ")
    )))
}

// ---------------------------------------------------------------------- planning entry point

/// Plans one of the four move commands; `None` for any other container command.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: &ContainerCommand,
) -> Option<Result<Planned, ModelError>> {
    Some(match command {
        ContainerCommand::Combine { keys, name } => combine(doc, registry, keys, name),
        ContainerCommand::Uncombine { container } => uncombine(doc, registry, container),
        ContainerCommand::MoveIntoContainer {
            key,
            container,
            index,
        } => move_into(doc, registry, key, container, *index),
        ContainerCommand::MoveOutOfContainer { key, index } => move_out(doc, registry, key, *index),
        _ => return None,
    })
}

impl RackDoc {
    /// What `command` leaves behind, as lines for the user (the automation lanes it drops, the
    /// macro mappings that go away, a note entry that changed): the same text for every run of
    /// the command on this document. Empty for a command that drops nothing and for every
    /// command that is not `Combine`, `Uncombine`, `MoveIntoContainer` or `MoveOutOfContainer`.
    ///
    /// # Errors
    /// The error the command itself would give.
    pub fn container_move_notes(
        &self,
        registry: &Registry,
        command: &ContainerCommand,
    ) -> Result<Vec<String>, ModelError> {
        plan(self, registry, command).map_or_else(|| Ok(Vec::new()), |p| p.map(|p| p.notes))
    }

    /// Whether the container at the path key `key` has its `bypass` param on. A macro of an
    /// outer container that drives the param is not considered here (the expansion does).
    pub fn container_bypassed(&self, key: &DeviceKey) -> bool {
        self.resolve(key).is_some_and(|d| {
            d.inner.is_some()
                && d.params
                    .get(crate::expand::BYPASS_PARAM)
                    .is_some_and(|v| *v >= 0.5)
        })
    }
}

// ---------------------------------------------------------------------- moving into a container

/// Where the moving devices go.
enum Target {
    /// A new container, inserted at the rack `index`.
    New {
        device: Box<RackDevice>,
        index: usize,
    },
    /// The existing top-level container, at position `at` among its devices.
    Existing { key: DeviceKey, at: usize },
}

/// One move of top-level devices into a container.
struct Enter<'a> {
    /// The moving devices (top-level keys, in rack order).
    moved: &'a [DeviceKey],
    target: Target,
    /// Moving devices the song's notes must reach: each gets a split.
    entry: &'a [DeviceKey],
    /// "combine" or "move", for the refusal text.
    verb: &'static str,
}

/// The wiring of a move into a container, worked out before anything changes.
#[derive(Default)]
struct Wiring {
    /// Local key of each moving device.
    locals: BTreeMap<DeviceKey, DeviceKey>,
    /// Cables inside the container (local keys, boundary ends included).
    inner: Vec<Cable>,
    /// Cables outside that the container gets (its own sockets at one end).
    outer: Vec<Cable>,
    /// The splits the container gains, in order.
    splits: Vec<NoteSplit>,
}

fn enter(p: &mut Planner<'_>, e: &Enter<'_>) -> Result<(), ModelError> {
    let wiring = enter_wiring(p, e)?;
    let container = match &e.target {
        Target::New { device, .. } => device.key.clone(),
        Target::Existing { key, .. } => key.clone(),
    };
    // Take the devices out of the rack, each with the cables still on it.
    let mut devices: Vec<RackDevice> = Vec::new();
    for key in e.moved {
        let index = rules::index_of(p.doc(), key)?;
        let device = p.doc().devices[index].clone();
        let cables = p
            .doc()
            .cables
            .iter()
            .filter(|c| &c.from.device == key || &c.to.device == key)
            .cloned()
            .collect();
        devices.push(device.clone());
        p.push(Edit::Remove {
            index,
            device,
            cables,
        });
    }
    for (device, key) in devices.iter_mut().zip(e.moved) {
        device.key = wiring.locals[key].clone();
        // Inside a container every device is in column 0.
        device.column = 0;
    }
    match &e.target {
        Target::New { device, index } => {
            // The container arrives with its inside, announced to the engine as one device.
            let mut new = (**device).clone();
            let mut inside = ContainerDoc {
                devices,
                cables: wiring.inner,
                splits: wiring.splits,
                ..ContainerDoc::default()
            };
            inside.cables.sort();
            new.inner = Some(Box::new(inside));
            p.push(Edit::Insert {
                index: rules::top_index(p.doc(), *index),
                device: new,
                cables: Vec::new(),
            });
        }
        Target::Existing { at, .. } => {
            let before = p
                .doc()
                .inner(&container)
                .ok_or_else(|| bad_scope(format!("{container} is not a container")))?
                .splits
                .clone();
            for (i, device) in devices.into_iter().enumerate() {
                let len = p.doc().inner(&container).map_or(0, |c| c.devices.len());
                p.push(Edit::InnerInsert {
                    container: container.clone(),
                    index: (at + i).min(len),
                    device,
                    cables: Vec::new(),
                });
            }
            for cable in wiring.inner {
                p.push(Edit::InnerConnect {
                    container: container.clone(),
                    cable,
                });
            }
            if !wiring.splits.is_empty() {
                let mut new = before.clone();
                new.extend(wiring.splits);
                p.push(Edit::Splits {
                    container: container.clone(),
                    old: before,
                    new,
                });
            }
        }
    }
    for cable in wiring.outer {
        p.connect(cable)?;
    }
    Ok(())
}

/// Works out the cables of a move into a container and refuses what cannot be wired.
#[allow(clippy::too_many_lines)]
fn enter_wiring(p: &Planner<'_>, e: &Enter<'_>) -> Result<Wiring, ModelError> {
    let doc = p.doc();
    let registry = p.registry;
    let (container, existing) = match &e.target {
        Target::New { device, .. } => (device.key.clone(), false),
        Target::Existing { key, .. } => (key.clone(), true),
    };
    let inside: ContainerDoc = match &e.target {
        Target::New { .. } => ContainerDoc::default(),
        Target::Existing { key, .. } => doc
            .inner(key)
            .cloned()
            .ok_or_else(|| bad_scope(format!("{key} is not a container")))?,
    };
    if inside.devices.len() + e.moved.len() > MAX_INNER_DEVICES {
        return Err(ModelError::ContainerFull(format!(
            "a container holds at most {MAX_INNER_DEVICES} devices"
        )));
    }
    let mut wiring = Wiring::default();
    let mut taken: BTreeSet<String> = inside.devices.iter().map(|d| d.key.0.clone()).collect();
    for key in e.moved {
        let usable =
            rules::is_valid_key(&key.0) && key.0 != BOUNDARY_KEY && key.0 != crate::INTERFACE_KEY;
        let local = if usable && !taken.contains(&key.0) {
            taken.insert(key.0.clone());
            key.clone()
        } else {
            free_key(&mut taken, if usable { &key.0 } else { "device" })
        };
        wiring.locals.insert(key.clone(), local);
    }
    let local_of = |k: &DeviceKey| wiring.locals[k].clone();
    let moves = |k: &DeviceKey| e.moved.contains(k);
    // Sort the cables on the moving devices.
    let mut inner_cables: Vec<Cable> = Vec::new();
    let mut ins: Vec<&Cable> = Vec::new();
    let mut outs: Vec<&Cable> = Vec::new();
    let mut offenders: Vec<String> = Vec::new();
    for cable in &doc.cables {
        match (moves(&cable.from.device), moves(&cable.to.device)) {
            (true, true) => inner_cables.push(wire(
                at(&local_of(&cable.from.device), &cable.from.socket),
                at(&local_of(&cable.to.device), &cable.to.socket),
            )),
            (false, true) => {
                if existing && cable.from.device == container {
                    offenders.push(format!(
                        "{cable} comes from the container itself; remove the cable first"
                    ));
                } else {
                    ins.push(cable);
                }
            }
            (true, false) => {
                if existing && cable.to.device == container {
                    offenders.push(format!(
                        "{cable} goes into the container itself; remove the cable first"
                    ));
                } else {
                    outs.push(cable);
                }
            }
            (false, false) => {}
        }
    }
    let kind_of = |at: &SocketRef| -> Result<SocketKind, ModelError> {
        Ok(scope_socket(&doc.devices, registry, at)?.0)
    };
    // Audio entering, by source.
    let mut audio_in: Vec<&Cable> = Vec::new();
    let mut notes_in: Vec<&Cable> = Vec::new();
    for cable in ins {
        match kind_of(&cable.to)? {
            SocketKind::Audio => audio_in.push(cable),
            SocketKind::Notes => notes_in.push(cable),
            SocketKind::Cv => offenders.push(format!("{cable}: a container has no CV input")),
        }
    }
    let mut audio_out: Vec<&Cable> = Vec::new();
    for cable in outs {
        match kind_of(&cable.from)? {
            SocketKind::Audio => audio_out.push(cable),
            SocketKind::Notes => {
                offenders.push(format!("{cable}: a container has no notes output"))
            }
            SocketKind::Cv => offenders.push(format!("{cable}: a container has no CV output")),
        }
    }
    // The sockets the container can lend.
    let held_in: [Option<SocketRef>; 2] = std::array::from_fn(|s| {
        existing
            .then(|| {
                doc.cables
                    .iter()
                    .find(|c| c.to == at(&container, IN_SOCKETS[s]))
                    .map(|c| c.from.clone())
            })
            .flatten()
    });
    let free_in: [bool; 2] = std::array::from_fn(|s| {
        held_in[s].is_none()
            && !inside
                .cables
                .iter()
                .any(|c| c.from == boundary(IN_SOCKETS[s]))
    });
    let free_out: [bool; 2] = std::array::from_fn(|s| {
        !inside
            .cables
            .iter()
            .any(|c| c.to == boundary(OUT_SOCKETS[s]))
            && !(existing
                && doc
                    .cables
                    .iter()
                    .any(|c| c.from == at(&container, OUT_SOCKETS[s])))
    });
    // Entering audio: one container input per distinct source.
    let sources = distinct(audio_in.iter().map(|c| c.from.clone()));
    let wanted: Vec<(SocketRef, Option<usize>)> = sources
        .iter()
        .map(|src| {
            let hint = audio_in
                .iter()
                .find(|c| &c.from == src)
                .and_then(|c| side(&c.to.socket));
            (src.clone(), side(&src.socket).or(hint))
        })
        .collect();
    let slots = assign(&wanted, &held_in, free_in);
    for (src, slot) in sources.iter().zip(&slots) {
        match slot {
            Some(s) => {
                if held_in[*s].is_none() {
                    wiring
                        .outer
                        .push(wire(src.clone(), at(&container, IN_SOCKETS[*s])));
                }
                for cable in audio_in.iter().filter(|c| &c.from == src) {
                    wiring.inner.push(wire(
                        boundary(IN_SOCKETS[*s]),
                        at(&local_of(&cable.to.device), &cable.to.socket),
                    ));
                }
            }
            None => {
                for cable in audio_in.iter().filter(|c| &c.from == src) {
                    offenders.push(format!(
                        "{cable} needs a third audio input, and a container has two"
                    ));
                }
            }
        }
    }
    // Leaving audio: one container output per distinct moving output.
    let leaving = distinct(audio_out.iter().map(|c| c.from.clone()));
    let wanted: Vec<(SocketRef, Option<usize>)> = leaving
        .iter()
        .map(|out| (out.clone(), side(&out.socket)))
        .collect();
    let slots = assign(&wanted, &[None, None], free_out);
    for (out, slot) in leaving.iter().zip(&slots) {
        match slot {
            Some(s) => {
                wiring.inner.push(wire(
                    at(&local_of(&out.device), &out.socket),
                    boundary(OUT_SOCKETS[*s]),
                ));
                for cable in audio_out.iter().filter(|c| &c.from == out) {
                    wiring
                        .outer
                        .push(wire(at(&container, OUT_SOCKETS[*s]), cable.to.clone()));
                }
            }
            None => {
                for cable in audio_out.iter().filter(|c| &c.from == out) {
                    offenders.push(format!(
                        "{cable} needs a third audio output, and a container has two"
                    ));
                }
            }
        }
    }
    // Notes: one source for the container's notes input, a split per receiving device.
    let note_sources = distinct(notes_in.iter().map(|c| c.from.clone()));
    let held_notes = existing
        .then(|| {
            doc.cables
                .iter()
                .find(|c| c.to == at(&container, NOTES_IN))
                .map(|c| c.from.clone())
        })
        .flatten();
    let mut note_targets: Vec<DeviceKey> = Vec::new();
    if let Some(src) = note_sources.first() {
        for extra in &note_sources[1..] {
            for cable in notes_in.iter().filter(|c| &c.from == extra) {
                offenders.push(format!(
                    "{cable}: a container has one notes input, and {src} already uses it"
                ));
            }
        }
        match &held_notes {
            Some(h) if h == src => {}
            Some(h) => offenders.push(format!(
                "{src}: the container's notes input is already fed by {h}"
            )),
            None if !inside.splits.is_empty() => offenders.push(format!(
                "{src}: the container's splits already take the song's notes"
            )),
            None => wiring
                .outer
                .push(wire(src.clone(), at(&container, NOTES_IN))),
        }
        note_targets.extend(
            notes_in
                .iter()
                .filter(|c| &c.from == src)
                .map(|c| c.to.device.clone()),
        );
    }
    note_targets.extend(e.entry.iter().cloned());
    let note_targets: Vec<DeviceKey> = e
        .moved
        .iter()
        .filter(|k| note_targets.contains(k))
        .cloned()
        .collect();
    if inside.splits.len() + note_targets.len() > MAX_SPLITS {
        return Err(ModelError::BadSplit(format!(
            "a container has at most {MAX_SPLITS} splits: {} devices would need one",
            inside.splits.len() + note_targets.len()
        )));
    }
    for target in &note_targets {
        let device = rules::device(doc, target)?;
        if notes_input(rules::descriptor(registry, &device.type_id)?).is_none() {
            return Err(ModelError::BadSplit(format!(
                "{} has no notes input",
                label(doc, target)
            )));
        }
        let local = local_of(target);
        // A cable inside that feeds the notes input leaves no room for the song's notes.
        let fed_inside = inner_cables.iter().any(|c| {
            c.to.device == local
                && scope_socket(&doc.devices, registry, &at(target, &c.to.socket))
                    .is_ok_and(|(kind, _)| kind == SocketKind::Notes)
        });
        if fed_inside {
            return Err(ModelError::BadSplit(format!(
                "{} takes notes from a cable inside the group, so the song's notes cannot enter it",
                label(doc, target)
            )));
        }
        wiring.splits.push(NoteSplit::full(local));
    }
    refuse_cables(e.verb, offenders)?;
    wiring.inner.extend(inner_cables);
    Ok(wiring)
}

// ---------------------------------------------------------------------- the song and the console

/// What happens to the song track of the owner of a combined group.
fn combined_track(
    track: &Track,
    container: &DeviceKey,
    has_notes: bool,
    entry_moves: bool,
) -> (Option<Track>, usize) {
    if !has_notes {
        return (None, track.lanes.len());
    }
    let kept: Vec<Lane> = track
        .lanes
        .iter()
        .filter(|l| !matches!(l.kind, LaneKind::Automation(_)))
        .cloned()
        .collect();
    let dropped = track.lanes.len() - kept.len();
    (
        Some(Track {
            device: container.clone(),
            notes_to: if entry_moves {
                None
            } else {
                track.notes_to.clone()
            },
            lanes: kept,
            ..track.clone()
        }),
        dropped,
    )
}

/// Everything the song and the console know by device key, moved from `from` to `to`; the track
/// (when there is one) becomes `track`, or goes when that is `None`.
fn repoint(p: &mut Planner<'_>, from: &DeviceKey, to: &DeviceKey, track: Option<Option<Track>>) {
    if let Some(change) = track {
        let song = &p.doc().song;
        if let Some(index) = song.tracks.iter().position(|t| &t.device == from) {
            let old = song.tracks[index].clone();
            let kept = change.is_some();
            p.push(Edit::Track {
                index,
                old: Some(old),
                new: change,
            });
            if kept {
                rekey_track_data(p, from, to);
            }
        }
    }
    let channels: Vec<(usize, ConsoleChannel)> = p
        .doc()
        .console
        .channels
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, c)| c.source.as_ref() == Some(from))
        .collect();
    for (index, old) in channels {
        let new = ConsoleChannel {
            source: Some(to.clone()),
            ..old.clone()
        };
        p.push(Edit::ConsoleChannel {
            index,
            old: Some(old),
            new: Some(new),
        });
    }
    if let Some(slot) = p.doc().console.return_slot(from) {
        p.push(Edit::ConsoleReturn {
            slot,
            old: Some(from.clone()),
            new: Some(to.clone()),
        });
    }
}

/// The track's height, folder, colour, comment and folded lanes follow its device key.
fn rekey_track_data(p: &mut Planner<'_>, from: &DeviceKey, to: &DeviceKey) {
    let song = &p.doc().song;
    let mut edits = Vec::new();
    if let Some(&height) = song.track_heights.get(from) {
        let mut heights = song.track_heights.clone();
        heights.remove(from);
        heights.insert(to.clone(), height);
        edits.push(Edit::TrackHeights {
            old: song.track_heights.clone(),
            new: heights,
        });
    }
    let mut org = song.org.clone();
    if let Some(group) = org.members.remove(from) {
        org.members.insert(to.clone(), group);
    }
    if let Some(color) = org.colors.remove(from) {
        org.colors.insert(to.clone(), color);
    }
    if let Some(comment) = org.comments.remove(from) {
        org.comments.insert(to.clone(), comment);
    }
    org.folded_lanes = org
        .folded_lanes
        .iter()
        .map(|(d, param)| {
            (
                if d == from { to.clone() } else { d.clone() },
                param.clone(),
            )
        })
        .collect();
    if org != song.org {
        edits.push(crate::organise_cmd::org_edit(song, org));
    }
    for edit in edits {
        p.push(edit);
    }
}

// ---------------------------------------------------------------------- Combine

pub(crate) fn combine(
    doc: &RackDoc,
    registry: &Registry,
    keys: &[DeviceKey],
    name: &str,
) -> Result<Planned, ModelError> {
    if keys.is_empty() {
        return Err(bad_scope("select at least one device to combine"));
    }
    for key in keys {
        if key.0.contains('/') {
            return Err(bad_scope(format!(
                "only devices of the rack can be combined; {key} is inside a container"
            )));
        }
        rules::index_of(doc, key)?;
    }
    // In rack order, once each.
    let moved: Vec<DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| keys.contains(&d.key))
        .map(|d| d.key.clone())
        .collect();
    for key in &moved {
        check_can_enter(doc, registry, key)?;
    }
    // The song and the console name the container from now on: one owner at most.
    let owners: Vec<&DeviceKey> = moved.iter().filter(|k| has_role(doc, k)).collect();
    if owners.len() > 1 {
        let names: Vec<String> = owners.iter().map(|k| label(doc, k)).collect();
        return Err(bad_scope(format!(
            "{} each have a song track, a console channel or an FX return, and a container keeps \
             only one; combine them separately",
            names.join(", ")
        )));
    }
    for track in &doc.song.tracks {
        if !moved.contains(&track.device)
            && let Some(entry) = &track.notes_to
            && moved.contains(entry)
        {
            return Err(bad_scope(format!(
                "the track of {} sends its notes to {}, which would move inside the container",
                label(doc, &track.device),
                label(doc, entry)
            )));
        }
    }
    let first = rules::index_of(doc, &moved[0])?;
    let container_key = rules::generate_key(doc, CONTAINER_TYPE);
    let desc = rules::descriptor(registry, CONTAINER_TYPE)?;
    let name = name.trim();
    let name = if name.is_empty() {
        rules::unique_name(doc, desc.name)
    } else {
        name.to_owned()
    };
    let mut device = new_device(registry, desc, container_key.clone(), name);
    device.column = doc.devices[first].column;
    let mut p = Planner::new(doc, registry);
    let mut notes = Vec::new();
    let mut entry: Vec<DeviceKey> = Vec::new();
    if let Some(owner) = owners.first().copied() {
        let owner_device = rules::device(doc, owner)?;
        let has_notes = notes_input(rules::descriptor(registry, &owner_device.type_id)?).is_some();
        let new_track = doc.song.track(owner).map(|track| {
            let notes_entry = doc.notes_entry(track).clone();
            let entry_moves = notes_entry != *owner && moved.contains(&notes_entry);
            if has_notes {
                entry.push(if entry_moves {
                    notes_entry
                } else {
                    owner.clone()
                });
            }
            let (new, dropped) = combined_track(track, &container_key, has_notes, entry_moves);
            if dropped > 0 {
                notes.push(format!(
                    "Dropped {} of {}: they named parameters of devices that are now inside \
                     \"{}\"",
                    plural(dropped, "automation lane", "automation lanes"),
                    label(doc, owner),
                    device.name
                ));
            }
            new
        });
        repoint(&mut p, owner, &container_key, new_track);
    }
    let index = first;
    enter(
        &mut p,
        &Enter {
            moved: &moved,
            target: Target::New {
                device: Box::new(device),
                index,
            },
            entry: &entry,
            verb: "combine",
        },
    )?;
    finish(doc, registry, p, notes)
}

/// The rules about a device that is to go inside a container.
fn check_can_enter(doc: &RackDoc, registry: &Registry, key: &DeviceKey) -> Result<(), ModelError> {
    let device = rules::device(doc, key)?;
    if FORBIDDEN_INSIDE.contains(&device.type_id.as_str())
        || device.type_id == SPLIT_TYPE
        || device.type_id == MACRO_BANK_TYPE
    {
        return Err(ModelError::NotAllowedInContainer(format!(
            "{} cannot be placed inside a container",
            label(doc, key)
        )));
    }
    rules::descriptor(registry, &device.type_id)?;
    if let Some(a) = doc.macros.assigns.iter().find(|a| &a.device == key) {
        return Err(ModelError::BadMacro(format!(
            "{}.{} is driven by {} of {}: remove the assignment first",
            label(doc, key),
            a.param,
            doc.macro_name(&a.bank, a.index),
            label(doc, &a.bank)
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------- MoveIntoContainer

fn top_container(doc: &RackDoc, key: &DeviceKey) -> Result<usize, ModelError> {
    if key.0.contains('/') {
        return Err(bad_scope(format!(
            "{key} is inside a container; only a container of the rack takes devices this way"
        )));
    }
    let index = rules::index_of(doc, key)?;
    if doc.devices[index].inner.is_none() {
        return Err(bad_scope(format!("{key} is not a container")));
    }
    Ok(index)
}

pub(crate) fn move_into(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
    container: &DeviceKey,
    at: usize,
) -> Result<Planned, ModelError> {
    top_container(doc, container)?;
    if key.0.contains('/') {
        return Err(bad_scope(format!(
            "{key} is inside a container; move it out first"
        )));
    }
    rules::index_of(doc, key)?;
    if key == container {
        return Err(bad_scope("a container cannot go inside itself"));
    }
    check_can_enter(doc, registry, key)?;
    if has_role(doc, key) {
        return Err(bad_scope(format!(
            "{} has a song track, a console channel or an FX return; combine it into a new \
             container instead, which keeps them",
            label(doc, key)
        )));
    }
    if doc
        .song
        .tracks
        .iter()
        .any(|t| t.notes_to.as_ref() == Some(key))
    {
        return Err(bad_scope(format!(
            "a track sends its notes to {}, which would move inside the container",
            label(doc, key)
        )));
    }
    let mut p = Planner::new(doc, registry);
    enter(
        &mut p,
        &Enter {
            moved: std::slice::from_ref(key),
            target: Target::Existing {
                key: container.clone(),
                at,
            },
            entry: &[],
            verb: "move the device into the container",
        },
    )?;
    finish(doc, registry, p, Vec::new())
}

// ---------------------------------------------------------------------- moving out

/// One move of devices out of a top-level container.
struct Exit<'a> {
    container: &'a DeviceKey,
    /// The leaving devices (local keys, in the container's order).
    moved: Vec<DeviceKey>,
    /// The container goes too (`Uncombine`): every device leaves.
    dissolve: bool,
    /// Rack index of the first leaving device when the container stays.
    index: usize,
}

pub(crate) fn move_out(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
    index: usize,
) -> Result<Planned, ModelError> {
    let (parent, local) = crate::container::split_path(key);
    let Some(container) = parent else {
        return Err(bad_scope(format!(
            "{key} is in the rack, not inside a container"
        )));
    };
    top_container(doc, &container)?;
    let inside = doc
        .inner(&container)
        .ok_or_else(|| bad_scope(format!("{container} is not a container")))?;
    if !inside.devices.iter().any(|d| d.key == local) {
        return Err(ModelError::UnknownDevice(key.0.clone()));
    }
    exit(
        doc,
        registry,
        &Exit {
            container: &container,
            moved: vec![local],
            dissolve: false,
            index,
        },
    )
}

pub(crate) fn uncombine(
    doc: &RackDoc,
    registry: &Registry,
    container: &DeviceKey,
) -> Result<Planned, ModelError> {
    let index = top_container(doc, container)?;
    let inside = doc
        .inner(container)
        .ok_or_else(|| bad_scope(format!("{container} is not a container")))?;
    exit(
        doc,
        registry,
        &Exit {
            container,
            moved: inside.devices.iter().map(|d| d.key.clone()).collect(),
            dissolve: true,
            index,
        },
    )
}

/// The inner device that takes the container's song track, console channels and FX return when
/// the container is dissolved (see the module documentation); `None` when the container has none
/// of them.
fn owner_of(
    doc: &RackDoc,
    registry: &Registry,
    container: &DeviceKey,
    inside: &ContainerDoc,
) -> Result<Option<DeviceKey>, ModelError> {
    let track = doc.song.track(container);
    let channel = doc.console.channel_of(container).is_some();
    let ret = doc.console.return_slot(container).is_some();
    if track.is_none() && !channel && !ret {
        return Ok(None);
    }
    let kind = |d: &RackDevice| registry.get(&d.type_id).map(|f| f.descriptor.kind);
    let sole = || (inside.devices.len() == 1).then(|| inside.devices[0].key.clone());
    let chosen = if ret {
        // The device the container's input feeds first.
        let fed = distinct(
            inside
                .cables
                .iter()
                .filter(|c| c.from.device.0 == BOUNDARY_KEY && c.to.device.0 != BOUNDARY_KEY)
                .map(|c| c.to.device.clone()),
        );
        if fed.len() == 1 {
            fed.into_iter().next()
        } else {
            sole()
        }
    } else {
        let instruments: Vec<&RackDevice> = inside
            .devices
            .iter()
            .filter(|d| kind(d) == Some(DeviceKind::Instrument))
            .collect();
        if let [only] = instruments[..] {
            Some(only.key.clone())
        } else if track.is_some() {
            note_chain_end(registry, inside).or_else(sole)
        } else {
            sole()
        }
    };
    chosen.map(Some).ok_or_else(|| {
        bad_scope(format!(
            "{} has a song track, a console channel or an FX return, and none of its devices \
             stands out to take them: remove all but one device first",
            label(doc, container)
        ))
    })
}

/// The device the song's notes end at: the end of the note cables from the only split target.
fn note_chain_end(registry: &Registry, inside: &ContainerDoc) -> Option<DeviceKey> {
    let [split] = &inside.splits[..] else {
        return None;
    };
    let mut at = split.target.clone();
    for _ in 0..inside.devices.len() {
        let next = distinct(
            inside
                .cables
                .iter()
                .filter(|c| c.from.device == at && c.to.device.0 != BOUNDARY_KEY)
                .filter(|c| {
                    inside
                        .devices
                        .iter()
                        .find(|d| d.key == at)
                        .and_then(|d| registry.get(&d.type_id))
                        .and_then(|f| f.descriptor.socket_by_key(&c.from.socket))
                        .is_some_and(|s| s.kind == SocketKind::Notes)
                })
                .map(|c| c.to.device.clone()),
        );
        match &next[..] {
            [] => return Some(at),
            [one] => at = one.clone(),
            _ => return None,
        }
    }
    None
}

fn split_is_plain(s: &NoteSplit) -> bool {
    s.key_lo == 0 && s.key_hi == 127 && s.vel_lo == 1 && s.vel_hi == 127 && s.transpose == 0
}

#[allow(clippy::too_many_lines)]
fn exit(doc: &RackDoc, registry: &Registry, ex: &Exit<'_>) -> Result<Planned, ModelError> {
    let c = ex.container;
    let c_index = rules::index_of(doc, c)?;
    let c_device = &doc.devices[c_index];
    let inside: ContainerDoc = c_device
        .inner
        .as_deref()
        .cloned()
        .ok_or_else(|| bad_scope(format!("{c} is not a container")))?;
    let mut p = Planner::new(doc, registry);
    let mut notes: Vec<String> = Vec::new();
    // A key of the rack for each leaving device (the container's own key is not reused: the
    // engine would still be removing it).
    let mut taken: BTreeSet<String> = doc.devices.iter().map(|d| d.key.0.clone()).collect();
    let mut tops: BTreeMap<DeviceKey, DeviceKey> = BTreeMap::new();
    for local in &ex.moved {
        tops.insert(local.clone(), free_key(&mut taken, &local.0));
    }
    let top = |k: &DeviceKey| tops[k].clone();
    let moves = |k: &DeviceKey| ex.moved.contains(k);
    let outer_in: [Option<Cable>; 2] = std::array::from_fn(|s| {
        doc.cables
            .iter()
            .find(|x| x.to == at(c, IN_SOCKETS[s]))
            .cloned()
    });
    let outer_out: [Vec<Cable>; 2] = std::array::from_fn(|s| {
        doc.cables
            .iter()
            .filter(|x| x.from == at(c, OUT_SOCKETS[s]))
            .cloned()
            .collect()
    });
    let outer_notes: Option<Cable> = doc.cables.iter().find(|x| x.to == at(c, NOTES_IN)).cloned();
    let slot_of = |sockets: &[&str; 2], socket: &str| sockets.iter().position(|s| *s == socket);
    let mut new_top: Vec<Cable> = Vec::new();
    let mut unplug: Vec<Cable> = Vec::new();
    let mut offenders: Vec<String> = Vec::new();
    for cable in &inside.cables {
        let from_edge = cable.from.device.0 == BOUNDARY_KEY;
        let to_edge = cable.to.device.0 == BOUNDARY_KEY;
        let (from_moves, to_moves) = (moves(&cable.from.device), moves(&cable.to.device));
        match (from_edge, from_moves, to_edge, to_moves) {
            (_, true, _, true) => new_top.push(wire(
                at(&top(&cable.from.device), &cable.from.socket),
                at(&top(&cable.to.device), &cable.to.socket),
            )),
            // The container's input goes straight to the device.
            (true, _, _, true) => {
                if let Some(source) =
                    slot_of(&IN_SOCKETS, &cable.from.socket).and_then(|s| outer_in[s].as_ref())
                {
                    new_top.push(wire(
                        source.from.clone(),
                        at(&top(&cable.to.device), &cable.to.socket),
                    ));
                }
            }
            // The device feeds what the container's output fed.
            (_, true, true, _) => {
                if let Some(s) = slot_of(&OUT_SOCKETS, &cable.to.socket) {
                    for sink in &outer_out[s] {
                        new_top.push(wire(
                            at(&top(&cable.from.device), &cable.from.socket),
                            sink.to.clone(),
                        ));
                        if !ex.dissolve {
                            unplug.push(sink.clone());
                        }
                    }
                }
            }
            // The input passes straight through the container.
            (true, _, true, _) => {
                if ex.dissolve
                    && let (Some(i), Some(o)) = (
                        slot_of(&IN_SOCKETS, &cable.from.socket),
                        slot_of(&OUT_SOCKETS, &cable.to.socket),
                    )
                    && let Some(source) = &outer_in[i]
                {
                    for sink in &outer_out[o] {
                        new_top.push(wire(source.from.clone(), sink.to.clone()));
                    }
                }
            }
            (false, true, false, false) | (false, false, false, true) => {
                offenders.push(format!(
                    "{cable} joins a device that leaves with one that stays"
                ));
            }
            _ => {}
        }
    }
    // The splits of the leaving devices: the notes input's source feeds them directly.
    for s in inside.splits.iter().filter(|s| moves(&s.target)) {
        let wanted = outer_notes.is_some() || (ex.dissolve && doc.song.track(c).is_some());
        if wanted && !split_is_plain(s) {
            offenders.push(format!(
                "the split of \"{}\" narrows or transposes its notes",
                inside
                    .devices
                    .iter()
                    .find(|d| d.key == s.target)
                    .map_or("?", |d| d.name.as_str())
            ));
        }
        if let (Some(source), Some(socket)) = (
            &outer_notes,
            inside
                .devices
                .iter()
                .find(|d| d.key == s.target)
                .and_then(|d| registry.get(&d.type_id))
                .and_then(|f| notes_input(f.descriptor)),
        ) {
            new_top.push(wire(source.from.clone(), at(&top(&s.target), socket)));
        }
    }
    refuse_cables(
        if ex.dissolve {
            "uncombine"
        } else {
            "move the device out of the container"
        },
        offenders,
    )?;
    if ex.dissolve {
        let owner = owner_of(doc, registry, c, &inside)?;
        if let Some(owner) = &owner {
            let new_owner = top(owner);
            let owner_device = inside
                .devices
                .iter()
                .find(|d| &d.key == owner)
                .ok_or_else(|| ModelError::UnknownDevice(owner.0.clone()))?;
            let has_notes =
                notes_input(rules::descriptor(registry, &owner_device.type_id)?).is_some();
            let track = doc.song.track(c).map(|track| {
                let mut dropped = track
                    .lanes
                    .iter()
                    .filter(|l| matches!(l.kind, LaneKind::Automation(_)))
                    .count();
                if !has_notes {
                    dropped = track.lanes.len();
                    notes.push(format!(
                        "{} has no notes input: its song track and {} are dropped",
                        label(doc, c),
                        plural(dropped, "lane", "lanes")
                    ));
                    return Ok(None);
                }
                let targets: Vec<&NoteSplit> = inside.splits.iter().collect();
                let notes_to = match &track.notes_to {
                    Some(k) if doc.device(k).is_some() => Some(k.clone()),
                    _ => match &targets[..] {
                        [] => None,
                        [one] if &one.target == owner => None,
                        [one] => Some(top(&one.target)),
                        _ => {
                            return Err(bad_scope(format!(
                                "the track of {} plays {} devices through splits, and a track \
                                 plays one: remove the other splits first",
                                label(doc, c),
                                targets.len()
                            )));
                        }
                    },
                };
                if dropped > 0 {
                    notes.push(format!(
                        "Dropped {} of {}: they drove macros of the container",
                        plural(dropped, "automation lane", "automation lanes"),
                        label(doc, c)
                    ));
                }
                Ok(Some(Track {
                    device: new_owner.clone(),
                    notes_to,
                    lanes: track
                        .lanes
                        .iter()
                        .filter(|l| !matches!(l.kind, LaneKind::Automation(_)))
                        .cloned()
                        .collect(),
                    ..track.clone()
                }))
            });
            let track = track.transpose()?;
            repoint(&mut p, c, &new_owner, track);
        }
        let targets: usize = inside
            .macros
            .iter()
            .map(|m: &MacroDoc| m.targets.len())
            .sum();
        if targets > 0 {
            notes.push(format!(
                "The macros of {} are gone: {} drove inner params",
                label(doc, c),
                plural(targets, "mapping", "mappings")
            ));
        }
        let cables = doc
            .cables
            .iter()
            .filter(|x| &x.from.device == c || &x.to.device == c)
            .cloned()
            .collect();
        p.push(Edit::Remove {
            index: c_index,
            device: c_device.clone(),
            cables,
        });
    } else {
        // The container stays: what named the leaving devices inside it goes first.
        let kept: Vec<NoteSplit> = inside
            .splits
            .iter()
            .filter(|s| !moves(&s.target))
            .cloned()
            .collect();
        if kept.len() != inside.splits.len() {
            p.push(Edit::Splits {
                container: c.clone(),
                old: inside.splits.clone(),
                new: kept,
            });
        }
        let mut dropped = 0;
        for (i, m) in inside.macros.iter().enumerate() {
            let rest: Vec<_> = m
                .targets
                .iter()
                .filter(|t| {
                    !ex.moved
                        .iter()
                        .any(|k| t.device == *k || t.device.0.starts_with(&format!("{k}/")))
                })
                .cloned()
                .collect();
            if rest.len() != m.targets.len() {
                dropped += m.targets.len() - rest.len();
                p.push(Edit::Macro {
                    container: c.clone(),
                    index: i as u8,
                    old: m.clone(),
                    new: MacroDoc {
                        name: m.name.clone(),
                        targets: rest,
                    },
                });
            }
        }
        if dropped > 0 {
            notes.push(format!(
                "{} of the container's macros drove the device and no longer do",
                plural(dropped, "mapping", "mappings")
            ));
        }
        for cable in unplug {
            p.push(Edit::Disconnect(cable));
        }
        for local in &ex.moved {
            let current = p
                .doc()
                .inner(c)
                .ok_or_else(|| bad_scope(format!("{c} is not a container")))?;
            let index = current
                .devices
                .iter()
                .position(|d| &d.key == local)
                .ok_or_else(|| ModelError::UnknownDevice(local.0.clone()))?;
            let cables = current
                .cables
                .iter()
                .filter(|x| &x.from.device == local || &x.to.device == local)
                .cloned()
                .collect();
            p.push(Edit::InnerRemove {
                container: c.clone(),
                index,
                device: current.devices[index].clone(),
                cables,
            });
        }
    }
    // The devices arrive in the rack, where the container was (or where the drag put them).
    let base = if ex.dissolve { c_index } else { ex.index };
    for (i, local) in ex.moved.iter().enumerate() {
        let mut device = inside
            .devices
            .iter()
            .find(|d| &d.key == local)
            .cloned()
            .ok_or_else(|| ModelError::UnknownDevice(local.0.clone()))?;
        device.key = top(local);
        device.column = if ex.dissolve { c_device.column } else { 0 };
        p.push(Edit::Insert {
            index: rules::top_index(p.doc(), base + i),
            device,
            cables: Vec::new(),
        });
    }
    for cable in new_top {
        p.connect(cable)?;
    }
    finish(doc, registry, p, notes)
}
