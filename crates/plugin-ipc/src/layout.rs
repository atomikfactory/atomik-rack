//! The shared block: one `#[repr(C)]` struct mapped by both processes. Every offset is fixed
//! and pinned by the tests at the bottom of this file, so a change to a field is a change to
//! [`PROTOCOL_VERSION`].
//!
//! ```text
//! offset      size   what
//!      0        64   Header   magic, version, req_seq, done_seq, child_dead, doorbell word,
//!                             wake flag, processor info
//!     64    33 920   Request  ticket, op, channel counts, silent flags, transport, main_in,
//!                             sidechain, events (parent writes, child reads)
//! 33 984    16 960   Response ticket, status, ok, dropped, main_out, events
//!                             (child writes, parent reads)
//! 50 944             end
//! ```
//!
//! Who may touch what: the parent writes the request only while no request is outstanding
//! (`done_seq == req_seq`), then publishes it with a release store of `req_seq`. The child reads
//! it after an acquire load of `req_seq`, writes the response, then publishes it with a release
//! store of `done_seq`. The parent reads the response after an acquire load of `done_seq`. The
//! two areas never overlap and neither side touches an area the other may be using, so the
//! plain (non-atomic) fields are race free by construction.
//!
//! Every floating point value that crosses is stored as its raw bits (`u32` or `u64`) in the
//! wire structs, so no copy can quiet a NaN payload and every bit pattern is a valid value.

use atomik_core::{BATCH, Batch, ExprKind};
use atomik_plugin_host::{PluginEvent, PluginEventKind, PluginTransport};
use std::mem::size_of;
use std::sync::atomic::{AtomicU32, AtomicU64};

/// Version of the block layout and of the control protocol. The child refuses a block whose
/// version differs.
pub const PROTOCOL_VERSION: u32 = 1;
/// First word of a block: `ATSB`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"ATSB");
/// Channels of each audio area (main in, sidechain, main out): 0 to 2.
pub const MAX_CHANNELS: usize = 2;
/// Events one request carries; the parent drops the rest and counts them.
pub const MAX_EVENTS_IN: usize = 1024;
/// Events one response carries; the child drops the rest and counts them.
pub const MAX_EVENTS_OUT: usize = 512;
/// Largest control-channel frame (the JSON, and separately the blob), in bytes.
pub const MAX_FRAME_BYTES: usize = 64 << 20;
/// Size of the shared block, in bytes.
pub const BLOCK_SIZE: usize = size_of::<Block>();

/// Doorbell word value: nobody waits and nothing is rung.
pub const DOORBELL_IDLE: u32 = 0;
/// Doorbell word value: the child waits in the OS.
pub const DOORBELL_PARKED: u32 = 1;
/// Doorbell word value: the parent rang and the child has not consumed it yet.
pub const DOORBELL_RUNG: u32 = 2;

/// Event tag: a note on.
pub const EV_NOTE_ON: u32 = 1;
/// Event tag: a note off.
pub const EV_NOTE_OFF: u32 = 2;
/// Event tag: pitch bend.
pub const EV_PITCH_BEND: u32 = 3;
/// Event tag: a controller.
pub const EV_CC: u32 = 4;
/// Event tag: channel pressure.
pub const EV_CHANNEL_PRESSURE: u32 = 5;
/// Event tag: pressure of one note.
pub const EV_POLY_PRESSURE: u32 = 6;
/// Event tag: a note expression.
pub const EV_NOTE_EXPR: u32 = 7;
/// Event tag: a parameter change.
pub const EV_PARAM: u32 = 8;

/// A [`PluginEvent`] as 32 bytes of plain data. Fields not used by a kind are zero, so equal
/// events are equal bit for bit. `a` is the key, the controller number or the param id; `b` is
/// the note id; `c` is the [`ExprKind`] code; `value` is the `f32` bits; `wide` is the `f64`
/// bits of a param value.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WireEvent {
    /// Which kind of event (`EV_*`); 0 is an empty slot.
    pub tag: u32,
    /// Frame offset inside the batch.
    pub offset: u32,
    /// Key, controller number or param id.
    pub a: u32,
    /// Note id (an `i32` reinterpreted).
    pub b: u32,
    /// Expression kind code.
    pub c: u32,
    /// `f32` bits of the value.
    pub value: u32,
    /// `f64` bits of a param value.
    pub wide: u64,
}

/// A [`PluginTransport`] as plain data (floats as raw bits).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WireTransport {
    /// `f64` bits of the tempo in BPM.
    pub tempo_bpm: u64,
    /// `f64` bits of the position in quarter notes.
    pub song_pos_beats: u64,
    /// `f64` bits of the position in seconds.
    pub song_pos_seconds: u64,
    /// The engine's frame counter at the first frame of the batch.
    pub steady_frames: u64,
    /// `f64` bits of the loop start in quarter notes.
    pub loop_start_beats: u64,
    /// `f64` bits of the loop end in quarter notes.
    pub loop_end_beats: u64,
    /// `f64` bits of the sample rate.
    pub sample_rate: u64,
    /// 1 when playing.
    pub playing: u8,
    /// 1 when a loop region is active.
    pub loop_active: u8,
    /// Time signature numerator.
    pub ts_num: u8,
    /// Time signature denominator.
    pub ts_den: u8,
    /// Padding, always zero.
    pub pad: u32,
}

/// The first 64 bytes of the block.
#[repr(C, align(64))]
#[derive(Debug)]
pub struct Header {
    /// [`MAGIC`], written once by the creator before the child opens the file.
    pub magic: u32,
    /// [`PROTOCOL_VERSION`], written once by the creator.
    pub version: u32,
    /// Sequence number of the last submitted request (the parent's ticket counter).
    pub req_seq: AtomicU64,
    /// Sequence number of the last completed request.
    pub done_seq: AtomicU64,
    /// Non-zero once the child process is gone.
    pub child_dead: AtomicU32,
    /// The doorbell word (`DOORBELL_*`): the futex word on Linux and macOS.
    pub child_parked: AtomicU32,
    /// Non-zero when the plugin asked to be processed.
    pub wake: AtomicU32,
    /// The processor's sleep rule code.
    pub info_rule: AtomicU32,
    /// The processor's tail code (`u64::MAX` is infinite).
    pub info_tail: AtomicU64,
}

/// What the parent sends for one exchange.
#[repr(C, align(64))]
pub struct Request {
    /// The ticket (sequence number) of this request.
    pub ticket: u64,
    /// An `AudioOp` code.
    pub op: u32,
    /// Main input channels (0 to 2).
    pub main_in_ch: u32,
    /// 1 when no main input is cabled.
    pub main_in_silent: u32,
    /// Sidechain channels (0 to 2).
    pub sc_ch: u32,
    /// 1 when no sidechain is cabled.
    pub sc_silent: u32,
    /// Main output channels (0 to 2).
    pub main_out_ch: u32,
    /// Events in `events`.
    pub n_events: u32,
    /// Padding, always zero.
    pub pad: u32,
    /// Transport for the batch.
    pub transport: WireTransport,
    /// Main input samples.
    pub main_in: [Batch; MAX_CHANNELS],
    /// Sidechain samples.
    pub sc: [Batch; MAX_CHANNELS],
    /// Input events.
    pub events: [WireEvent; MAX_EVENTS_IN],
}

/// What the child answers.
#[repr(C, align(64))]
pub struct Response {
    /// The ticket this response answers.
    pub ticket: u64,
    /// A `ProcessStatus` code.
    pub status: u32,
    /// The op's boolean result (`Start`).
    pub ok: u32,
    /// Events dropped on the child's side of the exchange for this request.
    pub dropped: u32,
    /// Events in `events`.
    pub n_events: u32,
    /// Main output samples.
    pub main_out: [Batch; MAX_CHANNELS],
    /// Output events.
    pub events: [WireEvent; MAX_EVENTS_OUT],
}

/// The whole mapped block.
#[repr(C, align(64))]
pub struct Block {
    /// Sequence numbers and flags.
    pub header: Header,
    /// Parent to child.
    pub request: Request,
    /// Child to parent.
    pub response: Response,
}

const _: () = assert!(BATCH == 64, "the layout offsets assume 64-frame batches");

impl WireEvent {
    /// Encodes `event`. Bit exact: floats are stored as their raw bits.
    pub fn encode(event: &PluginEvent) -> Self {
        let mut w = Self {
            offset: u32::from(event.offset),
            ..Self::default()
        };
        match event.kind {
            PluginEventKind::NoteOn {
                key,
                velocity,
                note_id,
            } => {
                w.tag = EV_NOTE_ON;
                w.a = u32::from(key);
                w.b = note_id as u32;
                w.value = velocity.to_bits();
            }
            PluginEventKind::NoteOff { key, note_id } => {
                w.tag = EV_NOTE_OFF;
                w.a = u32::from(key);
                w.b = note_id as u32;
            }
            PluginEventKind::PitchBend { value } => {
                w.tag = EV_PITCH_BEND;
                w.value = value.to_bits();
            }
            PluginEventKind::Cc { controller, value } => {
                w.tag = EV_CC;
                w.a = u32::from(controller);
                w.value = value.to_bits();
            }
            PluginEventKind::ChannelPressure { value } => {
                w.tag = EV_CHANNEL_PRESSURE;
                w.value = value.to_bits();
            }
            PluginEventKind::PolyPressure {
                key,
                note_id,
                value,
            } => {
                w.tag = EV_POLY_PRESSURE;
                w.a = u32::from(key);
                w.b = note_id as u32;
                w.value = value.to_bits();
            }
            PluginEventKind::NoteExpr {
                key,
                note_id,
                kind,
                value,
            } => {
                w.tag = EV_NOTE_EXPR;
                w.a = u32::from(key);
                w.b = note_id as u32;
                w.c = expr_code(kind);
                w.value = value.to_bits();
            }
            PluginEventKind::Param { id, value } => {
                w.tag = EV_PARAM;
                w.a = id;
                w.wide = value.to_bits();
            }
        }
        w
    }

    /// Decodes an event; `None` for an empty slot or a value no encoder writes.
    pub fn decode(&self) -> Option<PluginEvent> {
        let offset = u16::try_from(self.offset).ok()?;
        let key = || u8::try_from(self.a).ok();
        let value = f32::from_bits(self.value);
        let note_id = self.b as i32;
        let kind = match self.tag {
            EV_NOTE_ON => PluginEventKind::NoteOn {
                key: key()?,
                velocity: value,
                note_id,
            },
            EV_NOTE_OFF => PluginEventKind::NoteOff {
                key: key()?,
                note_id,
            },
            EV_PITCH_BEND => PluginEventKind::PitchBend { value },
            EV_CC => PluginEventKind::Cc {
                controller: key()?,
                value,
            },
            EV_CHANNEL_PRESSURE => PluginEventKind::ChannelPressure { value },
            EV_POLY_PRESSURE => PluginEventKind::PolyPressure {
                key: key()?,
                note_id,
                value,
            },
            EV_NOTE_EXPR => PluginEventKind::NoteExpr {
                key: key()?,
                note_id,
                kind: expr_from_code(self.c)?,
                value,
            },
            EV_PARAM => PluginEventKind::Param {
                id: self.a,
                value: f64::from_bits(self.wide),
            },
            _ => return None,
        };
        Some(PluginEvent { offset, kind })
    }
}

fn expr_code(kind: ExprKind) -> u32 {
    match kind {
        ExprKind::Tuning => 0,
        ExprKind::Pressure => 1,
        ExprKind::Slide => 2,
        ExprKind::Volume => 3,
        ExprKind::Pan => 4,
    }
}

fn expr_from_code(code: u32) -> Option<ExprKind> {
    Some(match code {
        0 => ExprKind::Tuning,
        1 => ExprKind::Pressure,
        2 => ExprKind::Slide,
        3 => ExprKind::Volume,
        4 => ExprKind::Pan,
        _ => return None,
    })
}

impl WireTransport {
    /// Encodes a transport. Bit exact.
    pub fn encode(t: &PluginTransport) -> Self {
        Self {
            tempo_bpm: t.tempo_bpm.to_bits(),
            song_pos_beats: t.song_pos_beats.to_bits(),
            song_pos_seconds: t.song_pos_seconds.to_bits(),
            steady_frames: t.steady_frames,
            loop_start_beats: t.loop_start_beats.to_bits(),
            loop_end_beats: t.loop_end_beats.to_bits(),
            sample_rate: t.sample_rate.to_bits(),
            playing: u8::from(t.playing),
            loop_active: u8::from(t.loop_active),
            ts_num: t.time_signature.0,
            ts_den: t.time_signature.1,
            pad: 0,
        }
    }

    /// Decodes a transport. Bit exact.
    pub fn decode(&self) -> PluginTransport {
        PluginTransport {
            playing: self.playing != 0,
            tempo_bpm: f64::from_bits(self.tempo_bpm),
            song_pos_beats: f64::from_bits(self.song_pos_beats),
            song_pos_seconds: f64::from_bits(self.song_pos_seconds),
            steady_frames: self.steady_frames,
            time_signature: (self.ts_num, self.ts_den),
            loop_active: self.loop_active != 0,
            loop_start_beats: f64::from_bits(self.loop_start_beats),
            loop_end_beats: f64::from_bits(self.loop_end_beats),
            sample_rate: f64::from_bits(self.sample_rate),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn the_constants_are_the_ones_of_the_contract() {
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(MAGIC.to_le_bytes(), *b"ATSB");
        assert_eq!(MAX_CHANNELS, 2);
        assert_eq!(MAX_EVENTS_IN, 1024);
        assert_eq!(MAX_EVENTS_OUT, 512);
        assert_eq!(MAX_FRAME_BYTES, 64 << 20);
    }

    #[test]
    fn a_wire_event_is_32_bytes_of_plain_data() {
        assert_eq!(size_of::<WireEvent>(), 32);
        assert_eq!(align_of::<WireEvent>(), 8);
        assert_eq!(offset_of!(WireEvent, tag), 0);
        assert_eq!(offset_of!(WireEvent, offset), 4);
        assert_eq!(offset_of!(WireEvent, a), 8);
        assert_eq!(offset_of!(WireEvent, b), 12);
        assert_eq!(offset_of!(WireEvent, c), 16);
        assert_eq!(offset_of!(WireEvent, value), 20);
        assert_eq!(offset_of!(WireEvent, wide), 24);
    }

    #[test]
    fn the_wire_transport_is_64_bytes() {
        assert_eq!(size_of::<WireTransport>(), 64);
        assert_eq!(align_of::<WireTransport>(), 8);
    }

    #[test]
    fn the_header_offsets_are_pinned() {
        assert_eq!(size_of::<Header>(), 64);
        assert_eq!(offset_of!(Header, magic), 0);
        assert_eq!(offset_of!(Header, version), 4);
        assert_eq!(offset_of!(Header, req_seq), 8);
        assert_eq!(offset_of!(Header, done_seq), 16);
        assert_eq!(offset_of!(Header, child_dead), 24);
        assert_eq!(offset_of!(Header, child_parked), 28);
        assert_eq!(offset_of!(Header, wake), 32);
        assert_eq!(offset_of!(Header, info_rule), 36);
        assert_eq!(offset_of!(Header, info_tail), 40);
    }

    #[test]
    fn the_request_offsets_are_pinned() {
        assert_eq!(offset_of!(Request, ticket), 0);
        assert_eq!(offset_of!(Request, op), 8);
        assert_eq!(offset_of!(Request, main_in_ch), 12);
        assert_eq!(offset_of!(Request, main_in_silent), 16);
        assert_eq!(offset_of!(Request, sc_ch), 20);
        assert_eq!(offset_of!(Request, sc_silent), 24);
        assert_eq!(offset_of!(Request, main_out_ch), 28);
        assert_eq!(offset_of!(Request, n_events), 32);
        assert_eq!(offset_of!(Request, transport), 40);
        assert_eq!(offset_of!(Request, main_in), 104);
        assert_eq!(offset_of!(Request, sc), 616);
        assert_eq!(offset_of!(Request, events), 1128);
        assert_eq!(size_of::<Request>(), 33_920);
    }

    #[test]
    fn the_response_offsets_are_pinned() {
        assert_eq!(offset_of!(Response, ticket), 0);
        assert_eq!(offset_of!(Response, status), 8);
        assert_eq!(offset_of!(Response, ok), 12);
        assert_eq!(offset_of!(Response, dropped), 16);
        assert_eq!(offset_of!(Response, n_events), 20);
        assert_eq!(offset_of!(Response, main_out), 24);
        assert_eq!(offset_of!(Response, events), 536);
        assert_eq!(size_of::<Response>(), 16_960);
    }

    #[test]
    fn the_block_is_header_then_request_then_response() {
        assert_eq!(offset_of!(Block, header), 0);
        assert_eq!(offset_of!(Block, request), 64);
        assert_eq!(offset_of!(Block, response), 64 + 33_920);
        assert_eq!(BLOCK_SIZE, 50_944);
        assert_eq!(BLOCK_SIZE, size_of::<Block>());
        assert_eq!(align_of::<Block>(), 64);
    }
}
