//! `atomik.midi-out`: sends notes and controllers to a MIDI output port, so a hardware synth (or
//! any program listening on a virtual port) plays what a track holds.
//!
//! The device makes no sound and has no audio outputs. It has a notes input, which a track can
//! play or a player can feed, and eight CV inputs that become controller messages. It turns
//! everything it receives into MIDI bytes, stamps each with the engine host frame it belongs to
//! and queues them on a [`MidiTap`]; the app's MIDI output thread (see `ARCHITECTURE.md`, the
//! threading table) reads the queue, converts frames to wall-clock times and sends the bytes to
//! the port chosen on the device (a document field, not a param: see `atomik_model::MidiOutDoc`).
//! The device itself never touches a port or a clock, so `process` allocates and locks nothing.
//!
//! * **Channel.** Every message goes out on `channel` (1 to 16).
//! * **Transpose.** `transpose` shifts note keys. A result outside 0..=127 is dropped, not
//!   clamped. The device remembers which key every sounding note went out on, so a change of
//!   `transpose` or `channel` while a key is held still releases the note that was started.
//! * **Controllers.** Controller, pitch bend and pressure events pass through on the channel. Each
//!   of the eight CV inputs, when connected, drives one controller: the value sent is
//!   `round(127 * (cv * scale + offset))` clamped to 0..=127, sent when it changes and at most
//!   100 times a second per input (the latest value is never held back for longer).
//! * **Program and bank.** A `program` of -1 sends nothing. Otherwise a program change goes out
//!   (preceded by the bank select pair when `bank` is not -1) when the device starts and whenever
//!   `program` or `bank` changes.
//! * **Latency.** `latency_ms` moves every message of the device in time: a positive value sends
//!   later, a negative value earlier (to line a hardware synth's audio, which comes back through
//!   an input, up with the rest of the song). A message whose time is already past when the
//!   output thread reads it goes out at once.
//! * **Releasing.** An `AllOff` (the engine sends one on stop, on a locate and at a loop wrap)
//!   releases every note this device started, one note off each, then sends All Notes Off on the
//!   channels it used. A `reset` does the same at the start of the next batch.
//! * **Offline.** A bounce never sends MIDI: in [`RunMode::Offline`] the device is silent, so a
//!   project with and without it renders the same bits (it has no audio output either way).
//!
//! The device reports no processing latency: it has no audio path, so there is nothing for the
//! rest of the rack to compensate.

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, MidiEvent, MidiTap,
    PrepareInfo, ProcessContext, RunMode, SocketDescriptor, SocketId, SocketKind, velocity_to_7bit,
};
use std::sync::Arc;

const NOTES_IN: SocketId = SocketId(0);

macro_rules! cv_in {
    ($n:expr, $key:expr, $name:expr) => {
        SocketDescriptor {
            id: SocketId($n),
            key: $key,
            name: $name,
            kind: SocketKind::Cv,
            direction: Direction::Input,
        }
    };
}

static SOCKETS: [SocketDescriptor; 9] = [
    SocketDescriptor {
        id: NOTES_IN,
        key: "notes_in",
        name: "Notes in",
        kind: SocketKind::Notes,
        direction: Direction::Input,
    },
    cv_in!(1, "cv1", "CV 1"),
    cv_in!(2, "cv2", "CV 2"),
    cv_in!(3, "cv3", "CV 3"),
    cv_in!(4, "cv4", "CV 4"),
    cv_in!(5, "cv5", "CV 5"),
    cv_in!(6, "cv6", "CV 6"),
    cv_in!(7, "cv7", "CV 7"),
    cv_in!(8, "cv8", "CV 8"),
];

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    range: (f32, f32, f32),
    unit: Unit,
    discrete: bool,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: range.0,
        max: range.1,
        default: range.2,
        taper: Taper::Linear,
        unit,
        discrete,
    }
}

const P_CHANNEL: u32 = 0;
const P_PROGRAM: u32 = 1;
const P_BANK: u32 = 2;
const P_TRANSPOSE: u32 = 3;
const P_LATENCY: u32 = 4;
/// First CC param: three per CV input (number, scale, offset).
const P_CC_BASE: u32 = 5;
/// CV inputs, each mapped to one controller.
pub const CC_SLOTS: usize = 8;

/// The controller each CV input starts on: modulation, volume, pan, expression, resonance,
/// cutoff, reverb send and chorus send.
const DEFAULT_CC: [f32; CC_SLOTS] = [1.0, 7.0, 10.0, 11.0, 71.0, 74.0, 91.0, 93.0];

macro_rules! cc_params {
    ($n:expr, $default:expr, $num:expr, $scale:expr, $offset:expr, $num_name:expr, $scale_name:expr, $offset_name:expr) => {
        [
            param(
                P_CC_BASE + 3 * $n,
                $num,
                $num_name,
                (0.0, 127.0, $default),
                Unit::None,
                true,
            ),
            param(
                P_CC_BASE + 3 * $n + 1,
                $scale,
                $scale_name,
                (-4.0, 4.0, 1.0),
                Unit::Percent,
                false,
            ),
            param(
                P_CC_BASE + 3 * $n + 2,
                $offset,
                $offset_name,
                (-1.0, 1.0, 0.0),
                Unit::Percent,
                false,
            ),
        ]
    };
}

const CC1: [ParamDescriptor; 3] = cc_params!(
    0,
    1.0,
    "cc1.number",
    "cc1.scale",
    "cc1.offset",
    "CC 1 number",
    "CC 1 scale",
    "CC 1 offset"
);
const CC2: [ParamDescriptor; 3] = cc_params!(
    1,
    7.0,
    "cc2.number",
    "cc2.scale",
    "cc2.offset",
    "CC 2 number",
    "CC 2 scale",
    "CC 2 offset"
);
const CC3: [ParamDescriptor; 3] = cc_params!(
    2,
    10.0,
    "cc3.number",
    "cc3.scale",
    "cc3.offset",
    "CC 3 number",
    "CC 3 scale",
    "CC 3 offset"
);
const CC4: [ParamDescriptor; 3] = cc_params!(
    3,
    11.0,
    "cc4.number",
    "cc4.scale",
    "cc4.offset",
    "CC 4 number",
    "CC 4 scale",
    "CC 4 offset"
);
const CC5: [ParamDescriptor; 3] = cc_params!(
    4,
    71.0,
    "cc5.number",
    "cc5.scale",
    "cc5.offset",
    "CC 5 number",
    "CC 5 scale",
    "CC 5 offset"
);
const CC6: [ParamDescriptor; 3] = cc_params!(
    5,
    74.0,
    "cc6.number",
    "cc6.scale",
    "cc6.offset",
    "CC 6 number",
    "CC 6 scale",
    "CC 6 offset"
);
const CC7: [ParamDescriptor; 3] = cc_params!(
    6,
    91.0,
    "cc7.number",
    "cc7.scale",
    "cc7.offset",
    "CC 7 number",
    "CC 7 scale",
    "CC 7 offset"
);
const CC8: [ParamDescriptor; 3] = cc_params!(
    7,
    93.0,
    "cc8.number",
    "cc8.scale",
    "cc8.offset",
    "CC 8 number",
    "CC 8 scale",
    "CC 8 offset"
);

static PARAMS: [ParamDescriptor; 5 + 3 * CC_SLOTS] = [
    param(
        P_CHANNEL,
        "channel",
        "Channel",
        (1.0, 16.0, 1.0),
        Unit::None,
        true,
    ),
    param(
        P_PROGRAM,
        "program",
        "Program",
        (-1.0, 127.0, -1.0),
        Unit::None,
        true,
    ),
    param(
        P_BANK,
        "bank",
        "Bank",
        (-1.0, 16383.0, -1.0),
        Unit::None,
        true,
    ),
    param(
        P_TRANSPOSE,
        "transpose",
        "Transpose",
        (-48.0, 48.0, 0.0),
        Unit::Semitones,
        true,
    ),
    param(
        P_LATENCY,
        "latency_ms",
        "Latency",
        (-200.0, 200.0, 0.0),
        Unit::Ms,
        true,
    ),
    CC1[0],
    CC1[1],
    CC1[2],
    CC2[0],
    CC2[1],
    CC2[2],
    CC3[0],
    CC3[1],
    CC3[2],
    CC4[0],
    CC4[1],
    CC4[2],
    CC5[0],
    CC5[1],
    CC5[2],
    CC6[0],
    CC6[1],
    CC6[2],
    CC7[0],
    CC7[1],
    CC7[2],
    CC8[0],
    CC8[1],
    CC8[2],
];

/// Static description of the MIDI Out device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.midi-out",
    name: "MIDI Out",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`MidiOut`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(MidiOut::new()),
    }
}

/// The shortest time between two controller messages of one CV input, in seconds.
const CC_INTERVAL_S: f64 = 0.01;

/// A note this device started: the key and channel it went out on.
#[derive(Debug, Clone, Copy, Default)]
struct Held {
    active: bool,
    key: u8,
    channel: u8,
}

#[derive(Debug, Clone, Copy)]
struct CcSlot {
    number: u8,
    scale: f32,
    offset: f32,
    /// The value last sent, or -1 for none yet.
    last: i16,
    /// Frames until the input may send again.
    cooldown: u64,
}

/// The MIDI Out device.
#[derive(Debug)]
pub struct MidiOut {
    tap: Arc<MidiTap>,
    sample_rate: f64,
    offline: bool,
    channel: u8,
    program: i16,
    bank: i32,
    transpose: i32,
    latency_ms: f32,
    latency_frames: i64,
    ccs: [CcSlot; CC_SLOTS],
    /// By input key.
    held: [Held; 128],
    /// Bit `c` set: a note or a controller went out on channel `c` since the last release.
    channels_used: u16,
    /// Send the bank and program at the start of the next batch.
    program_pending: bool,
    /// Release everything at the start of the next batch (a `reset` while notes were held).
    flush: bool,
}

impl Default for MidiOut {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiOut {
    /// A device with default params (channel 1, no program, no latency) and a new tap.
    pub fn new() -> Self {
        let slot = |n: usize| CcSlot {
            number: DEFAULT_CC[n] as u8,
            scale: 1.0,
            offset: 0.0,
            last: -1,
            cooldown: 0,
        };
        Self {
            tap: Arc::new(MidiTap::new()),
            sample_rate: 48_000.0,
            offline: false,
            channel: 0,
            program: -1,
            bank: -1,
            transpose: 0,
            latency_ms: 0.0,
            latency_frames: 0,
            ccs: std::array::from_fn(slot),
            held: [Held::default(); 128],
            channels_used: 0,
            program_pending: true,
            flush: false,
        }
    }

    fn update_latency(&mut self) {
        self.latency_frames =
            (f64::from(self.latency_ms) * self.sample_rate / 1000.0).round() as i64;
    }

    /// Queues a message due `offset` frames into the batch that starts at `base`.
    fn send(&self, base: u64, offset: u16, bytes: &[u8]) {
        let frame = (base as i64)
            .saturating_add(i64::from(offset))
            .saturating_add(self.latency_frames)
            .max(0) as u64;
        if let Some(event) = MidiEvent::new(frame, bytes) {
            // A full queue counts the loss on the tap; there is nothing else to do here.
            let _ = self.tap.push(event);
        }
    }

    fn note_off(&mut self, base: u64, offset: u16, key: u8) {
        let held = self.held[usize::from(key)];
        if held.active {
            self.send(base, offset, &[0x80 | held.channel, held.key, 0]);
            self.held[usize::from(key)].active = false;
        }
    }

    /// Releases every note this device started, then All Notes Off on the channels it used.
    fn release_all(&mut self, base: u64, offset: u16) {
        for key in 0..128u8 {
            self.note_off(base, offset, key);
        }
        for channel in 0..16u8 {
            if self.channels_used & (1 << channel) != 0 {
                self.send(base, offset, &[0xB0 | channel, 123, 0]);
            }
        }
        self.channels_used = 0;
    }

    fn event(&mut self, base: u64, offset: u16, kind: NoteKind) {
        let channel = self.channel;
        let shifted = |key: u8, by: i32| {
            let k = i32::from(key) + by;
            (0..=127).contains(&k).then_some(k as u8)
        };
        match kind {
            NoteKind::On { key, velocity } => {
                // A key that is still down is released first: the new note is a retrigger.
                self.note_off(base, offset, key);
                if let Some(out) = shifted(key, self.transpose) {
                    self.send(
                        base,
                        offset,
                        &[0x90 | channel, out, velocity_to_7bit(velocity)],
                    );
                    self.held[usize::from(key)] = Held {
                        active: true,
                        key: out,
                        channel,
                    };
                    self.channels_used |= 1 << channel;
                }
            }
            NoteKind::Off { key } => self.note_off(base, offset, key),
            NoteKind::AllOff => self.release_all(base, offset),
            NoteKind::PitchBend { value } => {
                let bend = (value * 8192.0 + 8192.0).round().clamp(0.0, 16383.0) as u16;
                self.send(
                    base,
                    offset,
                    &[0xE0 | channel, (bend & 0x7F) as u8, (bend >> 7) as u8],
                );
                self.channels_used |= 1 << channel;
            }
            NoteKind::Cc { controller, value } => {
                self.send(
                    base,
                    offset,
                    &[0xB0 | channel, controller & 0x7F, to_7bit(value)],
                );
                self.channels_used |= 1 << channel;
            }
            NoteKind::ChannelPressure { value } => {
                self.send(base, offset, &[0xD0 | channel, to_7bit(value)]);
                self.channels_used |= 1 << channel;
            }
            NoteKind::PolyPressure { key, value } => {
                if let Some(out) = shifted(key, self.transpose) {
                    self.send(base, offset, &[0xA0 | channel, out, to_7bit(value)]);
                    self.channels_used |= 1 << channel;
                }
            }
            // Per-note expressions have no MIDI 1.0 message.
            NoteKind::NoteExpr { .. } => {}
        }
    }

    fn send_program(&mut self, base: u64) {
        let channel = self.channel;
        if self.program < 0 {
            return;
        }
        if self.bank >= 0 {
            self.send(base, 0, &[0xB0 | channel, 0, (self.bank >> 7) as u8 & 0x7F]);
            self.send(base, 0, &[0xB0 | channel, 32, (self.bank & 0x7F) as u8]);
        }
        self.send(base, 0, &[0xC0 | channel, self.program as u8]);
        self.channels_used |= 1 << channel;
    }
}

fn to_7bit(value: f32) -> u8 {
    if value.is_nan() {
        0
    } else {
        (value * 127.0).round().clamp(0.0, 127.0) as u8
    }
}

impl Device for MidiOut {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz();
        self.update_latency();
        self.program_pending = true;
        self.reset();
    }

    fn reset(&mut self) {
        // Notes that were started are released by the next batch; the controllers send again.
        self.flush = self.flush || self.held.iter().any(|h| h.active) || self.channels_used != 0;
        for cc in &mut self.ccs {
            cc.last = -1;
            cc.cooldown = 0;
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_CHANNEL => self.channel = (plain.round() as i32).clamp(1, 16) as u8 - 1,
            P_PROGRAM => {
                let program = (plain.round() as i32).clamp(-1, 127) as i16;
                self.program_pending |= program != self.program;
                self.program = program;
            }
            P_BANK => {
                let bank = (plain.round() as i32).clamp(-1, 16383);
                self.program_pending |= bank != self.bank;
                self.bank = bank;
            }
            P_TRANSPOSE => self.transpose = (plain.round() as i32).clamp(-48, 48),
            P_LATENCY => {
                self.latency_ms = plain.clamp(-200.0, 200.0);
                self.update_latency();
            }
            n if (P_CC_BASE..P_CC_BASE + 3 * CC_SLOTS as u32).contains(&n) => {
                let slot = &mut self.ccs[((n - P_CC_BASE) / 3) as usize];
                match (n - P_CC_BASE) % 3 {
                    0 => slot.number = (plain.round() as i32).clamp(0, 127) as u8,
                    1 => slot.scale = plain,
                    _ => slot.offset = plain,
                }
            }
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.offline {
            return;
        }
        let base = ctx.time.frame;
        if self.flush {
            self.flush = false;
            self.release_all(base, 0);
        }
        if self.program_pending {
            self.program_pending = false;
            self.send_program(base);
        }
        for event in ctx.notes_in(NOTES_IN) {
            self.event(base, event.offset, event.kind);
        }
        // Controllers from the CV inputs: the value at the end of the batch.
        let interval = (CC_INTERVAL_S * self.sample_rate) as u64;
        for k in 0..CC_SLOTS {
            let slot = &mut self.ccs[k];
            slot.cooldown = slot.cooldown.saturating_sub(BATCH as u64);
            let socket = SocketId(1 + k as u16);
            if !ctx.is_connected(socket) || slot.cooldown > 0 {
                continue;
            }
            let cv = ctx.input(socket)[BATCH - 1];
            let value = i16::from(to_7bit(cv * slot.scale + slot.offset));
            if value != slot.last {
                slot.last = value;
                slot.cooldown = interval;
                let (number, channel) = (slot.number, self.channel);
                self.channels_used |= 1 << channel;
                self.send(
                    base,
                    BATCH as u16 - 1,
                    &[0xB0 | channel, number, value as u8],
                );
            }
        }
    }

    fn midi_tap(&self) -> Option<Arc<MidiTap>> {
        Some(Arc::clone(&self.tap))
    }

    fn set_run_mode(&mut self, mode: RunMode) {
        self.offline = mode == RunMode::Offline;
    }
}

#[cfg(test)]
mod tests;
