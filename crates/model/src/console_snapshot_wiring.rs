//! The wiring a mixer snapshot keeps: where each strip plays and which effects sit on its insert
//! chain (see [`crate::console_snapshot`]).
//!
//! A snapshot made by this build holds, for the channels, the buses and the master, the
//! **output** (the master, a bus or an Audio I/O pair; a strip cabled by hand to anything else
//! holds none, and recall leaves it alone) and the **insert chain** (the effects in signal order;
//! the chain is derived from cables, see [`crate::console_insert`]). Each effect of the chain is
//! kept with its **state**: the whole device as it was (its type, name, params, a container's
//! inside, a plugin's saved state), in [`SnapshotWiring::effects`]. A snapshot that holds no
//! wiring at all ([`MixerState::wiring`] is `None`: one made by an older build, or the compare
//! slot of such a snapshot) leaves the routing and the chains as they are; a strip whose wiring
//! holds no effect states (`effects` is `None`: a snapshot of an older build) still moves the
//! cables between the devices the rack has, and nothing more.
//!
//! **Recall** re-cables what differs, in the same undo step as the params: the old chains come
//! off their loops first, then the effects the snapshot does not name leave the rack (one that has
//! automation in the song only leaves the chain and waits in the rack, uncabled), the ones it
//! names that the rack lacks are added again from their kept state (a plugin comes back as
//! one added by an undo does, loading off the interface thread), each strip's chain is cabled
//! from the snapshot's members, then the outputs are routed. An effect the rack has and the
//! snapshot names is the same instance: it is moved into place and its params are updated by
//! the params of the recall, never made anew. It never fails as a whole: a member that is not a
//! free stereo effect (it is on a return, feeds a channel, sits on the chain of a strip the
//! snapshot does not hold, or is cabled elsewhere), one whose key another device has taken or
//! whose type is not available (a plugin that is not installed), an effect that cannot be
//! deleted, an output bus that is gone, a route that would make a loop and an Audio I/O pair
//! that another cable has taken are skipped, and the strip keeps what it has there
//! ([`RackDoc::snapshot_notes`] names what it can).

use crate::console::{OutputDest, is_console_type, output_dest};
use crate::console_bus::set_output;
use crate::console_cmd::Planner;
use crate::console_insert::{insert_chain, stereo_effect_type, unwire, wire_chain};
use crate::console_snapshot::{MixerState, SnapshotScope, bits};
use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDevice, RackDoc, rules};
use atomik_device_sdk::Registry;
use std::collections::BTreeMap;

/// The wiring of one strip as a snapshot keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotWiring {
    /// Where the strip played: `Master`, `Bus` or `AudioIo`. `None` for a strip cabled by hand
    /// elsewhere, and always for the master (recall leaves it alone).
    pub output: Option<OutputDest>,
    /// The effects on the strip's insert chain, in signal order (empty for no chain).
    pub inserts: Vec<DeviceKey>,
    /// The effects of `inserts` as they were, one device each in the same order: what recall
    /// adds back when the rack no longer has one. `None` for a snapshot that holds only the keys
    /// (an older build), which never adds or removes an effect.
    pub effects: Option<Vec<RackDevice>>,
}

/// `device` as a snapshot keeps it: sparse params (one at its default is dropped, as in a snapshot's
/// other params) and no place in the rack, so the same effect compares equal while only its place
/// or fold changes.
fn kept_effect(registry: &Registry, device: &RackDevice) -> RackDevice {
    let mut kept = device.clone();
    if let Some(desc) = registry.get(&kept.type_id).map(|f| f.descriptor) {
        kept.params.retain(|k, v| {
            desc.param_by_key(k)
                .is_none_or(|p| p.clamp(*v).to_bits() != p.default.to_bits())
        });
    }
    kept.column = 0;
    kept.folded = false;
    kept
}

/// What recalling the wiring of a state changes, computed without touching the document.
#[derive(Debug, Default)]
pub(crate) struct WiringChanges {
    /// The strips whose output changes, with the new destination.
    outputs: Vec<(DeviceKey, OutputDest)>,
    /// The strips whose insert chain changes, with the new chain.
    chains: Vec<(DeviceKey, Vec<DeviceKey>)>,
    /// The effects of the new chains that the rack lacks, as the snapshot kept them.
    adds: Vec<RackDevice>,
    /// The effects of the old chains that no chain of the snapshot names: they leave the rack.
    removes: Vec<DeviceKey>,
}

impl WiringChanges {
    pub(crate) fn is_empty(&self) -> bool {
        self.outputs.is_empty()
            && self.chains.is_empty()
            && self.adds.is_empty()
            && self.removes.is_empty()
    }
}

const IN: [&str; 2] = ["in_l", "in_r"];
const OUT: [&str; 2] = ["out_l", "out_r"];

impl RackDoc {
    /// The strips a snapshot keeps the wiring of, in console order: the channels, the buses and
    /// the master.
    fn wiring_strips(&self) -> Vec<DeviceKey> {
        self.console
            .strip_keys()
            .chain(self.console.master.iter())
            .cloned()
            .collect()
    }

    /// True when `strip` has the insert loop (`insert_send_*` and `insert_return_*` sockets).
    fn has_insert_loop(&self, registry: &Registry, strip: &DeviceKey) -> bool {
        self.device(strip)
            .and_then(|d| registry.get(&d.type_id))
            .is_some_and(|f| {
                [
                    "insert_send_l",
                    "insert_send_r",
                    "insert_return_l",
                    "insert_return_r",
                ]
                .iter()
                .all(|k| f.descriptor.socket_by_key(k).is_some())
            })
    }

    /// The wiring now of every strip.
    pub(crate) fn capture_wiring(
        &self,
        registry: &Registry,
    ) -> BTreeMap<DeviceKey, SnapshotWiring> {
        let mut out = BTreeMap::new();
        for strip in self.wiring_strips() {
            if self.device(&strip).is_none() {
                continue;
            }
            let output = match output_dest(self, &strip) {
                dest @ (OutputDest::Master | OutputDest::Bus(_) | OutputDest::AudioIo(_)) => {
                    Some(dest)
                }
                _ => None,
            };
            let inserts = if self.has_insert_loop(registry, &strip) {
                insert_chain(self, registry, &strip)
            } else {
                Vec::new()
            };
            let effects = inserts
                .iter()
                .filter_map(|k| self.device(k))
                .map(|d| kept_effect(registry, d))
                .collect();
            out.insert(
                strip,
                SnapshotWiring {
                    output,
                    inserts,
                    effects: Some(effects),
                },
            );
        }
        out
    }

    /// True when `effect` can be an insert of a snapshot recall: a stereo effect of the rack that
    /// is no console device, return or channel source, and that is either on an insert chain
    /// already or has no cable on its stereo sockets.
    fn recall_member(&self, registry: &Registry, effect: &DeviceKey, in_chain: bool) -> bool {
        let Some(device) = self.device(effect) else {
            return false;
        };
        let stereo = registry
            .get(&device.type_id)
            .is_some_and(|f| stereo_effect_type(f.descriptor));
        if !stereo
            || is_console_type(&device.type_id)
            || self.console.return_slot(effect).is_some()
            || self.console.channel_of(effect).is_some()
        {
            return false;
        }
        in_chain
            || !self.cables.iter().any(|c| {
                (&c.to.device == effect && IN.contains(&c.to.socket.as_str()))
                    || (&c.from.device == effect && OUT.contains(&c.from.socket.as_str()))
            })
    }

    /// True when the rack can take `device` back as an insert: no device has its key, and its
    /// type is a stereo effect the registry has (a plugin only while it is registered).
    fn can_recreate(&self, registry: &Registry, device: &RackDevice) -> bool {
        self.device(&device.key).is_none()
            && !is_console_type(&device.type_id)
            && rules::check_new_key(self, &device.key.0).is_ok()
            && registry
                .get(&device.type_id)
                .is_some_and(|f| stereo_effect_type(f.descriptor))
            && device.plugin.is_some() == registry.is_plugin(&device.type_id)
    }

    /// What recalling the wiring of `state` with `scope` changes.
    pub(crate) fn wiring_changes(
        &self,
        registry: &Registry,
        state: &MixerState,
        scope: SnapshotScope,
    ) -> WiringChanges {
        let mut changes = WiringChanges::default();
        let Some(wiring) = state.wiring.as_ref().filter(|_| scope.has(bits::ROUTING)) else {
            return changes;
        };
        let strips = self.wiring_strips();
        let held = |k: &DeviceKey| wiring.contains_key(k) && self.has_insert_loop(registry, k);
        // The effects on the chains of the strips the snapshot does not hold stay where they are.
        let mut taken: Vec<DeviceKey> = strips
            .iter()
            .filter(|k| !held(k))
            .flat_map(|k| insert_chain(self, registry, k))
            .collect();
        for strip in strips.iter().filter(|k| wiring.contains_key(*k)) {
            let w = &wiring[strip];
            if let Some(dest) = &w.output
                && output_dest(self, strip) != *dest
                && crate::console_bus::check_dest(self, strip, dest).is_ok()
            {
                changes.outputs.push((strip.clone(), dest.clone()));
            }
            if !held(strip) {
                continue;
            }
            let now = insert_chain(self, registry, strip);
            let mut wanted: Vec<DeviceKey> = Vec::new();
            for m in &w.inserts {
                if taken.contains(m) || wanted.contains(m) {
                    continue;
                }
                let on_a_chain = strips
                    .iter()
                    .any(|k| insert_chain(self, registry, k).contains(m));
                let kept = w.effects.iter().flatten().find(|d| d.key == *m);
                // A device of another type under the same key is not the effect the snapshot kept.
                let same_type =
                    kept.is_none_or(|d| self.device(m).is_some_and(|x| x.type_id == d.type_id));
                if same_type && self.recall_member(registry, m, on_a_chain) {
                    wanted.push(m.clone());
                } else if let Some(kept) = kept.filter(|d| self.can_recreate(registry, d)) {
                    changes.adds.push(kept.clone());
                    wanted.push(m.clone());
                }
            }
            taken.extend(wanted.iter().cloned());
            if wanted != now {
                changes.chains.push((strip.clone(), wanted));
            }
        }
        // A strip whose snapshot kept the states of its effects says exactly which ones play:
        // the others, that no chain of the snapshot names, leave the rack. One that has a track in
        // the song (its automation) only leaves the chain: deleting it would delete the track, and
        // a recall back could not bring it back. It waits in the rack, uncabled, and takes its place
        // in the chain again when a snapshot names it.
        for strip in strips
            .iter()
            .filter(|k| held(k) && wiring[*k].effects.is_some())
        {
            for e in insert_chain(self, registry, strip) {
                if !taken.contains(&e)
                    && !changes.removes.contains(&e)
                    && self.song.track(&e).is_none()
                {
                    changes.removes.push(e);
                }
            }
        }
        changes
    }
}

/// Inserts `device`, an effect a snapshot kept, into the rack right after `after` under its own
/// key. Nothing is cabled.
fn insert_kept(
    p: &mut Planner<'_>,
    after: &DeviceKey,
    device: &RackDevice,
) -> Result<(), ModelError> {
    rules::check_new_key(p.doc(), &device.key.0)?;
    let index = rules::index_of(p.doc(), after)? + 1;
    let mut device = device.clone();
    device.name = rules::unique_name(p.doc(), &device.name);
    p.push(Edit::Insert {
        index: rules::top_index(p.doc(), index),
        device,
        cables: Vec::new(),
    });
    Ok(())
}

/// Pushes the edits of `changes`: the changed chains come off their loops, the effects that
/// leave are deleted (one that cannot be stays in the rack, uncabled), the effects that come
/// back are inserted after their predecessor in the chain, each chain is cabled anew from the
/// devices that exist (a chain that cannot be is put back as it was) and a strip that gained an
/// effect stacks its chain under it again; then the outputs are routed. A route that does not
/// fit is skipped.
pub(crate) fn apply(p: &mut Planner<'_>, changes: WiringChanges) {
    let registry = p.registry;
    let olds: Vec<Vec<DeviceKey>> = changes
        .chains
        .iter()
        .map(|(strip, _)| insert_chain(p.doc(), registry, strip))
        .collect();
    for ((strip, _), old) in changes.chains.iter().zip(&olds) {
        unwire(p, strip, old);
    }
    for key in &changes.removes {
        let _ = p.attempt(|p| crate::console_cmd::remove_device(p, key));
    }
    for ((strip, new), old) in changes.chains.iter().zip(&olds) {
        let mut grew = false;
        for k in new {
            let Some(kept) = changes.adds.iter().find(|a| a.key == *k) else {
                continue;
            };
            let after = new
                .iter()
                .take_while(|m| *m != k)
                .filter(|m| p.doc().device(m).is_some())
                .last()
                .unwrap_or(strip)
                .clone();
            grew |= p.attempt(|p| insert_kept(p, &after, kept)).is_ok();
        }
        let present = |p: &Planner<'_>, chain: &[DeviceKey]| -> Vec<DeviceKey> {
            chain
                .iter()
                .filter(|k| p.doc().device(k).is_some())
                .cloned()
                .collect()
        };
        let (new, old) = (present(p, new), present(p, old));
        if p.attempt(|p| wire_chain(p, strip, &new)).is_err() {
            let _ = p.attempt(|p| wire_chain(p, strip, &old));
        }
        if grew {
            crate::console_insert::stack(p, strip);
        }
    }
    for (strip, dest) in &changes.outputs {
        let _ = p.attempt(|p| set_output(p, strip, dest));
    }
}

/// The wiring at position `at` between two states: the strips of one side keep its wiring, those
/// of both hold the nearer one's (the choice flips at the half way point, as a switch does).
pub(crate) fn morph(
    a: &Option<BTreeMap<DeviceKey, SnapshotWiring>>,
    b: &Option<BTreeMap<DeviceKey, SnapshotWiring>>,
    at: f32,
) -> Option<BTreeMap<DeviceKey, SnapshotWiring>> {
    match (a, b) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (Some(x), Some(y)) => {
            let (far, near) = if at < 0.5 { (y, x) } else { (x, y) };
            let mut out = far.clone();
            out.extend(near.iter().map(|(k, v)| (k.clone(), v.clone())));
            Some(out)
        }
    }
}

impl RackDoc {
    /// What recalling the wiring of snapshot state `state` would skip because it cannot: one
    /// line each (see the module documentation).
    pub(crate) fn wiring_notes(&self, state: &MixerState) -> Vec<String> {
        let Some(wiring) = &state.wiring else {
            return Vec::new();
        };
        let mut notes = Vec::new();
        for (strip, w) in wiring {
            if self.device(strip).is_none() {
                continue;
            }
            if let Some(OutputDest::Bus(b)) = &w.output
                && self.console.bus_index(b).is_none()
            {
                notes.push(format!(
                    "{b} (the output of {strip}) is no longer in the console"
                ));
            }
            // An effect whose state the snapshot kept is added back by a recall.
            let kept = |m: &DeviceKey| w.effects.iter().flatten().any(|d| d.key == *m);
            for m in w
                .inserts
                .iter()
                .filter(|m| self.device(m).is_none() && !kept(m))
            {
                notes.push(format!("{m} (an insert of {strip}) no longer exists"));
            }
        }
        notes
    }
}
