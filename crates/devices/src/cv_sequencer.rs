//! `atomik.cv-sequencer`: the step sequencer's note row plus two rows of control voltages.
//!
//! The device plays the [`atomik_device_sdk::CvSeqData`] installed with
//! [`Device::swap_player_data`]: the same patterns as `atomik.step-sequencer` (a plain
//! [`atomik_device_sdk::StepSeqData`] is accepted too and gives flat curves) and, per pattern,
//! curve rows A and B of 64 steps. Like the step sequencer it takes the musical position from
//! `TimeInfo` and derives the step from the tick every batch, so a locate lands on the right step
//! and a bounce, a live run and a loop agree.
//!
//! * **Chain.** `pattern` selects the first pattern and `chain` how many patterns play in order
//!   (`pattern`, `pattern + 1`, ..., wrapping at the pattern count; 1 plays one pattern like the
//!   step sequencer). The position is a pure function of the tick (`crate::chain_clock`): the
//!   chain repeats every `P` ticks, the sum of each pattern's length times its own step size, and
//!   a pattern's swing counts from its own first step. So with a chain of one the note row is
//!   the step sequencer's, except that a pattern of odd length restarts its swing at each pass
//!   instead of continuing it. A change of `pattern` or `chain` with `pattern.switch = at-end`
//!   is adopted at the start of the next pass of the running chain (at once after a jump or
//!   while stopped); `now` adopts at the next batch.
//! * **Notes** are played by the step sequencer's rules (`crate::note_clock::play_step`):
//!   monophonic steps with gate, tie, probability, `transpose`, `follow`, `velocity.scale` and
//!   `gate.scale`. A legato or tied note ends at the next step of the chain.
//! * **Curves.** Each row is a control voltage held for the step; a step with `slide` ramps
//!   linearly to the next step's value over its own length. The value of every sample comes
//!   from that sample's tick (no accumulation), so it is the same whatever the block size. A
//!   value is -100..=100 %: `bipolar` outputs it as -1..=1, `unipolar` as 0..=1 ((v + 1) / 2),
//!   then `curve_x.amount` scales it. `mute` silences the notes only; the curves keep running.
//! * **Stop and bypass.** The curves hold their last value and the gate drops (the notes are
//!   closed). While bypassed the curves hold their last value too and `notes_in` goes straight
//!   to `notes_out` (see `crate::note_fx::PlayerBypass`). Before anything has played a curve
//!   outputs 0.
//! * **Note safety** is the step sequencer's: notes close on stop, jump (locate, loop wrap,
//!   start), `mute`, `AllOff` and `reset`.
//! * **Outputs.** `gate_cv_out`, `pitch_cv_out`, `velocity_cv_out`, `trig_cv_out` as the step
//!   sequencer's, then `curve_a_cv_out` and `curve_b_cv_out`.
//! * **Meter values** (`has_meter_values`): the pattern of the last started step (0-based), its
//!   step index and 1 while the transport runs.

use atomik_core::{
    BATCH, MAX_EVENTS_PER_BATCH, NoteEvent, NoteKind, ParamDescriptor, ParamId, Taper, Unit,
};
use atomik_device_sdk::{
    CURVE_ROWS, CurveStep, Device, DeviceDescriptor, DeviceFactory, DeviceKind, PlayerDataRef,
    PrepareInfo, ProcessContext, StepData,
};

use crate::bypass::bypass_param;
use crate::chain_clock::{ChainClock, ChainStep};
use crate::note_clock::{
    CURVE_A_CV, CURVE_B_CV, CV_SEQUENCER_SOCKETS, HeldKeys, JumpTracker, NOTES_IN, NOTES_OUT,
    NoteCore, Segment, StepAt, StepHost, StepRules, play_step, run_segment, seed_mix,
};
use crate::note_fx::PlayerBypass;

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

const SWITCH: Unit = Unit::Choice(&["now", "at-end"]);
const FOLLOW: Unit = Unit::Choice(&["off", "transpose"]);
const POLARITY: Unit = Unit::Choice(&["bipolar", "unipolar"]);

const P_PATTERN: u32 = 0;
const P_SWITCH: u32 = 1;
const P_CHAIN: u32 = 2;
const P_TRANSPOSE: u32 = 3;
const P_FOLLOW: u32 = 4;
const P_PROB: u32 = 5;
const P_SEED: u32 = 6;
const P_VELOCITY: u32 = 7;
const P_GATE: u32 = 8;
const P_POLARITY_A: u32 = 9;
const P_POLARITY_B: u32 = 10;
const P_AMOUNT_A: u32 = 11;
const P_AMOUNT_B: u32 = 12;
const P_MUTE: u32 = 13;
const P_BYPASS: u32 = 14;

static PARAMS: [ParamDescriptor; 15] = [
    param(
        P_PATTERN,
        "pattern",
        "Pattern",
        (1.0, 8.0, 1.0),
        Unit::None,
        true,
    ),
    param(
        P_SWITCH,
        "pattern.switch",
        "Pattern switch",
        (0.0, 1.0, 1.0),
        SWITCH,
        true,
    ),
    param(P_CHAIN, "chain", "Chain", (1.0, 8.0, 1.0), Unit::None, true),
    param(
        P_TRANSPOSE,
        "transpose",
        "Transpose",
        (-24.0, 24.0, 0.0),
        Unit::Semitones,
        true,
    ),
    param(P_FOLLOW, "follow", "Follow", (0.0, 1.0, 0.0), FOLLOW, true),
    param(
        P_PROB,
        "probability.amount",
        "Probability amount",
        (0.0, 1.0, 1.0),
        Unit::Percent,
        false,
    ),
    param(
        P_SEED,
        "random.seed",
        "Random seed",
        (0.0, 999.0, 0.0),
        Unit::None,
        true,
    ),
    param(
        P_VELOCITY,
        "velocity.scale",
        "Velocity scale",
        (0.0, 2.0, 1.0),
        Unit::None,
        false,
    ),
    param(
        P_GATE,
        "gate.scale",
        "Gate scale",
        (0.25, 2.0, 1.0),
        Unit::None,
        false,
    ),
    param(
        P_POLARITY_A,
        "curve_a.polarity",
        "Curve A polarity",
        (0.0, 1.0, 0.0),
        POLARITY,
        true,
    ),
    param(
        P_POLARITY_B,
        "curve_b.polarity",
        "Curve B polarity",
        (0.0, 1.0, 0.0),
        POLARITY,
        true,
    ),
    param(
        P_AMOUNT_A,
        "curve_a.amount",
        "Curve A amount",
        (0.0, 1.0, 1.0),
        Unit::Percent,
        false,
    ),
    param(
        P_AMOUNT_B,
        "curve_b.amount",
        "Curve B amount",
        (0.0, 1.0, 1.0),
        Unit::Percent,
        false,
    ),
    param(P_MUTE, "mute", "Mute", (0.0, 1.0, 0.0), Unit::Toggle, true),
    bypass_param(P_BYPASS),
];

/// Static description of the CV sequencer.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.cv-sequencer",
    name: "CV sequencer",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &CV_SEQUENCER_SOCKETS,
};

/// Factory registering [`CvSequencer`] (register it with `register_with_curves`).
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(CvSequencer::new()),
    }
}

/// The CV sequencer device.
pub struct CvSequencer {
    data: Option<PlayerDataRef>,
    core: NoteCore,
    clock: JumpTracker,
    bypass: PlayerBypass,
    seed: u64,
    seed_mix: u64,
    // Params.
    queued: usize,
    queued_chain: usize,
    cur: usize,
    cur_chain: usize,
    switch_now: bool,
    transpose: i32,
    follow: bool,
    prob_amount: f64,
    mute: bool,
    vel_scale: f64,
    gate_scale: f64,
    unipolar: [bool; CURVE_ROWS],
    amount: [f64; CURVE_ROWS],
    close_pending: bool,
    // Clock state.
    chain: ChainClock,
    next_step: i64,
    tpf: f64,
    held_tie: Option<u8>,
    // Inputs.
    inbuf: Vec<NoteEvent>,
    in_cursor: usize,
    held: HeldKeys,
    // Curve outputs: the frames written this batch, the last value (percent) of each row.
    curve_pos: usize,
    curve_last: [f64; CURVE_ROWS],
    have_curve: bool,
    // Display.
    last_pattern: u32,
    last_step_idx: u32,
    running: bool,
}

impl CvSequencer {
    /// A new sequencer without data.
    pub fn new() -> Self {
        Self {
            data: None,
            core: NoteCore::new(),
            clock: JumpTracker::new(),
            bypass: PlayerBypass::new(),
            seed: 0,
            seed_mix: 0,
            queued: 0,
            queued_chain: 1,
            cur: 0,
            cur_chain: 1,
            switch_now: false,
            transpose: 0,
            follow: false,
            prob_amount: 1.0,
            mute: false,
            vel_scale: 1.0,
            gate_scale: 1.0,
            unipolar: [false; CURVE_ROWS],
            amount: [1.0; CURVE_ROWS],
            close_pending: false,
            chain: ChainClock::new(None, 0, 1),
            next_step: 0,
            tpf: 1.0,
            held_tie: None,
            inbuf: Vec::with_capacity(MAX_EVENTS_PER_BATCH),
            in_cursor: 0,
            held: HeldKeys::new(),
            curve_pos: 0,
            curve_last: [0.0; CURVE_ROWS],
            have_curve: false,
            last_pattern: 0,
            last_step_idx: 0,
            running: false,
        }
    }

    fn build_chain(&self) -> ChainClock {
        ChainClock::new(
            self.data.as_deref().and_then(|d| d.notes()),
            self.cur,
            self.cur_chain,
        )
    }

    /// Step data of chain step `k`.
    fn step_at(&self, k: i64) -> (Option<StepData>, ChainStep) {
        let cs = self.chain.step(k);
        let sd = self
            .data
            .as_deref()
            .and_then(|d| d.notes()?.pattern(cs.pattern))
            .map(|p| p.steps[cs.index]);
        (sd, cs)
    }

    /// Curve step of row `row` at chain step `k` (a flat zero without curve data).
    fn curve_step(&self, k: i64, row: usize) -> CurveStep {
        let cs = self.chain.step(k);
        self.data
            .as_deref()
            .and_then(|d| d.curve(cs.pattern, row))
            .map_or_else(CurveStep::default, |c| c[cs.index])
    }

    fn rules(&self) -> StepRules {
        StepRules {
            mute: self.mute,
            transpose: self.transpose,
            follow_offset: self.held.follow_offset(self.follow),
            prob_amount: self.prob_amount,
            vel_scale: self.vel_scale,
            gate_scale: self.gate_scale,
            seed_mix: self.seed_mix,
        }
    }

    /// A curve value in percent as the output sample of `row`.
    fn map_curve(&self, row: usize, pct: f64) -> f32 {
        let x = pct / 100.0;
        let x = if self.unipolar[row] {
            (x + 1.0) / 2.0
        } else {
            x
        };
        (x * self.amount[row]) as f32
    }

    /// Writes the curves of the frames from the last written one up to `upto` (exclusive) of
    /// `seg`, from the tick of each frame.
    fn fill_curves(&mut self, ctx: &mut ProcessContext<'_>, seg: &Segment, upto: usize) {
        let upto = upto.min(seg.b);
        let mut f = self.curve_pos.max(seg.a);
        if f >= upto {
            return;
        }
        let tick_of = |g: usize| seg.t_a + (g - seg.a) as f64 * seg.tpf;
        let mut k = self.chain.last_at_or_before(tick_of(f), seg.tpf);
        while f < upto {
            let (s0, s1) = (self.chain.start(k), self.chain.start(k + 1));
            let fe = seg.frame_of(s1).clamp(f + 1, upto);
            for row in 0..CURVE_ROWS {
                let cur = self.curve_step(k, row);
                let v0 = f64::from(cur.value);
                let v1 = if cur.slide {
                    f64::from(self.curve_step(k + 1, row).value)
                } else {
                    v0
                };
                let mut pct = v0;
                for g in f..fe {
                    if cur.slide {
                        let frac = ((tick_of(g) - s0) / (s1 - s0)).clamp(0.0, 1.0);
                        pct = v0 + (v1 - v0) * frac;
                    }
                    let sample = self.map_curve(row, pct);
                    ctx.output(if row == 0 { CURVE_A_CV } else { CURVE_B_CV })[g] = sample;
                }
                self.curve_last[row] = pct;
            }
            self.have_curve = true;
            f = fe;
            k += 1;
        }
        self.curve_pos = upto;
    }

    /// Holds the last curve values for the rest of the batch (stopped or bypassed).
    fn hold_curves(&mut self, ctx: &mut ProcessContext<'_>) {
        let from = self.curve_pos;
        for row in 0..CURVE_ROWS {
            let v = if self.have_curve {
                self.map_curve(row, self.curve_last[row])
            } else {
                0.0
            };
            ctx.output(if row == 0 { CURVE_A_CV } else { CURVE_B_CV })[from..BATCH].fill(v);
        }
        self.curve_pos = BATCH;
    }

    /// Applies the input events up to `upto` (`inclusive` for the event at that frame).
    fn apply_inputs(&mut self, ctx: &mut ProcessContext<'_>, upto: usize, inclusive: bool) {
        while self.in_cursor < self.inbuf.len() {
            let e = self.inbuf[self.in_cursor];
            let at = usize::from(e.offset);
            if at > upto || (at == upto && !inclusive) {
                break;
            }
            self.in_cursor += 1;
            match e.kind {
                NoteKind::On { key, .. } => self.held.press(key),
                NoteKind::Off { key } => self.held.release(key),
                NoteKind::AllOff => {
                    self.held.clear();
                    self.core.close_all(ctx, at);
                    self.held_tie = None;
                }
                // Channel messages and expressions are ignored.
                _ => {}
            }
        }
    }

    /// Adopts the queued pattern and chain.
    fn adopt_queued(&mut self) {
        self.cur = self.queued;
        self.cur_chain = self.queued_chain;
    }

    /// Brings the chain in line with the playing pattern and chain; `jump` forces a recompute
    /// of the next step.
    fn refresh_chain(&mut self, tick: f64, jump: bool) {
        let c = self.build_chain();
        if jump || c != self.chain {
            self.chain = c;
            self.next_step = c.first_at_or_after(tick, self.tpf);
        }
    }

    /// A jump: close every note, adopt the queued pattern, recompute the next step.
    fn do_jump(&mut self, ctx: &mut ProcessContext<'_>, frame: usize, tick: f64) {
        self.core.close_all(ctx, frame);
        self.held_tie = None;
        self.adopt_queued();
        self.refresh_chain(tick, true);
    }

    /// Runs one segment of the batch, then writes its curves.
    fn run(&mut self, ctx: &mut ProcessContext<'_>, seg: &Segment) {
        run_segment(
            &mut Run {
                s: self,
                ctx,
                seg: *seg,
            },
            seg,
        );
        self.fill_curves(ctx, seg, seg.b);
    }
}

impl Default for CvSequencer {
    fn default() -> Self {
        Self::new()
    }
}

/// The sequencer bound to one segment of a batch, driven by [`run_segment`].
struct Run<'a, 'c> {
    s: &'a mut CvSequencer,
    ctx: &'a mut ProcessContext<'c>,
    seg: Segment,
}

impl StepHost for Run<'_, '_> {
    fn grid(&self) -> crate::note_clock::Grid {
        self.s.chain.grid_of(self.s.next_step)
    }

    fn start_of(&self, k: i64) -> f64 {
        self.s.chain.start(k)
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
        // The curves before this step belong to the chain that is playing now.
        s.fill_curves(self.ctx, &self.seg, frame);
        s.apply_inputs(self.ctx, frame, true);
        // A queued pattern or chain is adopted at the start of a pass of the playing chain.
        let queued = s.queued != s.cur || s.queued_chain != s.cur_chain;
        if !s.switch_now && queued && k.rem_euclid(s.chain.steps_per_pass()) == 0 {
            s.adopt_queued();
            s.chain = s.build_chain();
            s.next_step = s.chain.first_at_or_after(tick, s.tpf);
            return;
        }
        let (step, cs) = s.step_at(k);
        s.last_pattern = cs.pattern as u32;
        s.last_step_idx = cs.index as u32;
        let rules = s.rules();
        let at = StepAt {
            k,
            grid: s.chain.grid_of(k),
            tick,
            next_start: s.chain.start(k + 1),
            frame,
        };
        play_step(&mut s.core, self.ctx, &mut s.held_tie, &rules, step, at);
    }
}

impl Device for CvSequencer {
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
        if self.bypass.reset() {
            self.core.flush_alloff = true;
        }
        self.clock.invalidate();
        self.held_tie = None;
        self.held.clear();
        self.next_step = 0;
        self.adopt_queued();
        self.chain = self.build_chain();
        self.close_pending = false;
        self.running = false;
        self.curve_pos = 0;
        self.curve_last = [0.0; CURVE_ROWS];
        self.have_curve = false;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_PATTERN => self.queued = (plain.round() as usize).clamp(1, 8) - 1,
            P_SWITCH => self.switch_now = plain < 0.5,
            P_CHAIN => self.queued_chain = (plain.round() as usize).clamp(1, 8),
            P_TRANSPOSE => self.transpose = (plain.round() as i32).clamp(-24, 24),
            P_FOLLOW => self.follow = plain >= 0.5,
            P_PROB => self.prob_amount = f64::from(plain.clamp(0.0, 1.0)),
            P_SEED => {
                self.seed_mix = seed_mix(self.seed, plain.round().clamp(0.0, 999.0) as u16);
            }
            P_VELOCITY => self.vel_scale = f64::from(plain.clamp(0.0, 2.0)),
            P_GATE => self.gate_scale = f64::from(plain.clamp(0.25, 2.0)),
            P_POLARITY_A => self.unipolar[0] = plain >= 0.5,
            P_POLARITY_B => self.unipolar[1] = plain >= 0.5,
            P_AMOUNT_A => self.amount[0] = f64::from(plain.clamp(0.0, 1.0)),
            P_AMOUNT_B => self.amount[1] = f64::from(plain.clamp(0.0, 1.0)),
            P_MUTE => {
                let m = plain >= 0.5;
                if m && !self.mute {
                    self.close_pending = true;
                }
                self.mute = m;
            }
            P_BYPASS => self.bypass.set_param(plain),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let time = ctx.time;
        self.core.begin_batch(ctx);
        self.curve_pos = 0;
        if self.bypass.begin_batch(ctx.notes_out(NOTES_OUT)) {
            // The switch closes what the sequencer sent and starts it from nothing.
            self.core.close_all(ctx, 0);
            self.clock.invalidate();
            self.held_tie = None;
            self.held.clear();
            self.running = false;
            self.close_pending = false;
            self.adopt_queued();
        }
        self.inbuf.clear();
        self.in_cursor = 0;
        for i in 0..ctx.notes_in(NOTES_IN).len() {
            let e = ctx.notes_in(NOTES_IN)[i];
            if let Some(e) = self.bypass.route(ctx.notes_out(NOTES_OUT), e) {
                self.inbuf.push(e);
            }
        }
        if self.bypass.is_active() {
            self.running = false;
            self.core.end_batch(ctx);
            self.hold_curves(ctx);
            return;
        }
        self.tpf = time.ticks_per_frame;
        let valid = time.playing && self.tpf > 0.0 && self.tpf.is_finite();
        if !valid {
            self.core.close_all(ctx, 0);
            self.clock.invalidate();
            self.held_tie = None;
            self.running = false;
            self.close_pending = false;
            self.adopt_queued();
            self.apply_inputs(ctx, BATCH, true);
            self.core.end_batch(ctx);
            self.hold_curves(ctx);
            return;
        }
        self.running = true;
        let jump = self.clock.observe(time.tick, self.tpf, 0);
        if self.close_pending {
            self.close_pending = false;
            self.core.close_all(ctx, 0);
            self.held_tie = None;
        }
        if jump {
            self.do_jump(ctx, 0, time.tick);
        } else {
            if self.switch_now {
                self.adopt_queued();
            }
            self.refresh_chain(time.tick, false);
        }
        let tpf = self.tpf;
        let end = match time.loop_wrap {
            Some(w) if (1..BATCH).contains(&usize::from(w.offset)) => {
                let at = usize::from(w.offset);
                let seg = Segment {
                    a: 0,
                    b: at,
                    t_a: time.tick,
                    tpf,
                };
                self.run(ctx, &seg);
                self.apply_inputs(ctx, at, false);
                self.do_jump(ctx, at, w.to_tick);
                let seg = Segment {
                    a: at,
                    b: BATCH,
                    t_a: w.to_tick,
                    tpf,
                };
                self.run(ctx, &seg);
                seg.end_tick()
            }
            _ => {
                let seg = Segment {
                    a: 0,
                    b: BATCH,
                    t_a: time.tick,
                    tpf,
                };
                self.run(ctx, &seg);
                seg.end_tick()
            }
        };
        self.apply_inputs(ctx, BATCH, true);
        self.clock.advance(end, tpf, 0);
        self.core.end_batch(ctx);
    }

    fn swap_player_data(&mut self, data: Option<PlayerDataRef>) -> Option<PlayerDataRef> {
        std::mem::replace(&mut self.data, data)
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let v = [
            self.last_pattern as f32,
            self.last_step_idx as f32,
            if self.running { 1.0 } else { 0.0 },
        ];
        let n = v.len().min(out.len());
        out[..n].copy_from_slice(&v[..n]);
        n
    }
}

#[cfg(test)]
mod tests;
