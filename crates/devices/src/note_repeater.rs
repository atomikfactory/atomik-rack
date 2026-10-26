//! `atomik.note-repeater`: a note echo. Every note keeps playing as played (the dry note) and
//! adds repeats.
//!
//! * **Time.** The distance `D` between repeats is a note value (`time.sync`, converted to
//!   frames when the note arrives: `ticks / ticks_per_frame`, rounded half up) or free in
//!   milliseconds (`time.mode` free, `time.ms`: `ms * sample_rate / 1000`, rounded half up). A
//!   tempo change later does not move repeats that are already scheduled.
//! * **Repeats.** Repeat `n` of a note on at frame `f` starts at `f + n * D` with the key
//!   `key + n * pitch.step` (or, with `pitch.scale` in key, `n * pitch.step` scale degrees from
//!   the key; a key outside `0..=127` is dropped) and the velocity of the previous repeat times
//!   `1 - velocity.decay` (repeats stop below velocity 1/127). It ends `max(1, D * gate)`
//!   frames later. A repeat is its own note: the input `Off` does not touch it, so repeats ring
//!   out after the key is released, across a loop wrap, and while the transport is stopped.
//! * **Dry.** With `dry` off the original note is not passed, only its repeats.
//! * **Clock and queue.** Repeats live in a [`NoteQueue`](crate::note_queue::NoteQueue) on a
//!   device-owned frame clock that advances every batch, so the device behaves the same with the
//!   transport playing or stopped. A note is scheduled only when there is room for its end; when
//!   the queue is full, notes (and their repeats) are dropped, never an `Off`.
//! * **Events.** Bend, CC and pressure pass. `AllOff` (stop, locate) clears the queue, closes
//!   what sounds and passes on. Notes played by chase at play are echoed like played notes. A
//!   `reset` leaves an `AllOff` for the next batch when something sounded.
//! * **Bypass** (`bypass`, last param): see `crate::note_fx::PlayerBypass`; it also clears the
//!   queue.

use atomik_core::theory::{PITCH_NAMES, Scale, ScaleKind};
use atomik_core::{
    BATCH, NoteBuffer, NoteEvent, NoteKind, PPQ, ParamDescriptor, ParamId, Taper, Unit,
};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};

use crate::bypass::bypass_param;
use crate::key_chord::SCALE_KIND_LABELS;
use crate::note_fx::{PlayerBypass, push_sorted};
use crate::note_queue::{NoteQueue, NoteSink, round_frames};

const NOTES_IN: SocketId = SocketId(0);
const NOTES_OUT: SocketId = SocketId(1);

static SOCKETS: [SocketDescriptor; 2] = [
    SocketDescriptor {
        id: NOTES_IN,
        key: "notes_in",
        name: "Notes in",
        kind: SocketKind::Notes,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: NOTES_OUT,
        key: "notes_out",
        name: "Notes out",
        kind: SocketKind::Notes,
        direction: Direction::Output,
    },
];

/// The labels of `time.sync`.
static SYNC_LABELS: [&str; 8] = [
    "1/4", "1/8D", "1/8", "1/8T", "1/16D", "1/16", "1/16T", "1/32",
];

/// The length of every `time.sync` choice in ticks.
const SYNC_TICKS: [i64; 8] = [
    PPQ,
    PPQ / 2 * 3 / 2,
    PPQ / 2,
    PPQ / 3,
    PPQ / 4 * 3 / 2,
    PPQ / 4,
    PPQ / 6,
    PPQ / 8,
];

const TIME_MODES: Unit = Unit::Choice(&["sync", "free"]);
const PITCH_SCALES: Unit = Unit::Choice(&["off", "in key"]);

const P_SYNC: u32 = 0;
const P_MODE: u32 = 1;
const P_MS: u32 = 2;
const P_REPEATS: u32 = 3;
const P_DECAY: u32 = 4;
const P_STEP: u32 = 5;
const P_PSCALE: u32 = 6;
const P_ROOT: u32 = 7;
const P_KIND: u32 = 8;
const P_GATE: u32 = 9;
const P_DRY: u32 = 10;
const P_BYPASS: u32 = 11;

#[allow(clippy::too_many_arguments)]
const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    range: (f32, f32, f32),
    taper: Taper,
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
        taper,
        unit,
        discrete,
    }
}

static PARAMS: [ParamDescriptor; 12] = [
    param(
        P_SYNC,
        "time.sync",
        "Time",
        (0.0, 7.0, 1.0),
        Taper::Linear,
        Unit::Choice(&SYNC_LABELS),
        true,
    ),
    param(
        P_MODE,
        "time.mode",
        "Time mode",
        (0.0, 1.0, 0.0),
        Taper::Linear,
        TIME_MODES,
        true,
    ),
    param(
        P_MS,
        "time.ms",
        "Free time",
        (10.0, 2000.0, 250.0),
        Taper::Exponential,
        Unit::Ms,
        false,
    ),
    param(
        P_REPEATS,
        "repeats",
        "Repeats",
        (1.0, 16.0, 3.0),
        Taper::Linear,
        Unit::None,
        true,
    ),
    param(
        P_DECAY,
        "velocity.decay",
        "Decay",
        (0.0, 1.0, 0.25),
        Taper::Linear,
        Unit::Percent,
        false,
    ),
    param(
        P_STEP,
        "pitch.step",
        "Pitch step",
        (-12.0, 12.0, 0.0),
        Taper::Linear,
        Unit::Semitones,
        true,
    ),
    param(
        P_PSCALE,
        "pitch.scale",
        "Pitch in",
        (0.0, 1.0, 0.0),
        Taper::Linear,
        PITCH_SCALES,
        true,
    ),
    param(
        P_ROOT,
        "scale.root",
        "Root",
        (0.0, 11.0, 0.0),
        Taper::Linear,
        Unit::Choice(&PITCH_NAMES),
        true,
    ),
    param(
        P_KIND,
        "scale.kind",
        "Scale",
        (0.0, 15.0, 1.0),
        Taper::Linear,
        Unit::Choice(&SCALE_KIND_LABELS),
        true,
    ),
    param(
        P_GATE,
        "gate",
        "Gate",
        (0.05, 1.0, 0.5),
        Taper::Linear,
        Unit::Percent,
        false,
    ),
    param(
        P_DRY,
        "dry",
        "Dry",
        (0.0, 1.0, 1.0),
        Taper::Linear,
        Unit::Toggle,
        true,
    ),
    bypass_param(P_BYPASS),
];

/// Static description of the Note Repeater device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.note-repeater",
    name: "Note Repeater",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`NoteRepeater`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(NoteRepeater::new()),
    }
}

/// Velocity below which a repeat is not played.
const MIN_VELOCITY: f32 = 1.0 / 127.0;

/// The Note Repeater device.
#[derive(Debug, Clone)]
pub struct NoteRepeater {
    queue: NoteQueue,
    sink: NoteSink,
    bypass: PlayerBypass,
    /// Emit an `AllOff` at the start of the next batch (a `reset` while something sounded).
    flush_alloff: bool,
    /// The device frame clock: the frame at the start of the batch.
    now: u64,
    sample_rate: f64,
    /// Input keys whose dry `On` is scheduled and whose `Off` is still to come.
    dry_on: [bool; 128],
    sync: usize,
    free_time: bool,
    ms: f32,
    repeats: u32,
    decay: f32,
    step: i32,
    in_key: bool,
    scale: Scale,
    gate: f32,
    dry: bool,
}

impl NoteRepeater {
    fn new() -> Self {
        Self {
            queue: NoteQueue::new(),
            sink: NoteSink::new(),
            bypass: PlayerBypass::new(),
            flush_alloff: false,
            now: 0,
            sample_rate: 48_000.0,
            dry_on: [false; 128],
            sync: 1,
            free_time: false,
            ms: 250.0,
            repeats: 3,
            decay: 0.25,
            step: 0,
            in_key: false,
            scale: Scale::new(0, ScaleKind::Major),
            gate: 0.5,
            dry: true,
        }
    }

    /// The distance between repeats in frames (at least 1) at `ticks_per_frame`.
    fn distance(&self, tpf: f64) -> u64 {
        let frames = if self.free_time {
            f64::from(self.ms) * self.sample_rate / 1000.0
        } else {
            SYNC_TICKS[self.sync] as f64 / tpf
        };
        round_frames(frames).max(1)
    }

    /// The key of repeat `n`, if it stays in range.
    fn repeat_key(&self, key: u8, n: u32) -> Option<u8> {
        let steps = self.step * n as i32;
        if self.in_key {
            self.scale.step(key, steps)
        } else {
            let k = i32::from(key) + steps;
            (0..=127).contains(&k).then_some(k as u8)
        }
    }

    fn on(&mut self, frame: u64, key: u8, velocity: f32, tpf: f64) {
        let ik = usize::from(key & 0x7F);
        if self.dry_on[ik] {
            self.dry_on[ik] = false;
            self.queue.push_off_reserved(frame, key);
        }
        if self.dry {
            self.dry_on[ik] = self.queue.push_on_reserving(frame, key, velocity);
        }
        let d = self.distance(tpf);
        let len = round_frames(d as f64 * f64::from(self.gate)).max(1);
        let mut v = velocity;
        for n in 1..=self.repeats {
            v *= 1.0 - self.decay;
            if v < MIN_VELOCITY {
                break;
            }
            let Some(k) = self.repeat_key(key, n) else {
                continue;
            };
            let start = frame + u64::from(n) * d;
            if !self.queue.push_pair(start, start + len, k, v.min(1.0)) {
                break;
            }
        }
    }

    fn apply(&mut self, out: &mut NoteBuffer, e: NoteEvent, tpf: f64) {
        let frame = self.now + u64::from(e.offset);
        match e.kind {
            NoteKind::On { key, velocity } => self.on(frame, key, velocity, tpf),
            NoteKind::Off { key } => {
                let ik = usize::from(key & 0x7F);
                if std::mem::take(&mut self.dry_on[ik]) {
                    self.queue.push_off_reserved(frame, key);
                }
            }
            NoteKind::AllOff => {
                // Everything before the AllOff is void; what follows it starts afresh.
                self.queue.clear();
                self.dry_on = [false; 128];
                self.sink.emit(out, usize::from(e.offset), NoteKind::AllOff);
            }
            kind => {
                self.queue.push_other(frame, kind);
            }
        }
    }
}

impl Device for NoteRepeater {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz();
        self.reset();
        self.flush_alloff = false;
        self.now = 0;
    }

    fn reset(&mut self) {
        if self.sink.any_sounding() {
            self.flush_alloff = true;
        }
        self.queue.clear();
        self.sink.clear();
        self.dry_on = [false; 128];
        if self.bypass.reset() {
            self.flush_alloff = true;
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let int = |lo: f32, hi: f32| plain.round().clamp(lo, hi) as i32;
        match id.0 {
            P_SYNC => self.sync = int(0.0, 7.0) as usize,
            P_MODE => self.free_time = plain >= 0.5,
            P_MS => self.ms = plain.clamp(10.0, 2000.0),
            P_REPEATS => self.repeats = int(1.0, 16.0) as u32,
            P_DECAY => self.decay = plain.clamp(0.0, 1.0),
            P_STEP => self.step = int(-12.0, 12.0),
            P_PSCALE => self.in_key = plain >= 0.5,
            P_ROOT => self.scale = Scale::new(int(0.0, 11.0) as u8, self.scale.kind),
            P_KIND => {
                self.scale = Scale::new(self.scale.root, ScaleKind::ALL[int(0.0, 15.0) as usize]);
            }
            P_GATE => self.gate = plain.clamp(0.05, 1.0),
            P_DRY => self.dry = plain >= 0.5,
            P_BYPASS => self.bypass.set_param(plain),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let tpf = ctx.time.ticks_per_frame;
        let tpf = if tpf.is_finite() && tpf > 0.0 {
            tpf
        } else {
            // No usable tempo: repeat at the tick rate of 120 BPM.
            120.0 * PPQ as f64 / (60.0 * self.sample_rate)
        };
        if self.flush_alloff {
            self.flush_alloff = false;
            push_sorted(
                ctx.notes_out(NOTES_OUT),
                NoteEvent {
                    offset: 0,
                    kind: NoteKind::AllOff,
                },
            );
        }
        if self.bypass.begin_batch(ctx.notes_out(NOTES_OUT)) {
            // The switch closes what the device sent and starts it from nothing.
            self.sink.close_all(ctx.notes_out(NOTES_OUT), 0);
            self.queue.clear();
            self.dry_on = [false; 128];
        }
        for i in 0..ctx.notes_in(NOTES_IN).len() {
            let e = ctx.notes_in(NOTES_IN)[i];
            if let Some(e) = self.bypass.route(ctx.notes_out(NOTES_OUT), e) {
                self.apply(ctx.notes_out(NOTES_OUT), e, tpf);
            }
        }
        self.sink
            .drain(&mut self.queue, ctx.notes_out(NOTES_OUT), self.now);
        self.now += BATCH as u64;
    }
}

#[cfg(test)]
mod tests;
