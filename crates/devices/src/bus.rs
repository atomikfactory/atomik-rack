//! `atomik.bus`: a console output bus strip.
//!
//! The same strip as [`atomik.channel`](crate::channel): one DSP implementation, the same
//! params (same keys, ids and defaults, so channel settings copy across), the same insert loop,
//! eight sends, dynamics side-chain (`key_in`, `gr_cv_out`), CV inputs, parallel output and
//! latency behaviour. The only differences are the input: `in_l`/`in_r` are replaced by the
//! stereo **summing** input `bus_l`/`bus_r` (`Direction::SummingInput`, like the master's), so
//! any number of channel or bus outputs can be cabled to it; and the follow gain. A bus with
//! signal on `bus_l`/`bus_r` and default params is bit-identical to a channel fed the same
//! signal on `in_l`/`in_r`.
//!
//! A bus is created by the console, not from the device browser. Its outputs are cabled like a
//! channel's: `out_l/r` to the master's (or another bus's) `bus_l/r`, `send{k}_l/r` to the
//! master's `send_bus{k}_l/r`.
//!
//! ## Follow gain
//! `follow_out` (a control output) carries the linear gain the bus applies to its signal: the
//! fader (with the VCA offset) times the mute, sample by sample, times the gain of the bus above
//! when `follow_in` is cabled, so it cascades through nested buses. A strip that plays into the
//! bus gets it on its own `follow_in` and scales its post-fader sends by it: the send reaches the
//! master with the level the bus gives the strip's main signal, mute included. Pre-fader sends
//! are untouched. The gain comes in a stage of its own, ahead of the bus's two audio stages, that
//! reads nothing but the bus's own state, so cabling it back to the strips that feed the bus is
//! not a loop. It is the gain the bus will apply in the same batch (the smoothers run ahead on a
//! copy), so the send and the main signal stay sample-aligned; a fader CV (applied in the audio
//! stage) shows up in the follow gain one batch late. Nothing is computed while `follow_out` is
//! not cabled.

use crate::channel::{
    Channel, PARAMS, S_FOLLOW_IN, S_IN_L, S_IN_R, SOCKET_COUNT, SOCKETS, STAGES, socket,
};
use atomik_device_sdk::{
    DeviceDescriptor, DeviceFactory, DeviceKind, Direction, SocketDescriptor, SocketKind,
};

/// Number of sockets of the bus: the channel's and `follow_out`.
const BUS_SOCKET_COUNT: usize = SOCKET_COUNT + 1;
/// `follow_out`, after the channel's sockets.
pub(crate) const S_FOLLOW_OUT: u16 = SOCKET_COUNT as u16;

/// The channel's sockets with `in_l`/`in_r` (ids 0 and 1) replaced by the summing input, and
/// `follow_out` appended.
static BUS_SOCKETS: [SocketDescriptor; BUS_SOCKET_COUNT] = {
    let mut out = [SOCKETS[0]; BUS_SOCKET_COUNT];
    let mut i = 0;
    while i < SOCKET_COUNT {
        out[i] = SOCKETS[i];
        i += 1;
    }
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
    out[S_FOLLOW_OUT as usize] = socket(
        S_FOLLOW_OUT,
        "follow_out",
        "Follow out",
        SocketKind::Cv,
        Direction::Output,
    );
    out
};

/// The channel's stages moved up by one (the bus has three), with the follow gain in stage 0.
static BUS_STAGES: [u8; BUS_SOCKET_COUNT] = {
    let mut out = [0u8; BUS_SOCKET_COUNT];
    let mut i = 0;
    while i < SOCKET_COUNT {
        out[i] = STAGES[i] + 1;
        i += 1;
    }
    out[S_FOLLOW_IN as usize] = 0;
    out[S_FOLLOW_OUT as usize] = 0;
    out
};

/// Static description of the output bus strip.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &BUS_STAGES,
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
