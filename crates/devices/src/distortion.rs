//! `atomik.distortion`: oversampled saturation / waveshaper / bitcrusher.
//!
//! # Signal flow (per channel, per sample)
//!
//! ```text
//! in -> x drive -> + bias -> [ up (1x/2x/4x) -> curve -> down ] -> (bitcrush) -> DC blocker
//!    -> tone tilt -> x output = wet ---------------------------------------+
//! in -> delay (= oversampler latency) = dry ------------------------------ mix -> out
//! ```
//!
//! * **Curves** (`curve`; `u` is the driven, biased sample, all deterministic: only `+ - * /`,
//!   `sqrt` and `floor`):
//!   * *soft clip*: `u / sqrt(1 + u^2)` (algebraic sigmoid, unity small-signal gain, odd
//!     harmonics);
//!   * *hard clip*: `clamp(u, -1, 1)`;
//!   * *tube*: asymmetric, positive half `u / sqrt(1 + u^2)`, negative half
//!     `u / sqrt(1 + (1.25 u)^2)` (saturates at -0.8): the same unity slope at 0, a lower negative
//!     ceiling, so it generates even harmonics (and a DC offset that the blocker removes);
//!   * *foldback*: the triangle fold `|((u + 3) mod 4) - 2| - 1` (identity for `|u| <= 1`,
//!     reflects at +-1);
//!   * *bitcrush*: `round(u * 2^(bits-1)) / 2^(bits-1)` on `clamp(u, -1, 1)` followed by a
//!     sample-and-hold at `crush.rate`.
//! * **Bias** is added after the drive. The curve's value at zero input, `curve(bias)`, is
//!   subtracted so a bias never produces a step, and the DC blocker removes what the asymmetry
//!   still generates.
//! * **Oversampling** (`oversample`: off, 2x, 4x; default 4x) runs the three nonlinear curves
//!   between the halfband stages of [`atomik_dsp::oversample::Oversampler`]. **Bitcrush
//!   runs at the base rate, without oversampling** (its aliasing is the effect): the signal still
//!   travels through the oversampler with an identity curve, so the timing and latency are
//!   identical for every curve, and the crush and hold are applied after the way back down.
//! * **DC blocker:** one-pole high-pass at 8 Hz (`y = x - x1 + R y1`, `f64`), always on in the wet
//!   path.
//! * **Tone** is a tilt EQ: a low shelf of `-6 * tone` dB and a high shelf of `+6 * tone` dB at
//!   1 kHz (RBJ, Q 0.707, smoothed biquads): `tone` +1 is brighter, -1 darker; at exactly 0 the
//!   shelves are skipped.
//! * **Output** (`output`, dB) scales the wet signal only; **mix** blends
//!   `dry * (1 - mix) + wet * mix`.
//! * **Latency decision (dynamic).** `latency_frames()` is the oversampler's latency for the
//!   *current* `oversample` setting: 0 (off), 29 (2x) or 35 (4x) samples, independent of the
//!   sample rate (see [`atomik_dsp::oversample`]). The engine publishes device latencies after
//!   each batch boundary's parameter changes and bumps `latency_epoch` when it moves, so a host
//!   recompiling on that epoch re-aligns PDC; switching the setting clears the oversampler filters
//!   (a short transient). The dry path is delayed by exactly the same latency, so **`mix = 0` is
//!   the input delayed by the latency, bit for bit** (the input itself when oversampling is off).
//!   The latency does not depend on the curve.
//! * **CV:** `drive_cv_in` and `mix_cv_in` (batch mean, clamped to -1..+1) times `drive_cv_trim` /
//!   `mix_cv_trim` (-1..+1, default +1) are added to the knob in normalised 0..1 space (drive over
//!   its 0..48 dB range), clamped, then smoothed like the knob. Unconnected: bit-identical.
//! * **Smoothing:** drive, bias, output and mix are smoothed per batch (BlockSmoother, 20 ms) and
//!   ramped linearly across the batch; curve, bits, rate and oversample are discrete.
//! * **Meter values:** `[input peak L/R max, output peak L/R max]`, linear, last batch.
//! * **Safety:** non-finite input samples are treated as 0; the curves are bounded by 1 so the
//!   output stays bounded for any drive.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{add_norm, batch_mean};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::oversample::{OversampleFactor, Oversampler};
use atomik_dsp::{BiquadShape, BlockSmoother, SmoothedBiquad, exp2_det, exp2_det_f64};

#[cfg(test)]
mod tests;

const P_CURVE: u32 = 0;
const P_DRIVE: u32 = 1;
const P_BIAS: u32 = 2;
const P_TONE: u32 = 3;
const P_OUTPUT: u32 = 4;
const P_MIX: u32 = 5;
const P_BITS: u32 = 6;
const P_RATE: u32 = 7;
const P_OVERSAMPLE: u32 = 8;
const P_DRIVE_TRIM: u32 = 9;
const P_MIX_TRIM: u32 = 10;

/// Range of the drive knob in dB (its CV is applied in normalised space over it).
const DRIVE_MAX_DB: f32 = 48.0;

static PARAMS: [ParamDescriptor; 12] = [
    ParamDescriptor {
        id: ParamId(P_CURVE),
        key: "curve",
        name: "Curve",
        min: 0.0,
        max: 4.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["soft clip", "hard clip", "tube", "foldback", "bitcrush"]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_DRIVE),
        key: "drive",
        name: "Drive",
        min: 0.0,
        max: 48.0,
        default: 12.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_BIAS),
        key: "bias",
        name: "Bias",
        min: -1.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_TONE),
        key: "tone",
        name: "Tone",
        min: -1.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_OUTPUT),
        key: "output",
        name: "Output",
        min: -24.0,
        max: 12.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_MIX),
        key: "mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_BITS),
        key: "crush.bits",
        name: "Crush bits",
        min: 1.0,
        max: 16.0,
        default: 8.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_RATE),
        key: "crush.rate",
        name: "Crush rate",
        min: 500.0,
        max: 96_000.0,
        default: 96_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_OVERSAMPLE),
        key: "oversample",
        name: "Oversampling",
        min: 0.0,
        max: 2.0,
        default: 2.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["off", "2x", "4x"]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_DRIVE_TRIM),
        key: "drive_cv_trim",
        name: "Drive CV trim",
        min: -1.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_MIX_TRIM),
        key: "mix_cv_trim",
        name: "Mix CV trim",
        min: -1.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    bypass_param(11),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const DRIVE_CV: SocketId = SocketId(4);
const MIX_CV: SocketId = SocketId(5);

static SOCKETS: [SocketDescriptor; 6] = [
    SocketDescriptor {
        id: IN_L,
        key: "in_l",
        name: "In L",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: IN_R,
        key: "in_r",
        name: "In R",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: OUT_L,
        key: "out_l",
        name: "Out L",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: OUT_R,
        key: "out_r",
        name: "Out R",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: DRIVE_CV,
        key: "drive_cv_in",
        name: "Drive CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: MIX_CV,
        key: "mix_cv_in",
        name: "Mix CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
];

/// Static description of the distortion.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.distortion",
    name: "Distortion",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Distortion`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Distortion::new())),
    }
}

/// The waveshaping curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// `u / sqrt(1 + u^2)`.
    Soft,
    /// `clamp(u, -1, 1)`.
    Hard,
    /// Asymmetric soft clip (even harmonics).
    Tube,
    /// Triangle fold at +-1.
    Fold,
    /// Quantise and sample-and-hold, at the base rate.
    Crush,
}

impl Curve {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Soft,
            1 => Self::Hard,
            2 => Self::Tube,
            3 => Self::Fold,
            _ => Self::Crush,
        }
    }
}

/// The static (memoryless) part of the curves; `Crush` is the identity here.
#[inline]
pub fn shape(curve: Curve, u: f32) -> f32 {
    match curve {
        Curve::Soft => u / (1.0 + u * u).sqrt(),
        Curve::Hard => u.clamp(-1.0, 1.0),
        Curve::Tube => {
            if u >= 0.0 {
                u / (1.0 + u * u).sqrt()
            } else {
                let v = 1.25 * u;
                u / (1.0 + v * v).sqrt()
            }
        }
        Curve::Fold => {
            let x = u.clamp(-1.0e4, 1.0e4) + 3.0;
            let m = x - 4.0 * (x * 0.25).floor();
            (m - 2.0).abs() - 1.0
        }
        Curve::Crush => u,
    }
}

/// Dry-path ring length (a power of two above the largest latency).
const DRY_RING: usize = 64;
/// Tilt range in dB per unit of `tone`.
const TILT_DB: f32 = 6.0;
/// DC blocker corner in Hz.
const DC_HZ: f64 = 8.0;
const LOG2_10_20: f32 = 0.166_096_4;

#[derive(Debug, Clone)]
struct Channel {
    os: Oversampler,
    /// Driven and biased block at the base rate.
    pre: [f32; BATCH],
    ring: [f32; DRY_RING],
    hold: f32,
    hold_acc: f32,
    dc_x1: f64,
    dc_y1: f64,
}

impl Channel {
    fn new() -> Self {
        Self {
            os: Oversampler::new(BATCH),
            pre: [0.0; BATCH],
            ring: [0.0; DRY_RING],
            hold: 0.0,
            hold_acc: 0.0,
            dc_x1: 0.0,
            dc_y1: 0.0,
        }
    }

    fn reset(&mut self) {
        self.os.reset();
        self.pre = [0.0; BATCH];
        self.ring = [0.0; DRY_RING];
        self.hold = 0.0;
        self.hold_acc = 0.0;
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
    }
}

/// The distortion device.
#[derive(Debug, Clone)]
pub struct Distortion {
    sample_rate: f32,
    curve: Curve,
    drive_db: f32,
    bias: f32,
    tone: f32,
    output_db: f32,
    mix: f32,
    drive_trim: f32,
    mix_trim: f32,
    bits: f32,
    rate: f32,
    factor: OversampleFactor,
    fresh: bool,
    drive_sm: BlockSmoother,
    bias_sm: BlockSmoother,
    out_sm: BlockSmoother,
    mix_sm: BlockSmoother,
    ch: [Channel; 2],
    low: SmoothedBiquad,
    high: SmoothedBiquad,
    dc_r: f64,
    wpos: usize,
    meters: [f32; 2],
}

impl Default for Distortion {
    fn default() -> Self {
        Self::new()
    }
}

impl Distortion {
    /// A distortion with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut d = Self {
            sample_rate: 48_000.0,
            curve: Curve::Soft,
            drive_db: 12.0,
            bias: 0.0,
            tone: 0.0,
            output_db: 0.0,
            mix: 1.0,
            drive_trim: 1.0,
            mix_trim: 1.0,
            bits: 8.0,
            rate: 96_000.0,
            factor: OversampleFactor::X4,
            fresh: true,
            drive_sm: BlockSmoother::new(1.0),
            bias_sm: BlockSmoother::new(0.0),
            out_sm: BlockSmoother::new(1.0),
            mix_sm: BlockSmoother::new(1.0),
            ch: [Channel::new(), Channel::new()],
            low: SmoothedBiquad::new(BiquadShape::LowShelf, 1_000.0, 0.707, 0.0, 48_000.0),
            high: SmoothedBiquad::new(BiquadShape::HighShelf, 1_000.0, 0.707, 0.0, 48_000.0),
            dc_r: 0.999,
            wpos: 0,
            meters: [0.0; 2],
        };
        d.setup(48_000.0);
        d.reset();
        d
    }

    fn setup(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        for sm in [
            &mut self.drive_sm,
            &mut self.bias_sm,
            &mut self.out_sm,
            &mut self.mix_sm,
        ] {
            sm.set_time(20.0, sample_rate, BATCH);
        }
        self.low = SmoothedBiquad::new(BiquadShape::LowShelf, 1_000.0, 0.707, 0.0, sample_rate);
        self.high = SmoothedBiquad::new(BiquadShape::HighShelf, 1_000.0, 0.707, 0.0, sample_rate);
        self.apply_tone();
        // R = exp(-2 pi fc / sr).
        self.dc_r = exp2_det_f64(
            -std::f64::consts::TAU * DC_HZ / f64::from(sample_rate) * std::f64::consts::LOG2_E,
        );
        for c in &mut self.ch {
            c.os.set_factor(self.factor);
        }
    }

    fn apply_tone(&mut self) {
        self.low.set_gain_db(-TILT_DB * self.tone);
        self.high.set_gain_db(TILT_DB * self.tone);
    }

    fn latency(&self) -> usize {
        self.factor.latency_frames() as usize
    }
}

impl Device for Distortion {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.setup(info.sample_rate.hz() as f32);
        self.reset();
    }

    fn reset(&mut self) {
        for c in &mut self.ch {
            c.reset();
        }
        self.low.reset();
        self.high.reset();
        self.wpos = 0;
        self.meters = [0.0; 2];
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        let Some(p) = PARAMS.get(id.0 as usize) else {
            return;
        };
        let v = v.clamp(p.min, p.max);
        match id.0 {
            P_CURVE => self.curve = Curve::from_index(v.round() as usize),
            P_DRIVE => self.drive_db = v,
            P_BIAS => self.bias = v,
            P_TONE => {
                self.tone = v;
                self.apply_tone();
            }
            P_OUTPUT => self.output_db = v,
            P_MIX => self.mix = v,
            P_DRIVE_TRIM => self.drive_trim = v,
            P_MIX_TRIM => self.mix_trim = v,
            P_BITS => self.bits = v.round().clamp(1.0, 16.0),
            P_RATE => self.rate = v,
            P_OVERSAMPLE => {
                self.factor = match v.round() as usize {
                    0 => OversampleFactor::X1,
                    1 => OversampleFactor::X2,
                    _ => OversampleFactor::X4,
                };
                for c in &mut self.ch {
                    c.os.set_factor(self.factor);
                }
            }
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
        let input = [*ctx.input(IN_L), *ctx.input(IN_R)];
        let sr = self.sample_rate;

        let drive_db = if ctx.is_connected(DRIVE_CV) {
            DRIVE_MAX_DB
                * add_norm(
                    self.drive_db / DRIVE_MAX_DB,
                    self.drive_trim,
                    batch_mean(ctx.input(DRIVE_CV)),
                )
        } else {
            self.drive_db
        };
        let mix_v = if ctx.is_connected(MIX_CV) {
            add_norm(self.mix, self.mix_trim, batch_mean(ctx.input(MIX_CV)))
        } else {
            self.mix
        };
        let drive_t = exp2_det(drive_db * LOG2_10_20);
        let out_t = exp2_det(self.output_db * LOG2_10_20);
        if self.fresh {
            self.drive_sm.set_immediate(drive_t);
            self.bias_sm.set_immediate(self.bias);
            self.out_sm.set_immediate(out_t);
            self.mix_sm.set_immediate(mix_v);
            self.fresh = false;
        }
        let drive = self.drive_sm.advance(drive_t, BATCH);
        let bias = self.bias_sm.advance(self.bias, BATCH);
        let out_g = self.out_sm.advance(out_t, BATCH);
        let mix = self.mix_sm.advance(mix_v, BATCH);

        let curve = self.curve;
        let crush = curve == Curve::Crush;
        let levels = exp2_det(self.bits - 1.0);
        let period = (sr / self.rate).max(1.0);
        let lat = self.latency();
        // The static offset of the curve at the batch's end bias, so a bias is not a step.
        let c0 = shape(curve, bias.at(BATCH));
        let dc_r = self.dc_r;
        let tilt = self.tone.to_bits() != 0 || !self.low.is_settled() || !self.high.is_settled();

        // Stage 1: drive, bias, oversampled curve, per channel.
        let mut wet = [[0.0f32; BATCH]; 2];
        let mut dry = [[0.0f32; BATCH]; 2];
        let mut pk_in = 0.0f32;
        for c in 0..2 {
            let ch = &mut self.ch[c];
            for n in 0..BATCH {
                let x = finite(input[c][n]);
                pk_in = pk_in.max(x.abs());
                // Dry path: write, then read `lat` samples back (lat 0 reads the new sample).
                let w = self.wpos.wrapping_add(n);
                ch.ring[w & (DRY_RING - 1)] = x;
                dry[c][n] = ch.ring[w.wrapping_sub(lat) & (DRY_RING - 1)];
                ch.pre[n] = x * drive.at(n) + bias.at(n);
            }
            let hi = ch.os.upsample(&ch.pre);
            if !crush {
                for v in hi.iter_mut() {
                    *v = shape(curve, *v);
                }
            }
            let mut y = [0.0f32; BATCH];
            ch.os.downsample(&mut y);
            for n in 0..BATCH {
                let mut v = if crush {
                    // Quantise the clamped signal, then sample-and-hold at the base rate.
                    let q = (y[n].clamp(-1.0, 1.0) * levels).round() / levels;
                    ch.hold_acc += 1.0;
                    if ch.hold_acc >= period {
                        ch.hold_acc -= period;
                        ch.hold = q;
                    }
                    ch.hold
                } else {
                    y[n] - c0
                };
                if !v.is_finite() {
                    v = 0.0;
                }
                // DC blocker.
                let xd = f64::from(v);
                let yd = xd - ch.dc_x1 + dc_r * ch.dc_y1;
                ch.dc_x1 = xd;
                ch.dc_y1 = if yd.abs() < 1e-30 { 0.0 } else { yd };
                wet[c][n] = ch.dc_y1 as f32;
            }
        }
        self.wpos = self.wpos.wrapping_add(BATCH);

        // Stage 2: tone tilt, output gain, mix.
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut pk_out = 0.0f32;
        for n in 0..BATCH {
            let (mut wl, mut wr) = (wet[0][n], wet[1][n]);
            if tilt {
                (wl, wr) = self.low.process(wl, wr);
                (wl, wr) = self.high.process(wl, wr);
            }
            let g = out_g.at(n);
            let (wl, wr) = (wl * g, wr * g);
            let m = mix.at(n);
            let (yl, yr) = if m == 0.0 {
                (dry[0][n], dry[1][n])
            } else {
                (
                    dry[0][n] * (1.0 - m) + wl * m,
                    dry[1][n] * (1.0 - m) + wr * m,
                )
            };
            out_l[n] = yl;
            out_r[n] = yr;
            pk_out = pk_out.max(yl.abs()).max(yr.abs());
        }
        self.meters = [pk_in, pk_out];
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
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
        self.factor.latency_frames()
    }

    fn variable_latency(&self) -> bool {
        true
    }
}
