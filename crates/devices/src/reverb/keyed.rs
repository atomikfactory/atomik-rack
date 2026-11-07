//! The keyed part of both reverbs: a duck and a gate on the WET path, driven by a key signal.
//!
//! The key is `sc_in_l/r` when one of them is connected (a lone connected side feeds both) and
//! the main input otherwise. Its level is `max(|l|, |r|)` followed by a peak follower with a fixed
//! 5 ms attack and a `duck_release` fall. Both the duck and the gate read the same threshold,
//! `duck_threshold`:
//!
//! * **Duck.** `engage = clamp((follower / threshold - 0.5) * 2, 0, 1)`: nothing below half the
//!   threshold (6 dB under), everything at the threshold, linear in between (no `log` on the audio
//!   path). `depth = duck * engage` is the share of the ducked part that is taken out, so at
//!   `depth` its level is `1 - depth`. *Broadband* multiplies the wet signal by `1 - depth` (with
//!   `depth == 0` the wet signal is untouched, bit for bit). *Low band* splits the wet signal at
//!   `duck_freq` with a Linkwitz-Riley pair (two Butterworth sections each, 24 dB per octave) and
//!   returns `high + (1 - depth) * low`: the two halves add up to an allpass of the wet signal, so
//!   with nothing ducking it is the wet signal with a phase shift around `duck_freq`, not the same
//!   bits. *Band* is a bell at `duck_freq` (Q 2) cut to `1 - depth` of the level at its
//!   centre (never more than 40 dB), designed again every 32 samples. The dry path is never in
//!   here.
//! * **Gate.** With `key_gate` on the wet output closes `key_gate_hold` ms after the key follower
//!   last exceeded the threshold (a 12 ms fade) and opens again within 1 ms, the cut-off tail of a
//!   gated reverb that follows the dry track instead of the reverb's own input. A hold of 0 keeps
//!   the gate open.

use super::Gate;
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_dsp::{Biquad, BiquadCoeffs, BiquadShape, exp2_det, flush_denormal};

/// The number of params of the group.
pub(super) const PARAMS: u32 = 7;

const AMOUNT: u32 = 0;
const RELEASE: u32 = 1;
const MODE: u32 = 2;
const FREQ: u32 = 3;
const THRESHOLD: u32 = 4;
const GATE_ON: u32 = 5;
const GATE_HOLD: u32 = 6;

/// The follower's attack in ms.
const ATTACK_MS: f32 = 5.0;
/// The Butterworth Q of every split section.
const Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// The Q of the band mode's bell.
const BELL_Q: f64 = 2.0;
/// The deepest cut of the band mode's bell, as a gain: -40 dB.
const BELL_FLOOR: f64 = 0.01;
/// Samples between two designs of the band mode's bell.
const BELL_EVERY: u32 = 32;

/// The descriptor of param `slot` (0..[`PARAMS`]) of the group, with id `id`.
pub(crate) const fn param(slot: u32, id: u32) -> ParamDescriptor {
    let (key, name, min, max, default, taper, unit) = match slot {
        AMOUNT => ("duck", "Duck", 0.0, 1.0, 0.0, Taper::Linear, Unit::Percent),
        RELEASE => (
            "duck_release",
            "Duck release",
            20.0,
            2_000.0,
            300.0,
            Taper::Exponential,
            Unit::Ms,
        ),
        MODE => (
            "duck_mode",
            "Duck mode",
            0.0,
            2.0,
            0.0,
            Taper::Linear,
            Unit::Choice(&["broadband", "low band", "band"]),
        ),
        FREQ => (
            "duck_freq",
            "Duck frequency",
            60.0,
            8_000.0,
            250.0,
            Taper::Exponential,
            Unit::Hz,
        ),
        THRESHOLD => (
            "duck_threshold",
            "Key threshold",
            -60.0,
            0.0,
            -30.0,
            Taper::Linear,
            Unit::Db,
        ),
        GATE_ON => (
            "key_gate",
            "Gate on key",
            0.0,
            1.0,
            0.0,
            Taper::Linear,
            Unit::Toggle,
        ),
        _ => (
            "key_gate_hold",
            "Key gate hold",
            0.0,
            500.0,
            80.0,
            Taper::Linear,
            Unit::Ms,
        ),
    };
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper,
        unit,
        discrete: matches!(unit, Unit::Choice(_) | Unit::Toggle),
    }
}

/// The duck and the gate of one reverb.
#[derive(Debug, Clone)]
pub(super) struct Keyed {
    amount: f32,
    release_ms: f32,
    mode: u8,
    freq: f32,
    threshold_db: f32,
    gate_on: bool,
    gate_hold_ms: f32,
    sample_rate: f32,
    dirty: bool,
    // Derived once per batch when a param changed.
    threshold: f32,
    attack: f32,
    release: f32,
    hold: u32,
    /// The low-pass and the high-pass of the low band split.
    coeffs: [BiquadCoeffs; 2],
    /// The band mode's bell and the samples left until it is designed again.
    bell: BiquadCoeffs,
    bell_wait: u32,
    // State.
    env: f32,
    gate: Gate,
    /// Per channel: two low-pass and two high-pass sections (the bell uses the first).
    bands: [[Biquad; 4]; 2],
}

impl Keyed {
    pub(super) fn new() -> Self {
        Self {
            amount: 0.0,
            release_ms: 300.0,
            mode: 0,
            freq: 250.0,
            threshold_db: -30.0,
            gate_on: false,
            gate_hold_ms: 80.0,
            sample_rate: 48_000.0,
            dirty: true,
            threshold: 0.0,
            attack: 0.0,
            release: 0.0,
            hold: 0,
            coeffs: [BiquadCoeffs::IDENTITY; 2],
            bell: BiquadCoeffs::IDENTITY,
            bell_wait: 0,
            env: 0.0,
            gate: Gate::new(),
            bands: [[Biquad::default(); 4]; 2],
        }
    }

    pub(super) fn set_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.gate.set_rate(sr);
        self.dirty = true;
    }

    pub(super) fn clear(&mut self) {
        self.env = 0.0;
        self.gate.clear();
        self.clear_bands();
    }

    fn clear_bands(&mut self) {
        self.bell_wait = 0;
        for ch in &mut self.bands {
            for b in ch {
                b.reset();
            }
        }
    }

    /// Sets param `slot` of the group.
    pub(super) fn set(&mut self, slot: u32, v: f32) {
        match slot {
            AMOUNT => self.amount = v.clamp(0.0, 1.0),
            RELEASE => self.release_ms = v.clamp(20.0, 2_000.0),
            MODE => {
                let mode = (v.max(0.0).round() as u8).min(2);
                if mode != self.mode {
                    self.clear_bands();
                }
                self.mode = mode;
            }
            FREQ => self.freq = v.clamp(60.0, 8_000.0),
            THRESHOLD => self.threshold_db = v.clamp(-60.0, 0.0),
            GATE_ON => self.gate_on = v >= 0.5,
            GATE_HOLD => self.gate_hold_ms = v.clamp(0.0, 500.0),
            _ => return,
        }
        self.dirty = true;
    }

    /// Whether the duck or the gate does anything.
    pub(super) fn active(&self) -> bool {
        self.amount > 0.0 || self.gate_on
    }

    /// Derives the batch-rate values after a param or the sample rate changed.
    pub(super) fn batch(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let sr = self.sample_rate;
        let pole = |ms: f32| exp2_det(-1000.0 / (ms * sr) * std::f32::consts::LOG2_E);
        self.threshold = exp2_det(self.threshold_db * (std::f32::consts::LOG2_10 / 20.0));
        self.attack = 1.0 - pole(ATTACK_MS);
        self.release = pole(self.release_ms);
        self.hold = (self.gate_hold_ms * 0.001 * sr).round() as u32;
        let (f, sr) = (f64::from(self.freq), f64::from(sr));
        self.coeffs = [
            BiquadCoeffs::design(BiquadShape::Lowpass, f, Q, 0.0, sr),
            BiquadCoeffs::design(BiquadShape::Highpass, f, Q, 0.0, sr),
        ];
        self.bell_wait = 0;
    }

    /// Ducks and gates one wet sample pair from the key level `key` (a linear peak).
    #[inline]
    pub(super) fn tick(&mut self, key: f32, wet: [f32; 2]) -> [f32; 2] {
        let mut out = wet;
        if self.amount > 0.0 {
            self.env = if key > self.env {
                self.env + (key - self.env) * self.attack
            } else {
                flush_denormal(key + (self.env - key) * self.release)
            };
            let engage = ((self.env / self.threshold - 0.5) * 2.0).clamp(0.0, 1.0);
            let depth = self.amount * engage;
            match self.mode {
                0 => {
                    let g = 1.0 - depth;
                    out = [wet[0] * g, wet[1] * g];
                }
                1 => {
                    for ch in 0..2 {
                        let [l1, l2, h1, h2] = &mut self.bands[ch];
                        let x = f64::from(wet[ch]);
                        let low = l2.process(&self.coeffs[0], l1.process(&self.coeffs[0], x));
                        let high = h2.process(&self.coeffs[1], h1.process(&self.coeffs[1], x));
                        out[ch] = (high + f64::from(1.0 - depth) * low) as f32;
                    }
                }
                _ => {
                    if self.bell_wait == 0 {
                        let gain = f64::from(1.0 - depth).max(BELL_FLOOR);
                        self.bell = BiquadCoeffs::design(
                            BiquadShape::Peaking,
                            f64::from(self.freq),
                            BELL_Q,
                            20.0 * gain.log10(),
                            f64::from(self.sample_rate),
                        );
                        self.bell_wait = BELL_EVERY;
                    }
                    self.bell_wait -= 1;
                    for ch in 0..2 {
                        out[ch] = self.bands[ch][0].process(&self.bell, f64::from(wet[ch])) as f32;
                    }
                }
            }
        }
        if self.gate_on {
            let g = self.gate.tick(key, self.threshold, self.hold);
            out = [out[0] * g, out[1] * g];
        }
        out
    }
}
