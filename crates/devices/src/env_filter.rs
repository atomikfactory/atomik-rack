//! `atomik.env-filter`: a state-variable filter whose cutoff is swept by an ADSR envelope that a
//! gate starts.
//!
//! # Signal flow
//!
//! ```text
//! gate:   gate_cv_in (high above 0.5, low below 0.3) OR any held note on notes_in
//!         (trigger = gate/notes)           or the input level (peak, 10 ms fall) against
//!                                          `threshold`, re-arming 6 dB lower (trigger = input)
//! gate -> ADSR (attack, decay, sustain, release) -> e in 0..1  (also on env_cv_out)
//! cutoff = cutoff_knob * 2^(amount * e)                 (amount in octaves, -6..+6)
//! in -> SVF (mode, resonance) at that cutoff -> mix with the dry input -> out
//! ```
//!
//! * **Filter.** The 12 dB/octave zero-delay-feedback filter of [`atomik_dsp::Svf`]
//!   (`resonance` 0..1 maps to Q 0.5..20); `notch` is the sum of its low-pass and high-pass
//!   outputs. The cutoff is clamped to 20 Hz..0.49 of the sample rate. The coefficients are
//!   refreshed every 8 samples from the envelope at that moment; the exponent is the
//!   deterministic `exp2_det`.
//! * **Envelope.** The synth ADSR of [`atomik_dsp::Adsr`]. A new gate while the envelope is
//!   running restarts the attack from the current level. With no gate the envelope is 0 and the
//!   cutoff is the knob.
//! * **Notes.** Note-ons hold the gate (a table of 128 keys), note-offs and `AllOff` release it
//!   once no key is held; the gate follows the event's sample offset.
//! * **`mix`** blends the filtered signal with the dry input (1 = filtered only; at 1 the dry path
//!   is skipped).
//! * **`env_cv_out`** is the envelope level 0..1, one value per sample.
//! * No latency. Non-finite input samples count as 0; allocation-free.
//!
//! The step-gate variant (a pattern that gates the envelope on the song grid) is not part of the
//! device: the note players and the CV sequencer already produce such gates for `gate_cv_in`.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{audio_in, audio_out, choice, cv_in, cv_out, expo, finite, linear, sock};
use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{Adsr, Svf, exp2_det, exp2_det_f64};

const P_MODE: u32 = 0;
const P_CUTOFF: u32 = 1;
const P_RESONANCE: u32 = 2;
const P_AMOUNT: u32 = 3;
const P_ATTACK: u32 = 4;
const P_DECAY: u32 = 5;
const P_SUSTAIN: u32 = 6;
const P_RELEASE: u32 = 7;
const P_TRIGGER: u32 = 8;
const P_THRESHOLD: u32 = 9;
const P_MIX: u32 = 10;

static MODES: [&str; 4] = ["lowpass", "bandpass", "highpass", "notch"];
static TRIGGERS: [&str; 2] = ["gate or notes", "input level"];

/// Samples between filter coefficient refreshes.
const UPDATE_EVERY: usize = 8;
/// Gate CV thresholds (Schmitt trigger).
const CV_HIGH: f32 = 0.5;
const CV_LOW: f32 = 0.3;
/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The input-level follower's fall time, ms.
const FOLLOW_FALL_MS: f32 = 10.0;
/// The input trigger re-arms this far under the threshold, dB.
const REARM_DB: f32 = 6.0;

static PARAMS: [ParamDescriptor; 12] = [
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    expo(
        P_CUTOFF,
        "cutoff",
        "Cutoff",
        20.0,
        20_000.0,
        400.0,
        Unit::Hz,
    ),
    linear(
        P_RESONANCE,
        "resonance",
        "Resonance",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    linear(P_AMOUNT, "amount", "Amount", -6.0, 6.0, 3.0, Unit::Octaves),
    expo(P_ATTACK, "attack", "Attack", 0.1, 2_000.0, 5.0, Unit::Ms),
    expo(P_DECAY, "decay", "Decay", 1.0, 5_000.0, 250.0, Unit::Ms),
    linear(
        P_SUSTAIN,
        "sustain",
        "Sustain",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    expo(
        P_RELEASE,
        "release",
        "Release",
        1.0,
        5_000.0,
        250.0,
        Unit::Ms,
    ),
    choice(P_TRIGGER, "trigger", "Trigger", &TRIGGERS, 0.0),
    linear(
        P_THRESHOLD,
        "threshold",
        "Threshold",
        -60.0,
        0.0,
        -30.0,
        Unit::Db,
    ),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    bypass_param(11),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const NOTES: SocketId = SocketId(2);
const GATE_CV: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const ENV_CV: SocketId = SocketId(6);

static SOCKETS: [SocketDescriptor; 7] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    sock(
        2,
        "notes_in",
        "Notes in",
        SocketKind::Notes,
        Direction::Input,
    ),
    cv_in(3, "gate_cv_in", "Gate CV"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
    cv_out(6, "env_cv_out", "Envelope CV"),
];

/// Static description of the envelope filter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.env-filter",
    name: "Envelope Filter",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`EnvFilter`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(EnvFilter::new())),
    }
}

/// The envelope filter device.
#[derive(Debug, Clone)]
pub struct EnvFilter {
    sample_rate: f32,
    mode: u8,
    cutoff: f32,
    resonance: f32,
    amount: f32,
    from_input: bool,
    threshold_db: f32,
    mix: f32,
    // Derived.
    thr_lin: f32,
    fall_alpha: f64,
    // State.
    adsr: Adsr,
    filters: [Svf; 2],
    held: [bool; 128],
    held_count: u32,
    cv_high: bool,
    follow: f32,
    input_high: bool,
    gate: bool,
}

impl Default for EnvFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvFilter {
    /// An envelope filter with default params.
    pub fn new() -> Self {
        let mut f = Self {
            sample_rate: 48_000.0,
            mode: 0,
            cutoff: 400.0,
            resonance: 0.3,
            amount: 3.0,
            from_input: false,
            threshold_db: -30.0,
            mix: 1.0,
            thr_lin: 0.0,
            fall_alpha: 0.0,
            adsr: Adsr::new(48_000.0),
            filters: [Svf::new(); 2],
            held: [false; 128],
            held_count: 0,
            cv_high: false,
            follow: 0.0,
            input_high: false,
            gate: false,
        };
        f.adsr.set_attack_ms(5.0);
        f.adsr.set_decay_ms(250.0);
        f.adsr.set_sustain(0.0);
        f.adsr.set_release_ms(250.0);
        f.derive();
        f
    }

    fn derive(&mut self) {
        self.thr_lin = exp2_det(self.threshold_db * LOG2_10_20);
        let n = f64::from(FOLLOW_FALL_MS) * 0.001 * f64::from(self.sample_rate);
        self.fall_alpha = exp2_det_f64(-std::f64::consts::LOG2_E / n);
    }

    fn note_on(&mut self, key: u8) {
        let k = usize::from(key.min(127));
        if !self.held[k] {
            self.held[k] = true;
            self.held_count += 1;
        }
    }

    fn note_off(&mut self, key: u8) {
        let k = usize::from(key.min(127));
        if self.held[k] {
            self.held[k] = false;
            self.held_count -= 1;
        }
    }

    fn set_gate(&mut self, on: bool) {
        if on != self.gate {
            self.gate = on;
            if on {
                self.adsr.gate_on();
            } else {
                self.adsr.gate_off();
            }
        }
    }

    /// Refreshes the filter coefficients for envelope level `e`.
    fn refresh(&mut self, e: f32) {
        let hz = self.cutoff * exp2_det(self.amount * e);
        let g = Svf::cutoff_to_g(hz, self.sample_rate);
        let k = 1.0 / Svf::resonance_to_q(self.resonance);
        for f in &mut self.filters {
            f.set_coefficients(g, k);
        }
    }
}

impl Device for EnvFilter {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.adsr.set_sample_rate(self.sample_rate);
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.adsr.reset();
        for f in &mut self.filters {
            f.reset();
        }
        self.held = [false; 128];
        self.held_count = 0;
        self.cv_high = false;
        self.follow = 0.0;
        self.input_high = false;
        self.gate = false;
        self.refresh(0.0);
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 3.0).round() as u8,
            P_CUTOFF => self.cutoff = v.clamp(20.0, 20_000.0),
            P_RESONANCE => self.resonance = v.clamp(0.0, 1.0),
            P_AMOUNT => self.amount = v.clamp(-6.0, 6.0),
            P_ATTACK => self.adsr.set_attack_ms(v.clamp(0.1, 2_000.0)),
            P_DECAY => self.adsr.set_decay_ms(v.clamp(1.0, 5_000.0)),
            P_SUSTAIN => self.adsr.set_sustain(v.clamp(0.0, 1.0)),
            P_RELEASE => self.adsr.set_release_ms(v.clamp(1.0, 5_000.0)),
            P_TRIGGER => self.from_input = v >= 0.5,
            P_THRESHOLD => {
                self.threshold_db = v.clamp(-60.0, 0.0);
                self.derive();
            }
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let gate_cv = *ctx.input(GATE_CV);
        let cv_conn = ctx.is_connected(GATE_CV);
        let rearm = self.thr_lin * exp2_det(-REARM_DB * LOG2_10_20);

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut env_cv = [0.0f32; BATCH];
        let mut next_event = 0usize;
        for n in 0..BATCH {
            // Notes at this sample.
            let events = ctx.notes_in(NOTES);
            while next_event < events.len() && usize::from(events[next_event].offset) <= n {
                match events[next_event].kind {
                    NoteKind::On { key, .. } => self.note_on(key),
                    NoteKind::Off { key } => self.note_off(key),
                    NoteKind::AllOff => {
                        self.held = [false; 128];
                        self.held_count = 0;
                    }
                    _ => {}
                }
                next_event += 1;
            }
            let x = [finite(in_l[n]), finite(in_r[n])];
            let gate = if self.from_input {
                let k = x[0].abs().max(x[1].abs());
                self.follow = k.max((f64::from(self.follow) * self.fall_alpha) as f32);
                if self.follow < 1e-20 {
                    self.follow = 0.0;
                }
                if !self.input_high && self.follow >= self.thr_lin {
                    self.input_high = true;
                } else if self.input_high && self.follow < rearm {
                    self.input_high = false;
                }
                self.input_high
            } else {
                if cv_conn {
                    let v = finite(gate_cv[n]);
                    if !self.cv_high && v >= CV_HIGH {
                        self.cv_high = true;
                    } else if self.cv_high && v < CV_LOW {
                        self.cv_high = false;
                    }
                } else {
                    self.cv_high = false;
                }
                self.cv_high || self.held_count > 0
            };
            self.set_gate(gate);
            let e = self.adsr.next_sample();
            env_cv[n] = e;
            if n % UPDATE_EVERY == 0 {
                self.refresh(e);
            }
            let mut y = [0.0f32; 2];
            for c in 0..2 {
                let o = self.filters[c].process(x[c]);
                let wet = match self.mode {
                    0 => o.lp,
                    1 => o.bp,
                    2 => o.hp,
                    _ => o.lp + o.hp,
                };
                y[c] = if self.mix >= 1.0 {
                    wet
                } else {
                    x[c] * (1.0 - self.mix) + wet * self.mix
                };
            }
            out_l[n] = y[0];
            out_r[n] = y[1];
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(ENV_CV) = env_cv;
    }
}

#[cfg(test)]
mod tests;
