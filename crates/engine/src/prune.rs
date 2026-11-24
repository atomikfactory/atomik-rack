//! Dead-end pruning: which devices of a graph the compiled plan runs at all.
//!
//! A device whose output reaches nothing that can be observed is left out of the plan, when the
//! compile is asked to ([`crate::Topology::set_prune_dead_ends`], [`crate::Graph::set_prune_dead_ends`]:
//! the live session and a project bounce ask; a plain compile runs every device). The rule
//! is a pure function of the devices, the cables and the list of observers, so two compiles of
//! the same graph leave out the same devices, live, offline and with any worker count.
//!
//! **Live devices.** A device is live when something outside the audio graph can observe it, or
//! when a live device depends on it through any cable (audio, CV, notes, sidechain, key
//! inputs). The roots are
//!
//! * every device with a cable into the audio interface (the host outputs, a bounce's capture
//!   points and the record tap's gained channels are cables or interface channels),
//! * every device with a metered output (`Topology::add_meter`: console strips, track headers,
//!   the Audio I/O panel, the sources of CV cables for the activity lights),
//! * every pinned device ([`crate::Topology::pin`], [`crate::Graph::pin`]): the devices that
//!   publish something themselves (`Device::has_meter_values`, `scope_tap`, `midi_tap`, a plugin
//!   device), that report a latency that can change, that run on the audio thread only, and the
//!   devices a tap reads (a note tap, a record tap that resamples an output).
//!
//! Everything else is a dead end: it is in no step of the plan, costs nothing, and stays in the
//! arena with its parameters. It is brought to a clean state when a later routing runs it again
//! (see `RoutingPlan::install`).
//!
//! **The switch.** [`without`] turns the pruning off on the calling thread, for the tests that
//! compare a render with the pruning on against the same render with it off. It is not a
//! setting and nothing is saved; with it unused, a compile that asks to prune prunes.

use crate::graph::rules::CableMap;
use crate::graph::{DeviceInstanceId, Graph};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

thread_local! {
    static OFF: Cell<bool> = const { Cell::new(false) };
}

/// Whether compiles on this thread may leave dead ends out (always, outside the tests that call
/// [`without`]).
pub(crate) fn enabled() -> bool {
    !OFF.with(Cell::get)
}

/// Runs `f` with the dead-end pruning off on this thread: every compile inside keeps every
/// device in the plan, as before the pruning existed. Test and benchmark hook, not a setting.
#[doc(hidden)]
pub fn without<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            OFF.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(OFF.with(|c| c.replace(true)));
    f()
}

/// Whether the device itself publishes something that is read outside the audio graph, or runs
/// where the engine cannot see: it is an endpoint in its own right, whatever its cables.
///
/// * `has_meter_values`: the panels and the QA server read its values (an analyser has no audio
///   consumer at all);
/// * `scope_tap` and `midi_tap`: the interface reads the rings it fills (a MIDI out device has
///   no output socket);
/// * `variable_latency`: the latency report reads its changing latency (every plugin device
///   also answers the questions above, so a plugin device is always live);
/// * `pinned_to_audio_thread`: it runs where the engine must not move it.
///
/// Read once per device, on the non-RT side.
pub(crate) fn observed(device: &dyn atomik_device_sdk::Device) -> bool {
    device.has_meter_values()
        || device.scope_tap().is_some()
        || device.midi_tap().is_some()
        || device.variable_latency()
        || device.pinned_to_audio_thread()
}

/// The live devices among `ids`: the roots (`roots`, plus every device with a cable into the
/// interface) and everything a live device reads through any cable. The interface itself is
/// never in the set.
pub(crate) fn live_set(
    ids: &[DeviceInstanceId],
    cables: &CableMap,
    roots: impl Iterator<Item = DeviceInstanceId>,
) -> BTreeSet<DeviceInstanceId> {
    let known: BTreeSet<DeviceInstanceId> = ids.iter().copied().collect();
    let mut producers: BTreeMap<DeviceInstanceId, Vec<DeviceInstanceId>> = BTreeMap::new();
    let mut pending: Vec<DeviceInstanceId> = roots.collect();
    for c in cables.values() {
        producers
            .entry(c.to.device)
            .or_default()
            .push(c.from.device);
        if c.to.device == Graph::INTERFACE {
            pending.push(c.from.device);
        }
    }
    let mut live: BTreeSet<DeviceInstanceId> = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if id != Graph::INTERFACE && known.contains(&id) && live.insert(id) {
            pending.extend(producers.get(&id).map(Vec::as_slice).unwrap_or_default());
        }
    }
    live
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Cable, Endpoint};
    use atomik_device_sdk::SocketId;

    fn ep(d: u32, s: u16) -> Endpoint {
        Endpoint {
            device: DeviceInstanceId(d),
            socket: SocketId(s),
        }
    }

    fn cables(list: &[(Endpoint, Endpoint)]) -> CableMap {
        list.iter()
            .map(|&(from, to)| ((to, from), Cable { from, to }))
            .collect()
    }

    fn ids(n: u32) -> Vec<DeviceInstanceId> {
        (1..=n).map(DeviceInstanceId).collect()
    }

    fn set(list: &[u32]) -> BTreeSet<DeviceInstanceId> {
        list.iter().copied().map(DeviceInstanceId).collect()
    }

    #[test]
    fn a_chain_into_the_interface_is_live_and_a_stray_device_is_not() {
        let c = cables(&[(ep(1, 1), ep(2, 0)), (ep(2, 1), ep(0, 0))]);
        let live = live_set(&ids(3), &c, std::iter::empty());
        assert_eq!(live, set(&[1, 2]));
    }

    #[test]
    fn a_chain_that_feeds_nothing_is_dead() {
        let c = cables(&[(ep(1, 1), ep(2, 0)), (ep(2, 1), ep(3, 0))]);
        assert!(live_set(&ids(3), &c, std::iter::empty()).is_empty());
    }

    #[test]
    fn a_root_keeps_its_producers_alive_through_any_socket() {
        let c = cables(&[
            (ep(1, 1), ep(3, 0)),
            (ep(2, 1), ep(3, 5)),
            (ep(4, 1), ep(5, 0)),
        ]);
        let live = live_set(&ids(5), &c, [DeviceInstanceId(3)].into_iter());
        assert_eq!(live, set(&[1, 2, 3]));
    }

    #[test]
    fn the_interface_and_unknown_ids_are_never_live() {
        let c = cables(&[(ep(0, 1), ep(1, 0))]);
        let roots = [Graph::INTERFACE, DeviceInstanceId(9)];
        assert!(live_set(&ids(1), &c, roots.into_iter()).is_empty());
    }

    #[test]
    fn without_restores_the_switch_even_on_a_panic() {
        assert!(enabled());
        without(|| assert!(!enabled()));
        let _ = std::panic::catch_unwind(|| without(|| panic!("inside")));
        assert!(enabled());
    }
}
