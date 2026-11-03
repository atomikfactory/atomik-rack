//! The synthesised pad modules of the drum machine (see the module docs of `drum_machine`).
//!
//! A pad whose `module` is not `sample` plays a small synthesis voice instead of its zones. Every
//! voice is mono, preallocated and deterministic (noise from a seeded generator, sines and
//! filter coefficients from the deterministic maths); the pad's own level, pan, decay, mode,
//! choke group, velocity and effect chain apply exactly as for a sample. Three module knobs
//! shape the sound, each meaning something module specific:
//!
//! | Module | `body` | `snap` | `sweep` |
//! |---|---|---|---|
//! | `kick` | saturation of the sine | click | depth of the pitch fall |
//! | `snare` | level of the shell (three modes) | level of the noise | (unused) |
//! | `hat` | level of the metallic modes | level of the high noise | (unused) |
//! | `tom` | level of the second partial | click | depth of the pitch fall |
//! | `tone` | upper partials | click | depth of the pitch fall |
//! | `noise` | filter cutoff | filter resonance | depth of the cutoff fall |
//!
//! The base pitch of a module is fixed (kick 52 Hz, tom 110 Hz, tone middle C, the snare and hat
//! modes around 190 Hz and 2.4 kHz) and `padN.tune`, `padN.fine`, the master tune and the pitch CV
//! move it. The sines and the noise filter follow tune and CV live; the resonator modes of the
//! snare and the hat take the tune at the hit.

use atomik_dsp::{FilterMode, Modal, Rng, Svf, exp2_det, sin_cycles};

use super::Pad;

/// What a pad plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Module {
    /// The zones of the pad's key (the sampled drum machine).
    Sample,
    Kick,
    Snare,
    Hat,
    Tom,
    Tone,
    Noise,
}

/// The choice names of `padN.module`, in index order.
pub(super) const NAMES: [&str; 7] = ["sample", "kick", "snare", "hat", "tom", "tone", "noise"];

impl Module {
    /// The module of a stored `padN.module` value (rounded, clamped).
    pub fn from_value(v: f32) -> Self {
        let i = if v.is_nan() {
            0
        } else {
            v.round().clamp(0.0, 6.0) as u8
        };
        match i {
            1 => Self::Kick,
            2 => Self::Snare,
            3 => Self::Hat,
            4 => Self::Tom,
            5 => Self::Tone,
            6 => Self::Noise,
            _ => Self::Sample,
        }
    }

    pub fn is_synth(self) -> bool {
        self != Self::Sample
    }
}

const LOG2_E: f32 = std::f32::consts::LOG2_E;
const KICK_HZ: f64 = 52.0;
const TOM_HZ: f64 = 110.0;
const TONE_HZ: f64 = 261.625_565;
const SNARE_HZ: f32 = 190.0;
const SNARE_RATIOS: [f32; 3] = [1.0, 1.84, 2.38];
const SNARE_GAINS: [f32; 3] = [1.0, 0.7, 0.5];
const SNARE_NOISE_HZ: f32 = 4_500.0;
const HAT_HZ: f32 = 2_400.0;
const HAT_RATIOS: [f32; 6] = [1.0, 1.447, 1.617, 1.927, 2.503, 2.664];
const HAT_NOISE_HZ: f32 = 7_000.0;
const NOISE_BASE_HZ: f32 = 200.0;
/// Seconds of the noise burst that excites the resonators and of the click.
const BURST_S: f32 = 0.001_5;
const CLICK_S: f32 = 0.003;
/// Excitation gains of the resonator banks (calibrated so default hits peak near 0.7).
const SNARE_EXCITE: f32 = 90.0;
const HAT_EXCITE: f32 = 70.0;
/// Samples between coefficient updates of the noise filter.
const FILTER_STRIDE: u8 = 8;

/// `2^(-1 / (seconds * sr))`: the per-sample factor of an exponential fall with time constant
/// `seconds` (deterministic).
fn fall(seconds: f32, sr: f32) -> f32 {
    exp2_det(-LOG2_E / (seconds * sr))
}

/// `x / (1 + |x|)`.
fn sat(x: f32) -> f32 {
    x / (1.0 + x.abs())
}

/// One voice's synthesis state.
#[derive(Debug, Clone)]
pub(super) struct SynthVoice {
    module: Module,
    phase: f64,
    sweep: f32,
    sweep_coef: f32,
    burst: f32,
    burst_coef: f32,
    click: f32,
    click_coef: f32,
    rng: Rng,
    modal: Modal,
    filter: Svf,
    tick: u8,
}

impl SynthVoice {
    pub fn new() -> Self {
        Self {
            module: Module::Sample,
            phase: 0.0,
            sweep: 0.0,
            sweep_coef: 0.0,
            burst: 0.0,
            burst_coef: 0.0,
            click: 0.0,
            click_coef: 0.0,
            rng: Rng::new(0),
            modal: Modal::new(48_000.0),
            filter: Svf::new(),
            tick: 0,
        }
    }

    /// Starts a hit of `module` on `pad` (`ratio` is the tune at the hit, `seed` seeds the noise).
    pub fn start(&mut self, module: Module, pad: &Pad, ratio: f64, seed: u64, sr: f32) {
        self.module = module;
        self.phase = 0.0;
        self.sweep = 1.0;
        let tau = match module {
            Module::Kick => 0.025,
            Module::Tom => 0.04,
            Module::Tone => 0.08,
            _ => 0.05,
        };
        self.sweep_coef = fall(tau, sr);
        self.burst = 1.0;
        self.burst_coef = fall(BURST_S, sr);
        self.click = 1.0;
        self.click_coef = fall(CLICK_S, sr);
        self.rng = Rng::new(seed);
        self.tick = 0;
        self.filter.reset();
        self.modal = Modal::new(sr);
        let ratio = ratio as f32;
        let rt60 = (pad.decay_ms * 0.001).clamp(0.01, 10.0);
        match module {
            Module::Snare => {
                for (i, (r, g)) in SNARE_RATIOS.iter().zip(SNARE_GAINS).enumerate() {
                    self.modal.set_mode(i, SNARE_HZ * r * ratio, rt60 * 0.5, g);
                }
                self.modal.set_active(SNARE_RATIOS.len());
                self.set_filter(SNARE_NOISE_HZ, 0.15, sr);
            }
            Module::Hat => {
                for (i, r) in HAT_RATIOS.iter().enumerate() {
                    self.modal.set_mode(i, HAT_HZ * r * ratio, rt60 * 0.8, 1.0);
                }
                self.modal.set_active(HAT_RATIOS.len());
                self.set_filter(HAT_NOISE_HZ, 0.1, sr);
            }
            _ => {}
        }
    }

    fn set_filter(&mut self, hz: f32, resonance: f32, sr: f32) {
        self.filter.set_coefficients(
            Svf::cutoff_to_g_det(hz, sr),
            Svf::resonance_to_k_det(resonance),
        );
    }

    /// The next sample before the amplitude envelope. `ratio` is the live pitch ratio.
    pub fn next(&mut self, pad: &Pad, ratio: f64, sr: f32) -> f32 {
        let sr64 = f64::from(sr);
        let (body, snap, sweep) = (pad.body, pad.snap, pad.sweep);
        let env = self.sweep;
        self.sweep *= self.sweep_coef;
        let burst = self.burst;
        self.burst *= self.burst_coef;
        let click = self.click;
        self.click *= self.click_coef;
        let noise = self.rng.next_bipolar();
        let out = match self.module {
            Module::Sample => 0.0,
            Module::Kick => {
                let s = self.sine(KICK_HZ * ratio * f64::from(1.0 + sweep * 7.0 * env), sr64);
                let drive = 1.0 + body * 3.0;
                sat(s * drive) / sat(drive) + noise * click * snap * 0.6
            }
            Module::Tom => {
                let f = TOM_HZ * ratio * f64::from(1.0 + sweep * 2.5 * env);
                let s = self.sine(f, sr64);
                let s2 = sin_cycles(2.0 * self.phase) as f32;
                (0.8 * s + 0.4 * body * s2) + noise * click * snap * 0.5
            }
            Module::Tone => {
                let s = self.sine(TONE_HZ * ratio * f64::from(1.0 + sweep * env), sr64);
                let s2 = sin_cycles(2.0 * self.phase) as f32;
                let s3 = sin_cycles(3.0 * self.phase) as f32;
                (s + body * 0.5 * s2 + body * 0.33 * s3) / (1.0 + body * 0.83)
                    + noise * click * snap * 0.4
            }
            Module::Snare => {
                let (l, r) = self.modal.process(noise * burst * SNARE_EXCITE);
                let n = self.filter.process_mode(noise, FilterMode::Bandpass);
                (l + r) * body + n * snap * 1.5
            }
            Module::Hat => {
                let (l, r) = self.modal.process(noise * burst * HAT_EXCITE);
                let n = self.filter.process_mode(noise, FilterMode::Highpass);
                (l + r) * body + n * snap * 1.2
            }
            Module::Noise => {
                if self.tick == 0 {
                    let hz = NOISE_BASE_HZ * exp2_det(body * 6.0 + sweep * 3.0 * env);
                    self.set_filter(hz, snap * 0.6, sr);
                }
                self.tick = (self.tick + 1) % FILTER_STRIDE;
                sat(self.filter.process_mode(noise, FilterMode::Lowpass) * 1.5) * 1.5
            }
        };
        if out.is_finite() {
            out.clamp(-2.0, 2.0)
        } else {
            0.0
        }
    }

    /// Advances the main sine at `hz` and returns it.
    fn sine(&mut self, hz: f64, sr: f64) -> f32 {
        self.phase += hz / sr;
        self.phase -= self.phase.floor();
        sin_cycles(self.phase) as f32
    }
}
