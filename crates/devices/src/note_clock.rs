//! Shared machinery of the note players: the step grid and its integer tick rule,
//! jump detection on the engine transport, the fixed pending-off table, the CV writer and the
//! step driver. Used by `atomik.step-sequencer` and `atomik.arpeggiator`.
//!
//! Everything here is allocation-free and uses only f64 `+ - * /`, `floor` and `ceil` (no libm),
//! so the streams are bit-identical on every platform. Time comes only from the batch's
//! [`TimeInfo`](atomik_core::TimeInfo) (or a device-held tick advanced from it).
//!
//! * **Grid.** A step of `s` ticks starts at `k * s` plus, for odd `k`, the swing shift
//!   `(swing * s + 100) / 200` ticks (integer division). Step `k` is due at the first frame
//!   `ceil((start - batch_tick) / ticks_per_frame)`; a value within 1e-6 frame above an integer
//!   counts as that integer, so f64 noise of the accumulated tick never moves an event.
//! * **Jumps.** A batch is a jump when it is the first, when the clock mode changed, or when its
//!   tick is not the previous end tick within `64 * |tpf - previous tpf|` (the tempo map can change
//!   the tick rate inside a batch, so a bare epsilon would fire at every tempo change).
//! * **Pending offs.** A note's end is scheduled at its absolute tick in a table of 64 slots; a
//!   tie holds a note with an infinite end until the next step resolves it. When the table is
//!   full the oldest entry is closed at once.

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_device_sdk::{
    Direction, ProcessContext, SocketDescriptor, SocketId, SocketKind, StepData,
};
use atomik_dsp::rng::{hash64, unit_f64};

/// `notes_in` socket (both players).
pub(crate) const NOTES_IN: SocketId = SocketId(0);
/// `notes_out` socket.
pub(crate) const NOTES_OUT: SocketId = SocketId(1);
/// `gate_cv_out` socket.
pub(crate) const GATE_CV: SocketId = SocketId(2);
/// `pitch_cv_out` socket.
pub(crate) const PITCH_CV: SocketId = SocketId(3);
/// `velocity_cv_out` socket.
pub(crate) const VELOCITY_CV: SocketId = SocketId(4);
/// `trig_cv_out` socket.
pub(crate) const TRIG_CV: SocketId = SocketId(5);

const fn sock(
    id: u16,
    key: &'static str,
    name: &'static str,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind,
        direction,
    }
}

/// The six sockets of both players, in contract order.
pub(crate) static PLAYER_SOCKETS: [SocketDescriptor; 6] = [
    sock(
        0,
        "notes_in",
        "Notes in",
        SocketKind::Notes,
        Direction::Input,
    ),
    sock(
        1,
        "notes_out",
        "Notes out",
        SocketKind::Notes,
        Direction::Output,
    ),
    sock(
        2,
        "gate_cv_out",
        "Gate CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
    sock(
        3,
        "pitch_cv_out",
        "Pitch CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
    sock(
        4,
        "velocity_cv_out",
        "Velocity CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
    sock(
        5,
        "trig_cv_out",
        "Trig CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
];

/// `curve_a_cv_out` socket (CV sequencer).
pub(crate) const CURVE_A_CV: SocketId = SocketId(6);
/// `curve_b_cv_out` socket (CV sequencer).
pub(crate) const CURVE_B_CV: SocketId = SocketId(7);

/// The eight sockets of the CV sequencer: the six player sockets, then its two curve outputs.
pub(crate) static CV_SEQUENCER_SOCKETS: [SocketDescriptor; 8] = [
    PLAYER_SOCKETS[0],
    PLAYER_SOCKETS[1],
    PLAYER_SOCKETS[2],
    PLAYER_SOCKETS[3],
    PLAYER_SOCKETS[4],
    PLAYER_SOCKETS[5],
    sock(
        6,
        "curve_a_cv_out",
        "Curve A CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
    sock(
        7,
        "curve_b_cv_out",
        "Curve B CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
];

/// Golden-ratio constant mixing `random.seed` into the device seed.
const SEED_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// The hash seed of a device: its `PrepareInfo::seed` combined with the `random.seed` param.
pub(crate) fn seed_mix(device_seed: u64, random_seed: u16) -> u64 {
    device_seed ^ u64::from(random_seed).wrapping_mul(SEED_MIX)
}

/// Tolerance in frames when mapping a tick to a frame.
pub(crate) const EPS_FRAMES: f64 = 1e-6;
/// Slots of the pending-off table.
pub(crate) const PENDING_CAP: usize = 64;
/// Upper bound of driver iterations per segment (a safety net, never reached in practice).
const MAX_ITERATIONS: usize = 4_096;

/// A step grid: step length in ticks and swing percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grid {
    /// Step length in ticks.
    pub s: u32,
    /// Swing, 0..=75.
    pub swing: u8,
}

impl Grid {
    /// 1/16, no swing.
    pub const DEFAULT: Grid = Grid { s: 3_840, swing: 0 };

    /// Swing shift of an odd step in ticks: `(swing * s + 100) / 200`.
    pub fn shift(self) -> i64 {
        (i64::from(self.swing) * i64::from(self.s) + 100) / 200
    }

    /// Start tick of step `k` (swing included).
    pub fn start(self, k: i64) -> f64 {
        let odd = k & 1 == 1;
        (k * i64::from(self.s) + if odd { self.shift() } else { 0 }) as f64
    }

    /// The first step whose start is at or after `tick` (0 for a tick at or before zero).
    pub fn first_step_at_or_after(self, tick: f64, tpf: f64) -> i64 {
        let tol = tpf * EPS_FRAMES;
        if tick <= tol {
            return 0;
        }
        let k0 = (tick / f64::from(self.s)).floor() as i64;
        if self.start(k0) >= tick - tol {
            k0
        } else {
            k0 + 1
        }
    }

    /// End tick of a note that starts at `tick` (step `k`) and lasts `pct` percent of the step:
    /// at least 1 tick, never past the next step's start; 100 % (or more) is legato, ending
    /// exactly at the next step's start.
    pub fn gate_end(self, tick: f64, k: i64, pct: f64) -> f64 {
        self.gate_end_to(tick, self.start(k + 1), pct)
    }

    /// [`Grid::gate_end`] for a step whose successor starts at `next` (the next pattern of a
    /// chain starts off this grid).
    pub fn gate_end_to(self, tick: f64, next: f64, pct: f64) -> f64 {
        if pct >= 100.0 {
            return next;
        }
        let len = (f64::from(self.s) * pct / 100.0).floor().max(1.0);
        (tick + len).min(next)
    }
}

/// Detects locates and other transport jumps (see the module docs).
#[derive(Debug, Clone, Copy)]
pub(crate) struct JumpTracker {
    expected: Option<f64>,
    tpf: f64,
    mode: u8,
}

impl JumpTracker {
    pub const fn new() -> Self {
        Self {
            expected: None,
            tpf: 0.0,
            mode: 0,
        }
    }

    /// Forgets the position: the next batch is a jump.
    pub fn invalidate(&mut self) {
        self.expected = None;
    }

    /// Whether the batch starting at `tick` is a jump. `mode` distinguishes clock sources (a
    /// change of source is a jump).
    pub fn observe(&self, tick: f64, tpf: f64, mode: u8) -> bool {
        match self.expected {
            None => true,
            Some(e) => {
                mode != self.mode
                    || (tick - e).abs()
                        > BATCH as f64 * (tpf - self.tpf).abs() + 1e-6 + tick.abs() * 1e-12
            }
        }
    }

    /// Records where the batch ended.
    pub fn advance(&mut self, end_tick: f64, tpf: f64, mode: u8) {
        self.expected = Some(end_tick);
        self.tpf = tpf;
        self.mode = mode;
    }
}

/// A run of frames `a..b` of a batch whose tick at frame `a` is `t_a`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Segment {
    pub a: usize,
    pub b: usize,
    pub t_a: f64,
    pub tpf: f64,
}

impl Segment {
    /// The first frame at or after `tick`, clamped to `a`; `b` stands for "not in this segment".
    pub fn frame_of(&self, tick: f64) -> usize {
        let x = (tick - self.t_a) / self.tpf - EPS_FRAMES;
        if x <= 0.0 {
            self.a
        } else if x >= (self.b - self.a) as f64 {
            self.b
        } else {
            self.a + x.ceil() as usize
        }
    }

    /// The tick at the end of the batch (frame 64) when this is the last segment.
    pub fn end_tick(&self) -> f64 {
        self.t_a + (BATCH - self.a) as f64 * self.tpf
    }
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    tick: f64,
    key: u8,
    seq: u32,
    used: bool,
}

/// Fixed table of scheduled note ends (capacity [`PENDING_CAP`]).
#[derive(Debug, Clone)]
pub(crate) struct PendingOffs {
    slots: [Slot; PENDING_CAP],
    seq: u32,
}

impl PendingOffs {
    pub const fn new() -> Self {
        Self {
            slots: [Slot {
                tick: 0.0,
                key: 0,
                seq: 0,
                used: false,
            }; PENDING_CAP],
            seq: 0,
        }
    }

    pub fn clear(&mut self) {
        for s in &mut self.slots {
            s.used = false;
        }
    }

    pub fn any(&self) -> bool {
        self.slots.iter().any(|s| s.used)
    }

    pub fn is_full(&self) -> bool {
        self.slots.iter().all(|s| s.used)
    }

    /// The slot index of the oldest entry.
    fn oldest(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.used)
            .min_by_key(|(_, s)| s.seq)
            .map(|(i, _)| i)
    }

    /// Schedules the end of `key` at `tick` (infinity holds it). The table must not be full.
    pub fn push(&mut self, tick: f64, key: u8) {
        if let Some(slot) = self.slots.iter_mut().find(|s| !s.used) {
            self.seq = self.seq.wrapping_add(1);
            *slot = Slot {
                tick,
                key,
                seq: self.seq,
                used: true,
            };
        }
    }

    /// The earliest finite end as `(slot, tick)`, ties by insertion order.
    pub fn earliest(&self) -> Option<(usize, f64)> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.used && s.tick.is_finite())
            .min_by(|(_, a), (_, b)| a.tick.total_cmp(&b.tick).then(a.seq.cmp(&b.seq)))
            .map(|(i, s)| (i, s.tick))
    }

    /// Frees slot `i` and returns its key.
    pub fn take(&mut self, i: usize) -> u8 {
        self.slots[i].used = false;
        self.slots[i].key
    }

    /// The slot of a sounding `key`, whatever its scheduled end.
    pub fn find_key(&self, key: u8) -> Option<usize> {
        self.slots.iter().position(|s| s.used && s.key == key)
    }

    /// The slot of a held (infinite-end) `key`.
    pub fn find_hold(&self, key: u8) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.used && s.key == key && s.tick == f64::INFINITY)
    }

    /// Changes the end of slot `i`.
    pub fn set_tick(&mut self, i: usize, tick: f64) {
        self.slots[i].tick = tick;
    }
}

/// CV writer state: gate, pitch, velocity and trig, filled per sample up to each event.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CvState {
    /// Notes currently sounding (gate is high while non-zero).
    pub gate_count: u32,
    pitch: f32,
    vel: f32,
    trig_left: usize,
    trig_len: usize,
    pos: usize,
}

impl CvState {
    pub const fn new() -> Self {
        Self {
            gate_count: 0,
            pitch: 0.0,
            vel: 0.0,
            trig_left: 0,
            trig_len: 48,
            pos: 0,
        }
    }

    /// The trig pulse length for `sample_rate` (1 ms, at least 1 sample).
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.trig_len = ((sample_rate * 0.001).round() as usize).max(1);
    }

    pub fn reset(&mut self) {
        self.gate_count = 0;
        self.pitch = 0.0;
        self.vel = 0.0;
        self.trig_left = 0;
        self.pos = 0;
    }

    /// Writes the held values from the last position up to `upto` (exclusive).
    pub fn fill_to(&mut self, ctx: &mut ProcessContext<'_>, upto: usize) {
        let upto = upto.min(BATCH);
        if upto <= self.pos {
            return;
        }
        let (from, n) = (self.pos, upto - self.pos);
        let gate = if self.gate_count > 0 { 1.0 } else { 0.0 };
        ctx.output(GATE_CV)[from..upto].fill(gate);
        ctx.output(PITCH_CV)[from..upto].fill(self.pitch);
        ctx.output(VELOCITY_CV)[from..upto].fill(self.vel);
        let t = self.trig_left.min(n);
        ctx.output(TRIG_CV)[from..from + t].fill(1.0);
        self.trig_left -= t;
        self.pos = upto;
    }
}

/// The pitch CV of a key: `(key - 60) / 24` clamped to ±1.
pub(crate) fn pitch_cv(key: u8) -> f32 {
    ((f32::from(key) - 60.0) / 24.0).clamp(-1.0, 1.0)
}

/// Note output with its pending table and CV writer: everything a player emits goes through
/// here, so offsets are non-decreasing and every On has its scheduled Off.
#[derive(Debug, Clone)]
pub(crate) struct NoteCore {
    pub pending: PendingOffs,
    pub cv: CvState,
    /// Emit an `AllOff` at the start of the next batch (set by `reset` when notes sounded).
    pub flush_alloff: bool,
    last_offset: u16,
}

impl NoteCore {
    pub const fn new() -> Self {
        Self {
            pending: PendingOffs::new(),
            cv: CvState::new(),
            flush_alloff: false,
            last_offset: 0,
        }
    }

    /// Starts a batch: CV position and event ordering restart at frame 0.
    pub fn begin_batch(&mut self, ctx: &mut ProcessContext<'_>) {
        self.cv.pos = 0;
        self.last_offset = 0;
        if self.flush_alloff {
            self.flush_alloff = false;
            let _ = ctx.notes_out(NOTES_OUT).push(NoteEvent {
                offset: 0,
                kind: NoteKind::AllOff,
            });
        }
    }

    /// Finishes a batch: fills the CV outputs to the end.
    pub fn end_batch(&mut self, ctx: &mut ProcessContext<'_>) {
        self.cv.fill_to(ctx, BATCH);
    }

    /// `reset`: forget everything, remembering to send an `AllOff` if a note was sounding.
    pub fn reset(&mut self) {
        if self.pending.any() || self.cv.gate_count > 0 {
            self.flush_alloff = true;
        }
        self.pending.clear();
        self.cv.reset();
        self.last_offset = 0;
    }

    fn emit(&mut self, ctx: &mut ProcessContext<'_>, frame: usize, kind: NoteKind) -> bool {
        let offset = (frame.min(BATCH - 1) as u16).max(self.last_offset);
        self.cv.fill_to(ctx, usize::from(offset));
        if ctx
            .notes_out(NOTES_OUT)
            .push(NoteEvent { offset, kind })
            .is_err()
        {
            return false;
        }
        self.last_offset = offset;
        true
    }

    /// Emits a note-off for `key` and lowers the gate count.
    pub fn note_off(&mut self, ctx: &mut ProcessContext<'_>, frame: usize, key: u8) {
        if self.emit(ctx, frame, NoteKind::Off { key }) {
            self.cv.gate_count = self.cv.gate_count.saturating_sub(1);
        }
    }

    /// Emits a note-on and schedules its end at `end` (infinity holds it). A note of the same key
    /// that still sounds (its gate outlasted a change of grid) is closed first, at the same frame,
    /// so a receiver never sees two Ons of one key.
    pub fn note_on(
        &mut self,
        ctx: &mut ProcessContext<'_>,
        frame: usize,
        key: u8,
        velocity: f32,
        end: f64,
    ) {
        if let Some(i) = self.pending.find_key(key) {
            self.pending.take(i);
            self.note_off(ctx, frame, key);
        }
        if self.pending.is_full()
            && let Some(i) = self.pending.oldest()
        {
            let old = self.pending.take(i);
            self.note_off(ctx, frame, old);
        }
        if self.emit(ctx, frame, NoteKind::On { key, velocity }) {
            self.cv.gate_count += 1;
            self.pending.push(end, key);
        }
    }

    /// Sets the pitch and velocity CV and fires a trig pulse at `frame`.
    pub fn trigger(&mut self, ctx: &mut ProcessContext<'_>, frame: usize, key: u8, velocity: f32) {
        let offset = (frame.min(BATCH - 1) as u16).max(self.last_offset);
        self.cv.fill_to(ctx, usize::from(offset));
        self.cv.pitch = pitch_cv(key);
        self.cv.vel = velocity;
        self.cv.trig_left = self.cv.trig_len;
    }

    /// Closes the earliest pending note at `frame`.
    pub fn pop_earliest(&mut self, ctx: &mut ProcessContext<'_>, frame: usize) {
        if let Some((i, _)) = self.pending.earliest() {
            let key = self.pending.take(i);
            self.note_off(ctx, frame, key);
        }
    }

    /// Closes the held `key` at `frame`, if it is still held.
    pub fn release_hold(&mut self, ctx: &mut ProcessContext<'_>, frame: usize, key: u8) {
        if let Some(i) = self.pending.find_hold(key) {
            self.pending.take(i);
            self.note_off(ctx, frame, key);
        }
    }

    /// Ends the hold of `key` at `tick` instead (the next step plays the same key).
    pub fn retarget_hold(&mut self, key: u8, tick: f64) -> bool {
        match self.pending.find_hold(key) {
            Some(i) => {
                self.pending.set_tick(i, tick);
                true
            }
            None => false,
        }
    }

    /// Closes every sounding note at `frame`, oldest first.
    pub fn close_all(&mut self, ctx: &mut ProcessContext<'_>, frame: usize) {
        while let Some(i) = self.pending.oldest() {
            let key = self.pending.take(i);
            self.note_off(ctx, frame, key);
        }
        self.cv.gate_count = 0;
    }
}

/// Keys held on `notes_in`, most recent last (the `follow` transpose reads the last one).
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeldKeys {
    keys: [u8; 128],
    n: usize,
}

impl HeldKeys {
    pub const fn new() -> Self {
        Self {
            keys: [0; 128],
            n: 0,
        }
    }

    /// Forgets every key.
    pub fn clear(&mut self) {
        self.n = 0;
    }

    /// `key` goes down: it becomes the most recent.
    pub fn press(&mut self, key: u8) {
        self.release(key);
        if self.n < self.keys.len() {
            self.keys[self.n] = key;
            self.n += 1;
        }
    }

    /// `key` comes up.
    pub fn release(&mut self, key: u8) {
        if let Some(i) = self.keys[..self.n].iter().position(|k| *k == key) {
            self.keys.copy_within(i + 1..self.n, i);
            self.n -= 1;
        }
    }

    /// The most recent key minus 60 when `follow` is on and a key is held, else 0.
    pub fn follow_offset(&self, follow: bool) -> i32 {
        if follow && self.n > 0 {
            i32::from(self.keys[self.n - 1]) - 60
        } else {
            0
        }
    }
}

/// What a played step needs besides its own data: the params of the note row, shared by the
/// step sequencer and the CV sequencer so both play a step the same way.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StepRules {
    pub mute: bool,
    pub transpose: i32,
    /// Added to the key: the most recent held key minus 60 when `follow` is on, else 0.
    pub follow_offset: i32,
    pub prob_amount: f64,
    pub vel_scale: f64,
    pub gate_scale: f64,
    pub seed_mix: u64,
}

impl StepRules {
    /// Whether a step with probability `prob` percent plays at global step number `k`.
    pub fn prob_pass(&self, k: i64, prob: u8) -> bool {
        if prob >= 100 && self.prob_amount >= 1.0 {
            return true;
        }
        let eff = f64::from(prob) * self.prob_amount;
        eff > 0.0 && unit_f64(hash64(self.seed_mix, k as u64, 0)) * 100.0 < eff
    }
}

/// Plays step `k` (`step` is its data, `None` when there is none) that starts at `tick`, is due
/// at `frame` and whose successor starts at `next_start`: probability, key, velocity, gate, tie
/// and trig. `held_tie` is the key tied into this step by the previous one; on return it holds
/// the key this step ties into the next.
pub(crate) fn play_step(
    core: &mut NoteCore,
    ctx: &mut ProcessContext<'_>,
    held_tie: &mut Option<u8>,
    rules: &StepRules,
    step: Option<StepData>,
    at: StepAt,
) {
    let hold = held_tie.take();
    let played = step.filter(|sd| !rules.mute && sd.on && rules.prob_pass(at.k, sd.prob));
    let Some(sd) = played else {
        if let Some(hk) = hold {
            core.release_hold(ctx, at.frame, hk);
        }
        return;
    };
    let key = (i32::from(sd.key) + rules.transpose + rules.follow_offset).clamp(0, 127) as u8;
    let vel7 = ((f64::from(sd.velocity) * rules.vel_scale).round() as i64).clamp(1, 127);
    let velocity = vel7 as f32 / 127.0;
    let end = if sd.tie {
        f64::INFINITY
    } else {
        let pct = (f64::from(sd.gate) * rules.gate_scale).min(100.0);
        at.grid.gate_end_to(at.tick, at.next_start, pct)
    };
    match hold {
        Some(hk) if hk == key && core.retarget_hold(hk, end) => {}
        Some(hk) => {
            core.note_on(ctx, at.frame, key, velocity, end);
            core.release_hold(ctx, at.frame, hk);
        }
        None => core.note_on(ctx, at.frame, key, velocity, end),
    }
    core.trigger(ctx, at.frame, key, velocity);
    if sd.tie {
        *held_tie = Some(key);
    }
}

/// Where and when a step plays.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StepAt {
    /// The global step number (the probability hash input).
    pub k: i64,
    /// The grid of the step's pattern.
    pub grid: Grid,
    /// The step's start tick.
    pub tick: f64,
    /// The successor's start tick (the end of a legato note).
    pub next_start: f64,
    /// The frame the step is due at.
    pub frame: usize,
}

/// A device driven by the step grid; [`run_segment`] calls it in time order.
pub(crate) trait StepHost {
    /// The grid the next step lies on.
    fn grid(&self) -> Grid;
    /// The start tick of step `k`; the grid's own rule unless the host plays a chain of grids.
    fn start_of(&self, k: i64) -> f64 {
        self.grid().start(k)
    }
    /// The next step number to play.
    fn next_step(&self) -> i64;
    /// Sets the next step number (the driver sets `k + 1` before calling [`StepHost::on_step`];
    /// the host may override it, e.g. after a change of grid).
    fn set_next_step(&mut self, k: i64);
    /// The earliest pending note end, if any.
    fn earliest_off(&self) -> Option<f64>;
    /// Emits that note end at `frame`.
    fn pop_off(&mut self, frame: usize);
    /// Plays step `k`, which starts at `tick` and is due at `frame`.
    fn on_step(&mut self, k: i64, tick: f64, frame: usize);
}

/// Runs every note end and step due inside `seg` in time order; at the same frame ends go before
/// steps.
pub(crate) fn run_segment<H: StepHost>(host: &mut H, seg: &Segment) {
    for _ in 0..MAX_ITERATIONS {
        let off = host.earliest_off().map(|t| seg.frame_of(t));
        let k = host.next_step();
        let tick = host.start_of(k);
        let frame = seg.frame_of(tick);
        if let Some(fo) = off
            && fo < seg.b
            && fo <= frame
        {
            host.pop_off(fo);
            continue;
        }
        if frame >= seg.b {
            break;
        }
        host.set_next_step(k + 1);
        host.on_step(k, tick, frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swing_shift_is_the_integer_rule() {
        assert_eq!(Grid { s: 3_840, swing: 0 }.shift(), 0);
        assert_eq!(
            Grid {
                s: 3_840,
                swing: 50
            }
            .shift(),
            960
        );
        assert_eq!(
            Grid {
                s: 3_840,
                swing: 75
            }
            .shift(),
            1_440
        );
        assert_eq!(
            Grid {
                s: 1_280,
                swing: 33
            }
            .shift(),
            (33 * 1_280 + 100) / 200
        );
        // Half-up rounding: 57.6 rounds to 58, 25.6 to 26.
        assert_eq!(Grid { s: 3_840, swing: 3 }.shift(), 58);
        assert_eq!(Grid { s: 1_280, swing: 4 }.shift(), 26);
    }

    #[test]
    fn grid_start_and_first_step() {
        let g = Grid {
            s: 1_000,
            swing: 50,
        };
        assert_eq!(g.start(0), 0.0);
        assert_eq!(g.start(1), 1_250.0);
        assert_eq!(g.start(2), 2_000.0);
        assert_eq!(g.first_step_at_or_after(-5.0, 1.0), 0);
        assert_eq!(g.first_step_at_or_after(0.0, 1.0), 0);
        assert_eq!(g.first_step_at_or_after(0.5, 1.0), 1);
        assert_eq!(g.first_step_at_or_after(1_250.0, 1.0), 1);
        assert_eq!(g.first_step_at_or_after(1_251.0, 1.0), 2);
        assert_eq!(g.first_step_at_or_after(1_999.999_999_9, 1.0), 2);
    }

    #[test]
    fn gate_end_is_clamped_and_legato_at_full() {
        let g = Grid {
            s: 1_000,
            swing: 50,
        };
        assert_eq!(g.gate_end(0.0, 0, 50.0), 500.0);
        assert_eq!(
            g.gate_end(0.0, 0, 100.0),
            1_250.0,
            "legato reaches the swung step"
        );
        assert_eq!(g.gate_end(1_250.0, 1, 100.0), 2_000.0);
        assert_eq!(
            g.gate_end(1_250.0, 1, 90.0),
            2_000.0,
            "clamped to the next start"
        );
        assert_eq!(g.gate_end(0.0, 0, 0.01), 1.0, "at least one tick");
    }

    #[test]
    fn jump_tracker_accepts_tempo_changes_but_not_locates() {
        let mut j = JumpTracker::new();
        assert!(j.observe(0.0, 0.64, 0), "first batch is a jump");
        j.advance(40.96, 0.64, 0);
        assert!(!j.observe(40.96, 0.64, 0));
        assert!(j.observe(41.5, 0.64, 0), "a real locate");
        // A tempo change inside the previous batch: new tpf, tick between the two predictions.
        let new = 0.8;
        assert!(!j.observe(40.96 + 64.0 * (new - 0.64) / 2.0, new, 0));
        assert!(!j.observe(40.96 + 64.0 * (new - 0.64), new, 0));
        assert!(j.observe(40.96 + 64.0 * (new - 0.64) * 2.0 + 1.0, new, 0));
        assert!(
            j.observe(40.96, 0.64, 1),
            "a change of clock source is a jump"
        );
        j.invalidate();
        assert!(j.observe(40.96, 0.64, 0));
    }

    #[test]
    fn segment_maps_ticks_to_frames_with_a_tolerance() {
        let s = Segment {
            a: 0,
            b: 64,
            t_a: 0.0,
            tpf: 0.64,
        };
        assert_eq!(s.frame_of(0.0), 0);
        assert_eq!(
            s.frame_of(0.64),
            1,
            "an exact boundary is not pushed to the next frame"
        );
        assert_eq!(s.frame_of(0.641), 2);
        assert_eq!(s.frame_of(-4.0), 0);
        assert_eq!(s.frame_of(1_000.0), 64);
        assert!((s.end_tick() - 40.96).abs() < 1e-12);
    }

    #[test]
    fn pending_table_orders_by_tick_then_insertion_and_holds() {
        let mut p = PendingOffs::new();
        p.push(30.0, 1);
        p.push(10.0, 2);
        p.push(10.0, 3);
        p.push(f64::INFINITY, 9);
        let (i, t) = p.earliest().unwrap();
        assert_eq!((p.take(i), t), (2, 10.0));
        let (i, _) = p.earliest().unwrap();
        assert_eq!(p.take(i), 3);
        let (i, _) = p.earliest().unwrap();
        assert_eq!(p.take(i), 1);
        assert!(p.earliest().is_none(), "a held note has no due end");
        assert!(p.any());
        let h = p.find_hold(9).unwrap();
        p.set_tick(h, 5.0);
        assert!(p.find_hold(9).is_none());
        assert!(p.earliest().is_some());
    }

    #[test]
    fn seed_mix_changes_with_the_param_and_is_the_seed_at_zero() {
        assert_eq!(seed_mix(5, 0), 5);
        assert_ne!(seed_mix(5, 1), seed_mix(5, 2));
        assert_ne!(seed_mix(5, 1), 5);
    }

    #[test]
    fn pitch_cv_is_semitones_over_24_clamped() {
        assert_eq!(pitch_cv(60), 0.0);
        assert_eq!(pitch_cv(72), 0.5);
        assert_eq!(pitch_cv(127), 1.0);
        assert_eq!(pitch_cv(0), -1.0);
    }
}
