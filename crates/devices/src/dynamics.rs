//! `atomik.dynamics`: a gate followed by a compressor, with an input gain, makeup and a dry/wet mix
//! in one device.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! x -> input gain -> gate (stereo-linked, on its own signal) -> compressor -> makeup -> y
//! out = x * (1 - mix) + y * mix           (x: the input before the input gain)
//! ```
//!
//! * **Gate.** [`atomik_dsp::Gate`] in its deterministic mode: threshold, range, release, a fixed
//!   20 ms hold and 1 ms attack. At the minimum threshold (-80 dB, the default) it is skipped.
//! * **Compressor.** A feed-forward stereo-linked peak compressor: the larger of `|L|` and `|R|`
//!   (after the gate) in dB (`log2_det`), the soft-knee curve
//!   [`atomik_dsp::static_gain_reduction_knee_db`], ballistics on the gain reduction in dB (peak
//!   hold release stage, then the attack smoother, as the compressor device), and the gain
//!   through `exp2_det`. `attack` and `release` are the 63.2 % / 36.8 % time constants.
//! * **`gr_cv_out`** is `1 - g` with `g` the compressor's linear gain (0 at rest, towards 1 with
//!   heavy reduction); the gate's gain is not part of it.
//! * Input gain, makeup and mix are smoothed per batch (20 ms, `BlockSmoother`). With the defaults
//!   (0 dB, gate never closing, ratio fully open at 0 dB threshold, no makeup) the output is the
//!   input bit for bit. No latency; non-finite input samples count as 0; allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{audio_in, audio_out, cv_out, expo, finite, linear};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{BlockSmoother, exp2_det, exp2_det_f64, log2_det, static_gain_reduction_knee_db};

const P_IN_GAIN: u32 = 0;
const P_GATE_THRESHOLD: u32 = 1;
const P_GATE_RANGE: u32 = 2;
const P_GATE_RELEASE: u32 = 3;
const P_THRESHOLD: u32 = 4;
const P_RATIO: u32 = 5;
const P_KNEE: u32 = 6;
const P_ATTACK: u32 = 7;
const P_RELEASE: u32 = 8;
const P_MAKEUP: u32 = 9;
const P_MIX: u32 = 10;

const GATE_HOLD_MS: f32 = 20.0;
const GATE_ATTACK_MS: f32 = 1.0;
/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// `20 * log10(2)`: dB per octave of amplitude.
const DB_PER_LOG2: f32 = 6.020_6;

static PARAMS: [ParamDescriptor; 12] = [
    linear(
        P_IN_GAIN,
        "in_gain",
        "Input gain",
        -24.0,
        24.0,
        0.0,
        Unit::Db,
    ),
    linear(
        P_GATE_THRESHOLD,
        "gate_threshold",
        "Gate threshold",
        -80.0,
        0.0,
        -80.0,
        Unit::Db,
    ),
    linear(
        P_GATE_RANGE,
        "gate_range",
        "Gate range",
        0.0,
        90.0,
        40.0,
        Unit::Db,
    ),
    expo(
        P_GATE_RELEASE,
        "gate_release",
        "Gate release",
        5.0,
        2_000.0,
        150.0,
        Unit::Ms,
    ),
    linear(
        P_THRESHOLD,
        "threshold",
        "Threshold",
        -60.0,
        0.0,
        0.0,
        Unit::Db,
    ),
    expo(P_RATIO, "ratio", "Ratio", 1.0, 20.0, 1.0, Unit::None),
    linear(P_KNEE, "knee", "Knee", 0.0, 24.0, 6.0, Unit::Db),
    expo(P_ATTACK, "attack", "Attack", 0.1, 200.0, 10.0, Unit::Ms),
    expo(
        P_RELEASE,
        "release",
        "Release",
        5.0,
        2_000.0,
        150.0,
        Unit::Ms,
    ),
    linear(P_MAKEUP, "makeup", "Makeup", 0.0, 24.0, 0.0, Unit::Db),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    bypass_param(11),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const GR_CV: SocketId = SocketId(4);

static SOCKETS: [SocketDescriptor; 5] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_out(2, "out_l", "Out L"),
    audio_out(3, "out_r", "Out R"),
    cv_out(4, "gr_cv_out", "GR CV"),
];

/// Static description of the dynamics device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.dynamics",
    name: "Gate + Compressor",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Dynamics`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Dynamics::new())),
    }
}

fn alpha(ms: f32, sr: f32) -> f64 {
    let n = f64::from(ms) * 0.001 * f64::from(sr);
    if n > 0.0 {
        exp2_det_f64(-std::f64::consts::LOG2_E / n)
    } else {
        0.0
    }
}

#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < 1e-18 { 0.0 } else { v }
}

/// The dynamics device.
#[derive(Debug, Clone)]
pub struct Dynamics {
    sample_rate: f32,
    in_gain_db: f32,
    threshold: f32,
    ratio: f32,
    knee: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_db: f32,
    mix: f32,
    gate: atomik_dsp::Gate,
    /// The gate is bypassed while its threshold sits at the -80 dB minimum.
    gate_on: bool,
    a_attack: f64,
    a_release: f64,
    y1: f64,
    gr: f64,
    fresh: bool,
    in_sm: BlockSmoother,
    makeup_sm: BlockSmoother,
    mix_sm: BlockSmoother,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self::new()
    }
}

impl Dynamics {
    /// A device with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut d = Self {
            sample_rate: 48_000.0,
            in_gain_db: 0.0,
            threshold: 0.0,
            ratio: 1.0,
            knee: 6.0,
            attack_ms: 10.0,
            release_ms: 150.0,
            makeup_db: 0.0,
            mix: 1.0,
            gate: atomik_dsp::Gate::new(48_000.0),
            gate_on: false,
            a_attack: 0.0,
            a_release: 0.0,
            y1: 0.0,
            gr: 0.0,
            fresh: true,
            in_sm: BlockSmoother::new(1.0),
            makeup_sm: BlockSmoother::new(1.0),
            mix_sm: BlockSmoother::new(1.0),
        };
        d.gate.set_deterministic(true);
        d.configure_gate();
        d.derive();
        d
    }

    fn configure_gate(&mut self) {
        let g = &mut self.gate;
        g.set_threshold_db(-80.0);
        g.set_range_db(40.0);
        g.set_hold_ms(GATE_HOLD_MS);
        g.set_attack_ms(GATE_ATTACK_MS);
        g.set_release_ms(150.0);
        g.set_expander(false);
    }

    fn derive(&mut self) {
        self.a_attack = alpha(self.attack_ms, self.sample_rate);
        self.a_release = alpha(self.release_ms, self.sample_rate);
    }
}

impl Device for Dynamics {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        // `set_sample_rate` keeps the stored gate params (it only recomputes and resets).
        self.gate.set_sample_rate(self.sample_rate);
        for s in [&mut self.in_sm, &mut self.makeup_sm, &mut self.mix_sm] {
            s.set_time(20.0, self.sample_rate, BATCH);
        }
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.gate.reset();
        self.y1 = 0.0;
        self.gr = 0.0;
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_IN_GAIN => self.in_gain_db = v.clamp(-24.0, 24.0),
            P_GATE_THRESHOLD => {
                let v = v.clamp(-80.0, 0.0);
                self.gate.set_threshold_db(v);
                self.gate_on = v > -79.9;
            }
            P_GATE_RANGE => self.gate.set_range_db(v.clamp(0.0, 90.0)),
            P_GATE_RELEASE => self.gate.set_release_ms(v.clamp(5.0, 2_000.0)),
            P_THRESHOLD => self.threshold = v.clamp(-60.0, 0.0),
            P_RATIO => self.ratio = v.clamp(1.0, 20.0),
            P_KNEE => self.knee = v.clamp(0.0, 24.0),
            P_ATTACK => {
                self.attack_ms = v.clamp(0.1, 200.0);
                self.derive();
            }
            P_RELEASE => {
                self.release_ms = v.clamp(5.0, 2_000.0);
                self.derive();
            }
            P_MAKEUP => self.makeup_db = v.clamp(0.0, 24.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let (in_l, in_r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        let in_target = exp2_det(self.in_gain_db * LOG2_10_20);
        let makeup_target = exp2_det(self.makeup_db * LOG2_10_20);
        if self.fresh {
            self.in_sm.set_immediate(in_target);
            self.makeup_sm.set_immediate(makeup_target);
            self.mix_sm.set_immediate(self.mix);
            self.fresh = false;
        }
        let gain_in = self.in_sm.advance(in_target, BATCH);
        let makeup = self.makeup_sm.advance(makeup_target, BATCH);
        let mix = self.mix_sm.advance(self.mix, BATCH);

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            let gi = gain_in.at(n);
            let s = [x[0] * gi, x[1] * gi];
            let g = if self.gate_on {
                let gg = self.gate.process(s[0], s[1]);
                [s[0] * gg, s[1] * gg]
            } else {
                s
            };

            let level = g[0].abs().max(g[1].abs());
            let level_db = (DB_PER_LOG2 * log2_det(level)).max(-200.0);
            let target = f64::from(static_gain_reduction_knee_db(
                level_db,
                self.threshold,
                self.ratio,
                self.knee,
            ));
            self.y1 = flush(if self.y1 > target {
                target + self.a_release * (self.y1 - target)
            } else {
                target
            });
            self.gr = flush(if self.y1 > self.gr {
                self.y1 + self.a_attack * (self.gr - self.y1)
            } else {
                self.y1
            });
            let comp = exp2_det(-(self.gr as f32) * LOG2_10_20);
            cv[n] = 1.0 - comp;
            let total = comp * makeup.at(n);
            let m = mix.at(n);
            for c in 0..2 {
                let wet = g[c] * total;
                let y = if m >= 1.0 {
                    wet
                } else {
                    x[c] * (1.0 - m) + wet * m
                };
                if c == 0 {
                    out_l[n] = y;
                } else {
                    out_r[n] = y;
                }
            }
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(GR_CV) = cv;
    }
}

#[cfg(test)]
mod tests;
