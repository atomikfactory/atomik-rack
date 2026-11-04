//! `atomik.auto-filter`: a state-variable filter whose cutoff is swept by an LFO and by the level
//! of the input.
//!
//! # Signal flow
//!
//! ```text
//! lfo (shape, rate or note value, per-channel phase spread)      -1..+1
//! follower of the input (5 ms attack, 100 ms release) -> 0..1
//! octaves = depth * 4 * lfo + follow * 4 * level + cutoff_cv * 4
//! cutoff  = cutoff_knob * 2^octaves
//! in -> SVF (mode, resonance) [-> a second SVF with the same settings at 24 dB] -> mix with dry
//! ```
//!
//! * **Filter.** The 12 dB/octave zero-delay filter of [`atomik_dsp::Svf`] (`resonance` 0..1 maps
//!   to Q 0.5..20); `slope` 24 dB runs a second one behind it. `notch` is the sum of the low-pass
//!   and high-pass outputs. The cutoff is clamped to 20 Hz..0.49 of the sample rate and refreshed
//!   every 8 samples. A sweep that follows an LFO is the whole point, so the coefficient steps
//!   are smoothed only by that rate.
//! * **LFO.** Sine, triangle, rising saw, square or sample and hold (a new value from a seeded
//!   generator at each cycle). `rate` is in Hz, or with `sync` on a note value at the transport
//!   tempo; while the transport plays, a synced LFO takes its phase from the song position, so a
//!   locate and a bounce give the same sweep, and the phase is free-running while stopped.
//!   `spread` offsets the right channel's phase by up to half a cycle (0 keeps both channels the
//!   same) for a stereo sweep.
//! * **Follower.** `follow` (-1..+1) moves the cutoff up (or down) with the input level: the peak
//!   of both channels in dB, 0 at -50 dBFS and below, 1 at 0 dBFS and above, times 4 octaves.
//! * **CV.** `cutoff_cv_in` adds `trim * cv * 4` octaves, `depth_cv_in` adds `trim * cv` to the
//!   depth, `rate_cv_in` multiplies the free rate by `2^(trim * cv * 3)` (all as the batch mean).
//!   `lfo_cv_out` is the left LFO, -1..+1, one value per sample.
//! * **Level.** The result is soft-limited above +-4 (12 dB over full scale; a resonant sweep can
//!   exceed it), bit for bit unchanged below that.
//! * No latency. Deterministic maths (`exp2_det`, `sin_cycles`, `log2_det`, the `tan` of the
//!   filter). Non-finite input samples count as 0; allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{batch_mean, clean};
use crate::tool_support::{
    audio_in, audio_out, choice, cv_in, cv_out, expo, finite, linear, toggle, trim,
};
use atomik_core::{BATCH, PPQ, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{Rng, Svf, exp2_det, exp2_det_f64, log2_det, sin_cycles};

const P_MODE: u32 = 0;
const P_SLOPE: u32 = 1;
const P_CUTOFF: u32 = 2;
const P_RESONANCE: u32 = 3;
const P_DEPTH: u32 = 4;
const P_RATE: u32 = 5;
const P_SYNC: u32 = 6;
const P_DIVISION: u32 = 7;
const P_SHAPE: u32 = 8;
const P_SPREAD: u32 = 9;
const P_FOLLOW: u32 = 10;
const P_MIX: u32 = 11;
const P_CUTOFF_TRIM: u32 = 12;
const P_DEPTH_TRIM: u32 = 13;
const P_RATE_TRIM: u32 = 14;

static MODES: [&str; 4] = ["lowpass", "bandpass", "highpass", "notch"];
static SLOPES: [&str; 2] = ["12 dB", "24 dB"];
static SHAPES: [&str; 5] = ["sine", "triangle", "saw", "square", "sample and hold"];
static DIVISIONS: [&str; 12] = [
    "1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4", "1/4D", "1/2", "1/1",
];
/// Length of each note value in quarter notes (the order of [`DIVISIONS`]).
const DIVISION_BEATS: [f64; 12] = crate::stereo_delay::SYNC_BEATS;

/// Samples between filter coefficient refreshes.
const UPDATE_EVERY: usize = 8;
/// The sweep of an LFO, follower or CV of 1 at full depth, octaves.
const SWEEP_OCTAVES: f32 = 4.0;
/// The rate CV range at full trim, octaves of rate.
const RATE_CV_OCTAVES: f32 = 3.0;
/// The follower's rise and fall times, ms.
const FOLLOW_ATTACK_MS: f32 = 5.0;
const FOLLOW_RELEASE_MS: f32 = 100.0;
/// Input level (dB) that reads 0 and 1 on the follower.
const FOLLOW_FLOOR_DB: f32 = -50.0;
/// `20 * log10(2)`.
const DB_PER_OCTAVE: f32 = 6.020_6;
/// The output is soft-limited above this.
const KNEE: f32 = 4.0;

static PARAMS: [ParamDescriptor; 16] = [
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    choice(P_SLOPE, "slope", "Slope", &SLOPES, 0.0),
    expo(
        P_CUTOFF,
        "cutoff",
        "Cutoff",
        20.0,
        20_000.0,
        800.0,
        Unit::Hz,
    ),
    linear(
        P_RESONANCE,
        "resonance",
        "Resonance",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    linear(P_DEPTH, "depth", "Depth", 0.0, 1.0, 0.5, Unit::Percent),
    expo(P_RATE, "rate", "Rate", 0.02, 20.0, 1.0, Unit::Hz),
    toggle(P_SYNC, "sync", "Sync", 0.0),
    choice(P_DIVISION, "division", "Division", &DIVISIONS, 8.0),
    choice(P_SHAPE, "shape", "Shape", &SHAPES, 0.0),
    linear(P_SPREAD, "spread", "Spread", 0.0, 1.0, 0.0, Unit::Percent),
    linear(P_FOLLOW, "follow", "Follow", -1.0, 1.0, 0.0, Unit::Percent),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    trim(P_CUTOFF_TRIM, "cutoff_cv_trim", "Cutoff CV trim"),
    trim(P_DEPTH_TRIM, "depth_cv_trim", "Depth CV trim"),
    trim(P_RATE_TRIM, "rate_cv_trim", "Rate CV trim"),
    bypass_param(15),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const CUTOFF_CV: SocketId = SocketId(2);
const DEPTH_CV: SocketId = SocketId(3);
const RATE_CV: SocketId = SocketId(4);
const OUT_L: SocketId = SocketId(5);
const OUT_R: SocketId = SocketId(6);
const LFO_CV: SocketId = SocketId(7);

static SOCKETS: [SocketDescriptor; 8] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    cv_in(2, "cutoff_cv_in", "Cutoff CV"),
    cv_in(3, "depth_cv_in", "Depth CV"),
    cv_in(4, "rate_cv_in", "Rate CV"),
    audio_out(5, "out_l", "Out L"),
    audio_out(6, "out_r", "Out R"),
    cv_out(7, "lfo_cv_out", "LFO CV"),
];

/// Static description of the auto-filter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.auto-filter",
    name: "Auto-Filter",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`AutoFilter`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(AutoFilter::new())),
    }
}

/// One LFO value, -1..+1, for `phase` in cycles (`0..1`) of every shape but sample and hold.
fn wave(shape: u8, phase: f64) -> f32 {
    match shape {
        0 => sin_cycles(phase) as f32,
        1 => (1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs()) as f32,
        2 => (2.0 * phase - 1.0) as f32,
        _ => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

/// `x` unchanged up to [`KNEE`], then a smooth approach to `2 * KNEE`.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let t = (a - KNEE) / KNEE;
        (KNEE + KNEE * t / (1.0 + t)).copysign(x)
    }
}

/// The auto-filter device.
#[derive(Debug, Clone)]
pub struct AutoFilter {
    sample_rate: f32,
    seed: u64,
    mode: u8,
    steep: bool,
    cutoff: f32,
    resonance: f32,
    depth: f32,
    rate: f32,
    sync: bool,
    beats: f64,
    shape: u8,
    spread: f32,
    follow: f32,
    mix: f32,
    cutoff_trim: f32,
    depth_trim: f32,
    rate_trim: f32,
    // Derived.
    damping: f32,
    rise: f32,
    fall: f32,
    // State.
    filters: [[Svf; 2]; 2],
    phase: f64,
    held: [f32; 2],
    rng: Rng,
    level: f32,
}

impl Default for AutoFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoFilter {
    /// An auto-filter with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut a = Self {
            sample_rate: 48_000.0,
            seed: 0,
            mode: 0,
            steep: false,
            cutoff: 800.0,
            resonance: 0.35,
            depth: 0.5,
            rate: 1.0,
            sync: false,
            beats: DIVISION_BEATS[8],
            shape: 0,
            spread: 0.0,
            follow: 0.0,
            mix: 1.0,
            cutoff_trim: 1.0,
            depth_trim: 1.0,
            rate_trim: 1.0,
            damping: 1.0,
            rise: 0.0,
            fall: 0.0,
            filters: [[Svf::new(); 2]; 2],
            phase: 0.0,
            held: [0.0; 2],
            rng: Rng::new(0),
            level: 0.0,
        };
        a.derive();
        a.reset();
        a
    }

    fn coefficient(ms: f32, sample_rate: f32) -> f32 {
        let n = f64::from(ms) * 0.001 * f64::from(sample_rate);
        exp2_det_f64(-std::f64::consts::LOG2_E / n) as f32
    }

    fn derive(&mut self) {
        // Per-sample decay factors.
        self.rise = Self::coefficient(FOLLOW_ATTACK_MS, self.sample_rate);
        self.fall = Self::coefficient(FOLLOW_RELEASE_MS, self.sample_rate);
    }

    fn refresh(&mut self, channel: usize, octaves: f32) {
        let hz = self.cutoff * exp2_det(octaves.clamp(-12.0, 12.0));
        let g = Svf::cutoff_to_g_det(hz, self.sample_rate);
        let k = 1.0 / Svf::resonance_to_q(self.resonance);
        self.damping = k;
        for f in &mut self.filters[channel] {
            f.set_coefficients(g, k);
        }
    }

    fn run(&mut self, channel: usize, x: f32) -> f32 {
        let k = self.damping;
        let one = |f: &mut Svf, v: f32, mode: u8| {
            let o = f.process(v);
            match mode {
                0 => o.lp,
                // Unity gain at the centre.
                1 => o.bp * k,
                2 => o.hp,
                _ => o.lp + o.hp,
            }
        };
        let mut y = one(&mut self.filters[channel][0], x, self.mode);
        if self.steep {
            y = one(&mut self.filters[channel][1], y, self.mode);
        }
        y
    }
}

impl Device for AutoFilter {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        for pair in &mut self.filters {
            for f in pair {
                f.reset();
            }
        }
        self.phase = 0.0;
        self.rng = Rng::new(self.seed);
        self.held = [self.rng.next_bipolar(), self.rng.next_bipolar()];
        self.level = 0.0;
        self.refresh(0, 0.0);
        self.refresh(1, 0.0);
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 3.0).round() as u8,
            P_SLOPE => self.steep = v >= 0.5,
            P_CUTOFF => self.cutoff = v.clamp(20.0, 20_000.0),
            P_RESONANCE => self.resonance = v.clamp(0.0, 1.0),
            P_DEPTH => self.depth = v.clamp(0.0, 1.0),
            P_RATE => self.rate = v.clamp(0.02, 20.0),
            P_SYNC => self.sync = v >= 0.5,
            P_DIVISION => {
                let i = (v.clamp(0.0, 11.0).round() as usize).min(DIVISION_BEATS.len() - 1);
                self.beats = DIVISION_BEATS[i];
            }
            P_SHAPE => self.shape = v.clamp(0.0, 4.0).round() as u8,
            P_SPREAD => self.spread = v.clamp(0.0, 1.0),
            P_FOLLOW => self.follow = v.clamp(-1.0, 1.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_CUTOFF_TRIM => self.cutoff_trim = v.clamp(-1.0, 1.0),
            P_DEPTH_TRIM => self.depth_trim = v.clamp(-1.0, 1.0),
            P_RATE_TRIM => self.rate_trim = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let cutoff_cv = clean(batch_mean(ctx.input(CUTOFF_CV)));
        let depth =
            (self.depth + self.depth_trim * clean(batch_mean(ctx.input(DEPTH_CV)))).clamp(0.0, 1.0);
        let rate_cv = clean(batch_mean(ctx.input(RATE_CV)));

        // Phase advance per sample, and the phase at the start of a locked batch.
        let locked = self.sync && ctx.time.playing;
        let inc = if locked {
            ctx.time.ticks_per_frame / (PPQ as f64 * self.beats)
        } else {
            let hz = if self.sync {
                ctx.time.tempo.bpm() / 60.0 / self.beats
            } else {
                f64::from(self.rate * exp2_det(self.rate_trim * rate_cv * RATE_CV_OCTAVES))
            };
            hz / f64::from(self.sample_rate)
        };
        if locked {
            self.phase = (ctx.time.tick / (PPQ as f64 * self.beats)).rem_euclid(1.0);
        }
        let spread = f64::from(self.spread) * 0.5;

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut lfo_out = [0.0f32; BATCH];
        let mut lfo = [0.0f32; 2];
        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            // LFO values, one per channel.
            for (c, v) in lfo.iter_mut().enumerate() {
                let p = (self.phase + if c == 1 { spread } else { 0.0 }).fract();
                *v = if self.shape >= 4 {
                    self.held[c]
                } else {
                    wave(self.shape, p)
                };
            }
            lfo_out[n] = lfo[0];
            // Level follower (peak of both channels).
            let k = x[0].abs().max(x[1].abs());
            self.level = if k > self.level {
                k + (self.level - k) * self.rise
            } else {
                k + (self.level - k) * self.fall
            };
            if self.level < 1e-20 {
                self.level = 0.0;
            }
            if n % UPDATE_EVERY == 0 {
                let norm = if self.follow != 0.0 && self.level > 0.0 {
                    ((log2_det(self.level) * DB_PER_OCTAVE - FOLLOW_FLOOR_DB) / -FOLLOW_FLOOR_DB)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let base = self.follow * SWEEP_OCTAVES * norm
                    + self.cutoff_trim * cutoff_cv * SWEEP_OCTAVES;
                for (c, l) in lfo.into_iter().enumerate() {
                    self.refresh(c, base + depth * SWEEP_OCTAVES * l);
                }
            }
            let mut y = [0.0f32; 2];
            for c in 0..2 {
                let wet = self.run(c, x[c]);
                let mixed = if self.mix >= 1.0 {
                    wet
                } else {
                    x[c] * (1.0 - self.mix) + wet * self.mix
                };
                y[c] = soft_limit(mixed);
            }
            out_l[n] = y[0];
            out_r[n] = y[1];
            // Advance the phase; a wrap draws a new held value for sample and hold.
            self.phase += inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.held = [self.rng.next_bipolar(), self.rng.next_bipolar()];
            }
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(LFO_CV) = lfo_out;
    }
}

#[cfg(test)]
mod tests;
