//! `atomik.note-delay`: moves every note event later in time. It never adds a note.
//!
//! * **Delay.** Every event is delayed by `delay.sync` (a note value, converted to frames when
//!   the event arrives: `ticks / ticks_per_frame`, rounded half up; the first choice is no
//!   delay) or, in free mode, `delay.ms`. A later tempo change does not move events that are
//!   already scheduled.
//! * **Swing** (`swing` percent of the `swing.grid` step, only while the transport plays): a note
//!   whose position lies in the first half of an odd step of the grid is delayed by the swing
//!   shift of that grid (`(swing * step + 100) / 200` ticks, the rule the step players use), so
//!   only the off-beat notes move.
//! * **Humanize.** `humanize.time` delays a note by up to that many milliseconds (late only) and
//!   `humanize.velocity` moves its velocity by up to that many steps of 127 in either direction.
//!   Both are drawn with a hash of `random.seed` and the device seed, the note's transport tick
//!   (the device frame while stopped) and its key, so the same bounce always gives the same
//!   result.
//! * **Lengths kept.** An `Off` is delayed by the amount its `On` got (remembered per key), so
//!   note lengths do not change. Bend, CC and pressure get the base delay.
//! * **Clock and queue.** Events wait in a [`NoteQueue`](crate::note_queue::NoteQueue) on a
//!   device-owned frame clock that advances every batch, so the device behaves the same with the
//!   transport playing or stopped. A note is scheduled only when there is room for its end; a
//!   full queue drops notes, never an `Off`.
//! * **Events.** `AllOff` (stop, locate) clears the queue, closes what sounds and passes on. A
//!   `reset` leaves an `AllOff` for the next batch when something sounded.
//! * **Bypass** (`bypass`, last param): see `crate::note_fx::PlayerBypass`; it also clears the
//!   queue.

use atomik_core::{
    BATCH, NoteBuffer, NoteEvent, NoteKind, PPQ, ParamDescriptor, ParamId, Taper, Unit,
};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::rng::{hash64, unit_f64};

use crate::bypass::bypass_param;
use crate::note_clock::{Grid, seed_mix};
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

/// The labels of `delay.sync` (the first is no delay).
static SYNC_LABELS: [&str; 9] = [
    "0", "1/64", "1/32T", "1/32", "1/16T", "1/16", "1/8T", "1/8", "1/4",
];

/// The length of every `delay.sync` choice in ticks.
const SYNC_TICKS: [i64; 9] = [
    0,
    PPQ / 16,
    PPQ / 12,
    PPQ / 8,
    PPQ / 6,
    PPQ / 4,
    PPQ / 3,
    PPQ / 2,
    PPQ,
];

/// The step of every `swing.grid` choice in ticks.
const GRID_TICKS: [u32; 2] = [(PPQ / 2) as u32, (PPQ / 4) as u32];

const TIME_MODES: Unit = Unit::Choice(&["sync", "free"]);
const GRIDS: Unit = Unit::Choice(&["1/8", "1/16"]);

const P_MODE: u32 = 0;
const P_SYNC: u32 = 1;
const P_MS: u32 = 2;
const P_SWING: u32 = 3;
const P_GRID: u32 = 4;
const P_HUM_TIME: u32 = 5;
const P_HUM_VEL: u32 = 6;
const P_SEED: u32 = 7;
const P_BYPASS: u32 = 8;

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

static PARAMS: [ParamDescriptor; 9] = [
    param(
        P_MODE,
        "delay.mode",
        "Delay mode",
        (0.0, 1.0, 0.0),
        TIME_MODES,
        true,
    ),
    param(
        P_SYNC,
        "delay.sync",
        "Delay",
        (0.0, 8.0, 5.0),
        Unit::Choice(&SYNC_LABELS),
        true,
    ),
    param(
        P_MS,
        "delay.ms",
        "Free delay",
        (0.0, 1000.0, 0.0),
        Unit::Ms,
        false,
    ),
    param(
        P_SWING,
        "swing",
        "Swing",
        (0.0, 75.0, 0.0),
        Unit::None,
        false,
    ),
    param(
        P_GRID,
        "swing.grid",
        "Swing grid",
        (0.0, 1.0, 1.0),
        GRIDS,
        true,
    ),
    param(
        P_HUM_TIME,
        "humanize.time",
        "Humanize time",
        (0.0, 20.0, 0.0),
        Unit::Ms,
        false,
    ),
    param(
        P_HUM_VEL,
        "humanize.velocity",
        "Humanize velocity",
        (0.0, 32.0, 0.0),
        Unit::None,
        true,
    ),
    param(
        P_SEED,
        "random.seed",
        "Random seed",
        (0.0, 999.0, 0.0),
        Unit::None,
        true,
    ),
    bypass_param(P_BYPASS),
];

/// Static description of the Note Delay device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.note-delay",
    name: "Note Delay",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`NoteDelay`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(NoteDelay::new()),
    }
}

/// Hash slot of the time draw (the velocity draw uses `key + 128`).
const SLOT_TIME: u64 = 0;
/// Hash slot offset of the velocity draw.
const SLOT_VELOCITY: u64 = 128;

/// The Note Delay device.
#[derive(Debug, Clone)]
pub struct NoteDelay {
    queue: NoteQueue,
    sink: NoteSink,
    bypass: PlayerBypass,
    /// Emit an `AllOff` at the start of the next batch (a `reset` while something sounded).
    flush_alloff: bool,
    /// The device frame clock: the frame at the start of the batch.
    now: u64,
    sample_rate: f64,
    seed: u64,
    seed_mix: u64,
    /// Input keys whose delayed `On` is scheduled and whose `Off` is still to come.
    pending: [bool; 128],
    /// The delay in frames each pending `On` got.
    shift: [u64; 128],
    free_time: bool,
    sync: usize,
    ms: f32,
    swing: u8,
    grid: usize,
    hum_ms: f32,
    hum_vel: f32,
}

impl NoteDelay {
    fn new() -> Self {
        Self {
            queue: NoteQueue::new(),
            sink: NoteSink::new(),
            bypass: PlayerBypass::new(),
            flush_alloff: false,
            now: 0,
            sample_rate: 48_000.0,
            seed: 0,
            seed_mix: seed_mix(0, 0),
            pending: [false; 128],
            shift: [0; 128],
            free_time: false,
            sync: 5,
            ms: 0.0,
            swing: 0,
            grid: 1,
            hum_ms: 0.0,
            hum_vel: 0.0,
        }
    }

    /// The base delay in frames at `ticks_per_frame`.
    fn base(&self, tpf: f64) -> u64 {
        if self.free_time {
            round_frames(f64::from(self.ms) * self.sample_rate / 1000.0)
        } else {
            round_frames(SYNC_TICKS[self.sync] as f64 / tpf)
        }
    }

    fn on(
        &mut self,
        frame: u64,
        key: u8,
        velocity: f32,
        time: &atomik_core::TimeInfo,
        offset: u16,
    ) {
        let ik = usize::from(key & 0x7F);
        let tpf = time.ticks_per_frame;
        let mut shift = self.base(tpf);
        // The position of the note: the transport tick while playing, else the device frame.
        let tau = time.tick + f64::from(offset) * tpf;
        if time.playing && self.swing > 0 {
            let grid = Grid {
                s: GRID_TICKS[self.grid],
                swing: self.swing,
            };
            let s = f64::from(grid.s);
            // A tick within a millionth of a frame below a step start counts as on it.
            let te = tau + tpf * 1e-6;
            let k = (te / s).floor();
            let odd = (k as i64) & 1 == 1;
            if odd && te - k * s < s / 2.0 {
                shift += round_frames(grid.shift() as f64 / tpf);
            }
        }
        let position = if time.playing {
            round_frames(tau)
        } else {
            frame
        };
        if self.hum_ms > 0.0 {
            let u = unit_f64(hash64(self.seed_mix, position, u64::from(key) + SLOT_TIME));
            shift += (u * f64::from(self.hum_ms) * self.sample_rate / 1000.0).floor() as u64;
        }
        let mut velocity = velocity;
        if self.hum_vel > 0.0 {
            let u = unit_f64(hash64(
                self.seed_mix,
                position,
                u64::from(key) + SLOT_VELOCITY,
            ));
            let v = f64::from(velocity) * 127.0 + (u * 2.0 - 1.0) * f64::from(self.hum_vel);
            velocity = (v + 0.5).floor().clamp(1.0, 127.0) as f32 / 127.0;
        }
        let at = frame + shift;
        if self.pending[ik] {
            self.queue.push_off_reserved(at, key);
        }
        self.pending[ik] = self.queue.push_on_reserving(at, key, velocity);
        self.shift[ik] = shift;
    }

    fn apply(
        &mut self,
        out: &mut NoteBuffer,
        e: NoteEvent,
        time: &atomik_core::TimeInfo,
        tpf: f64,
    ) {
        let frame = self.now + u64::from(e.offset);
        match e.kind {
            NoteKind::On { key, velocity } => self.on(frame, key, velocity, time, e.offset),
            NoteKind::Off { key } => {
                let ik = usize::from(key & 0x7F);
                if std::mem::take(&mut self.pending[ik]) {
                    self.queue.push_off_reserved(frame + self.shift[ik], key);
                }
            }
            NoteKind::AllOff => {
                // Everything before the AllOff is void; what follows it starts afresh.
                self.queue.clear();
                self.pending = [false; 128];
                self.sink.emit(out, usize::from(e.offset), NoteKind::AllOff);
            }
            kind => {
                self.queue.push_other(frame + self.base(tpf), kind);
            }
        }
    }
}

impl Device for NoteDelay {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz();
        self.seed = info.seed;
        self.seed_mix = seed_mix(info.seed, 0);
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
        self.pending = [false; 128];
        if self.bypass.reset() {
            self.flush_alloff = true;
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let int = |lo: f32, hi: f32| plain.round().clamp(lo, hi) as i32;
        match id.0 {
            P_MODE => self.free_time = plain >= 0.5,
            P_SYNC => self.sync = int(0.0, 8.0) as usize,
            P_MS => self.ms = plain.clamp(0.0, 1000.0),
            P_SWING => self.swing = int(0.0, 75.0) as u8,
            P_GRID => self.grid = int(0.0, 1.0) as usize,
            P_HUM_TIME => self.hum_ms = plain.clamp(0.0, 20.0),
            P_HUM_VEL => self.hum_vel = plain.round().clamp(0.0, 32.0),
            P_SEED => self.seed_mix = seed_mix(self.seed, int(0.0, 999.0) as u16),
            P_BYPASS => self.bypass.set_param(plain),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let time = ctx.time;
        let tpf = if time.ticks_per_frame.is_finite() && time.ticks_per_frame > 0.0 {
            time.ticks_per_frame
        } else {
            // No usable tempo: delay at the tick rate of 120 BPM.
            120.0 * PPQ as f64 / (60.0 * self.sample_rate)
        };
        let time = atomik_core::TimeInfo {
            ticks_per_frame: tpf,
            ..time
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
            self.pending = [false; 128];
        }
        for i in 0..ctx.notes_in(NOTES_IN).len() {
            let e = ctx.notes_in(NOTES_IN)[i];
            if let Some(e) = self.bypass.route(ctx.notes_out(NOTES_OUT), e) {
                self.apply(ctx.notes_out(NOTES_OUT), e, &time, tpf);
            }
        }
        self.sink
            .drain(&mut self.queue, ctx.notes_out(NOTES_OUT), self.now);
        self.now += BATCH as u64;
    }
}

#[cfg(test)]
mod tests;
