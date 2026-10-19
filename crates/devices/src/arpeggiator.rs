//! `atomik.arpeggiator`: turns the held keys of `notes_in` into a tempo-synced figure on
//! `notes_out`. It takes no pattern data.
//!
//! * **Order.** The held set is sorted by key for `up`, `down`, `up-down` and `random`, and kept
//!   in press order for `as-played`. `octaves` stacks transposed copies (`key + 12 * octave`; a
//!   key above 127 is folded down by octaves). `up-down` does not repeat its end points. `random`
//!   draws a uniform index from the pooled notes with a stateless hash of the seed, the global
//!   step number and slot 1 (`atomik_dsp::rng::hash64`). `chord` plays the whole set at every step,
//!   one octave offset per step cycling through the range.
//! * **Time.** Steps are on the same grid as the sequencer (see `crate::note_clock`): rate 1/4 to
//!   1/32T, swing 0 to 75 %, gate 5 to 100 % of the step (100 % is legato: the note ends exactly at
//!   the next step). Notes start on a step boundary, so a key pressed mid-step waits for the
//!   next one; the cycle restarts from its first note when the set goes from empty to non-empty.
//! * **Hold.** Released keys stay in the set; the next press once every physical key is up starts a
//!   fresh set; turning `hold` off drops the latched keys that are not down.
//! * **Run.** `always` (default): with the transport stopped the arpeggiator runs on an internal
//!   clock that advances at the current tempo from the last playhead tick (a change of clock source
//!   is a jump, so no note hangs); `transport`: silent while stopped.
//! * **Note safety.** Notes are closed on a jump (locate, loop wrap, start or stop of the
//!   transport, change of clock source), `AllOff`, and a `reset` leaves an `AllOff` for the next
//!   batch. A released key does not cut its note: the scheduled gate end closes it.
//! * **Velocity** is the key's input velocity (`input`) or `velocity.fixed`.
//! * **Meter values:** the number of held keys, then the next 8 keys the arpeggiator will play (0 =
//!   none; in chord mode the lowest key of the chord).

use atomik_core::{
    BATCH, MAX_EVENTS_PER_BATCH, NoteEvent, NoteKind, ParamDescriptor, ParamId, Taper, Unit,
};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext, SeqResolution,
};
use atomik_dsp::rng::{below, hash64};

use crate::note_clock::{
    Grid, JumpTracker, NOTES_IN, NoteCore, PLAYER_SOCKETS, Segment, StepHost, run_segment, seed_mix,
};

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

const MODES: Unit = Unit::Choice(&["up", "down", "up-down", "random", "as-played", "chord"]);
const RATES: Unit = Unit::Choice(&["1/4", "1/8", "1/8T", "1/16", "1/16T", "1/32", "1/32T"]);
const VELOCITY_MODES: Unit = Unit::Choice(&["input", "fixed"]);
const RUN: Unit = Unit::Choice(&["transport", "always"]);

const P_MODE: u32 = 0;
const P_RATE: u32 = 1;
const P_OCTAVES: u32 = 2;
const P_GATE: u32 = 3;
const P_SWING: u32 = 4;
const P_HOLD: u32 = 5;
const P_VEL_MODE: u32 = 6;
const P_VEL_FIXED: u32 = 7;
const P_RUN: u32 = 8;
const P_SEED: u32 = 9;

static PARAMS: [ParamDescriptor; 10] = [
    param(P_MODE, "mode", "Mode", (0.0, 5.0, 0.0), MODES, true),
    param(P_RATE, "rate", "Rate", (0.0, 6.0, 3.0), RATES, true),
    param(
        P_OCTAVES,
        "octaves",
        "Octaves",
        (1.0, 4.0, 1.0),
        Unit::None,
        true,
    ),
    param(
        P_GATE,
        "gate",
        "Gate",
        (5.0, 100.0, 80.0),
        Unit::None,
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
    param(P_HOLD, "hold", "Hold", (0.0, 1.0, 0.0), Unit::Toggle, true),
    param(
        P_VEL_MODE,
        "velocity.mode",
        "Velocity mode",
        (0.0, 1.0, 0.0),
        VELOCITY_MODES,
        true,
    ),
    param(
        P_VEL_FIXED,
        "velocity.fixed",
        "Fixed velocity",
        (1.0, 127.0, 100.0),
        Unit::None,
        true,
    ),
    param(P_RUN, "run", "Run", (0.0, 1.0, 1.0), RUN, true),
    param(
        P_SEED,
        "random.seed",
        "Random seed",
        (0.0, 999.0, 0.0),
        Unit::None,
        true,
    ),
];

/// Static description of the arpeggiator.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.arpeggiator",
    name: "Arpeggiator",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &PLAYER_SOCKETS,
};

/// Factory registering [`Arpeggiator`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Arpeggiator::new()),
    }
}

/// Arpeggio order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Up,
    Down,
    UpDown,
    Random,
    AsPlayed,
    Chord,
}

impl Mode {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Up,
            1 => Self::Down,
            2 => Self::UpDown,
            3 => Self::Random,
            4 => Self::AsPlayed,
            _ => Self::Chord,
        }
    }
}

/// Keys per set (all MIDI keys).
const KEYS: usize = 128;
/// Next keys published by [`Device::meter_values`].
const PREVIEW: usize = 8;

/// The held key set: members (what is arpeggiated), physical keys down and press order.
#[derive(Debug, Clone)]
struct KeySet {
    member: [bool; KEYS],
    down: [bool; KEYS],
    order: [u8; KEYS],
    vel: [f32; KEYS],
    n: usize,
    phys: usize,
}

impl KeySet {
    const fn new() -> Self {
        Self {
            member: [false; KEYS],
            down: [false; KEYS],
            order: [0; KEYS],
            vel: [0.0; KEYS],
            n: 0,
            phys: 0,
        }
    }

    fn clear(&mut self) {
        *self = Self::new();
    }

    fn clear_members(&mut self) {
        self.member = [false; KEYS];
        self.n = 0;
    }

    fn remove(&mut self, key: u8) {
        let k = usize::from(key);
        if self.member[k] {
            self.member[k] = false;
            if let Some(i) = self.order[..self.n].iter().position(|x| *x == key) {
                self.order.copy_within(i + 1..self.n, i);
                self.n -= 1;
            }
        }
    }

    /// A key goes down; returns whether the cycle must restart (the set was empty or fresh).
    fn press(&mut self, key: u8, velocity: f32, hold: bool) -> bool {
        let mut restart = self.n == 0;
        if hold && self.phys == 0 && self.n > 0 {
            self.clear_members();
            restart = true;
        }
        let k = usize::from(key & 0x7F);
        if !self.down[k] {
            self.down[k] = true;
            self.phys += 1;
        }
        if !self.member[k] {
            self.member[k] = true;
            self.order[self.n] = k as u8;
            self.n += 1;
        }
        self.vel[k] = velocity;
        restart
    }

    fn release(&mut self, key: u8, hold: bool) {
        let k = usize::from(key & 0x7F);
        if self.down[k] {
            self.down[k] = false;
            self.phys -= 1;
        }
        if !hold {
            self.remove(k as u8);
        }
    }

    /// Drops the latched keys that are not physically down.
    fn unlatch(&mut self) {
        for k in 0..KEYS {
            if self.member[k] && !self.down[k] {
                self.remove(k as u8);
            }
        }
    }

    /// Writes the members in ascending key order and returns how many.
    fn fill_sorted(&self, out: &mut [u8; KEYS]) -> usize {
        let mut m = 0;
        for (k, member) in self.member.iter().enumerate() {
            if *member {
                out[m] = k as u8;
                m += 1;
            }
        }
        m
    }
}

/// Folds a key above 127 down by octaves.
fn fold(mut key: u32) -> u8 {
    while key > 127 {
        key -= 12;
    }
    key as u8
}

/// The base key and the output key of arpeggio position `n` for every mode but chord (chord returns
/// the lowest key of the chord for octave `n % octaves`). `sorted` and `order` are the set in key
/// and press order; `rnd` is the step's random hash. `sorted` must not be empty.
fn pick(mode: Mode, sorted: &[u8], order: &[u8], octaves: u32, n: u64, rnd: u64) -> (u8, u8) {
    let m = sorted.len() as u64;
    let oct = u64::from(octaves.max(1));
    let l = m * oct;
    let (base, o) = match mode {
        Mode::Chord => (sorted[0], n % oct),
        _ => {
            let i = match mode {
                Mode::Up | Mode::AsPlayed => n % l,
                Mode::Down => l - 1 - n % l,
                Mode::UpDown => {
                    if l == 1 {
                        0
                    } else {
                        let p = 2 * l - 2;
                        let c = n % p;
                        if c < l { c } else { p - c }
                    }
                }
                Mode::Random => u64::from(below(rnd, l as u32)),
                Mode::Chord => 0,
            };
            let keys = if mode == Mode::AsPlayed {
                order
            } else {
                sorted
            };
            (keys[(i % m) as usize], i / m)
        }
    };
    (base, fold(u32::from(base) + 12 * o as u32))
}

/// The arpeggiator device.
pub struct Arpeggiator {
    core: NoteCore,
    clock: JumpTracker,
    seed: u64,
    seed_mix: u64,
    // Params.
    mode: Mode,
    rate: usize,
    octaves: u32,
    gate: f64,
    swing: u8,
    hold: bool,
    unlatch: bool,
    fixed_velocity: bool,
    velocity: u8,
    run_always: bool,
    // Clock.
    grid: Grid,
    next_step: i64,
    tpf: f64,
    int_tick: f64,
    int_valid: bool,
    cycle: u64,
    // Inputs.
    inbuf: Vec<NoteEvent>,
    in_cursor: usize,
    set: KeySet,
    // Display.
    held_count: u8,
    preview: [u8; PREVIEW],
}

impl Arpeggiator {
    /// A new arpeggiator with the default params.
    pub fn new() -> Self {
        Self {
            core: NoteCore::new(),
            clock: JumpTracker::new(),
            seed: 0,
            seed_mix: 0,
            mode: Mode::Up,
            rate: 3,
            octaves: 1,
            gate: 80.0,
            swing: 0,
            hold: false,
            unlatch: false,
            fixed_velocity: false,
            velocity: 100,
            run_always: true,
            grid: Grid::DEFAULT,
            next_step: 0,
            tpf: 1.0,
            int_tick: 0.0,
            int_valid: false,
            cycle: 0,
            inbuf: Vec::with_capacity(MAX_EVENTS_PER_BATCH),
            in_cursor: 0,
            set: KeySet::new(),
            held_count: 0,
            preview: [0; PREVIEW],
        }
    }

    fn param_grid(&self) -> Grid {
        Grid {
            s: SeqResolution::ALL[self.rate.min(6)].step_ticks(),
            swing: self.swing,
        }
    }

    fn refresh_grid(&mut self, tick: f64, jump: bool) {
        let g = self.param_grid();
        if jump || g != self.grid {
            self.grid = g;
            self.next_step = g.first_step_at_or_after(tick, self.tpf);
        }
    }

    fn apply_inputs(&mut self, ctx: &mut ProcessContext<'_>, upto: usize, inclusive: bool) {
        while self.in_cursor < self.inbuf.len() {
            let e = self.inbuf[self.in_cursor];
            let at = usize::from(e.offset);
            if at > upto || (at == upto && !inclusive) {
                break;
            }
            self.in_cursor += 1;
            match e.kind {
                NoteKind::On { key, velocity } => {
                    if self.set.press(key, velocity, self.hold) {
                        self.cycle = 0;
                    }
                }
                NoteKind::Off { key } => self.set.release(key, self.hold),
                NoteKind::AllOff => {
                    self.set.clear();
                    self.core.close_all(ctx, at);
                }
                // Channel messages and expressions are not held keys; the arpeggiator ignores
                // them.
                _ => {}
            }
        }
    }

    fn rnd(&self, k: i64) -> u64 {
        hash64(self.seed_mix, k as u64, 1)
    }

    /// Fills the published preview of the next notes.
    fn update_preview(&mut self) {
        self.held_count = self.set.n.min(255) as u8;
        self.preview = [0; PREVIEW];
        if self.set.n == 0 {
            return;
        }
        let mut sorted = [0u8; KEYS];
        let m = self.set.fill_sorted(&mut sorted);
        for i in 0..PREVIEW {
            let (_, key) = pick(
                self.mode,
                &sorted[..m],
                &self.set.order[..self.set.n],
                self.octaves,
                self.cycle + i as u64,
                self.rnd(self.next_step + i as i64),
            );
            self.preview[i] = key;
        }
    }

    fn run(
        &mut self,
        ctx: &mut ProcessContext<'_>,
        tick: f64,
        tpf: f64,
        wrap: Option<atomik_core::LoopWrap>,
    ) -> f64 {
        match wrap {
            Some(w) if (1..BATCH).contains(&usize::from(w.offset)) => {
                let at = usize::from(w.offset);
                let seg = Segment {
                    a: 0,
                    b: at,
                    t_a: tick,
                    tpf,
                };
                run_segment(
                    &mut Run {
                        s: &mut *self,
                        ctx: &mut *ctx,
                    },
                    &seg,
                );
                self.apply_inputs(ctx, at, false);
                self.core.close_all(ctx, at);
                self.refresh_grid(w.to_tick, true);
                let seg = Segment {
                    a: at,
                    b: BATCH,
                    t_a: w.to_tick,
                    tpf,
                };
                run_segment(
                    &mut Run {
                        s: &mut *self,
                        ctx: &mut *ctx,
                    },
                    &seg,
                );
                seg.end_tick()
            }
            _ => {
                let seg = Segment {
                    a: 0,
                    b: BATCH,
                    t_a: tick,
                    tpf,
                };
                run_segment(
                    &mut Run {
                        s: &mut *self,
                        ctx: &mut *ctx,
                    },
                    &seg,
                );
                seg.end_tick()
            }
        }
    }
}

impl Default for Arpeggiator {
    fn default() -> Self {
        Self::new()
    }
}

/// The arpeggiator bound to one batch's context, driven by [`run_segment`].
struct Run<'a, 'c> {
    s: &'a mut Arpeggiator,
    ctx: &'a mut ProcessContext<'c>,
}

impl StepHost for Run<'_, '_> {
    fn grid(&self) -> Grid {
        self.s.grid
    }

    fn next_step(&self) -> i64 {
        self.s.next_step
    }

    fn set_next_step(&mut self, k: i64) {
        self.s.next_step = k;
    }

    fn earliest_off(&self) -> Option<f64> {
        self.s.core.pending.earliest().map(|(_, t)| t)
    }

    fn pop_off(&mut self, frame: usize) {
        self.s.core.pop_earliest(self.ctx, frame);
    }

    fn on_step(&mut self, k: i64, tick: f64, frame: usize) {
        let s = &mut *self.s;
        s.apply_inputs(self.ctx, frame, true);
        if s.set.n == 0 {
            return;
        }
        let n = s.cycle;
        s.cycle += 1;
        let mut sorted = [0u8; KEYS];
        let m = s.set.fill_sorted(&mut sorted);
        let end = s.grid.gate_end(tick, k, s.gate);
        let velocity_of = |s: &Arpeggiator, base: u8| {
            if s.fixed_velocity {
                f32::from(s.velocity) / 127.0
            } else {
                s.set.vel[usize::from(base)].clamp(1.0 / 127.0, 1.0)
            }
        };
        if s.mode == Mode::Chord {
            let o = 12 * (n % u64::from(s.octaves)) as u32;
            for &base in &sorted[..m] {
                let key = fold(u32::from(base) + o);
                let vel = velocity_of(s, base);
                s.core.note_on(self.ctx, frame, key, vel, end);
                s.core.trigger(self.ctx, frame, key, vel);
            }
        } else {
            let (base, key) = pick(
                s.mode,
                &sorted[..m],
                &s.set.order[..s.set.n],
                s.octaves,
                n,
                s.rnd(k),
            );
            let vel = velocity_of(s, base);
            s.core.note_on(self.ctx, frame, key, vel, end);
            s.core.trigger(self.ctx, frame, key, vel);
        }
    }
}

impl Device for Arpeggiator {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.seed = info.seed;
        self.seed_mix = seed_mix(info.seed, 0);
        self.core.cv.set_sample_rate(info.sample_rate.hz());
        self.reset();
        self.core.flush_alloff = false;
    }

    fn reset(&mut self) {
        self.core.reset();
        self.clock.invalidate();
        self.set.clear();
        self.cycle = 0;
        self.next_step = 0;
        self.int_valid = false;
        self.unlatch = false;
        self.held_count = 0;
        self.preview = [0; PREVIEW];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_MODE => self.mode = Mode::from_index(plain.round().clamp(0.0, 5.0) as usize),
            P_RATE => self.rate = plain.round().clamp(0.0, 6.0) as usize,
            P_OCTAVES => self.octaves = plain.round().clamp(1.0, 4.0) as u32,
            P_GATE => self.gate = f64::from(plain.clamp(5.0, 100.0)),
            P_SWING => self.swing = plain.round().clamp(0.0, 75.0) as u8,
            P_HOLD => {
                let h = plain >= 0.5;
                if self.hold && !h {
                    self.unlatch = true;
                }
                self.hold = h;
            }
            P_VEL_MODE => self.fixed_velocity = plain >= 0.5,
            P_VEL_FIXED => self.velocity = plain.round().clamp(1.0, 127.0) as u8,
            P_RUN => self.run_always = plain >= 0.5,
            P_SEED => {
                self.seed_mix = seed_mix(self.seed, plain.round().clamp(0.0, 999.0) as u16);
            }
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let time = ctx.time;
        self.inbuf.clear();
        self.inbuf.extend_from_slice(ctx.notes_in(NOTES_IN));
        self.in_cursor = 0;
        self.core.begin_batch(ctx);
        if self.unlatch {
            self.unlatch = false;
            self.set.unlatch();
        }
        let tpf = time.ticks_per_frame;
        self.tpf = tpf;
        let tpf_ok = tpf > 0.0 && tpf.is_finite();
        // Clock source: the transport, or the internal clock while stopped (`run = always`).
        let source = if !tpf_ok {
            None
        } else if time.playing {
            self.int_valid = false;
            Some((time.tick, 0u8))
        } else if self.run_always {
            if !self.int_valid {
                self.int_tick = time.tick;
                self.int_valid = true;
            }
            let t = self.int_tick;
            self.int_tick += BATCH as f64 * tpf;
            Some((t, 1u8))
        } else {
            self.int_valid = false;
            None
        };
        let Some((tick, mode)) = source else {
            self.core.close_all(ctx, 0);
            self.clock.invalidate();
            self.apply_inputs(ctx, BATCH, true);
            self.update_preview();
            self.core.end_batch(ctx);
            return;
        };
        let jump = self.clock.observe(tick, tpf, mode);
        if jump {
            self.core.close_all(ctx, 0);
        }
        self.refresh_grid(tick, jump);
        let wrap = if mode == 0 { time.loop_wrap } else { None };
        let end = self.run(ctx, tick, tpf, wrap);
        self.apply_inputs(ctx, BATCH, true);
        self.clock.advance(end, tpf, mode);
        self.update_preview();
        self.core.end_batch(ctx);
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let mut v = [0.0f32; 1 + PREVIEW];
        v[0] = f32::from(self.held_count);
        for (dst, k) in v[1..].iter_mut().zip(self.preview) {
            *dst = f32::from(k);
        }
        let n = v.len().min(out.len());
        out[..n].copy_from_slice(&v[..n]);
        n
    }
}

#[cfg(test)]
mod tests;
