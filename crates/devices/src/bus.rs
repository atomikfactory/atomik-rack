//! `atomik.bus`: a console output bus strip.
//!
//! The same strip as [`atomik.channel`](crate::channel): one DSP implementation, the same
//! params (same keys, ids and defaults, so channel settings copy across), the same insert loop,
//! eight sends, dynamics side-chain (`key_in`, `gr_cv_out`), CV inputs, stages and latency
//! behaviour. The only difference is the input: `in_l`/`in_r` are replaced by the stereo
//! **summing** input `bus_l`/`bus_r` (`Direction::SummingInput`, like the master's), so any
//! number of channel or bus outputs can be cabled to it. A bus with signal on `bus_l`/`bus_r`
//! and default params is bit-identical to a channel fed the same signal on `in_l`/`in_r`.
//!
//! A bus is created by the console, not from the device browser. Its outputs are cabled like a
//! channel's: `out_l/r` to the master's (or another bus's) `bus_l/r`, `send{k}_l/r` to the
//! master's `send_bus{k}_l/r`.

use crate::channel::{Channel, PARAMS, S_IN_L, S_IN_R, SOCKET_COUNT, SOCKETS, STAGES, socket};
use atomik_device_sdk::{
    DeviceDescriptor, DeviceFactory, DeviceKind, Direction, SocketDescriptor, SocketKind,
};

/// The channel's sockets with `in_l`/`in_r` (ids 0 and 1) replaced by the summing input.
static BUS_SOCKETS: [SocketDescriptor; SOCKET_COUNT] = {
    let mut out = SOCKETS;
    out[S_IN_L as usize] = socket(
        S_IN_L,
        "bus_l",
        "Bus L",
        SocketKind::Audio,
        Direction::SummingInput,
    );
    out[S_IN_R as usize] = socket(
        S_IN_R,
        "bus_r",
        "Bus R",
        SocketKind::Audio,
        Direction::SummingInput,
    );
    out
};

/// Static description of the output bus strip.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &STAGES,
    type_id: "atomik.bus",
    name: "Output Bus",
    kind: DeviceKind::Mixer,
    version: 1,
    params: &PARAMS,
    sockets: &BUS_SOCKETS,
};

/// The bus strip: the channel strip reporting [`DESCRIPTOR`].
pub fn new() -> Channel {
    Channel::with_descriptor(&DESCRIPTOR)
}

/// Factory registering the bus.
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(new()),
    }
}

#[cfg(test)]
mod tests;
