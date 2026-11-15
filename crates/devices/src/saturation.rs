//! `atomik.saturation`: a one-knob stereo saturator. A smooth, asymmetric tanh curve adds mostly
//! odd harmonics and a little even-order colour; a three-way `type` switch decides which part of
//! the spectrum the curve works on; an auto-level law keeps the loudness where it was.
//!
//! # Signal flow (per channel; the two channels are processed independently)
//!
//! ```text
//! x -> split at 400 Hz: low, high (24 dB/oct Linkwitz-Riley)
//!   in  = n * x + l * low + h * high         (the part that is saturated)
//!   keep = x - in                            (the part that stays clean)
//!   shaped = [oversample -> f(g * in) / g -> filter]
//!   wet = keep + L * (in + DC block(shaped - in))        (all delayed by the latency)
//! out = (dry * (1 - mix) + wet * mix) * output gain      (dry: x delayed by the latency)
//! ```
//!
//! * **Knob.** `saturation` 0..100 % maps to a drive gain `g = 15.85 * (16^k - 1) / 15`: 0 at
//!   the bottom (the curve is a straight line), about +24 dB into the curve at the top. The
//!   curve is `f(u) = (tanh(u + 0.1) - tanh(0.1)) / (1 - tanh(0.1)^2)`, written as
//!   `tanh(u) / (1 + tanh(0.1) tanh(u))` (the same function, without the cancellation near 0),
//!   applied as `f(g x) / g`,
//!   so its slope at 0 is exactly 1 for every drive: a quiet signal passes untouched, a loud one
//!   is rounded off first. The small bias makes the curve asymmetric (second and fourth
//!   harmonics beside the odd ones); the offset it leaves is taken out by an 8 Hz high-pass on
//!   the nonlinear part only, so the linear part of the signal is never filtered.
//! * **Type.** The split is a 24 dB/oct Linkwitz-Riley crossover at 400 Hz, and the saturated
//!   part is `n * x + l * low + h * high` with `(n, l, h)` = `(1, 0, 0)` for *neutral* (the whole
//!   band goes through the curve as one signal, intermodulation included; the crossover is not
//!   in the path), `(0, 0, 1)` for *keep low* (only what is above the split is saturated, the
//!   lows pass clean and the harmonics land above them) and `(0, 1, 0)` for *keep high* (only
//!   the lows and low mids are saturated). The clean part is the exact complement
//!   `x - in`, so with no drive every type sums back to the input whatever the crossover's
//!   phase. The weights are smoothed over 20 ms, so a switch is a crossfade, not a click.
//! * **Auto level.** `auto_level` (on by default) multiplies the shaped part by a static gain
//!   `L(knob)` that depends only on the knob and the type, never on the signal: no pumping. The
//!   table behind it is computed when the device is built: the gain that gives back the RMS
//!   of a sine of a reference level through the curve (neutral: 0.18 RMS, i.e. about -15 dBFS;
//!   keep low and keep high: the share of a typical programme above and below the split). A
//!   sine is the reference because it sits between a single tone and noise: measured over the
//!   whole knob, a sine of that level stays within 0.1 dB and 40 Hz high-passed pink noise within
//!   1 dB. Much quieter signals stay nearly linear under the curve and come out louder at high
//!   knob settings; much louder ones come out quieter. Off, the gain is 1.
//! * **Oversampling.** `oversampling` off, 2x or 4x runs the curve between the halfband stages
//!   of [`atomik_dsp::oversample::Oversampler`]; the reported latency is 0, 29 or 35 samples and
//!   changes with the setting (`variable_latency`). The dry path and the clean part are delayed
//!   by the same amount.
//! * **Transparent when off.** With `saturation` 0 (after the CV and the smoothing) the output
//!   is the dry input (times `output`): bit for bit when `oversampling` is off and `output` is
//!   0 dB, and the input delayed by the latency, bit for bit, when oversampling is on.
//! * **CV.** `saturation_cv_in` adds `saturation_cv_trim * cv` (the batch mean, clamped to
//!   -1..+1) to the knob in 0..1 and clamps the sum to 0..1. The trim is 0 by default.
//! * **Meters** (`meter_values`): `drive` 0..1 (how hard the curve is working: one minus the
//!   ratio of the RMS after and before the curve, doubled and clamped, with a 150 ms release),
//!   the peak of the input and the peak of the output (linear).
//! * Knob, mix, output, type weights and gain law are smoothed per batch (10 ms; 20 ms for the
//!   type). Deterministic maths; non-finite input samples count as 0; allocation-free after
//!   construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{batch_mean, clean};
use crate::tool_support::{audio_in, audio_out, choice, cv_in, finite, linear, toggle, trim};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::clipper::DcBlock;
use atomik_dsp::oversample::{OversampleFactor, Oversampler};
use atomik_dsp::{BlockSmoother, Crossover, DelayLine, exp2_det, exp2_det_f64, sin_cycles};
use std::sync::OnceLock;

const P_SATURATION: u32 = 0;
const P_TYPE: u32 = 1;
const P_AUTO_LEVEL: u32 = 2;
const P_MIX: u32 = 3;
const P_OVERSAMPLING: u32 = 4;
const P_OUTPUT: u32 = 5;
const P_CV_TRIM: u32 = 6;

/// The `type` choices (index 0 = keep high, 1 = neutral, 2 = keep low).
static TYPES: [&str; 3] = ["Keep high", "Neutral", "Keep low"];
/// The `oversampling` choices.
static OVERSAMPLINGS: [&str; 3] = ["Off", "2x", "4x"];

/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The meter values: drive lamp, input peak, output peak.
pub const METERS: usize = 3;
/// Indices into the meter values.
pub mod meter {
    /// How hard the curve is working, 0..1.
    pub const DRIVE: usize = 0;
    /// Peak of the input (linear).
    pub const INPUT: usize = 1;
    /// Peak of the output (linear).
    pub const OUTPUT: usize = 2;
}

/// The drive gain at the top of the knob (+24 dB).
const G_MAX: f32 = 15.848_932;
/// The curve's bias (asymmetry).
/// `tanh(0.1)`: the curve's bias (asymmetry).
const TANH_BIAS: f32 = 0.099_667_995;
/// Below this drive gain the curve is a straight line (the error is under 1e-3 of the signal).
const G_LINEAR: f32 = 1.0e-3;
/// The split between the lows and the rest, Hz.
const SPLIT_HZ: f32 = 400.0;
/// Points of the auto-level table (0..=TABLE knob positions).
const TABLE: usize = 64;
/// Integration points per cycle of the table's reference sine.
const TABLE_POINTS: usize = 720;
/// The reference RMS per type (keep high, neutral, keep low) of the signal that reaches the curve.
const REFERENCE_RMS: [f64; 3] = [0.12, 0.18, 0.135];

static PARAMS: [ParamDescriptor; 8] = [
    linear(
        P_SATURATION,
        "saturation",
        "Saturation",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    choice(P_TYPE, "type", "Type", &TYPES, 1.0),
    toggle(P_AUTO_LEVEL, "auto_level", "Auto level", 1.0),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    choice(
        P_OVERSAMPLING,
        "oversampling",
        "Oversampling",
        &OVERSAMPLINGS,
        1.0,
    ),
    linear(P_OUTPUT, "output", "Output", -24.0, 12.0, 0.0, Unit::Db),
    {
        let mut t = trim(P_CV_TRIM, "saturation_cv_trim", "Saturation CV trim");
        t.default = 0.0;
        t
    },
    bypass_param(7),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const SAT_CV: SocketId = SocketId(4);

static SOCKETS: [SocketDescriptor; 5] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_out(2, "out_l", "Out L"),
    audio_out(3, "out_r", "Out R"),
    cv_in(4, "saturation_cv_in", "Saturation CV"),
];

/// Static description of the saturation device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.saturation",
    name: "Saturation",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Saturation`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(Saturation::new())),
    }
}

/// Linear gain of `db` (exactly 1 at 0 dB).
#[inline]
fn lin(db: f32) -> f32 {
    if db == 0.0 {
        1.0
    } else {
        exp2_det(db * LOG2_10_20)
    }
}

/// The drive gain of a knob position `k` in 0..1 (0 at 0, [`G_MAX`] at 1).
#[inline]
pub fn drive_gain(k: f32) -> f32 {
    if k <= 0.0 {
        0.0
    } else {
        G_MAX * (exp2_det(4.0 * k) - 1.0) / 15.0
    }
}

/// `tanh(v)` from the deterministic exponential.
#[inline]
fn tanh_det(v: f32) -> f32 {
    let v = v.clamp(-15.0, 15.0);
    1.0 - 2.0 / (1.0 + exp2_det(2.0 * std::f32::consts::LOG2_E * v))
}

/// The curve over a unit-slope input: `f(g x) / g` (`x` itself below [`G_LINEAR`]).
#[inline]
pub fn shape(x: f32, g: f32) -> f32 {
    if g < G_LINEAR {
        x
    } else {
        let t = tanh_det(g * x);
        t / ((1.0 + TANH_BIAS * t) * g)
    }
}

/// `sin` of a phase in radians, from the deterministic sine.
fn sin64(th: f64) -> f64 {
    sin_cycles(th / std::f64::consts::TAU)
}

/// `tanh` in `f64` for the table.
fn tanh64(v: f64) -> f64 {
    let v = v.clamp(-15.0, 15.0);
    1.0 - 2.0 / (1.0 + exp2_det_f64(2.0 * std::f64::consts::LOG2_E * v))
}

/// The static auto-level gains, `[type][knob position]`.
fn level_table() -> &'static [[f32; TABLE + 1]; 3] {
    static T: OnceLock<[[f32; TABLE + 1]; 3]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [[1.0f32; TABLE + 1]; 3];
        for (ty, row) in t.iter_mut().enumerate() {
            let sigma = REFERENCE_RMS[ty];
            for (i, slot) in row.iter_mut().enumerate() {
                let g = f64::from(drive_gain(i as f32 / TABLE as f32));
                if g < f64::from(G_LINEAR) {
                    continue;
                }
                // Moments of f(g x) / g over one cycle of a sine of RMS `sigma`: a signal
                // with fewer peaks than noise, which sits between the two in practice.
                let (mut m1, mut m2) = (0.0f64, 0.0f64);
                let amp = sigma * std::f64::consts::SQRT_2;
                for j in 0..TABLE_POINTS {
                    let th = std::f64::consts::TAU * (j as f64 + 0.5) / TABLE_POINTS as f64;
                    let t = tanh64(g * amp * sin64(th));
                    let y = t / ((1.0 + f64::from(TANH_BIAS) * t) * g);
                    m1 += y;
                    m2 += y * y;
                }
                let n = TABLE_POINTS as f64;
                let (mean, m2) = (m1 / n, m2 / n);
                let var = (m2 - mean * mean).max(1.0e-12);
                *slot = (sigma / var.sqrt()).max(1.0) as f32;
            }
        }
        t
    })
}

/// The auto-level gain of type `ty` at knob position `k` (0..1), interpolated.
#[inline]
fn level_gain(table: &[[f32; TABLE + 1]; 3], ty: usize, k: f32) -> f32 {
    let p = k.clamp(0.0, 1.0) * TABLE as f32;
    let i = (p as usize).min(TABLE - 1);
    let f = p - i as f32;
    table[ty][i] + (table[ty][i + 1] - table[ty][i]) * f
}

/// The oversampling factor of an `oversampling` choice.
fn factor_of(choice: u8) -> OversampleFactor {
    match choice {
        0 => OversampleFactor::X1,
        1 => OversampleFactor::X2,
        _ => OversampleFactor::X4,
    }
}

/// The saturated-part weights `(whole, low, high)` of a type.
fn weights_of(ty: u8) -> [f32; 3] {
    match ty {
        0 => [0.0, 1.0, 0.0],
        2 => [0.0, 0.0, 1.0],
        _ => [1.0, 0.0, 0.0],
    }
}

/// The state of one channel.
#[derive(Debug, Clone)]
struct Lane {
    split: Crossover,
    os: Oversampler,
    dry: DelayLine,
    sat: DelayLine,
    dc: DcBlock,
}

impl Lane {
    fn new(sample_rate: f32, factor: OversampleFactor) -> Self {
        let mut os = Oversampler::new(BATCH);
        os.set_factor(factor);
        let mut split = Crossover::new(sample_rate);
        split.set_freq(SPLIT_HZ.min(0.4 * sample_rate));
        Self {
            split,
            os,
            dry: DelayLine::new(64),
            sat: DelayLine::new(64),
            dc: DcBlock::new(sample_rate),
        }
    }

    fn reset(&mut self) {
        self.split.reset();
        self.os.reset();
        self.dry.clear();
        self.sat.clear();
        self.dc.reset();
    }
}

/// Per-batch values shared by both channels.
struct Batch {
    /// Drive gain per sample.
    g: [f32; BATCH],
    /// Auto-level gain per sample.
    level: [f32; BATCH],
    /// Saturated-part weights (whole, low, high) per sample.
    w: [[f32; BATCH]; 3],
    /// Every drive gain of the batch is 0.
    off: bool,
    latency: usize,
    ratio: usize,
}

/// What one lane gives for one batch.
struct LaneOut {
    wet: [f32; BATCH],
    dry: [f32; BATCH],
    in_peak: f32,
    /// Energy before and after the curve (aligned).
    e_in: f32,
    e_shaped: f32,
}

/// The saturation device.
#[derive(Debug, Clone)]
pub struct Saturation {
    sample_rate: f32,
    saturation: f32,
    ty: u8,
    auto_level: bool,
    mix: f32,
    oversampling: u8,
    output_db: f32,
    cv_trim: f32,
    fresh: bool,
    knob: BlockSmoother,
    level: BlockSmoother,
    weights: [BlockSmoother; 3],
    mix_sm: BlockSmoother,
    output: BlockSmoother,
    lanes: [Lane; 2],
    table: [[f32; TABLE + 1]; 3],
    drive_lamp: f32,
    release: f32,
    meters: [f32; METERS],
}

impl Default for Saturation {
    fn default() -> Self {
        Self::new()
    }
}

impl Saturation {
    /// A saturation device with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut s = Self {
            sample_rate: 48_000.0,
            saturation: 0.0,
            ty: 1,
            auto_level: true,
            mix: 1.0,
            oversampling: 1,
            output_db: 0.0,
            cv_trim: 0.0,
            fresh: true,
            knob: BlockSmoother::new(0.0),
            level: BlockSmoother::new(1.0),
            weights: [BlockSmoother::new(1.0); 3],
            mix_sm: BlockSmoother::new(1.0),
            output: BlockSmoother::new(1.0),
            lanes: [
                Lane::new(48_000.0, OversampleFactor::X2),
                Lane::new(48_000.0, OversampleFactor::X2),
            ],
            table: *level_table(),
            drive_lamp: 0.0,
            release: 0.0,
            meters: [0.0; METERS],
        };
        s.set_times();
        s
    }

    fn factor(&self) -> OversampleFactor {
        factor_of(self.oversampling)
    }

    fn set_times(&mut self) {
        let sr = self.sample_rate;
        for s in [
            &mut self.knob,
            &mut self.level,
            &mut self.mix_sm,
            &mut self.output,
        ] {
            s.set_time(10.0, sr, BATCH);
        }
        for w in &mut self.weights {
            w.set_time(20.0, sr, BATCH);
        }
        self.release = exp2_det(-(BATCH as f32) / (0.15 * sr) * std::f32::consts::LOG2_E);
    }

    /// One batch of one channel.
    fn run_lane(lane: &mut Lane, b: &Batch, input: &[f32; BATCH]) -> LaneOut {
        let mut x = [0.0f32; BATCH];
        let mut inp = [0.0f32; BATCH];
        let mut in_peak = 0.0f32;
        for n in 0..BATCH {
            let v = finite(input[n]);
            let (low, high) = lane.split.process(v);
            let sat = b.w[0][n] * v + b.w[1][n] * low + b.w[2][n] * high;
            x[n] = v;
            inp[n] = sat;
            in_peak = in_peak.max(v.abs());
        }
        let mut shaped = [0.0f32; BATCH];
        if lane.os.factor() == OversampleFactor::X1 {
            for n in 0..BATCH {
                shaped[n] = shape(inp[n], b.g[n]);
            }
        } else {
            for (i, v) in lane.os.upsample(&inp).iter_mut().enumerate() {
                *v = shape(*v, b.g[i / b.ratio]);
            }
            lane.os.downsample(&mut shaped);
        }
        let mut wet = [0.0f32; BATCH];
        let mut dry = [0.0f32; BATCH];
        let (mut e_in, mut e_shaped) = (0.0f32, 0.0f32);
        for n in 0..BATCH {
            // Delay the dry input, the clean part and the curve's input by the latency.
            let (dry_d, sat_in_d) = if b.latency == 0 {
                (x[n], inp[n])
            } else {
                (
                    lane.dry.read(b.latency as f32),
                    lane.sat.read(b.latency as f32),
                )
            };
            lane.dry.write(x[n]);
            lane.sat.write(inp[n]);
            let keep_d = dry_d - sat_in_d;
            let residue = lane.dc.process(shaped[n] - sat_in_d);
            wet[n] = if b.off {
                dry_d
            } else {
                keep_d + b.level[n] * (sat_in_d + residue)
            };
            dry[n] = dry_d;
            e_in += sat_in_d * sat_in_d;
            e_shaped += shaped[n] * shaped[n];
        }
        LaneOut {
            wet,
            dry,
            in_peak,
            e_in,
            e_shaped,
        }
    }
}

impl Device for Saturation {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        let factor = self.factor();
        self.lanes = [
            Lane::new(self.sample_rate, factor),
            Lane::new(self.sample_rate, factor),
        ];
        self.set_times();
        self.reset();
    }

    fn reset(&mut self) {
        self.fresh = true;
        for lane in &mut self.lanes {
            lane.reset();
        }
        self.drive_lamp = 0.0;
        self.meters = [0.0; METERS];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_SATURATION => self.saturation = v.clamp(0.0, 1.0),
            P_TYPE => self.ty = v.round().clamp(0.0, 2.0) as u8,
            P_AUTO_LEVEL => self.auto_level = v >= 0.5,
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_OVERSAMPLING => {
                self.oversampling = v.round().clamp(0.0, 2.0) as u8;
                let factor = self.factor();
                for lane in &mut self.lanes {
                    lane.os.set_factor(factor);
                }
            }
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            P_CV_TRIM => self.cv_trim = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let inputs = [*ctx.input(IN_L), *ctx.input(IN_R)];
        let cv = clean(batch_mean(ctx.input(SAT_CV)));
        let k_target = (self.saturation + self.cv_trim * cv).clamp(0.0, 1.0);
        let ty = usize::from(self.ty);
        let level_target = if self.auto_level {
            level_gain(&self.table, ty, k_target)
        } else {
            1.0
        };
        let w_t = weights_of(self.ty);
        let out_t = lin(self.output_db);
        if self.fresh {
            self.knob.set_immediate(k_target);
            self.level.set_immediate(level_target);
            for (w, t) in self.weights.iter_mut().zip(w_t) {
                w.set_immediate(t);
            }
            self.mix_sm.set_immediate(self.mix);
            self.output.set_immediate(out_t);
            self.fresh = false;
        }
        let knob = self.knob.advance(k_target, BATCH);
        let level = self.level.advance(level_target, BATCH);
        let weights = [
            self.weights[0].advance(w_t[0], BATCH),
            self.weights[1].advance(w_t[1], BATCH),
            self.weights[2].advance(w_t[2], BATCH),
        ];
        let mix = self.mix_sm.advance(self.mix, BATCH);
        let output = self.output.advance(out_t, BATCH);

        let factor = self.factor();
        let mut b = Batch {
            g: [0.0; BATCH],
            level: [1.0; BATCH],
            w: [[0.0; BATCH]; 3],
            off: true,
            latency: factor.latency_frames() as usize,
            ratio: factor.ratio(),
        };
        for n in 0..BATCH {
            let k = knob.at(n).clamp(0.0, 1.0);
            b.g[n] = drive_gain(k);
            b.off &= b.g[n] == 0.0;
            b.level[n] = level.at(n);
            for (w, r) in b.w.iter_mut().zip(&weights) {
                w[n] = r.at(n).clamp(0.0, 1.0);
            }
        }

        let mut outs = [[0.0f32; BATCH]; 2];
        let (mut in_peak, mut out_peak) = (0.0f32, 0.0f32);
        let (mut e_in, mut e_shaped) = (0.0f32, 0.0f32);
        for (c, out) in outs.iter_mut().enumerate() {
            let r = Self::run_lane(&mut self.lanes[c], &b, &inputs[c]);
            for (n, slot) in out.iter_mut().enumerate() {
                let m = mix.at(n);
                let mixed = if m >= 1.0 {
                    r.wet[n]
                } else {
                    r.dry[n] * (1.0 - m) + r.wet[n] * m
                };
                // `x * 1.0` is exact, so output at 0 dB leaves the dry input bit for bit.
                let y = mixed * output.at(n);
                *slot = y;
                out_peak = out_peak.max(y.abs());
            }
            in_peak = in_peak.max(r.in_peak);
            e_in += r.e_in;
            e_shaped += r.e_shaped;
        }
        let lamp = if e_in > 1.0e-12 && !b.off {
            (2.0 * (1.0 - (e_shaped / e_in).sqrt())).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.drive_lamp = lamp.max(self.drive_lamp * self.release);
        if self.drive_lamp < 1.0e-4 {
            self.drive_lamp = 0.0;
        }
        self.meters = [self.drive_lamp, in_peak, out_peak];
        *ctx.output(OUT_L) = outs[0];
        *ctx.output(OUT_R) = outs[1];
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }

    fn latency_frames(&self) -> u32 {
        self.factor().latency_frames()
    }

    fn variable_latency(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
