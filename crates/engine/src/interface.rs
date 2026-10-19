//! The AudioInterface pseudo-device descriptor.
//!
//! **Approach:** a process-wide, leaked-once registry keyed by `(inputs, outputs)`. The first
//! request for a channel combination builds the socket table (`in_1..in_N` as Audio outputs,
//! then `out_1..out_M` as Audio inputs), leaks it with `Box::leak` and caches the resulting
//! `&'static DeviceDescriptor` in a `OnceLock<Mutex<BTreeMap<..>>>`. Socket keys and names come
//! from `'static` string tables, so nothing is formatted at runtime. At most
//! `33 × 33` descriptors can ever be leaked (a few hundred KiB worst case; in practice one).
//! This runs only in `Graph::new` (non-RT); the render path never touches the registry.

use atomik_device_sdk::{
    DeviceDescriptor, DeviceKind, Direction, SocketDescriptor, SocketId, SocketKind,
};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// Largest number of host input or output channels the AudioInterface supports.
pub const MAX_INTERFACE_CHANNELS: u16 = 32;

/// Type id of the AudioInterface pseudo-device.
pub const INTERFACE_TYPE_ID: &str = "atomik.audio-interface";

macro_rules! numbered {
    ($prefix:literal; $($n:literal)*) => { [$(concat!($prefix, $n)),*] };
}

const IN_KEYS: [&str; 32] = numbered!("in_"; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20
    21 22 23 24 25 26 27 28 29 30 31 32);
const OUT_KEYS: [&str; 32] = numbered!("out_"; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20
    21 22 23 24 25 26 27 28 29 30 31 32);
const IN_NAMES: [&str; 32] = numbered!("Input "; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19
    20 21 22 23 24 25 26 27 28 29 30 31 32);
const OUT_NAMES: [&str; 32] = numbered!("Output "; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18
    19 20 21 22 23 24 25 26 27 28 29 30 31 32);

type Cache = Mutex<BTreeMap<(u16, u16), &'static DeviceDescriptor>>;

/// The AudioInterface descriptor for `inputs` host inputs and `outputs` host outputs.
///
/// Socket `SocketId(k)` for `k < inputs` is `in_{k+1}` (Audio **output**: host input into the
/// graph); socket `SocketId(inputs + k)` is `out_{k+1}` (Audio **input**: graph to host output).
///
/// # Panics
/// If either count exceeds [`MAX_INTERFACE_CHANNELS`].
pub fn interface_descriptor(inputs: u16, outputs: u16) -> &'static DeviceDescriptor {
    assert!(
        inputs <= MAX_INTERFACE_CHANNELS && outputs <= MAX_INTERFACE_CHANNELS,
        "the AudioInterface supports at most {MAX_INTERFACE_CHANNELS} inputs and outputs"
    );
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry((inputs, outputs))
        .or_insert_with(|| build(inputs, outputs))
}

fn build(inputs: u16, outputs: u16) -> &'static DeviceDescriptor {
    let ins = (0..inputs).map(|k| (k, IN_KEYS, IN_NAMES, Direction::Output));
    let outs = (0..outputs).map(|k| (k, OUT_KEYS, OUT_NAMES, Direction::Input));
    let sockets: Vec<SocketDescriptor> = ins
        .chain(outs)
        .enumerate()
        .map(|(index, (k, keys, names, direction))| SocketDescriptor {
            id: SocketId(u16::try_from(index).expect("at most 64 interface sockets")),
            key: keys[usize::from(k)],
            name: names[usize::from(k)],
            kind: SocketKind::Audio,
            direction,
        })
        .collect();
    Box::leak(Box::new(DeviceDescriptor {
        stages: &[],
        type_id: INTERFACE_TYPE_ID,
        name: "Audio I/O",
        kind: DeviceKind::Interface,
        version: 1,
        params: &[],
        sockets: Box::leak(sockets.into_boxed_slice()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_descriptor_has_ordered_sockets_and_validates() {
        let d = interface_descriptor(2, 3);
        assert_eq!(d.validate(), Ok(()));
        assert_eq!(d.kind, DeviceKind::Interface);
        let keys: Vec<_> = d.sockets.iter().map(|s| s.key).collect();
        assert_eq!(keys, ["in_1", "in_2", "out_1", "out_2", "out_3"]);
        assert!(
            d.sockets[..2]
                .iter()
                .all(|s| s.direction == Direction::Output)
        );
        assert!(
            d.sockets[2..]
                .iter()
                .all(|s| s.direction == Direction::Input)
        );
        assert!(d.sockets.iter().all(|s| s.kind == SocketKind::Audio));
    }

    #[test]
    fn the_descriptor_carries_the_user_facing_name() {
        // One name everywhere a user can read it.
        assert_eq!(interface_descriptor(2, 2).name, "Audio I/O");
    }

    #[test]
    fn interface_descriptor_is_cached_per_channel_count() {
        assert!(std::ptr::eq(
            interface_descriptor(4, 4),
            interface_descriptor(4, 4)
        ));
        assert!(!std::ptr::eq(
            interface_descriptor(4, 4),
            interface_descriptor(4, 5)
        ));
        let max = interface_descriptor(32, 32);
        assert_eq!(max.validate(), Ok(()));
        assert_eq!(max.sockets[63].key, "out_32");
        assert_eq!(interface_descriptor(0, 0).sockets.len(), 0);
    }

    #[test]
    #[should_panic(expected = "at most 32")]
    fn interface_descriptor_too_many_channels_panics() {
        let _ = interface_descriptor(33, 2);
    }
}
