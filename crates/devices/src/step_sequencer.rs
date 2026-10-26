//! `atomik.step-sequencer`: a transport-synced monophonic note pattern player.
//!
//! The device plays the [`atomik_device_sdk::StepSeqData`] installed with
//! [`Device::swap_player_data`]: up to 8 patterns of 1 to 64 steps at 1/4 .. 1/32T with their own
//! swing. It takes the musical position from `TimeInfo` (`tick`, `ticks_per_frame`, `loop_wrap`)
//! and derives the step number from the tick every batch, so a locate lands on the right step and
//! a loop whose length is a multiple of the pattern repeats it in phase. Without a song the
//! transport "always plays" (the engine's song-less tick is a free-running accumulator). Timing
//! rules and the shared clock are in `crate::note_clock`.
//!
//! * **Step index** is the global step number modulo the pattern length. A pattern switch with
//!   `pattern.switch = at-end` is adopted when the global step is a multiple of the playing
//!   pattern's length (and at once after a jump or while stopped); `now` adopts at the next batch.
//!   A pattern with another resolution or swing re-grids the clock at the switch (the next step is
//!   the first one of the new grid at or after the current position).
//! * **Notes** are monophonic. A step that plays emits On at its start and an Off at
//!   `start + max(1, step * gate%)` (clamped to the next step, 100 % ends exactly at it).
//!   A **tie** holds the note into the next step; when the next step plays the same key the
//!   note just continues (no On, no Off, the trig still fires), a different key emits the new On
//!   before the old Off (legato), and a next step that does not play ends the note at that step.
//! * **Probability** uses a stateless hash of the seed, the global step number and slot 0
//!   (`atomik_dsp::rng::hash64`), so live, bounce, locate and loop passes agree.
//! * **Note safety.** Notes are closed on stop, jump (locate, loop wrap, start), `mute`, `AllOff`
//!   on `notes_in`, and a `reset` leaves an `AllOff` for the next batch. A data swap never touches
//!   a sounding note: its Off is already scheduled in the pending table.
//! * **Bypass** (`bypass`, last param): the sequencer closes its notes and the pattern stops, then
//!   `notes_in` goes straight to `notes_out` (see `crate::note_fx::PlayerBypass`). A key held at a
//!   switch is not retriggered: it is ignored until its own Off.
//! * **Outputs.** `gate_cv_out`, `pitch_cv_out` ((key - 60) / 24, +-1), `velocity_cv_out` and a
//!   1 ms `trig_cv_out` pulse at every played step (see `crate::note_clock`).
//! * **Meter values** (`has_meter_values`): the playing pattern (0-based), the step index of the
//!   last started step and 1 while the transport runs.

use atomik_core::{
    BATCH, MAX_EVENTS_PER_BATCH, NoteEvent, NoteKind, ParamDescriptor, ParamId, Taper, Unit,
};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PatternData, PlayerDataRef, PrepareInfo,
    ProcessContext, StepData,
};

use crate::bypass::bypass_param;
use crate::note_clock::{
    Grid, HeldKeys, JumpTracker, NOTES_IN, NOTES_OUT, NoteCore, PLAYER_SOCKETS, Segment, StepAt,
    StepHost, StepRules, play_step, run_segment, seed_mix,
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

const P_PATTERN: u32 = 0;
const P_SWITCH: u32 = 1;
const P_TRANSPOSE: u32 = 2;
const P_FOLLOW: u32 = 3;
const P_PROB: u32 = 4;
const P_SEED: u32 = 5;
const P_MUTE: u32 = 6;
const P_VELOCITY: u32 = 7;
const P_GATE: u32 = 8;
const P_BYPASS: u32 = 9;

static PARAMS: [ParamDescriptor; 10] = [
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
    param(P_MUTE, "mute", "Mute", (0.0, 1.0, 0.0), Unit::Toggle, true),
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
    bypass_param(P_BYPASS),
];

/// Static description of the step sequencer.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.step-sequencer",
    name: "Step sequencer",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &PLAYER_SOCKETS,
};

/// Factory registering [`StepSequencer`] (register it with `register_with_player_data`).
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(StepSequencer::new()),
    }
}

/// The step sequencer device.
pub struct StepSequencer {
    data: Option<PlayerDataRef>,
    core: NoteCore,
    clock: JumpTracker,
    bypass: PlayerBypass,
    seed: u64,
    seed_mix: u64,
    // Params.
    queued: usize,
    cur: usize,
    switch_now: bool,
    transpose: i32,
    follow: bool,
    prob_amount: f64,
    mute: bool,
    vel_scale: f64,
    gate_scale: f64,
    close_pending: bool,
    // Clock state.
    grid: Grid,
    next_step: i64,
    tpf: f64,
    held_tie: Option<u8>,
    // Inputs.
    inbuf: Vec<NoteEvent>,
    in_cursor: usize,
    held: HeldKeys,
    // Display.
    last_step_idx: u32,
    running: bool,
}

impl StepSequencer {
    /// A new sequencer without pattern data.
    pub fn new() -> Self {
        Self {
            data: None,
            core: NoteCore::new(),
            clock: JumpTracker::new(),
            bypass: PlayerBypass::new(),
            seed: 0,
            seed_mix: 0,
            queued: 0,
            cur: 0,
            switch_now: false,
            transpose: 0,
            follow: false,
            prob_amount: 1.0,
            mute: false,
            vel_scale: 1.0,
            gate_scale: 1.0,
            close_pending: false,
            grid: Grid::DEFAULT,
            next_step: 0,
            tpf: 1.0,
            held_tie: None,
            inbuf: Vec::with_capacity(MAX_EVENTS_PER_BATCH),
            in_cursor: 0,
            held: HeldKeys::new(),
            last_step_idx: 0,
            running: false,
        }
    }

    fn pattern(&self, index: usize) -> Option<&PatternData> {
        let d = self.data.as_deref()?.notes();
        let last = usize::from(d.count).max(1) - 1;
        d.pattern(index.min(last))
    }

    /// The grid of the playing pattern (the current grid when there is no data).
    fn pattern_grid(&self) -> Grid {
        self.pattern(self.cur).map_or(self.grid, |p| Grid {
            s: p.resolution.step_ticks(),
            swing: p.swing,
        })
    }

    fn cur_len(&self) -> u64 {
        self.pattern(self.cur)
            .map_or(1, |p| u64::from(p.length.clamp(1, 64)))
    }

    /// Step `k` of the playing pattern and its index.
    fn step_at(&self, k: i64) -> Option<(StepData, u32)> {
        let p = self.pattern(self.cur)?;
        let idx = (k as u64 % u64::from(p.length.clamp(1, 64))) as usize;
        Some((p.steps[idx], idx as u32))
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

    /// Brings the grid in line with the playing pattern; `jump` forces a recompute of the next
    /// step.
    fn refresh_grid(&mut self, tick: f64, jump: bool) {
        let g = self.pattern_grid();
        if jump || g != self.grid {
            self.grid = g;
            self.next_step = g.first_step_at_or_after(tick, self.tpf);
        }
    }

    /// A jump: close every note, adopt the queued pattern, recompute the next step.
    fn do_jump(&mut self, ctx: &mut ProcessContext<'_>, frame: usize, tick: f64) {
        self.core.close_all(ctx, frame);
        self.held_tie = None;
        self.cur = self.queued;
        self.refresh_grid(tick, true);
    }
}

impl Default for StepSequencer {
    fn default() -> Self {
        Self::new()
    }
}

/// The sequencer bound to one batch's context, driven by [`run_segment`].
struct Run<'a, 'c> {
    s: &'a mut StepSequencer,
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
        // A queued pattern is adopted at the start of a pass of the playing one.
        if !s.switch_now && s.queued != s.cur && (k as u64).is_multiple_of(s.cur_len()) {
            s.cur = s.queued;
            let g = s.pattern_grid();
            if g != s.grid {
                s.grid = g;
                s.next_step = g.first_step_at_or_after(tick, s.tpf);
                return;
            }
        }
        let step = s.step_at(k).map(|(sd, idx)| {
            s.last_step_idx = idx;
            sd
        });
        let rules = s.rules();
        let at = StepAt {
            k,
            grid: s.grid,
            tick,
            next_start: s.grid.start(k + 1),
            frame,
        };
        play_step(&mut s.core, self.ctx, &mut s.held_tie, &rules, step, at);
    }
}

impl Device for StepSequencer {
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
        self.cur = self.queued;
        self.close_pending = false;
        self.running = false;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_PATTERN => self.queued = (plain.round() as usize).clamp(1, 8) - 1,
            P_SWITCH => self.switch_now = plain < 0.5,
            P_TRANSPOSE => self.transpose = (plain.round() as i32).clamp(-24, 24),
            P_FOLLOW => self.follow = plain >= 0.5,
            P_PROB => self.prob_amount = f64::from(plain.clamp(0.0, 1.0)),
            P_SEED => {
                self.seed_mix = seed_mix(self.seed, plain.round().clamp(0.0, 999.0) as u16);
            }
            P_MUTE => {
                let m = plain >= 0.5;
                if m && !self.mute {
                    self.close_pending = true;
                }
                self.mute = m;
            }
            P_VELOCITY => self.vel_scale = f64::from(plain.clamp(0.0, 2.0)),
            P_GATE => self.gate_scale = f64::from(plain.clamp(0.25, 2.0)),
            P_BYPASS => self.bypass.set_param(plain),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let time = ctx.time;
        self.core.begin_batch(ctx);
        if self.bypass.begin_batch(ctx.notes_out(NOTES_OUT)) {
            // The switch closes what the sequencer sent and starts it from nothing.
            self.core.close_all(ctx, 0);
            self.clock.invalidate();
            self.held_tie = None;
            self.held.clear();
            self.running = false;
            self.close_pending = false;
            self.cur = self.queued;
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
            self.cur = self.queued;
            self.apply_inputs(ctx, BATCH, true);
            self.core.end_batch(ctx);
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
                self.cur = self.queued;
            }
            self.refresh_grid(time.tick, false);
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
                run_segment(
                    &mut Run {
                        s: &mut *self,
                        ctx: &mut *ctx,
                    },
                    &seg,
                );
                self.apply_inputs(ctx, at, false);
                self.do_jump(ctx, at, w.to_tick);
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
                    t_a: time.tick,
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
            self.cur as f32,
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
