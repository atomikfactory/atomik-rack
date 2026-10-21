//! `atomik.poly-synth`: 16-voice subtractive synthesizer (03 §4.1).
//!
//! Per voice: two PolyBLEP oscillators (crossfaded by `osc.mix`) plus white noise, a TPT
//! state-variable filter (LP/BP/HP 12 dB/oct), an amp ADSR and a filter ADSR. One global LFO
//! modulates pitch and cutoff and is written to `lfo_cv_out` every sample.
//!
//! * Voice states follow the amp envelope: Idle → Attack → Decay → Sustain → Release → Idle.
//! * Same-key note-on retriggers that voice (continuously from its current level). When every
//!   allowed voice is busy, the oldest started voice is stolen: it fades out linearly over 2 ms
//!   and then starts the new note from silence. `AllOff` releases every voice.
//! * Amp gain = envelope · (1 − velocity.amp·(1 − velocity)); velocity changes on retrigger glide
//!   over 2 ms.
//! * Cutoff = `filter.cutoff · 2^(env_amount·6·filter_env + key_track·(key − 60)/12 +
//!   lfo_to_cutoff·lfo + cv.cutoff_amount·cv)`, clamped to 20 Hz..0.45·sr. It is evaluated
//!   every 16 samples per voice (as is the pitch) and the filter coefficient is interpolated
//!   linearly in between.
//! * Output: sum of voices · 0.5 · master gain (5 ms smoothing), then a soft limiter that is
//!   linear up to ±2 and saturates smoothly towards ±4, which guarantees |x| < 4 for every param
//!   combination. There is no stereo spread in this slice, so `out_l == out_r`.
//! * Randomness (noise, sample & hold LFO) comes from `Rng`s derived from `PrepareInfo::seed`.

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{
    Adsr, AdsrStage, FilterMode, Lfo, LfoShape, PolyBlepOsc, Rng, Smoother, Svf, Waveform,
    balance_gains, note_to_hz,
};

use crate::cv::{clean, db_gain};

/// Number of preallocated voices.
pub const MAX_VOICES: usize = 16;

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    range: (f32, f32, f32),
    taper: Taper,
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
        taper,
        unit,
        discrete,
    }
}

const OSC_WAVES: Unit = Unit::Choice(&["saw", "square", "triangle", "sine"]);
const FILTER_MODES: Unit = Unit::Choice(&["LP12", "BP12", "HP12"]);
const LFO_WAVES: Unit = Unit::Choice(&["sine", "triangle", "saw", "square", "sample&hold"]);
const ENV_MS: (f32, f32) = (0.5, 10_000.0);

const P_OSC1_WAVE: u32 = 0;
const P_OSC2_WAVE: u32 = 1;
const P_OSC1_OCTAVE: u32 = 2;
const P_OSC2_OCTAVE: u32 = 3;
const P_OSC1_SEMI: u32 = 4;
const P_OSC2_SEMI: u32 = 5;
const P_OSC1_FINE: u32 = 6;
const P_OSC2_FINE: u32 = 7;
const P_OSC_MIX: u32 = 8;
const P_NOISE: u32 = 9;
const P_FILTER_MODE: u32 = 10;
const P_CUTOFF: u32 = 11;
const P_RESONANCE: u32 = 12;
const P_ENV_AMOUNT: u32 = 13;
const P_KEY_TRACK: u32 = 14;
const P_AMP_A: u32 = 15;
const P_AMP_D: u32 = 16;
const P_AMP_R: u32 = 17;
const P_AMP_S: u32 = 18;
const P_FLT_A: u32 = 19;
const P_FLT_D: u32 = 20;
const P_FLT_R: u32 = 21;
const P_FLT_S: u32 = 22;
const P_LFO_WAVE: u32 = 23;
const P_LFO_RATE: u32 = 24;
const P_LFO_PITCH: u32 = 25;
const P_LFO_CUTOFF: u32 = 26;
const P_VEL_AMP: u32 = 27;
const P_CV_AMOUNT: u32 = 28;
const P_VOICES: u32 = 29;
const P_MASTER: u32 = 30;
const P_CV_PITCH: u32 = 31;
const P_CV_RESONANCE: u32 = 32;
const P_CV_LEVEL: u32 = 33;
const P_CV_PAN: u32 = 34;
const PARAM_COUNT: usize = 35;

use Taper::{Decibel as Db, Exponential as Exp, Linear as Lin};

static PARAMS: [ParamDescriptor; PARAM_COUNT] = [
    param(
        P_OSC1_WAVE,
        "osc1.wave",
        "Osc 1 wave",
        (0.0, 3.0, 0.0),
        Lin,
        OSC_WAVES,
        true,
    ),
    param(
        P_OSC2_WAVE,
        "osc2.wave",
        "Osc 2 wave",
        (0.0, 3.0, 1.0),
        Lin,
        OSC_WAVES,
        true,
    ),
    param(
        P_OSC1_OCTAVE,
        "osc1.octave",
        "Osc 1 octave",
        (-3.0, 3.0, 0.0),
        Lin,
        Unit::Octaves,
        true,
    ),
    param(
        P_OSC2_OCTAVE,
        "osc2.octave",
        "Osc 2 octave",
        (-3.0, 3.0, 0.0),
        Lin,
        Unit::Octaves,
        true,
    ),
    param(
        P_OSC1_SEMI,
        "osc1.semi",
        "Osc 1 semi",
        (-12.0, 12.0, 0.0),
        Lin,
        Unit::Semitones,
        true,
    ),
    param(
        P_OSC2_SEMI,
        "osc2.semi",
        "Osc 2 semi",
        (-12.0, 12.0, 0.0),
        Lin,
        Unit::Semitones,
        true,
    ),
    param(
        P_OSC1_FINE,
        "osc1.fine",
        "Osc 1 fine",
        (-50.0, 50.0, 0.0),
        Lin,
        Unit::Cents,
        false,
    ),
    param(
        P_OSC2_FINE,
        "osc2.fine",
        "Osc 2 fine",
        (-50.0, 50.0, 7.0),
        Lin,
        Unit::Cents,
        false,
    ),
    param(
        P_OSC_MIX,
        "osc.mix",
        "Osc mix",
        (0.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_NOISE,
        "noise.level",
        "Noise",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_FILTER_MODE,
        "filter.mode",
        "Filter mode",
        (0.0, 2.0, 0.0),
        Lin,
        FILTER_MODES,
        true,
    ),
    param(
        P_CUTOFF,
        "filter.cutoff",
        "Cutoff",
        (20.0, 20_000.0, 4_000.0),
        Exp,
        Unit::Hz,
        false,
    ),
    param(
        P_RESONANCE,
        "filter.resonance",
        "Resonance",
        (0.0, 1.0, 0.2),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_ENV_AMOUNT,
        "filter.env_amount",
        "Filter env amount",
        (-1.0, 1.0, 0.3),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_KEY_TRACK,
        "filter.key_track",
        "Key tracking",
        (0.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_AMP_A,
        "env.amp.attack",
        "Amp attack",
        (ENV_MS.0, ENV_MS.1, 2.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_AMP_D,
        "env.amp.decay",
        "Amp decay",
        (ENV_MS.0, ENV_MS.1, 200.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_AMP_R,
        "env.amp.release",
        "Amp release",
        (ENV_MS.0, ENV_MS.1, 300.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_AMP_S,
        "env.amp.sustain",
        "Amp sustain",
        (0.0, 1.0, 0.7),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_FLT_A,
        "env.filter.attack",
        "Filter attack",
        (ENV_MS.0, ENV_MS.1, 2.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_FLT_D,
        "env.filter.decay",
        "Filter decay",
        (ENV_MS.0, ENV_MS.1, 400.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_FLT_R,
        "env.filter.release",
        "Filter release",
        (ENV_MS.0, ENV_MS.1, 300.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_FLT_S,
        "env.filter.sustain",
        "Filter sustain",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_LFO_WAVE,
        "lfo.wave",
        "LFO wave",
        (0.0, 4.0, 0.0),
        Lin,
        LFO_WAVES,
        true,
    ),
    param(
        P_LFO_RATE,
        "lfo.rate",
        "LFO rate",
        (0.05, 30.0, 2.0),
        Exp,
        Unit::Hz,
        false,
    ),
    param(
        P_LFO_PITCH,
        "lfo.to_pitch",
        "LFO to pitch",
        (0.0, 12.0, 0.0),
        Lin,
        Unit::Semitones,
        false,
    ),
    param(
        P_LFO_CUTOFF,
        "lfo.to_cutoff",
        "LFO to cutoff",
        (0.0, 4.0, 0.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_VEL_AMP,
        "velocity.amp",
        "Velocity to amp",
        (0.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_CV_AMOUNT,
        "cv.cutoff_amount",
        "CV to cutoff",
        (0.0, 4.0, 2.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_VOICES,
        "voices",
        "Voices",
        (1.0, 16.0, 16.0),
        Lin,
        Unit::None,
        true,
    ),
    param(
        P_MASTER,
        "master.level",
        "Master level",
        (-60.0, 6.0, -6.0),
        Db,
        Unit::Db,
        false,
    ),
    param(
        P_CV_PITCH,
        "cv.pitch_amount",
        "CV to pitch",
        (-24.0, 24.0, 12.0),
        Lin,
        Unit::Semitones,
        false,
    ),
    param(
        P_CV_RESONANCE,
        "cv.resonance_amount",
        "CV to resonance",
        (-1.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_CV_LEVEL,
        "cv.level_amount",
        "CV to level",
        (-24.0, 24.0, 12.0),
        Lin,
        Unit::Db,
        false,
    ),
    param(
        P_CV_PAN,
        "cv.pan_amount",
        "CV to pan",
        (-1.0, 1.0, 1.0),
        Lin,
        Unit::None,
        false,
    ),
];

const NOTES_IN: SocketId = SocketId(0);
const CUTOFF_CV_IN: SocketId = SocketId(1);
const PITCH_CV_IN: SocketId = SocketId(2);
const RESONANCE_CV_IN: SocketId = SocketId(3);
const LEVEL_CV_IN: SocketId = SocketId(4);
const PAN_CV_IN: SocketId = SocketId(5);
const OUT_L: SocketId = SocketId(6);
const OUT_R: SocketId = SocketId(7);
const LFO_CV_OUT: SocketId = SocketId(8);

const fn socket(
    id: SocketId,
    key: &'static str,
    name: &'static str,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id,
        key,
        name,
        kind,
        direction,
    }
}

static SOCKETS: [SocketDescriptor; 9] = [
    socket(
        NOTES_IN,
        "notes_in",
        "Notes in",
        SocketKind::Notes,
        Direction::Input,
    ),
    socket(
        CUTOFF_CV_IN,
        "cutoff_cv_in",
        "Cutoff CV",
        SocketKind::Cv,
        Direction::Input,
    ),
    socket(
        PITCH_CV_IN,
        "pitch_cv_in",
        "Pitch CV",
        SocketKind::Cv,
        Direction::Input,
    ),
    socket(
        RESONANCE_CV_IN,
        "resonance_cv_in",
        "Resonance CV",
        SocketKind::Cv,
        Direction::Input,
    ),
    socket(
        LEVEL_CV_IN,
        "level_cv_in",
        "Level CV",
        SocketKind::Cv,
        Direction::Input,
    ),
    socket(
        PAN_CV_IN,
        "pan_cv_in",
        "Pan CV",
        SocketKind::Cv,
        Direction::Input,
    ),
    socket(
        OUT_L,
        "out_l",
        "Out L",
        SocketKind::Audio,
        Direction::Output,
    ),
    socket(
        OUT_R,
        "out_r",
        "Out R",
        SocketKind::Audio,
        Direction::Output,
    ),
    socket(
        LFO_CV_OUT,
        "lfo_cv_out",
        "LFO CV",
        SocketKind::Cv,
        Direction::Output,
    ),
];

/// Static description of the poly synth.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.poly-synth",
    name: "Poly Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`PolySynth`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(PolySynth::new()),
    }
}

/// Samples between control-rate updates (pitch, cutoff) of a voice.
const CONTROL_INTERVAL: u32 = 16;
/// Per-voice output gain before the master stage.
const VOICE_GAIN: f32 = 0.5;
/// Glide time for velocity changes on retrigger.
const VELOCITY_GLIDE_MS: f32 = 2.0;
/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Highest filter cutoff as a fraction of the sample rate.
const MAX_CUTOFF_RATIO: f32 = 0.45;

/// Soft limiter: linear for |x| <= 2, then `2 + 2·tanh((|x| − 2)/2)`, so |y| < 4 always.
/// Non-finite input yields 0 (NaN) or the bound (infinities).
#[inline]
pub(crate) fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 2.0 {
        x
    } else if a.is_finite() {
        (2.0 + 2.0 * ((a - 2.0) * 0.5).tanh()).copysign(x)
    } else if x.is_nan() {
        0.0
    } else {
        4.0f32.copysign(x)
    }
}

/// Shared (per-device) synthesis parameters, as plain values.
#[derive(Debug, Clone, Copy)]
struct Shared {
    sample_rate: f32,
    pitch_offset: [f32; 2],
    osc_mix: f32,
    noise: f32,
    filter_mode: FilterMode,
    cutoff: f32,
    k: f32,
    /// Normalised resonance (the base of the resonance CV).
    resonance: f32,
    env_amount: f32,
    key_track: f32,
    lfo_to_pitch: f32,
    lfo_to_cutoff: f32,
    velocity_amount: f32,
    cv_amount: f32,
    cv_pitch_amount: f32,
    cv_resonance_amount: f32,
    /// Set per batch: the resonance CV socket is cabled.
    cv_resonance_on: bool,
}

/// Control-rate modulation values read by a voice.
#[derive(Debug, Clone, Copy)]
struct Ctl {
    lfo: f32,
    cutoff: f32,
    pitch: f32,
    resonance: f32,
}

/// Per-sample modulation sources of one render segment (all aligned with the output slice).
struct Mods<'a> {
    lfo: &'a [f32],
    cutoff: &'a [f32],
    pitch: &'a [f32],
    resonance: &'a [f32],
}

/// One preallocated voice.
#[derive(Debug, Clone)]
struct Voice {
    key: u8,
    order: u64,
    pending: Option<(u8, f32)>,
    osc: [PolyBlepOsc; 2],
    filter: Svf,
    amp: Adsr,
    fenv: Adsr,
    noise: Rng,
    vel_gain: Smoother,
    countdown: u32,
    g: f32,
    g_target: f32,
    g_step: f32,
    snap_g: bool,
    /// Filter damping while the resonance CV is cabled (`k` ramps like `g`).
    k: f32,
    k_target: f32,
    k_step: f32,
}

impl Voice {
    fn new(seed: u64) -> Self {
        Self {
            key: 0,
            order: 0,
            pending: None,
            osc: [PolyBlepOsc::new(Waveform::Saw); 2],
            filter: Svf::new(),
            amp: Adsr::new(48_000.0),
            fenv: Adsr::new(48_000.0),
            noise: Rng::new(seed),
            vel_gain: Smoother::new(0.0, VELOCITY_GLIDE_MS, 48_000.0),
            countdown: 0,
            g: 0.0,
            g_target: 0.0,
            g_step: 0.0,
            snap_g: true,
            k: 1.0,
            k_target: 1.0,
            k_step: 0.0,
        }
    }

    fn is_active(&self) -> bool {
        self.amp.is_active()
    }

    /// The key this voice is playing (or will play after its steal fade), if any.
    fn sounding_key(&self) -> Option<u8> {
        if let Some((k, _)) = self.pending {
            Some(k)
        } else if self.is_active() && self.amp.stage() != AdsrStage::Fade {
            Some(self.key)
        } else {
            None
        }
    }

    fn velocity_gain(p: &Shared, velocity: f32) -> f32 {
        1.0 - p.velocity_amount * (1.0 - velocity.clamp(0.0, 1.0))
    }

    /// Starts a note from silence (fresh oscillators and filter).
    fn start(&mut self, key: u8, velocity: f32, p: &Shared) {
        self.key = key;
        self.pending = None;
        for o in &mut self.osc {
            o.set_phase(0.0);
        }
        self.filter.reset();
        self.amp.reset();
        self.fenv.reset();
        self.amp.gate_on();
        self.fenv.gate_on();
        self.vel_gain
            .set_immediate(Self::velocity_gain(p, velocity));
        self.countdown = 0;
        self.snap_g = true;
    }

    /// Same-key retrigger: envelopes restart from their current levels.
    fn retrigger(&mut self, velocity: f32, p: &Shared) {
        self.amp.gate_on();
        self.fenv.gate_on();
        self.vel_gain.set_target(Self::velocity_gain(p, velocity));
    }

    fn release(&mut self) {
        self.pending = None;
        self.amp.gate_off();
        self.fenv.gate_off();
    }

    /// Control-rate update: oscillator pitch and the filter coefficient target.
    fn control(&mut self, c: Ctl, p: &Shared) {
        let (lfo, cv) = (c.lfo, c.cutoff);
        let key = f32::from(self.key);
        let cv_pitch = p.cv_pitch_amount * clean(c.pitch);
        for (o, offset) in self.osc.iter_mut().zip(p.pitch_offset) {
            o.set_frequency(
                note_to_hz(key + offset + lfo * p.lfo_to_pitch + cv_pitch, 0.0),
                p.sample_rate,
            );
        }
        let octaves = p.env_amount * 6.0 * self.fenv.level()
            + p.key_track * (key - 60.0) / 12.0
            + p.lfo_to_cutoff * lfo
            + p.cv_amount * cv.clamp(-1.0, 1.0);
        let cutoff = (p.cutoff * octaves.exp2()).clamp(20.0, MAX_CUTOFF_RATIO * p.sample_rate);
        let g = Svf::cutoff_to_g(cutoff, p.sample_rate);
        // Land exactly on the previous target (no drift), then ramp towards the new one.
        self.g = if self.snap_g { g } else { self.g_target };
        self.g_target = g;
        self.g_step = (g - self.g) / CONTROL_INTERVAL as f32;
        if p.cv_resonance_on {
            let r = (p.resonance + p.cv_resonance_amount * clean(c.resonance)).clamp(0.0, 1.0);
            let k = 1.0 / Svf::resonance_to_q(r);
            self.k = if self.snap_g { k } else { self.k_target };
            self.k_target = k;
            self.k_step = (k - self.k) / CONTROL_INTERVAL as f32;
        } else {
            self.k = p.k;
            self.k_target = p.k;
            self.k_step = 0.0;
        }
        if self.snap_g || self.g_step == 0.0 {
            self.filter.set_coefficients(self.g, self.k_now(p));
            self.snap_g = false;
        }
    }

    /// The filter damping in use: the base value, or the ramped value under resonance CV.
    #[inline]
    fn k_now(&self, p: &Shared) -> f32 {
        if p.cv_resonance_on { self.k } else { p.k }
    }

    /// Adds this voice into `out`. The modulation slices are aligned with `out`.
    fn render(&mut self, out: &mut [f32], m: &Mods<'_>, p: &Shared) {
        for (n, o) in out.iter_mut().enumerate() {
            if self.countdown == 0 {
                let c = Ctl {
                    lfo: m.lfo[n],
                    cutoff: m.cutoff[n],
                    pitch: m.pitch[n],
                    resonance: m.resonance[n],
                };
                self.control(c, p);
                self.countdown = CONTROL_INTERVAL;
            }
            self.countdown -= 1;
            if p.cv_resonance_on {
                self.k += self.k_step;
            }
            if self.g_step != 0.0 || (p.cv_resonance_on && self.k_step != 0.0) {
                self.g += self.g_step;
                self.filter.set_coefficients(self.g, self.k_now(p));
            }
            let o1 = self.osc[0].next_sample();
            let o2 = self.osc[1].next_sample();
            let mut x = o1 + (o2 - o1) * p.osc_mix;
            if p.noise > 0.0 {
                x += p.noise * self.noise.next_bipolar();
            }
            let mut y = self.filter.process_mode(x, p.filter_mode);
            if !y.is_finite() {
                self.filter.reset();
                y = 0.0;
            }
            self.fenv.next_sample();
            let amp = self.amp.next_sample();
            *o += y * amp * self.vel_gain.next_value() * VOICE_GAIN;
            if !self.amp.is_active() {
                match self.pending.take() {
                    Some((key, velocity)) => self.start(key, velocity, p),
                    None => return,
                }
            }
        }
    }
}

/// Derives an independent per-stream seed from the device seed.
pub(crate) fn derive_seed(seed: u64, stream: u64) -> u64 {
    seed ^ stream.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// The poly synth device.
#[derive(Debug)]
pub struct PolySynth {
    values: [f32; PARAM_COUNT],
    shared: Shared,
    voices: [Voice; MAX_VOICES],
    voice_limit: usize,
    next_order: u64,
    lfo: Lfo,
    master: Smoother,
    cv_level_db: f32,
    cv_pan_amount: f32,
    /// Set by `prepare`/`reset`: the next `process` snaps the master gain to its target.
    fresh: bool,
    seed: u64,
    #[cfg(test)]
    probe: Option<std::sync::Arc<tests::VoiceProbe>>,
}

impl Default for PolySynth {
    fn default() -> Self {
        Self::new()
    }
}

impl PolySynth {
    /// A synth with default params (call `prepare` before processing).
    pub fn new() -> Self {
        let mut s = Self {
            values: PARAMS.map(|p| p.default),
            shared: Shared {
                sample_rate: 48_000.0,
                pitch_offset: [0.0; 2],
                osc_mix: 0.5,
                noise: 0.0,
                filter_mode: FilterMode::Lowpass,
                cutoff: 4_000.0,
                k: 1.0,
                resonance: 0.2,
                env_amount: 0.0,
                key_track: 0.0,
                lfo_to_pitch: 0.0,
                lfo_to_cutoff: 0.0,
                velocity_amount: 0.5,
                cv_amount: 2.0,
                cv_pitch_amount: 12.0,
                cv_resonance_amount: 0.5,
                cv_resonance_on: false,
            },
            voices: std::array::from_fn(|i| Voice::new(derive_seed(0, i as u64 + 1))),
            voice_limit: MAX_VOICES,
            next_order: 0,
            lfo: Lfo::new(derive_seed(0, 0)),
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            cv_level_db: 12.0,
            cv_pan_amount: 1.0,
            fresh: true,
            seed: 0,
            #[cfg(test)]
            probe: None,
        };
        s.apply_all();
        let m = s.master.target();
        s.master.set_immediate(m);
        s
    }

    fn apply_all(&mut self) {
        for i in 0..PARAM_COUNT {
            self.apply(i as u32, self.values[i]);
        }
    }

    /// Applies one stored plain value. RT-safe (no allocation).
    fn apply(&mut self, id: u32, v: f32) {
        let sr = self.shared.sample_rate;
        let choice = |v: f32| v.max(0.0) as usize;
        let voices = &mut self.voices;
        match id {
            P_OSC1_WAVE | P_OSC2_WAVE => {
                let i = (id - P_OSC1_WAVE) as usize;
                let w = Waveform::from_index(choice(v));
                voices.iter_mut().for_each(|vc| vc.osc[i].set_waveform(w));
            }
            P_OSC1_OCTAVE..=P_OSC2_FINE => {
                let at = |base: u32, i: usize| self.values[base as usize + i];
                for i in 0..2 {
                    self.shared.pitch_offset[i] = 12.0 * at(P_OSC1_OCTAVE, i)
                        + at(P_OSC1_SEMI, i)
                        + 0.01 * at(P_OSC1_FINE, i);
                }
            }
            P_OSC_MIX => self.shared.osc_mix = v,
            P_NOISE => self.shared.noise = v,
            P_FILTER_MODE => self.shared.filter_mode = FilterMode::from_index(choice(v)),
            P_CUTOFF => self.shared.cutoff = v,
            P_RESONANCE => {
                self.shared.k = 1.0 / Svf::resonance_to_q(v);
                self.shared.resonance = v;
            }
            P_ENV_AMOUNT => self.shared.env_amount = v,
            P_KEY_TRACK => self.shared.key_track = v,
            P_AMP_A => voices.iter_mut().for_each(|vc| vc.amp.set_attack_ms(v)),
            P_AMP_D => voices.iter_mut().for_each(|vc| vc.amp.set_decay_ms(v)),
            P_AMP_R => voices.iter_mut().for_each(|vc| vc.amp.set_release_ms(v)),
            P_AMP_S => voices.iter_mut().for_each(|vc| vc.amp.set_sustain(v)),
            P_FLT_A => voices.iter_mut().for_each(|vc| vc.fenv.set_attack_ms(v)),
            P_FLT_D => voices.iter_mut().for_each(|vc| vc.fenv.set_decay_ms(v)),
            P_FLT_R => voices.iter_mut().for_each(|vc| vc.fenv.set_release_ms(v)),
            P_FLT_S => voices.iter_mut().for_each(|vc| vc.fenv.set_sustain(v)),
            P_LFO_WAVE => self.lfo.set_shape(LfoShape::from_index(choice(v))),
            P_LFO_RATE => self.lfo.set_rate(v, sr),
            P_LFO_PITCH => self.shared.lfo_to_pitch = v,
            P_LFO_CUTOFF => self.shared.lfo_to_cutoff = v,
            P_VEL_AMP => self.shared.velocity_amount = v,
            P_CV_AMOUNT => self.shared.cv_amount = v,
            P_CV_PITCH => self.shared.cv_pitch_amount = v,
            P_CV_RESONANCE => self.shared.cv_resonance_amount = v,
            P_CV_LEVEL => self.cv_level_db = v,
            P_CV_PAN => self.cv_pan_amount = v,
            P_VOICES => {
                self.voice_limit = (v.round().max(1.0) as usize).min(MAX_VOICES);
                for vc in &mut voices[self.voice_limit..] {
                    vc.pending = None;
                    vc.amp.steal_fade();
                }
            }
            P_MASTER => self
                .master
                .set_target(PARAMS[P_MASTER as usize].db_param_gain(v)),
            _ => {}
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32) {
        let p = self.shared;
        let order = self.next_order;
        self.next_order += 1;
        let voices = &mut self.voices[..self.voice_limit];
        // 1. Same key sounding (or pending after a steal): retrigger that voice.
        if let Some(v) = voices.iter_mut().find(|v| v.sounding_key() == Some(key)) {
            v.order = order;
            if v.pending.is_some() {
                v.pending = Some((key, velocity));
            } else {
                v.retrigger(velocity, &p);
            }
            return;
        }
        // 2. A free voice.
        if let Some(v) = voices.iter_mut().find(|v| !v.is_active()) {
            v.order = order;
            v.start(key, velocity, &p);
            return;
        }
        // 3. Steal the oldest started voice: 2 ms fade, then the new note.
        if let Some(v) = voices.iter_mut().min_by_key(|v| v.order) {
            v.order = order;
            v.pending = Some((key, velocity));
            v.amp.steal_fade();
        }
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
            NoteKind::Off { key } => self
                .voices
                .iter_mut()
                .filter(|v| v.sounding_key() == Some(key))
                .for_each(Voice::release),
            NoteKind::AllOff => self.voices.iter_mut().for_each(Voice::release),
            // Channel messages and expressions are ignored.
            _ => {}
        }
    }

    #[cfg(test)]
    fn update_probe(&self) {
        use std::sync::atomic::Ordering;
        if let Some(probe) = &self.probe {
            let mut active = 0;
            for (slot, v) in probe.keys.iter().zip(&self.voices) {
                let k = if v.is_active() {
                    active += 1;
                    u32::from(v.pending.map_or(v.key, |(k, _)| k)) + 1
                } else {
                    0
                };
                slot.store(k, Ordering::Relaxed);
            }
            probe.active.store(active, Ordering::Relaxed);
        }
    }
}

impl Device for PolySynth {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.shared.sample_rate = sr;
        for v in &mut self.voices {
            v.amp.set_sample_rate(sr);
            v.fenv.set_sample_rate(sr);
            v.vel_gain.set_time(VELOCITY_GLIDE_MS, sr);
        }
        self.master.set_time(MASTER_SMOOTH_MS, sr);
        self.lfo = Lfo::new(derive_seed(info.seed, 0));
        self.apply_all();
        self.reset();
    }

    fn reset(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            v.amp.reset();
            v.fenv.reset();
            v.filter.reset();
            v.pending = None;
            v.noise = Rng::new(derive_seed(self.seed, i as u64 + 1));
        }
        self.next_order = 0;
        self.lfo.reset();
        let m = self.master.target();
        self.master.set_immediate(m);
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let i = id.0 as usize;
        if i < PARAM_COUNT {
            self.values[i] = plain;
            self.apply(id.0, plain);
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            let m = self.master.target();
            self.master.set_immediate(m);
            self.fresh = false;
        }
        let mut lfo = [0.0f32; BATCH];
        for s in &mut lfo {
            *s = self.lfo.next_sample();
        }
        let cv = *ctx.input(CUTOFF_CV_IN);
        let cv_pitch = *ctx.input(PITCH_CV_IN);
        let cv_res = *ctx.input(RESONANCE_CV_IN);
        self.shared.cv_resonance_on = ctx.is_connected(RESONANCE_CV_IN);
        let mut mix = [0.0f32; BATCH];
        let events = ctx.notes_in(NOTES_IN);
        let mut next = 0;
        let mut start = 0;
        // Render in segments between note events so events are sample-accurate.
        while start < BATCH {
            while next < events.len() && (events[next].offset as usize).min(BATCH - 1) <= start {
                self.handle(events[next].kind);
                next += 1;
            }
            let end = events
                .get(next)
                .map_or(BATCH, |e| (e.offset as usize).clamp(start + 1, BATCH));
            let p = self.shared;
            let m = Mods {
                lfo: &lfo[start..end],
                cutoff: &cv[start..end],
                pitch: &cv_pitch[start..end],
                resonance: &cv_res[start..end],
            };
            for v in self.voices.iter_mut().filter(|v| v.is_active()) {
                v.render(&mut mix[start..end], &m, &p);
            }
            start = end;
        }
        let level_on = ctx.is_connected(LEVEL_CV_IN);
        let cv_level = *ctx.input(LEVEL_CV_IN);
        for (s, &lv) in mix.iter_mut().zip(&cv_level) {
            let mut g = self.master.next_value();
            if level_on {
                g *= db_gain(self.cv_level_db, lv);
            }
            *s = soft_limit(*s * g);
        }
        if ctx.is_connected(PAN_CV_IN) {
            // Mono voice sum balanced by the pan CV (unity at the centre, like a stereo balance).
            let cv_pan = *ctx.input(PAN_CV_IN);
            let (mut l, mut r) = (mix, mix);
            for n in 0..BATCH {
                let (gl, gr) = balance_gains(self.cv_pan_amount * clean(cv_pan[n]));
                l[n] *= gl;
                r[n] *= gr;
            }
            *ctx.output(OUT_L) = l;
            *ctx.output(OUT_R) = r;
        } else {
            *ctx.output(OUT_L) = mix;
            *ctx.output(OUT_R) = mix;
        }
        *ctx.output(LFO_CV_OUT) = lfo;
        #[cfg(test)]
        self.update_probe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SR, assert_keys, harness, off, on, sweep_params_min_max};
    use atomik_core::NoteEvent;
    use atomik_testkit::harness::DeviceHarness;
    use atomik_testkit::signal::{db, dominant_frequency, max_step, peak, rms};
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    /// Test-only window into the voice allocator, updated at the end of every `process`
    /// (atomic stores only: no allocation).
    #[derive(Debug, Default)]
    pub struct VoiceProbe {
        /// Number of non-idle voices.
        pub active: AtomicUsize,
        /// Per voice: 0 when idle, else 1 + the key it is playing (or will play after a steal).
        pub keys: [AtomicU32; MAX_VOICES],
    }

    impl VoiceProbe {
        fn keys(&self) -> Vec<u8> {
            let mut k: Vec<u8> = self
                .keys
                .iter()
                .map(|a| a.load(Ordering::Relaxed))
                .filter(|v| *v > 0)
                .map(|v| (v - 1) as u8)
                .collect();
            k.sort_unstable();
            k
        }
        fn active(&self) -> usize {
            self.active.load(Ordering::Relaxed)
        }
    }

    const OUT: [&str; 2] = ["out_l", "out_r"];

    fn synth() -> (DeviceHarness, Arc<VoiceProbe>) {
        let probe = Arc::new(VoiceProbe::default());
        let mut s = PolySynth::new();
        s.probe = Some(probe.clone());
        (harness(Box::new(s)), probe)
    }

    /// Single sine oscillator, filter wide open and static.
    fn pure_sine(h: &mut DeviceHarness) {
        h.set_param("osc1.wave", 3.0);
        h.set_param("osc.mix", 0.0);
        h.set_param("filter.cutoff", 20_000.0);
        h.set_param("filter.env_amount", 0.0);
        h.set_param("filter.key_track", 0.0);
        h.set_param("filter.resonance", 0.0);
    }

    /// RMS of the first difference: weights high harmonics (a filter-opening metric).
    fn brightness(x: &[f32]) -> f32 {
        let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&d)
    }

    fn seconds(s: f64) -> usize {
        (s * SR / BATCH as f64).ceil() as usize
    }

    #[test]
    fn descriptor_validates() {
        assert_keys(
            &DESCRIPTOR,
            &[
                "osc1.wave",
                "osc2.wave",
                "osc1.octave",
                "osc2.octave",
                "osc1.semi",
                "osc2.semi",
                "osc1.fine",
                "osc2.fine",
                "osc.mix",
                "noise.level",
                "filter.mode",
                "filter.cutoff",
                "filter.resonance",
                "filter.env_amount",
                "filter.key_track",
                "env.amp.attack",
                "env.amp.decay",
                "env.amp.release",
                "env.amp.sustain",
                "env.filter.attack",
                "env.filter.decay",
                "env.filter.release",
                "env.filter.sustain",
                "lfo.wave",
                "lfo.rate",
                "lfo.to_pitch",
                "lfo.to_cutoff",
                "velocity.amp",
                "cv.cutoff_amount",
                "voices",
                "master.level",
                "cv.pitch_amount",
                "cv.resonance_amount",
                "cv.level_amount",
                "cv.pan_amount",
            ],
            &[
                "notes_in",
                "cutoff_cv_in",
                "pitch_cv_in",
                "resonance_cv_in",
                "level_cv_in",
                "pan_cv_in",
                "out_l",
                "out_r",
                "lfo_cv_out",
            ],
        );
        assert_eq!(DESCRIPTOR.type_id, "atomik.poly-synth");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
        let c = DESCRIPTOR.param_by_key("filter.cutoff").unwrap();
        assert_eq!((c.taper, c.unit), (Taper::Exponential, Unit::Hz));
        assert!((DESCRIPTOR.param_by_key("osc2.fine").unwrap().default - 7.0).abs() < 1e-6);
        assert!(
            (DESCRIPTOR.param_by_key("env.filter.decay").unwrap().default - 400.0).abs() < 1e-6
        );
    }

    #[test]
    fn ac_d1_sine_key_69_is_440_hz() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.push_note("notes_in", on(0, 69, 1.0));
        let o = h.render(seconds(1.0), &OUT);
        let f = dominant_frequency(&o[0][..48_000], SR);
        assert!((f - 440.0).abs() < 1.0, "{f} Hz");
        assert!(rms(&o[0]) > 0.05, "audible: {}", rms(&o[0]));
        assert_eq!(o[0], o[1], "no stereo spread: L == R");
    }

    #[test]
    fn ac_d2_seventeen_notes_steal_the_oldest_voice() {
        let (mut h, probe) = synth();
        for (i, key) in (40u8..=56).enumerate() {
            h.push_note("notes_in", on(i as u16, key, 0.8));
        }
        let o = h.render(4, &OUT); // > 2 ms steal fade
        assert!(o.iter().flatten().all(|s| s.is_finite()));
        assert!(probe.active() <= 16, "active {}", probe.active());
        assert_eq!(
            probe.keys(),
            (41u8..=56).collect::<Vec<_>>(),
            "key 40 was stolen"
        );
        // Later: the oldest is now 41.
        h.push_note("notes_in", on(0, 70, 0.8));
        h.render(4, &OUT);
        assert!(!probe.keys().contains(&41) && probe.keys().contains(&70));
        assert_eq!(probe.active(), 16);
    }

    #[test]
    fn process_voices_param_limits_polyphony() {
        let (mut h, probe) = synth();
        h.set_param("voices", 2.0);
        for (i, key) in [60u8, 64, 67].iter().enumerate() {
            h.push_note("notes_in", on(i as u16, *key, 0.8));
        }
        h.render(4, &OUT);
        assert_eq!(probe.active(), 2);
        assert_eq!(probe.keys(), vec![64, 67]);
    }

    #[test]
    fn process_same_key_retriggers_same_voice_continuously() {
        let (mut h, probe) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.octave", -2.0); // 110 Hz: small sample steps
        h.push_note("notes_in", on(0, 69, 1.0));
        let mut out = h.render(seconds(0.3), &OUT).remove(0);
        h.push_note("notes_in", off(0, 69));
        out.extend(h.render(seconds(0.1), &OUT).remove(0));
        h.push_note("notes_in", on(10, 69, 0.3));
        out.extend(h.render(seconds(0.3), &OUT).remove(0));
        assert_eq!(probe.active(), 1);
        assert_eq!(probe.keys(), vec![69]);
        assert!(max_step(&out) < 0.02, "max step {}", max_step(&out));
    }

    #[test]
    fn process_steal_fades_over_2ms_without_click() {
        let (mut h, probe) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.octave", -2.0);
        h.set_param("voices", 1.0);
        h.push_note("notes_in", on(0, 69, 1.0));
        let mut out = h.render(seconds(0.2), &OUT).remove(0);
        h.push_note("notes_in", on(5, 72, 1.0));
        out.extend(h.render(seconds(0.2), &OUT).remove(0));
        assert_eq!(probe.keys(), vec![72]);
        assert!(max_step(&out) < 0.02, "max step {}", max_step(&out));
    }

    #[test]
    fn ac_d4_release_decays_below_minus_90_dbfs_and_voice_goes_idle() {
        let (mut h, probe) = synth();
        h.push_note("notes_in", on(0, 60, 1.0));
        h.render(seconds(0.2), &OUT);
        assert_eq!(probe.active(), 1);
        h.push_note("notes_in", off(0, 60));
        let o = h.render(seconds(0.5), &OUT); // release 300 ms
        let tail = &o[0][o[0].len() - BATCH..];
        assert!(db(peak(tail)) < -90.0, "tail {} dBFS", db(peak(tail)));
        assert_eq!(probe.active(), 0);
    }

    #[test]
    fn process_all_off_releases_every_voice() {
        let (mut h, probe) = synth();
        for (i, k) in [48u8, 52, 55].iter().enumerate() {
            h.push_note("notes_in", on(i as u16, *k, 1.0));
        }
        h.render(seconds(0.1), &OUT);
        assert_eq!(probe.active(), 3);
        h.push_note(
            "notes_in",
            NoteEvent {
                offset: 0,
                kind: NoteKind::AllOff,
            },
        );
        h.render(seconds(0.5), &OUT);
        assert_eq!(probe.active(), 0);
    }

    #[test]
    fn process_note_on_is_sample_accurate() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.wave", 0.0); // saw starts at -1: immediate output
        h.push_note("notes_in", on(37, 60, 1.0));
        let o = h.render(1, &OUT);
        assert!(o[0][..37].iter().all(|s| s.to_bits() == 0));
        assert!(o[0][38..].iter().any(|s| s.abs() > 1e-4));
    }

    #[test]
    fn process_velocity_amount_scales_amplitude() {
        let level = |vel_amount: f32, vel: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("velocity.amp", vel_amount);
            h.push_note("notes_in", on(0, 69, vel));
            rms(&h.render(seconds(0.5), &OUT)[0][12_000..])
        };
        assert!((level(1.0, 0.5) / level(1.0, 1.0) - 0.5).abs() < 0.01);
        assert!((level(0.0, 0.2) / level(0.0, 1.0) - 1.0).abs() < 0.01);
        assert!((level(0.5, 0.5) / level(0.5, 1.0) - 0.75).abs() < 0.01);
    }

    #[test]
    fn process_writes_lfo_to_cv_out_every_sample() {
        let (mut h, _) = synth();
        h.set_param("lfo.rate", 2.0);
        let o = h.render(seconds(1.0), &["lfo_cv_out"]).remove(0);
        assert!(o.iter().all(|v| (-1.0..=1.0).contains(v)));
        // Sine LFO at 2 Hz: +1 at 0.125 s, −1 at 0.375 s.
        assert!((o[6_000] - 1.0).abs() < 1e-3 && (o[18_000] + 1.0).abs() < 1e-3);
        assert!(max_step(&o) < 0.001);
    }

    #[test]
    fn process_lfo_to_pitch_shifts_by_semitones() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("lfo.wave", 3.0); // square: +1 first half
        h.set_param("lfo.rate", 0.5);
        h.set_param("lfo.to_pitch", 12.0);
        h.push_note("notes_in", on(0, 57, 1.0)); // 220 Hz
        let o = h.render(seconds(2.0), &OUT).remove(0);
        let up = dominant_frequency(&o[4_800..43_200], SR);
        let down = dominant_frequency(&o[52_800..91_200], SR);
        assert!((up - 440.0).abs() < 2.0, "{up}");
        assert!((down - 110.0).abs() < 2.0, "{down}");
    }

    #[test]
    fn process_cutoff_cv_opens_and_closes_filter() {
        let level = |cv: Option<f32>| {
            let (mut h, _) = synth();
            h.set_param("filter.cutoff", 200.0);
            h.set_param("filter.env_amount", 0.0);
            h.set_param("filter.key_track", 0.0);
            h.set_param("osc.mix", 0.0); // saw
            if let Some(cv) = cv {
                *h.input_mut("cutoff_cv_in") = [cv; BATCH];
            }
            h.push_note("notes_in", on(0, 36, 1.0));
            brightness(&h.render(seconds(0.5), &OUT)[0][12_000..])
        };
        let (none, zero, up, down) = (
            level(None),
            level(Some(0.0)),
            level(Some(1.0)),
            level(Some(-1.0)),
        );
        assert!((none - zero).abs() < 1e-6);
        assert!(up > zero * 2.0, "{up} vs {zero}");
        assert!(down < zero * 0.8, "{down} vs {zero}");
    }

    /// Renders 0.4 s of a held note with an optional CV on `socket` and the given amount.
    fn cv_render(
        setup: impl Fn(&mut DeviceHarness),
        socket: &str,
        amount_key: &str,
        amount: f32,
        cv: Option<f32>,
    ) -> Vec<Vec<f32>> {
        let (mut h, _) = synth();
        setup(&mut h);
        h.set_param(amount_key, amount);
        if let Some(cv) = cv {
            *h.input_mut(socket) = [cv; BATCH];
        }
        h.push_note("notes_in", on(0, 57, 1.0)); // 220 Hz
        h.render(seconds(0.4), &OUT)
    }

    #[test]
    fn process_pitch_cv_shifts_by_amount_semitones() {
        let freq = |cv: Option<f32>, amount: f32| {
            let o = cv_render(pure_sine, "pitch_cv_in", "cv.pitch_amount", amount, cv);
            dominant_frequency(&o[0][4_800..], SR)
        };
        let up = freq(Some(0.5), 12.0); // +6 semitones
        assert!((up - 220.0 * 2.0f64.powf(0.5)).abs() < 3.0, "{up}");
        let down = freq(Some(-1.0), 12.0);
        assert!((down - 110.0).abs() < 2.0, "{down}");
        let none = freq(Some(1.0), 0.0);
        assert!((none - 220.0).abs() < 2.0, "amount 0: {none}");
        let clamped = freq(Some(3.0), 12.0);
        assert!((clamped - 440.0).abs() < 4.0, "clamped to +1: {clamped}");
        let neg = freq(Some(1.0), -12.0);
        assert!((neg - 110.0).abs() < 2.0, "negative amount: {neg}");
    }

    #[test]
    fn process_resonance_cv_adds_to_resonance_and_clamps() {
        let level = |cv: Option<f32>, amount: f32| {
            let o = cv_render(
                |h| {
                    pure_sine(h);
                    h.set_param("filter.cutoff", 220.0);
                    h.set_param("filter.resonance", 0.0);
                    h.set_param("master.level", -40.0);
                },
                "resonance_cv_in",
                "cv.resonance_amount",
                amount,
                cv,
            );
            rms(&o[0][9_600..])
        };
        let base = level(None, 1.0);
        assert!(level(Some(1.0), 1.0) > base * 5.0);
        assert!(
            (level(Some(1.0), 0.0) - base).abs() < base * 1e-4,
            "amount 0 cancels"
        );
        assert!(
            (level(Some(-1.0), 1.0) - base).abs() < base * 1e-4,
            "clamped at 0"
        );
        let (a, b) = (level(Some(1.0), 1.0), level(Some(5.0), 1.0));
        assert!((a - b).abs() < a * 1e-6, "cv beyond +1 acts as +1");
    }

    #[test]
    fn process_level_cv_scales_gain_in_db() {
        let level = |cv: Option<f32>, amount: f32| {
            let o = cv_render(
                |h| {
                    pure_sine(h);
                    h.set_param("master.level", -24.0);
                },
                "level_cv_in",
                "cv.level_amount",
                amount,
                cv,
            );
            rms(&o[0][9_600..])
        };
        let base = level(None, 12.0);
        let up = level(Some(0.5), 12.0); // +6 dB
        assert!(
            (db(up) - db(base) - 6.0).abs() < 0.05,
            "{}",
            db(up) - db(base)
        );
        let down = level(Some(-1.0), 12.0);
        assert!((db(down) - db(base) + 12.0).abs() < 0.05);
        assert!((level(Some(1.0), 0.0) - base).abs() < base * 1e-6);
        assert!((level(Some(9.0), 12.0) - level(Some(1.0), 12.0)).abs() < 1e-9);
        assert!((db(level(Some(1.0), -12.0)) - db(base) + 12.0).abs() < 0.05);
    }

    #[test]
    fn process_pan_cv_balances_left_and_right() {
        let sides = |cv: Option<f32>, amount: f32| {
            let o = cv_render(pure_sine, "pan_cv_in", "cv.pan_amount", amount, cv);
            (rms(&o[0][9_600..]), rms(&o[1][9_600..]))
        };
        let (l0, r0) = sides(None, 1.0);
        assert_eq!(l0.to_bits(), r0.to_bits(), "unconnected: dual mono");
        let (l, r) = sides(Some(1.0), 1.0);
        assert!(l < r0 * 1e-3 && (r - r0).abs() < r0 * 1e-3, "{l} {r}");
        let (l, r) = sides(Some(-1.0), 1.0);
        assert!(r < r0 * 1e-3 && (l - l0).abs() < l0 * 1e-3, "{l} {r}");
        let (l, r) = sides(Some(0.0), 1.0);
        assert!(
            (l - l0).abs() < l0 * 1e-3 && (r - r0).abs() < r0 * 1e-3,
            "centre is unity"
        );
        let (l, r) = sides(Some(1.0), 0.0);
        assert!(
            (l - l0).abs() < l0 * 1e-3 && (r - r0).abs() < r0 * 1e-3,
            "amount 0 cancels"
        );
        let (l2, r2) = sides(Some(4.0), 1.0);
        let (l1, r1) = sides(Some(1.0), 1.0);
        assert_eq!((l1.to_bits(), r1.to_bits()), (l2.to_bits(), r2.to_bits()));
        let (l, r) = sides(Some(1.0), -1.0); // negative amount mirrors
        assert!(r < r0 * 1e-3 && l > l0 * 0.99, "{l} {r}");
    }

    #[test]
    fn process_new_cv_inputs_unconnected_leave_output_bit_identical() {
        let render = |amounts: Option<[f32; 4]>| {
            let (mut h, _) = synth();
            h.set_param("lfo.wave", 4.0);
            h.set_param("lfo.to_pitch", 1.0);
            h.set_param("filter.env_amount", 0.6);
            if let Some(a) = amounts {
                for (k, v) in [
                    "cv.pitch_amount",
                    "cv.resonance_amount",
                    "cv.level_amount",
                    "cv.pan_amount",
                ]
                .into_iter()
                .zip(a)
                {
                    h.set_param(k, v);
                }
            }
            h.push_note("notes_in", on(0, 45, 0.9));
            h.push_note("notes_in", on(10, 52, 0.7));
            h.render(seconds(0.3), &OUT)
        };
        let a = render(None);
        let b = render(Some([-20.0, -0.9, 17.0, -0.4]));
        for (x, y) in a.iter().flatten().zip(b.iter().flatten()) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }

    #[test]
    fn process_new_cv_inputs_at_zero_do_not_change_the_sound() {
        // Connected but silent CV: pitch, resonance and level add nothing.
        let base = cv_render(pure_sine, "none", "cv.pitch_amount", 12.0, None);
        let (mut h, _) = synth();
        pure_sine(&mut h);
        for s in ["pitch_cv_in", "resonance_cv_in", "level_cv_in"] {
            *h.input_mut(s) = [0.0; BATCH];
        }
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(0.4), &OUT);
        for (x, y) in base[0].iter().zip(&o[0]) {
            assert!((x - y).abs() < 1e-5);
        }
    }

    #[test]
    fn process_cv_inputs_are_allocation_free() {
        let (mut h, _) = synth();
        for s in ["pitch_cv_in", "resonance_cv_in", "level_cv_in", "pan_cv_in"] {
            *h.input_mut(s) = [0.7; BATCH];
        }
        h.push_note("notes_in", on(0, 50, 1.0));
        h.push_note("notes_in", on(5, 55, 1.0));
        for _ in 0..20 {
            h.process();
        }
    }

    #[test]
    fn process_filter_env_amount_brightens_attack() {
        let level = |amount: f32| {
            let (mut h, _) = synth();
            h.set_param("filter.cutoff", 200.0);
            h.set_param("filter.key_track", 0.0);
            h.set_param("filter.env_amount", amount);
            h.set_param("env.filter.decay", 5_000.0);
            h.set_param("osc.mix", 0.0);
            h.push_note("notes_in", on(0, 36, 1.0));
            brightness(&h.render(seconds(0.2), &OUT)[0])
        };
        let (open, closed) = (level(1.0), level(0.0));
        assert!(open > closed * 2.0, "{open} vs {closed}");
    }

    #[test]
    fn set_param_every_param_min_max_does_not_panic() {
        let (mut h, _) = synth();
        *h.input_mut("cutoff_cv_in") = [1.0; BATCH];
        for k in [24u8, 60, 96, 127] {
            h.push_note("notes_in", on(0, k, 1.0));
        }
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }

    #[test]
    fn process_extreme_settings_stay_finite_and_below_4() {
        for (cutoff, mode) in [(20_000.0, 0.0), (500.0, 0.0), (2_000.0, 1.0), (60.0, 2.0)] {
            let (mut h, _) = synth();
            for (k, v) in [
                ("filter.resonance", 1.0),
                ("filter.cutoff", cutoff),
                ("filter.mode", mode),
                ("noise.level", 1.0),
                ("master.level", 6.0),
                ("velocity.amp", 0.0),
                ("lfo.to_cutoff", 4.0),
                ("lfo.to_pitch", 12.0),
                ("lfo.rate", 30.0),
                ("env.amp.attack", 0.5),
                ("env.amp.sustain", 1.0),
                ("filter.env_amount", 1.0),
            ] {
                h.set_param(k, v);
            }
            *h.input_mut("cutoff_cv_in") = [1.0; BATCH];
            for i in 0..16u8 {
                h.push_note("notes_in", on(u16::from(i), 24 + 6 * i, 1.0));
            }
            let o = h.render(seconds(1.0), &OUT);
            let pk = peak(&o[0]);
            assert!(o[0].iter().all(|s| s.is_finite()) && pk <= 4.0, "peak {pk}");
            assert!(pk > 1.0, "limiter range exercised: {pk}");
        }
    }

    #[test]
    fn soft_limit_is_linear_below_2_and_bounded_by_4() {
        for x in [-2.0f32, -1.0, 0.0, 0.5, 2.0] {
            assert!((soft_limit(x) - x).abs() < 1e-7);
        }
        for x in [2.5f32, 10.0, 1e30, f32::INFINITY] {
            let y = soft_limit(x);
            assert!(
                y > 2.0 && y <= 4.0 && (soft_limit(-x) + y).abs() < 1e-6,
                "{x} -> {y}"
            );
        }
        assert_eq!(soft_limit(f32::NAN).to_bits(), 0.0f32.to_bits());
        // Continuous slope at the knee.
        assert!((soft_limit(2.001) - 2.001).abs() < 1e-5);
    }

    #[test]
    fn reset_silences_tails() {
        let (mut h, probe) = synth();
        h.set_param("env.amp.release", 10_000.0);
        h.push_note("notes_in", on(0, 60, 1.0));
        h.render(10, &OUT);
        h.device_mut().reset();
        assert_eq!(probe.active(), 1, "probe is updated by process only");
        let o = h.render(2, &OUT);
        assert!(o[0].iter().chain(&o[1]).all(|s| s.to_bits() == 0));
        assert_eq!(probe.active(), 0);
    }

    #[test]
    fn process_is_deterministic_for_a_seed() {
        let run = |seed: u64| {
            let mut h = DeviceHarness::with_options(Box::new(PolySynth::new()), SR, 120.0, seed);
            h.set_param("noise.level", 1.0);
            h.set_param("lfo.wave", 4.0);
            h.set_param("lfo.to_cutoff", 2.0);
            h.push_note("notes_in", on(0, 60, 1.0));
            h.render(20, &OUT).remove(0)
        };
        assert_eq!(run(5), run(5));
        assert_ne!(run(5), run(6));
    }

    #[test]
    fn process_is_allocation_free() {
        let (mut h, _) = synth();
        h.set_param("noise.level", 0.3);
        h.set_param("lfo.wave", 4.0);
        *h.input_mut("cutoff_cv_in") = [0.5; BATCH];
        for i in 0..400u32 {
            let key = 30 + (i * 7 % 60) as u8;
            h.push_note("notes_in", on((i % 64) as u16, key, 0.9));
            if i % 3 == 0 {
                h.push_note("notes_in", off(63, key));
            }
            if i % 97 == 0 {
                h.push_note(
                    "notes_in",
                    NoteEvent {
                        offset: 63,
                        kind: NoteKind::AllOff,
                    },
                );
            }
            h.set_param("voices", (1 + i % 16) as f32);
            h.set_param("filter.mode", (i % 3) as f32);
            h.process();
        }
        h.device_mut().reset();
        h.process();
    }

    #[derive(Debug, Clone)]
    enum Ev {
        On(u8, f32),
        Off(u8),
        AllOff,
    }

    fn ev() -> impl Strategy<Value = (usize, u16, Ev)> {
        let kind = prop_oneof![
            6 => (0u8..=127, 0.01f32..=1.0).prop_map(|(k, v)| Ev::On(k, v)),
            3 => (0u8..=127).prop_map(Ev::Off),
            1 => Just(Ev::AllOff),
        ];
        (0usize..1_500, 0u16..64, kind)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]
        #[test]
        fn ac_d3_any_params_and_notes_stay_finite_and_bounded(
            norms in prop::collection::vec(0.0f32..=1.0, PARAM_COUNT),
            mut events in prop::collection::vec(ev(), 0..120),
            cv in -1.5f32..=1.5,
            seed in any::<u64>(),
        ) {
            let mut h = DeviceHarness::with_options(Box::new(PolySynth::new()), SR, 120.0, seed);
            for (p, n) in PARAMS.iter().zip(&norms) {
                h.set_param(p.key, p.from_normalized(*n));
            }
            *h.input_mut("cutoff_cv_in") = [cv; BATCH];
            events.sort_by_key(|(b, o, _)| (*b, *o));
            let mut next = 0;
            for b in 0..seconds(2.0) {
                while next < events.len() && events[next].0 == b {
                    let (_, offset, e) = &events[next];
                    let kind = match e {
                        Ev::On(key, velocity) => NoteKind::On { key: *key, velocity: *velocity },
                        Ev::Off(key) => NoteKind::Off { key: *key },
                        Ev::AllOff => NoteKind::AllOff,
                    };
                    h.push_note("notes_in", NoteEvent { offset: *offset, kind });
                    next += 1;
                }
                h.process();
                for key in OUT {
                    for s in h.output(key) {
                        prop_assert!(s.is_finite() && s.abs() <= 4.0, "{key}: {s}");
                    }
                }
                prop_assert!(h.output("lfo_cv_out").iter().all(|v| (-1.0..=1.0).contains(v)));
            }
        }
    }

    #[test]
    fn master_level_set_before_the_first_batch_applies_without_ramp() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("master.level", -60.0); // −∞
        h.push_note("notes_in", on(0, 69, 1.0));
        h.process();
        assert!(
            h.output("out_l").iter().all(|v| v.abs() < 1e-9),
            "audible while the master ramps down from its default"
        );
        // A later change still ramps.
        h.set_param("master.level", 0.0);
        let o = h.render(1, &OUT);
        assert!(o[0][0].abs() < 1e-3, "ramps up from silence: {}", o[0][0]);
    }
}
