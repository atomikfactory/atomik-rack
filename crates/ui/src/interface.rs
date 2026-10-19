//! The audio interface pseudo-device shown at the top of the rack (key
//! [`atomik_model::INTERFACE_KEY`]). Its back panel carries `in_1..in_M`: the host inputs,
//! audio *outputs* seen from the graph (signals into the rack), then `out_1..out_N`: audio
//! *inputs* seen from the graph (signals sent to the host outputs), matching the engine's
//! interface sockets and the model's cable validation.

use atomik_device_sdk::{
    DeviceDescriptor, DeviceKind, Direction, SocketDescriptor, SocketId, SocketKind,
};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// Type id used for the interface panel.
pub const INTERFACE_TYPE_ID: &str = "atomik.audio-interface";
/// Most inputs and most outputs shown (each).
pub const MAX_OUTPUTS: usize = 32;

macro_rules! numbered {
    ($prefix:literal; $($n:literal)*) => { [$(concat!($prefix, $n)),*] };
}

const OUT_KEYS: [&str; MAX_OUTPUTS] = numbered!("out_"; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17
    18 19 20 21 22 23 24 25 26 27 28 29 30 31 32);
const OUT_NAMES: [&str; MAX_OUTPUTS] = numbered!("Output "; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
    17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32);
const IN_KEYS: [&str; MAX_OUTPUTS] = numbered!("in_"; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17
    18 19 20 21 22 23 24 25 26 27 28 29 30 31 32);
const IN_NAMES: [&str; MAX_OUTPUTS] = numbered!("Input "; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
    17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32);

type Cache = Mutex<BTreeMap<(usize, usize), &'static [SocketDescriptor]>>;

/// The interface socket table for `inputs` host inputs then `outputs` host outputs (each
/// capped at [`MAX_OUTPUTS`]), built once per combination and kept for the process lifetime
/// (at most 33 x 33 small tables; in practice one).
fn sockets(inputs: usize, outputs: usize) -> &'static [SocketDescriptor] {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.entry((inputs, outputs)).or_insert_with(|| {
        let ins = (0..inputs).map(|k| (IN_KEYS[k], IN_NAMES[k], Direction::Output));
        let outs = (0..outputs).map(|k| (OUT_KEYS[k], OUT_NAMES[k], Direction::Input));
        let table: Vec<SocketDescriptor> = ins
            .chain(outs)
            .enumerate()
            .map(|(i, (key, name, direction))| SocketDescriptor {
                id: SocketId(i as u16),
                key,
                name,
                kind: SocketKind::Audio,
                direction,
            })
            .collect();
        Box::leak(table.into_boxed_slice())
    })
}

/// The interface descriptor for `inputs` host inputs (`in_k`, audio outputs of the panel) and
/// `outputs` host outputs (`out_k`, audio inputs of the panel), each at most [`MAX_OUTPUTS`].
/// Sockets are listed inputs first, as the engine builds them.
pub fn interface_descriptor(inputs: u16, outputs: u16) -> DeviceDescriptor {
    DeviceDescriptor {
        stages: &[],
        type_id: INTERFACE_TYPE_ID,
        name: "Audio I/O",
        kind: DeviceKind::Interface,
        version: 1,
        params: &[],
        sockets: sockets(
            usize::from(inputs).min(MAX_OUTPUTS),
            usize::from(outputs).min(MAX_OUTPUTS),
        ),
    }
}

/// Type id of the INPUT pseudo-panel at the top of a container's inside.
pub const BOUNDARY_IN_TYPE_ID: &str = "atomik.boundary-in";
/// Type id of the OUTPUT pseudo-panel at the bottom of a container's inside.
pub const BOUNDARY_OUT_TYPE_ID: &str = "atomik.boundary-out";

const fn boundary_socket(
    id: u16,
    key: &'static str,
    name: &'static str,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind: SocketKind::Audio,
        direction,
    }
}

static BOUNDARY_IN_SOCKETS: [SocketDescriptor; 2] = [
    boundary_socket(0, "in_l", "Input L", Direction::Output),
    boundary_socket(1, "in_r", "Input R", Direction::Output),
];
static BOUNDARY_OUT_SOCKETS: [SocketDescriptor; 2] = [
    boundary_socket(0, "out_l", "Output L", Direction::Input),
    boundary_socket(1, "out_r", "Output R", Direction::Input),
];

/// The INPUT pseudo-panel of a container's inside: what arrives at the container (`in_l`,
/// `in_r`), audio *outputs* seen from the inner graph, like the interface's host inputs. Cables
/// use the pseudo-device key [`atomik_model::BOUNDARY_KEY`].
pub fn boundary_in_descriptor() -> DeviceDescriptor {
    DeviceDescriptor {
        stages: &[],
        type_id: BOUNDARY_IN_TYPE_ID,
        name: "Container input",
        kind: DeviceKind::Interface,
        version: 1,
        params: &[],
        sockets: &BOUNDARY_IN_SOCKETS,
    }
}

/// The OUTPUT pseudo-panel of a container's inside: what leaves through the container (`out_l`,
/// `out_r`), audio *inputs* seen from the inner graph.
pub fn boundary_out_descriptor() -> DeviceDescriptor {
    DeviceDescriptor {
        stages: &[],
        type_id: BOUNDARY_OUT_TYPE_ID,
        name: "Container output",
        kind: DeviceKind::Interface,
        version: 1,
        params: &[],
        sockets: &BOUNDARY_OUT_SOCKETS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boundary_panels_match_the_models_boundary_sockets() {
        let (i, o) = (boundary_in_descriptor(), boundary_out_descriptor());
        assert_eq!(i.validate(), Ok(()));
        assert_eq!(o.validate(), Ok(()));
        let ours: Vec<_> = i
            .sockets
            .iter()
            .chain(o.sockets)
            .map(|s| (s.key, s.kind, s.direction))
            .collect();
        assert_eq!(ours, atomik_model::boundary_sockets().to_vec());
    }

    #[test]
    fn interface_has_numbered_inputs_then_outputs() {
        let d = interface_descriptor(3, 2);
        assert_eq!(d.validate(), Ok(()));
        let keys: Vec<_> = d.sockets.iter().map(|s| s.key).collect();
        assert_eq!(keys, ["in_1", "in_2", "in_3", "out_1", "out_2"]);
        // Host inputs are outputs seen from the graph, host outputs are inputs.
        assert!(
            d.sockets[..3]
                .iter()
                .all(|s| s.direction == Direction::Output)
        );
        assert!(
            d.sockets[3..]
                .iter()
                .all(|s| s.direction == Direction::Input)
        );
        assert_eq!(d.sockets[0].name, "Input 1");
        assert_eq!(d.sockets[4].name, "Output 2");
    }

    #[test]
    fn interface_counts_are_capped() {
        assert_eq!(interface_descriptor(99, 99).sockets.len(), 2 * MAX_OUTPUTS);
        assert_eq!(interface_descriptor(99, 99).validate(), Ok(()));
        assert_eq!(interface_descriptor(0, 0).sockets.len(), 0);
        assert_eq!(interface_descriptor(2, 0).sockets.len(), 2);
    }

    #[test]
    fn interface_keys_match_the_model_naming() {
        let d = interface_descriptor(32, 32);
        assert_eq!(d.sockets[31].key, "in_32");
        assert_eq!(d.sockets[63].key, "out_32");
        let ids: std::collections::BTreeSet<_> = d.sockets.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 64, "unique ids");
    }

    #[test]
    fn descriptors_are_cached_per_count() {
        assert!(std::ptr::eq(
            interface_descriptor(2, 2).sockets,
            interface_descriptor(2, 2).sockets
        ));
    }
}
