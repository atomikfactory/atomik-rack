//! `atomik.gate`: a stereo gate / downward expander with a sidechain key.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! key = sidechain (when `sc_enable` and `sc_in_l/r` connected) else main input
//! key -> HP (`sc_hp`) -> LP (`sc_lp`) -> atomik_dsp::Gate (stereo-linked) -> gain
//! main x gain -> out
//! ```
//!
//! * **Gate.** The detector is the peak follower of [`atomik_dsp::Gate`] (instant attack, 5 ms
//!   release, linked over the two channels). While the key is at or above `threshold` the gate is
//!   open (gain 1); after it falls below, the gate stays open for `hold`, then the gain in dB
//!   moves to `-range` with the `release` time constant; it opens again with the `attack` one.
//!   In **expander** mode the gain below the threshold is `(level - threshold) * 1` dB (a 2:1
//!   downward expansion), floored at `-range`, instead of the fixed `-range`.
//! * **Deterministic maths.** The gate runs with `Gate::set_deterministic`: the level in dB comes
//!   from `log2_det`, the gain from `exp2_det` and the time constants from `exp2_det_f64`, so a
//!   bounce is bit-identical on every platform (the channel strip's gate keeps its `log10` /
//!   `powf` maths).
//! * **`gr_cv_out`** is `1 - gain`: 0 while open, towards 1 while closed (the compressor's
//!   convention, so it can duck or open something else).
//! * **Meter values** (`has_meter_values`): `[input peak (linear, max of L/R over the batch),
//!   output peak, attenuation in dB]`.
//! * No latency. Non-finite input samples count as 0; allocation-free after construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{audio_in, audio_out, cv_out, expo, finite, linear, toggle};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{Biquad, BiquadCoeffs, BiquadShape};

const P_THRESHOLD: u32 = 0;
const P_RANGE: u32 = 1;
const P_HOLD: u32 = 2;
const P_ATTACK: u32 = 3;
const P_RELEASE: u32 = 4;
const P_EXPANDER: u32 = 5;
const P_SC_ENABLE: u32 = 6;
const P_SC_HP: u32 = 7;
const P_SC_LP: u32 = 8;

static PARAMS: [ParamDescriptor; 10] = [
    linear(
        P_THRESHOLD,
        "threshold",
        "Threshold",
        -80.0,
        0.0,
        -40.0,
        Unit::Db,
    ),
    linear(P_RANGE, "range", "Range", 0.0, 90.0, 60.0, Unit::Db),
    linear(P_HOLD, "hold", "Hold", 0.0, 2_000.0, 50.0, Unit::Ms),
    expo(P_ATTACK, "attack", "Attack", 0.1, 100.0, 1.0, Unit::Ms),
    expo(
        P_RELEASE,
        "release",
        "Release",
        5.0,
        2_000.0,
        150.0,
        Unit::Ms,
    ),
    toggle(P_EXPANDER, "expander", "Expander", 0.0),
    toggle(P_SC_ENABLE, "sc_enable", "Sidechain", 0.0),
    expo(
        P_SC_HP,
        "sc_hp",
        "SC high-pass",
        20.0,
        1_000.0,
        20.0,
        Unit::Hz,
    ),
    expo(
        P_SC_LP,
        "sc_lp",
        "SC low-pass",
        1_000.0,
        20_000.0,
        20_000.0,
        Unit::Hz,
    ),
    bypass_param(9),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const SC_L: SocketId = SocketId(2);
const SC_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const GR_CV: SocketId = SocketId(6);

static SOCKETS: [SocketDescriptor; 7] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_in(2, "sc_in_l", "Sidechain L"),
    audio_in(3, "sc_in_r", "Sidechain R"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
    cv_out(6, "gr_cv_out", "GR CV"),
];

/// Static description of the gate.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.gate",
    name: "Gate",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Gate`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Gate::new())),
    }
}

/// The gate device.
#[derive(Debug, Clone)]
pub struct Gate {
    sample_rate: f32,
    gate: atomik_dsp::Gate,
    sc_enable: bool,
    sc_hp: f32,
    sc_lp: f32,
    hp_coeffs: BiquadCoeffs,
    lp_coeffs: BiquadCoeffs,
    hp_on: bool,
    lp_on: bool,
    hp: [Biquad; 2],
    lp: [Biquad; 2],
    meters: [f32; 3],
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    /// A gate with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut g = Self {
            sample_rate: 48_000.0,
            gate: atomik_dsp::Gate::new(48_000.0),
            sc_enable: false,
            sc_hp: 20.0,
            sc_lp: 20_000.0,
            hp_coeffs: BiquadCoeffs::IDENTITY,
            lp_coeffs: BiquadCoeffs::IDENTITY,
            hp_on: false,
            lp_on: false,
            hp: [Biquad::default(); 2],
            lp: [Biquad::default(); 2],
            meters: [0.0; 3],
        };
        g.gate.set_deterministic(true);
        g.apply_defaults();
        g.derive_filters();
        g
    }

    fn apply_defaults(&mut self) {
        let g = &mut self.gate;
        g.set_threshold_db(-40.0);
        g.set_range_db(60.0);
        g.set_hold_ms(50.0);
        g.set_attack_ms(1.0);
        g.set_release_ms(150.0);
        g.set_expander(false);
    }

    fn derive_filters(&mut self) {
        self.hp_on = self.sc_hp > 20.5;
        self.lp_on = self.sc_lp < 19_999.0;
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let sr = f64::from(self.sample_rate);
        self.hp_coeffs =
            BiquadCoeffs::design(BiquadShape::Highpass, f64::from(self.sc_hp), q, 0.0, sr);
        self.lp_coeffs =
            BiquadCoeffs::design(BiquadShape::Lowpass, f64::from(self.sc_lp), q, 0.0, sr);
    }
}

impl Device for Gate {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.gate.set_sample_rate(self.sample_rate);
        self.derive_filters();
        self.reset();
    }

    fn reset(&mut self) {
        self.gate.reset();
        self.hp = [Biquad::default(); 2];
        self.lp = [Biquad::default(); 2];
        self.meters = [0.0; 3];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_THRESHOLD => self.gate.set_threshold_db(v.clamp(-80.0, 0.0)),
            P_RANGE => self.gate.set_range_db(v.clamp(0.0, 90.0)),
            P_HOLD => self.gate.set_hold_ms(v.clamp(0.0, 2_000.0)),
            P_ATTACK => self.gate.set_attack_ms(v.clamp(0.1, 100.0)),
            P_RELEASE => self.gate.set_release_ms(v.clamp(5.0, 2_000.0)),
            P_EXPANDER => self.gate.set_expander(v >= 0.5),
            P_SC_ENABLE => self.sc_enable = v >= 0.5,
            P_SC_HP => {
                self.sc_hp = v.clamp(20.0, 1_000.0);
                self.derive_filters();
            }
            P_SC_LP => {
                self.sc_lp = v.clamp(1_000.0, 20_000.0);
                self.derive_filters();
            }
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let sc_l_conn = ctx.is_connected(SC_L);
        let sc_r_conn = ctx.is_connected(SC_R);
        let use_sc = self.sc_enable && (sc_l_conn || sc_r_conn);
        let sc_l = *ctx.input(SC_L);
        let sc_r = *ctx.input(SC_R);

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        let (mut pk_in, mut pk_out) = (0.0f32, 0.0f32);
        let mut att_db = 0.0f32;
        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            pk_in = pk_in.max(x[0].abs()).max(x[1].abs());
            let mut key = if use_sc {
                let (a, b) = (finite(sc_l[n]), finite(sc_r[n]));
                match (sc_l_conn, sc_r_conn) {
                    (true, false) => [a, a],
                    (false, true) => [b, b],
                    _ => [a, b],
                }
            } else {
                x
            };
            for (c, k) in key.iter_mut().enumerate() {
                let mut v = f64::from(*k);
                if self.hp_on {
                    v = self.hp[c].process(&self.hp_coeffs, v);
                }
                if self.lp_on {
                    v = self.lp[c].process(&self.lp_coeffs, v);
                }
                *k = v as f32;
            }
            let gain = self.gate.process(key[0], key[1]);
            att_db = att_db.max(self.gate.gain_reduction_db());
            out_l[n] = x[0] * gain;
            out_r[n] = x[1] * gain;
            cv[n] = 1.0 - gain;
            pk_out = pk_out.max(out_l[n].abs()).max(out_r[n].abs());
        }
        self.meters = [pk_in, pk_out, att_db];
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(GR_CV) = cv;
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
