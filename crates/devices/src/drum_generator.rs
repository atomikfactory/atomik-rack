//! `atomik.drum-generator`: a transport-synced drum pattern player that varies what is written.
//!
//! The device plays the [`atomik_device_sdk::DrumData`] installed with
//! [`Device::swap_player_data`]: up to 8 patterns of 16 lanes (one key each, by default the
//! pads of the drum machine) and 1 to 64 steps. The clock is the step sequencer's: the step
//! number comes from the transport tick every batch, so a locate lands on the right step and a
//! loop repeats in phase; a pattern switch with `pattern.switch = at-end` waits for the end of
//! the playing pass. What a lane plays at a step is a pure function of the step, the pattern, the
//! params and the time signature (see [`algo`]), so a bounce is bit-identical at any block size.
//!
//! * **Params.** `density` adds hits on empty steps or removes hits, weighted by what the lane
//!   is (kick on the beats, snare on the backbeats, hats on the eighths); `variation` moves
//!   hits a step and adds ghost notes in some bars; `fill.every` and `fill.amount` turn the end of
//!   every n-th bar into a fill; `accent` boosts the strong positions of each role; `gate` is the
//!   hit length in percent of a step; `random.seed` draws every chance; `mute` closes everything.
//!   `style` is the starting rhythm the app's Generate button writes (the device ignores it).
//! * **Bars** are counted from tick 0 with the time signature in effect at the start of the
//!   batch (a meter change mid-song shifts the bar count, documented).
//! * **Note safety.** Notes are closed on stop, jump, `mute`, `AllOff` on `notes_in` and bypass,
//!   a `reset` leaves an `AllOff` for the next batch. A data swap never touches a sounding hit: its
//!   Off is already scheduled.
//! * **Bypass** (`bypass`, last param): the pattern stops and `notes_in` goes straight to
//!   `notes_out` (see `crate::note_fx::PlayerBypass`).
//! * **Outputs.** `gate_cv_out` is high while a hit sounds, `trig_cv_out` pulses at every step
//!   that hits, `pitch_cv_out` and `velocity_cv_out` follow the loudest hit of that step.
//! * **Meter values** (`has_meter_values`): the playing pattern (0-based), the step index of the
//!   last started step and 1 while the transport runs.

mod algo;
mod generate;

pub use generate::{DrumStyle, generate_pattern};

use atomik_core::{
    BATCH, MAX_EVENTS_PER_BATCH, NoteEvent, NoteKind, ParamDescriptor, ParamId, Taper, Unit,
};
use atomik_device_sdk::{
    DRUM_LANES, Device, DeviceDescriptor, DeviceFactory, DeviceKind, DrumPattern, PlayerDataRef,
    PrepareInfo, ProcessContext,
};

use crate::bypass::bypass_param;
use crate::note_clock::{
    Grid, JumpTracker, NOTES_IN, NOTES_OUT, NoteCore, PLAYER_SOCKETS, Segment, StepHost,
    run_segment, seed_mix,
};
use crate::note_fx::PlayerBypass;
use algo::{Meter, Tuning, hit_velocity};

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
const FILL_EVERY: Unit = Unit::Choice(&["off", "1 bar", "2 bars", "4 bars", "8 bars"]);
const STYLE: Unit = Unit::Choice(&["four on the floor", "broken beat", "half time", "straight"]);

const P_PATTERN: u32 = 0;
const P_SWITCH: u32 = 1;
const P_DENSITY: u32 = 2;
const P_VARIATION: u32 = 3;
const P_FILL_EVERY: u32 = 4;
const P_FILL_AMOUNT: u32 = 5;
const P_VELOCITY: u32 = 6;
const P_ACCENT: u32 = 7;
const P_GATE: u32 = 8;
const P_SEED: u32 = 9;
const P_STYLE: u32 = 10;
const P_MUTE: u32 = 11;
const P_BYPASS: u32 = 12;

/// Bars per fill group for each `fill.every` choice.
const FILL_BARS: [u32; 5] = [0, 1, 2, 4, 8];

static PARAMS: [ParamDescriptor; 13] = [
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
        P_DENSITY,
        "density",
        "Density",
        (-1.0, 1.0, 0.0),
        Unit::Percent,
        false,
    ),
    param(
        P_VARIATION,
        "variation",
        "Variation",
        (0.0, 1.0, 0.0),
        Unit::Percent,
        false,
    ),
    param(
        P_FILL_EVERY,
        "fill.every",
        "Fill every",
        (0.0, 4.0, 0.0),
        FILL_EVERY,
        true,
    ),
    param(
        P_FILL_AMOUNT,
        "fill.amount",
        "Fill amount",
        (0.0, 1.0, 0.5),
        Unit::Percent,
        false,
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
        P_ACCENT,
        "accent",
        "Accent",
        (0.0, 1.0, 0.0),
        Unit::Percent,
        false,
    ),
    param(
        P_GATE,
        "gate",
        "Gate",
        (0.05, 1.0, 0.25),
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
    param(P_STYLE, "style", "Style", (0.0, 3.0, 0.0), STYLE, true),
    param(P_MUTE, "mute", "Mute", (0.0, 1.0, 0.0), Unit::Toggle, true),
    bypass_param(P_BYPASS),
];

/// Static description of the drum generator.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.drum-generator",
    name: "Drum generator",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &PLAYER_SOCKETS,
};

/// Factory registering [`DrumGenerator`] (register it with `register_with_drums`).
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(DrumGenerator::new()),
    }
}

/// The drum generator device.
pub struct DrumGenerator {
    data: Option<PlayerDataRef>,
    core: NoteCore,
    clock: JumpTracker,
    bypass: PlayerBypass,
    seed: u64,
    tuning: Tuning,
    meter: Meter,
    // Params.
    queued: usize,
    cur: usize,
    switch_now: bool,
    gate: f64,
    mute: bool,
    close_pending: bool,
    // Clock state.
    grid: Grid,
    next_step: i64,
    tpf: f64,
    // Inputs.
    inbuf: Vec<NoteEvent>,
    in_cursor: usize,
    // Display.
    last_step_idx: u32,
    running: bool,
}

impl DrumGenerator {
    /// A new generator without pattern data.
    pub fn new() -> Self {
        Self {
            data: None,
            core: NoteCore::new(),
            clock: JumpTracker::new(),
            bypass: PlayerBypass::new(),
            seed: 0,
            tuning: Tuning {
                density: 0.0,
                variation: 0.0,
                fill_every: 0,
                fill_amount: 0.5,
                accent: 0.0,
                vel_scale: 1.0,
                seed_mix: 0,
            },
            meter: Meter::new(4, 4),
            queued: 0,
            cur: 0,
            switch_now: false,
            gate: 0.25,
            mute: false,
            close_pending: false,
            grid: Grid::DEFAULT,
            next_step: 0,
            tpf: 1.0,
            inbuf: Vec::with_capacity(MAX_EVENTS_PER_BATCH),
            in_cursor: 0,
            last_step_idx: 0,
            running: false,
        }
    }

    fn pattern(&self, index: usize) -> Option<&DrumPattern> {
        let d = self.data.as_deref()?.drums()?;
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

    /// Applies the input events up to `upto` (`inclusive` for the event at that frame). Only
    /// `AllOff` matters: it closes what the generator sounds.
    fn apply_inputs(&mut self, ctx: &mut ProcessContext<'_>, upto: usize, inclusive: bool) {
        while self.in_cursor < self.inbuf.len() {
            let e = self.inbuf[self.in_cursor];
            let at = usize::from(e.offset);
            if at > upto || (at == upto && !inclusive) {
                break;
            }
            self.in_cursor += 1;
            if e.kind == NoteKind::AllOff {
                self.core.close_all(ctx, at);
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
        self.cur = self.queued;
        self.refresh_grid(tick, true);
    }

    /// Plays every lane that hits at step `k`.
    fn play_lanes(&mut self, ctx: &mut ProcessContext<'_>, k: i64, tick: f64, frame: usize) {
        let Some(p) = self.pattern(self.cur) else {
            return;
        };
        let p = *p;
        self.last_step_idx = (k as u64 % u64::from(p.length.clamp(1, 64))) as u32;
        if self.mute {
            return;
        }
        let next = self.grid.start(k + 1);
        let end = self.grid.gate_end_to(tick, next, self.gate * 100.0);
        let mut loudest: Option<(u8, u8)> = None;
        for lane in 0..DRUM_LANES {
            let l = p.lanes[lane];
            if l.mute {
                continue;
            }
            let v = hit_velocity(&p, lane, k, self.grid, &self.tuning, &self.meter);
            if v == 0 {
                continue;
            }
            self.core
                .note_on(ctx, frame, l.key.min(127), f32::from(v) / 127.0, end);
            if loudest.is_none_or(|(_, lv)| v > lv) {
                loudest = Some((l.key.min(127), v));
            }
        }
        if let Some((key, v)) = loudest {
            self.core.trigger(ctx, frame, key, f32::from(v) / 127.0);
        }
    }
}

impl Default for DrumGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// The generator bound to one batch's context, driven by [`run_segment`].
struct Run<'a, 'c> {
    s: &'a mut DrumGenerator,
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
        s.play_lanes(self.ctx, k, tick, frame);
    }
}

impl Device for DrumGenerator {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.seed = info.seed;
        self.tuning.seed_mix = seed_mix(info.seed, 0);
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
        self.next_step = 0;
        self.cur = self.queued;
        self.close_pending = false;
        self.running = false;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_PATTERN => self.queued = (plain.round() as usize).clamp(1, 8) - 1,
            P_SWITCH => self.switch_now = plain < 0.5,
            P_DENSITY => self.tuning.density = f64::from(plain.clamp(-1.0, 1.0)),
            P_VARIATION => self.tuning.variation = f64::from(plain.clamp(0.0, 1.0)),
            P_FILL_EVERY => {
                self.tuning.fill_every =
                    FILL_BARS[(plain.round() as usize).min(FILL_BARS.len() - 1)];
            }
            P_FILL_AMOUNT => self.tuning.fill_amount = f64::from(plain.clamp(0.0, 1.0)),
            P_VELOCITY => self.tuning.vel_scale = f64::from(plain.clamp(0.0, 2.0)),
            P_ACCENT => self.tuning.accent = f64::from(plain.clamp(0.0, 1.0)),
            P_GATE => self.gate = f64::from(plain.clamp(0.05, 1.0)),
            P_SEED => {
                self.tuning.seed_mix = seed_mix(self.seed, plain.round().clamp(0.0, 999.0) as u16);
            }
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
        if self.bypass.begin_batch(ctx.notes_out(NOTES_OUT)) {
            // The switch closes what the generator sent and starts it from nothing.
            self.core.close_all(ctx, 0);
            self.clock.invalidate();
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
        self.meter = Meter::new(time.time_signature.0, time.time_signature.1);
        let valid = time.playing && self.tpf > 0.0 && self.tpf.is_finite();
        if !valid {
            self.core.close_all(ctx, 0);
            self.clock.invalidate();
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
