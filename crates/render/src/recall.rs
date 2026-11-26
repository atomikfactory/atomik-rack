//! Snapshot recalls in a bounce: the lane of the master that recalls mixer snapshots, played the
//! way the app plays it.
//!
//! While a song plays, the app recalls a snapshot when the playhead crosses a point of the
//! snapshot lane ([`RackDoc::snapshot_lane_events`]): the model plans `RecallSnapshot` with every
//! category, applies it to the document, and the app sends what changed to the engine (params
//! the engine smooths, devices inserted and removed, a new routing). This module does the same
//! for an offline render, before it starts: each point of the lane becomes a [`TimedRecall`] at
//! the frame a live session would apply it at (one batch after the playhead reaches the point,
//! see [`live_frame`]; a bounce of a loop counts the laps), made by applying the same
//! command to a working copy of the document and turning the events it reports into the engine's
//! hot edits, in the order the app handles them (devices and params first, then the routing,
//! then the removals, which the engine only accepts once the routing has dropped the device).
//! The render loops schedule them at the first batch boundary at or after the frame
//! ([`crate::plugin_pass`]).
//!
//! The recalls are a function of the document alone, so a bounce stays the same for any worker
//! count, host block size and sandbox setting; a document without a snapshot lane plans none and
//! renders as it always did.
//!
//! What a bounce does not follow: a recall that adds or removes a plugin or a container effect,
//! or sets a param of a container, is skipped and a warning names it (the live app builds a
//! plugin's instance on a worker thread; a bounce would have to wait for it at a frame that
//! depends on the plugin). Markers that recall a snapshot are not followed either.

use crate::patch::Warning;
use crate::plugin_pass::TimedRecall;
use atomik_core::{BATCH, Ticks};
use atomik_device_sdk::Registry;
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, Graph, HotEdit, SongSnapshot, Topology,
};
use atomik_model::console::{SOLO_SILENCE_PARAM, VCA_DB_PARAM, VCA_MUTE_PARAM, VCA_SOLO_PARAM};
use atomik_model::{
    Command, ConsoleCommand, DeviceKey, Document, ExpandedRack, INTERFACE_KEY, RackDoc,
    SnapshotScope, SocketRef, SyncOp,
};
use std::collections::{BTreeMap, BTreeSet};

/// The frame a recall of the point at song frame `frame` takes effect at when a session plays the
/// song: the app reads the playhead once per batch, after it, and what it recalls reaches the
/// engine for the batch after that, so the recall of a point lands on the batch boundary after
/// the first batch that starts at or past the point.
fn live_frame(frame: u64) -> u64 {
    let batch = BATCH as u64;
    frame.div_ceil(batch) * batch + batch
}

/// The recalls of a bounce and what was left out.
#[derive(Debug, Default)]
pub(crate) struct Recalls {
    /// The steps, in frame order.
    pub steps: Vec<TimedRecall>,
    /// What a step that could not be followed was.
    pub warnings: Vec<Warning>,
}

/// The `(song frame, snapshot id)` of every recall of the lane: one for each point whose
/// snapshot differs from the point before it, in song order. With a loop count the laps repeat
/// the points inside the loop, and the render ends at the last loop end, so the points after it
/// are never reached; without one the points past the rendered length are left out.
pub(crate) fn recall_frames(
    doc: &RackDoc,
    snapshot: &SongSnapshot,
    frames: usize,
    loop_count: Option<u32>,
) -> Vec<(u64, u32)> {
    let events = doc.snapshot_lane_events();
    let sr = f64::from(doc.sample_rate);
    let frame = |tick: i64| snapshot.song_frame(Ticks(tick), sr);
    let mut out: Vec<(u64, u32)> = Vec::new();
    match loop_count.filter(|&n| n >= 1) {
        None => out.extend(
            events
                .iter()
                .map(|&(tick, id)| (frame(tick), id))
                .filter(|&(f, _)| f < frames as u64),
        ),
        Some(n) => {
            let (ls, le) = doc.song.loop_range;
            let lap = frame(le).saturating_sub(frame(ls));
            out.extend(
                events
                    .iter()
                    .filter(|&&(tick, _)| tick < le)
                    .map(|&(tick, id)| (frame(tick), id)),
            );
            for k in 1..u64::from(n) {
                out.extend(
                    events
                        .iter()
                        .filter(|&&(tick, _)| tick >= ls && tick < le)
                        .map(|&(tick, id)| (frame(tick) + k * lap, id)),
                );
            }
            out.sort_by_key(|&(f, _)| f);
        }
    }
    out
}

/// The routing of `expanded` as the app builds it for the engine: the devices the engine has,
/// under their ids, and the cables between their sockets (the compensation switches of the
/// document; latencies are filled in when the routing is submitted).
fn topology_of(
    doc: &RackDoc,
    expanded: &ExpandedRack,
    registry: &Registry,
    config: &EngineConfig,
    ids: &BTreeMap<DeviceKey, DeviceInstanceId>,
) -> Topology {
    let mut topology = Topology::new(config);
    topology.set_compensation(doc.console.compensation);
    topology.set_align_inputs(doc.console.align_returns);
    for device in expanded.devices.iter().filter(|d| !d.frozen) {
        if let (Some(id), Some(f)) = (ids.get(&device.key), registry.get(&device.type_id)) {
            let _ = topology.add_node(*id, f.descriptor);
        }
    }
    let endpoint = |topology: &Topology, socket: &SocketRef| -> Option<Endpoint> {
        let device = if socket.device.as_str() == INTERFACE_KEY {
            Graph::INTERFACE
        } else {
            *ids.get(&socket.device)?
        };
        let id = topology
            .descriptor(device)?
            .socket_by_key(&socket.socket)?
            .id;
        Some(Endpoint { device, socket: id })
    };
    for cable in &expanded.cables {
        if let (Some(from), Some(to)) = (
            endpoint(&topology, &cable.from),
            endpoint(&topology, &cable.to),
        ) {
            let _ = topology.connect(from, to);
        }
    }
    topology
}

/// The app-driven params of the document's console (`internal.solo_silence` and the VCA feeds)
/// that are not at their default, by device and param key.
fn internal_params(doc: &RackDoc, registry: &Registry) -> BTreeMap<(String, String), f32> {
    crate::project::console_options(doc, registry)
        .params
        .into_iter()
        .map(|(device, key, value)| ((device, key), value))
        .collect()
}

/// The default of an app-driven param (all of them are off at 0).
fn internal_default(param: &str) -> f32 {
    debug_assert!(
        [
            SOLO_SILENCE_PARAM,
            VCA_DB_PARAM,
            VCA_MUTE_PARAM,
            VCA_SOLO_PARAM
        ]
        .contains(&param)
    );
    0.0
}

/// Why a step cannot be followed, if it cannot: it adds or removes a plugin or a container, or
/// sets a param of a container (whose macros the app expands to the devices inside).
fn unsupported(before: &RackDoc, after: &RackDoc, ops: &[SyncOp]) -> Option<String> {
    let heavy = |doc: &RackDoc, key: &DeviceKey| {
        doc.device(key)
            .is_some_and(|d| d.plugin.is_some() || d.inner.is_some())
    };
    ops.iter().find_map(|op| match op {
        SyncOp::DeviceAdded(k) if heavy(after, k) => Some(format!("{k} would be added")),
        SyncOp::DeviceRemoved(k) if heavy(before, k) => Some(format!("{k} would be removed")),
        SyncOp::ParamChanged { key, .. }
            if after.device(key).is_some_and(|d| d.inner.is_some()) =>
        {
            Some(format!("a param of {key} would change"))
        }
        _ => None,
    })
}

/// Plans the recalls of the snapshot lane of `doc` for a bounce whose engine has `ids` (the
/// flat key to instance id of [`crate::project::expanded_ids`]) and `config`.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    config: &EngineConfig,
    ids: &BTreeMap<DeviceKey, DeviceInstanceId>,
    frames: &[(u64, u32)],
) -> Recalls {
    let mut out = Recalls::default();
    if frames.is_empty() {
        return out;
    }
    let mut ids = ids.clone();
    let mut next_id = ids.values().map(|i| i.0).max().map_or(1, |m| m + 1);
    let mut internal = internal_params(doc, registry);
    let mut working = Document::new(doc.clone());
    for &(frame, id) in frames {
        let before = working.doc().clone();
        let command = Command::Console(ConsoleCommand::RecallSnapshot {
            id,
            scope: SnapshotScope::ALL,
        });
        // A recall that changes nothing (the mix already is the snapshot) is refused: no step.
        let Ok(ops) = working.apply(command, registry) else {
            continue;
        };
        let after = working.doc().clone();
        if let Some(why) = unsupported(&before, &after, &ops) {
            let _ = working.undo();
            out.warnings.push(Warning {
                path: "console.snapshots".to_owned(),
                message: format!(
                    "the recall of snapshot {id} at frame {frame} was left out of the bounce: {why}"
                ),
            });
            continue;
        }
        let expanded = after.expand(registry);
        let mut step = TimedRecall {
            frame: live_frame(frame),
            edits: Vec::new(),
            routing: None,
            removals: Vec::new(),
        };
        let mut inserted: BTreeSet<DeviceKey> = BTreeSet::new();
        for op in &ops {
            match op {
                SyncOp::DeviceAdded(key) => {
                    let Some(device) = expanded.devices.iter().find(|d| d.key == *key) else {
                        continue;
                    };
                    let Some(instance) = registry.create(&device.type_id) else {
                        continue;
                    };
                    let desc = registry.get(&device.type_id).map(|f| f.descriptor);
                    let params = device
                        .params
                        .iter()
                        .filter_map(|(k, v)| Some((desc?.param_by_key(k)?.id, *v)))
                        .collect();
                    let instance_id = DeviceInstanceId(next_id);
                    next_id += 1;
                    ids.insert(key.clone(), instance_id);
                    step.edits.push(HotEdit::Insert {
                        id: instance_id,
                        device: instance,
                        params,
                    });
                    inserted.insert(key.clone());
                }
                SyncOp::ParamChanged { key, param, value } if !inserted.contains(key) => {
                    let pid = after
                        .device(key)
                        .and_then(|d| registry.get(&d.type_id))
                        .and_then(|f| f.descriptor.param_by_key(param))
                        .map(|p| p.id);
                    if let (Some(device), Some(param)) = (ids.get(key), pid) {
                        step.edits.push(HotEdit::Param {
                            device: *device,
                            param,
                            plain: *value,
                        });
                    }
                }
                SyncOp::DeviceRemoved(key) => {
                    if let Some(removed) = ids.remove(key) {
                        step.removals.push(removed);
                    }
                }
                _ => {}
            }
        }
        if ops.contains(&SyncOp::RoutingChanged) {
            step.routing = Some(topology_of(&after, &expanded, registry, config, &ids));
        }
        // The feeds the app drives from the console (solo, the VCA groups) follow the recall.
        let now = internal_params(&after, registry);
        let keys: BTreeSet<&(String, String)> = internal.keys().chain(now.keys()).collect();
        for (device, param) in keys {
            let value = now
                .get(&(device.clone(), param.clone()))
                .copied()
                .unwrap_or_else(|| internal_default(param));
            let was = internal
                .get(&(device.clone(), param.clone()))
                .copied()
                .unwrap_or_else(|| internal_default(param));
            if value.to_bits() == was.to_bits() {
                continue;
            }
            let pid = after
                .device(&DeviceKey(device.clone()))
                .and_then(|d| registry.get(&d.type_id))
                .and_then(|f| f.descriptor.param_by_key(param))
                .map(|p| p.id);
            if let (Some(target), Some(param)) = (ids.get(&DeviceKey(device.clone())), pid) {
                step.edits.push(HotEdit::Param {
                    device: *target,
                    param,
                    plain: value,
                });
            }
        }
        internal = now;
        out.steps.push(step);
    }
    out
}
