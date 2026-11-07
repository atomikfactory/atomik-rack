//! `atomik.transient-shaper`: raises or lowers the attack and the sustain of a signal, whatever
//! its level.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! in -> power (mean of the two squares) -> fast follower (`fast`)  --\
//!                                       -> slow follower (`slow`)  ---+-> d = fast - slow, in dB
//! gain dB = attack * clamp(d, 0, 12) + sustain * clamp(-d, 0, 12) + output
//! out = in x gain  (blended with the dry input by `mix`)
//! ```
//!
//! * **Detector.** Two one-pole followers of the stereo-linked signal power, a fast one and a
//!   slow one, both with the same time constant for rising and falling. Their ratio in dB, `d`, is
//!   what changes, not the level: a note onset makes the fast follower lead (`d` above 0, the
//!   *attack* part), a decaying tail makes it fall under the slow one (`d` below 0, the *sustain*
//!   part), and a steady tone gives `d = 0`, so the same settings work at any level. Power under
//!   `threshold` is read as `threshold`, so a noise floor does not move the gain.
//! * **Amounts.** `attack` and `sustain` run from -1 (down 12 dB at a 12 dB difference) through 0
//!   (off) to +1 (up 12 dB). The CV inputs add `trim * cv` to them. The total gain never exceeds
//!   +18 dB.
//! * **Deterministic maths.** The levels use `log2_det`, the gain `exp2_det` and the time
//!   constants `exp2_det_f64`, so a bounce is bit-identical on every platform.
//! * **`transient_cv_out`** is the attack part of `d` scaled to 0..1 (12 dB and more reads 1): a
//!   trigger-like signal that follows the onsets.
//! * **Meter values** (`has_meter_values`): `[input peak, output peak, gain in dB]`.
//! * No latency: the gain follows the fast follower, so the first milliseconds of an onset are
//!   shaped as the follower rises. Non-finite input samples count as 0; allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{batch_mean, clean};
use crate::tool_support::{audio_in, audio_out, cv_in, cv_out, expo, finite, linear, trim};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{exp2_det, exp2_det_f64, log2_det};

const P_ATTACK: u32 = 0;
const P_SUSTAIN: u32 = 1;
const P_FAST: u32 = 2;
const P_SLOW: u32 = 3;
const P_THRESHOLD: u32 = 4;
const P_OUTPUT: u32 = 5;
const P_MIX: u32 = 6;
const P_ATTACK_TRIM: u32 = 7;
const P_SUSTAIN_TRIM: u32 = 8;

/// `10 * log10(2)`: a power ratio of `2^x` is `x * DB_PER_OCTAVE` dB.
const DB_PER_OCTAVE: f32 = 3.010_3;
/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The follower difference, in dB, at which an amount of 1 gives its full 12 dB.
const FULL_DB: f32 = 12.0;
/// The most the gain may add, dB.
const MAX_GAIN_DB: f32 = 18.0;
/// The most it may take away, dB.
const MIN_GAIN_DB: f32 = -48.0;

static PARAMS: [ParamDescriptor; 10] = [
    linear(P_ATTACK, "attack", "Attack", -1.0, 1.0, 0.0, Unit::Percent),
    linear(
        P_SUSTAIN,
        "sustain",
        "Sustain",
        -1.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    expo(P_FAST, "fast", "Fast", 0.5, 30.0, 5.0, Unit::Ms),
    expo(P_SLOW, "slow", "Slow", 10.0, 500.0, 50.0, Unit::Ms),
    linear(
        P_THRESHOLD,
        "threshold",
        "Threshold",
        -90.0,
        -20.0,
        -70.0,
        Unit::Db,
    ),
    linear(P_OUTPUT, "output", "Output", -24.0, 12.0, 0.0, Unit::Db),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    trim(P_ATTACK_TRIM, "attack_cv_trim", "Attack CV trim"),
    trim(P_SUSTAIN_TRIM, "sustain_cv_trim", "Sustain CV trim"),
    bypass_param(9),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const ATTACK_CV: SocketId = SocketId(2);
const SUSTAIN_CV: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const TRANSIENT_CV: SocketId = SocketId(6);

static SOCKETS: [SocketDescriptor; 7] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    cv_in(2, "attack_cv_in", "Attack CV"),
    cv_in(3, "sustain_cv_in", "Sustain CV"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
    cv_out(6, "transient_cv_out", "Transient CV"),
];

/// Static description of the transient shaper.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.transient-shaper",
    name: "Transient Shaper",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`TransientShaper`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(TransientShaper::new())),
    }
}

/// The transient shaper device.
#[derive(Debug, Clone)]
pub struct TransientShaper {
    sample_rate: f32,
    attack: f32,
    sustain: f32,
    fast_ms: f32,
    slow_ms: f32,
    threshold_db: f32,
    output_db: f32,
    mix: f32,
    attack_trim: f32,
    sustain_trim: f32,
    // Derived.
    floor: f32,
    fast_a: f32,
    slow_a: f32,
    // State.
    fast: f32,
    slow: f32,
    meters: [f32; 3],
}

impl Default for TransientShaper {
    fn default() -> Self {
        Self::new()
    }
}

impl TransientShaper {
    /// A transient shaper with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut s = Self {
            sample_rate: 48_000.0,
            attack: 0.0,
            sustain: 0.0,
            fast_ms: 5.0,
            slow_ms: 50.0,
            threshold_db: -70.0,
            output_db: 0.0,
            mix: 1.0,
            attack_trim: 1.0,
            sustain_trim: 1.0,
            floor: 0.0,
            fast_a: 0.0,
            slow_a: 0.0,
            fast: 0.0,
            slow: 0.0,
            meters: [0.0; 3],
        };
        s.derive();
        s.reset();
        s
    }

    fn coefficient(ms: f32, sample_rate: f32) -> f32 {
        let n = f64::from(ms) * 0.001 * f64::from(sample_rate);
        (1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / n)) as f32
    }

    fn derive(&mut self) {
        // A power of `threshold_db` (an amplitude in dB): `10^(db/10) = 2^(db * 2 * LOG2_10_20)`.
        self.floor = exp2_det(self.threshold_db * LOG2_10_20 * 2.0);
        self.fast_a = Self::coefficient(self.fast_ms, self.sample_rate);
        self.slow_a = Self::coefficient(self.slow_ms, self.sample_rate);
    }
}

impl Device for TransientShaper {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.fast = self.floor;
        self.slow = self.floor;
        self.meters = [0.0; 3];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_ATTACK => self.attack = v.clamp(-1.0, 1.0),
            P_SUSTAIN => self.sustain = v.clamp(-1.0, 1.0),
            P_FAST => {
                self.fast_ms = v.clamp(0.5, 30.0);
                self.derive();
            }
            P_SLOW => {
                self.slow_ms = v.clamp(10.0, 500.0);
                self.derive();
            }
            P_THRESHOLD => {
                self.threshold_db = v.clamp(-90.0, -20.0);
                self.derive();
            }
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_ATTACK_TRIM => self.attack_trim = v.clamp(-1.0, 1.0),
            P_SUSTAIN_TRIM => self.sustain_trim = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let attack = (self.attack + self.attack_trim * clean(batch_mean(ctx.input(ATTACK_CV))))
            .clamp(-1.0, 1.0);
        let sustain = (self.sustain + self.sustain_trim * clean(batch_mean(ctx.input(SUSTAIN_CV))))
            .clamp(-1.0, 1.0);

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        let (mut pk_in, mut pk_out) = (0.0f32, 0.0f32);
        let mut last_db = 0.0f32;
        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            pk_in = pk_in.max(x[0].abs()).max(x[1].abs());
            let power = (x[0] * x[0] + x[1] * x[1]) * 0.5;
            let p = power.max(self.floor);
            self.fast += self.fast_a * (p - self.fast);
            self.slow += self.slow_a * (p - self.slow);
            let d = (log2_det(self.fast.max(self.floor)) - log2_det(self.slow.max(self.floor)))
                * DB_PER_OCTAVE;
            let up = d.clamp(0.0, FULL_DB);
            let down = (-d).clamp(0.0, FULL_DB);
            let shaped = attack * up + sustain * down;
            let db = (shaped + self.output_db).clamp(MIN_GAIN_DB, MAX_GAIN_DB);
            last_db = db;
            let gain = exp2_det(db * LOG2_10_20);
            for (c, o) in [&mut out_l, &mut out_r].into_iter().enumerate() {
                o[n] = if self.mix >= 1.0 {
                    x[c] * gain
                } else {
                    x[c] * (1.0 - self.mix) + x[c] * gain * self.mix
                };
            }
            cv[n] = up / FULL_DB;
            pk_out = pk_out.max(out_l[n].abs()).max(out_r[n].abs());
        }
        self.meters = [pk_in, pk_out, last_db];
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(TRANSIENT_CV) = cv;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }
}

#[cfg(test)]
mod tests;
