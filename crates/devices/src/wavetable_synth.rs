//! `atomik.wavetable-synth`: 16-voice wavetable synthesizer.
//!
//! Per voice: two wavetable oscillators (each reads one of the factory tables of
//! [`atomik_dsp::wavetable`] at a morphing frame position), a sub sine one octave below the key
//! and white noise, a stereo pair of TPT state-variable filters (LP/BP/HP 12 dB/oct), an amp ADSR
//! and a mod ADSR. One global LFO is written to `lfo_cv_out` every sample.
//!
//! * **Oscillators.** `osc1.*` and `osc2.*`: table, frame `position` (0 is the first frame, 1 the
//!   last; the output blends the two neighbouring frames, so it is continuous), octave, semitone
//!   and cents, level. Both oscillators run `unison.voices` (1..=7) detuned copies spread evenly
//!   over `±unison.detune` cents and `±unison.spread` of the stereo field (balance law: a lone
//!   copy is centred at unity gain), each copy at its own phase; the copies are scaled by
//!   `1/sqrt(n)`. Unison copies after the first start at seeded random phases.
//! * **Band limiting.** Every read uses the table's mip level for the highest frequency of the
//!   oscillator (the pitch plus the detune of the widest copy), picked every 16 samples, so no
//!   harmonic goes past Nyquist and the output does not alias, whatever the pitch or the pitch
//!   modulation. The tables are built once per process (see [`atomik_dsp::wavetable`]); the
//!   device only holds a reference to the shared bank, and a table change is a plain parameter.
//! * **Voice states** follow the amp envelope: Idle → Attack → Decay → Sustain → Release → Idle.
//!   Same-key note-on retriggers that voice (continuously from its current level). When every
//!   allowed voice is busy, the oldest started voice is stolen: it fades out linearly over 2 ms
//!   and then starts the new note from silence. `AllOff` releases every voice.
//! * **Gain.** Amp gain = envelope · (1 − velocity.amp·(1 − velocity)); velocity changes on
//!   retrigger glide over 2 ms.
//! * **Modulation** (evaluated every 16 samples per voice; the filter coefficient and the frame
//!   positions are interpolated linearly in between):
//!   * frame position of oscillator *n* = `oscN.position + env.mod.to_posN·mod_env +
//!     lfo.to_posN·lfo + cv.position_amount·position_cv`, clamped to 0..=1;
//!   * pitch (semitones) = `env.mod.to_pitch·mod_env + lfo·(lfo.to_pitch + wheel) + bend +
//!     pitch_cv_trim·24·pitch_cv`;
//!   * cutoff = `filter.cutoff · 2^(env_amount·6·mod_env + key_track·(key − 60)/12 +
//!     lfo.to_cutoff·lfo + cv.cutoff_amount·cutoff_cv)`, clamped to 20 Hz..0.45·sr.
//! * **Channel messages** as the poly synth: pitch bend moves every voice by up to ±2 semitones
//!   ([`BEND_RANGE`]) and the mod wheel (CC 1) adds up to one semitone of LFO vibrato
//!   ([`WHEEL_VIBRATO`]); both survive `AllOff` and `reset` clears them.
//! * **Output:** sum of voices · 0.5 · master gain (5 ms smoothing), then the poly synth's soft
//!   limiter (|x| < 4 for every param combination); with the pan CV cabled the stereo pair is
//!   balanced by it.
//! * Randomness (noise, unison phases, sample & hold LFO) comes from `Rng`s derived from
//!   `PrepareInfo::seed`.

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::wavetable::{TABLE_COUNT, TABLE_NAMES, Wavetable, WavetableBank, factory_bank};
use atomik_dsp::{
    Adsr, AdsrStage, FilterMode, Lfo, LfoShape, Rng, Smoother, Svf, balance_gains, note_to_hz,
    sin_cycles,
};

use crate::cv_in::{clean, db_gain};
use crate::poly_synth::{derive_seed, soft_limit};

/// Number of preallocated voices.
pub const MAX_VOICES: usize = 16;
/// Most unison copies per oscillator.
pub const MAX_UNISON: usize = 7;

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

const TABLES: Unit = Unit::Choice(&TABLE_NAMES);
const FILTER_MODES: Unit = Unit::Choice(&["LP12", "BP12", "HP12"]);
const LFO_WAVES: Unit = Unit::Choice(&["sine", "triangle", "saw", "square", "sample&hold"]);
const ENV_MS: (f32, f32) = (0.5, 10_000.0);
const LAST_TABLE: f32 = (TABLE_COUNT - 1) as f32;

const P_OSC1_TABLE: u32 = 0;
const P_OSC1_POSITION: u32 = 1;
const P_OSC1_OCTAVE: u32 = 2;
const P_OSC1_SEMI: u32 = 3;
const P_OSC1_FINE: u32 = 4;
const P_OSC1_LEVEL: u32 = 5;
const P_OSC2_TABLE: u32 = 6;
const P_OSC2_POSITION: u32 = 7;
const P_OSC2_OCTAVE: u32 = 8;
const P_OSC2_SEMI: u32 = 9;
const P_OSC2_FINE: u32 = 10;
const P_OSC2_LEVEL: u32 = 11;
const P_SUB: u32 = 12;
const P_NOISE: u32 = 13;
const P_UNISON_VOICES: u32 = 14;
const P_UNISON_DETUNE: u32 = 15;
const P_UNISON_SPREAD: u32 = 16;
const P_FILTER_MODE: u32 = 17;
const P_CUTOFF: u32 = 18;
const P_RESONANCE: u32 = 19;
const P_ENV_AMOUNT: u32 = 20;
const P_KEY_TRACK: u32 = 21;
const P_AMP_A: u32 = 22;
const P_AMP_D: u32 = 23;
const P_AMP_S: u32 = 24;
const P_AMP_R: u32 = 25;
const P_MOD_A: u32 = 26;
const P_MOD_D: u32 = 27;
const P_MOD_S: u32 = 28;
const P_MOD_R: u32 = 29;
const P_ENV_POS1: u32 = 30;
const P_ENV_POS2: u32 = 31;
const P_ENV_PITCH: u32 = 32;
const P_LFO_WAVE: u32 = 33;
const P_LFO_RATE: u32 = 34;
const P_LFO_PITCH: u32 = 35;
const P_LFO_CUTOFF: u32 = 36;
const P_LFO_POS1: u32 = 37;
const P_LFO_POS2: u32 = 38;
const P_VEL_AMP: u32 = 39;
const P_CV_CUTOFF: u32 = 40;
const P_CV_POSITION: u32 = 41;
const P_VOICES: u32 = 42;
const P_MASTER: u32 = 43;
const P_CV_PITCH: u32 = 44;
const P_CV_RESONANCE: u32 = 45;
const P_CV_LEVEL: u32 = 46;
const P_CV_PAN: u32 = 47;
const PARAM_COUNT: usize = 48;

use Taper::{Decibel as Db, Exponential as Exp, Linear as Lin};

static PARAMS: [ParamDescriptor; PARAM_COUNT] = [
    param(
        P_OSC1_TABLE,
        "osc1.table",
        "Osc 1 table",
        (0.0, LAST_TABLE, 0.0),
        Lin,
        TABLES,
        true,
    ),
    param(
        P_OSC1_POSITION,
        "osc1.position",
        "Osc 1 position",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
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
        P_OSC1_SEMI,
        "osc1.semi",
        "Osc 1 semi",
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
        P_OSC1_LEVEL,
        "osc1.level",
        "Osc 1 level",
        (0.0, 1.0, 0.7),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_OSC2_TABLE,
        "osc2.table",
        "Osc 2 table",
        (0.0, LAST_TABLE, 1.0),
        Lin,
        TABLES,
        true,
    ),
    param(
        P_OSC2_POSITION,
        "osc2.position",
        "Osc 2 position",
        (0.0, 1.0, 0.25),
        Lin,
        Unit::Percent,
        false,
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
        P_OSC2_SEMI,
        "osc2.semi",
        "Osc 2 semi",
        (-12.0, 12.0, 0.0),
        Lin,
        Unit::Semitones,
        true,
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
        P_OSC2_LEVEL,
        "osc2.level",
        "Osc 2 level",
        (0.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_SUB,
        "mix.sub",
        "Sub level",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_NOISE,
        "mix.noise",
        "Noise level",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_UNISON_VOICES,
        "unison.voices",
        "Unison voices",
        (1.0, MAX_UNISON as f32, 1.0),
        Lin,
        Unit::None,
        true,
    ),
    param(
        P_UNISON_DETUNE,
        "unison.detune",
        "Unison detune",
        (0.0, 100.0, 20.0),
        Lin,
        Unit::Cents,
        false,
    ),
    param(
        P_UNISON_SPREAD,
        "unison.spread",
        "Unison spread",
        (0.0, 1.0, 0.5),
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
        (20.0, 20_000.0, 8_000.0),
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
        "Mod env to cutoff",
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
        P_AMP_S,
        "env.amp.sustain",
        "Amp sustain",
        (0.0, 1.0, 0.7),
        Lin,
        Unit::Percent,
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
        P_MOD_A,
        "env.mod.attack",
        "Mod attack",
        (ENV_MS.0, ENV_MS.1, 2.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_MOD_D,
        "env.mod.decay",
        "Mod decay",
        (ENV_MS.0, ENV_MS.1, 600.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_MOD_S,
        "env.mod.sustain",
        "Mod sustain",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_MOD_R,
        "env.mod.release",
        "Mod release",
        (ENV_MS.0, ENV_MS.1, 300.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_ENV_POS1,
        "env.mod.to_pos1",
        "Mod env to osc 1 position",
        (-1.0, 1.0, 0.3),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_ENV_POS2,
        "env.mod.to_pos2",
        "Mod env to osc 2 position",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_ENV_PITCH,
        "env.mod.to_pitch",
        "Mod env to pitch",
        (-24.0, 24.0, 0.0),
        Lin,
        Unit::Semitones,
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
        P_LFO_POS1,
        "lfo.to_pos1",
        "LFO to osc 1 position",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_LFO_POS2,
        "lfo.to_pos2",
        "LFO to osc 2 position",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::None,
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
        P_CV_CUTOFF,
        "cv.cutoff_amount",
        "CV to cutoff",
        (0.0, 4.0, 2.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_CV_POSITION,
        "cv.position_amount",
        "CV to position",
        (-1.0, 1.0, 0.5),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_VOICES,
        "voices",
        "Voices",
        (1.0, MAX_VOICES as f32, 8.0),
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
        "pitch_cv_trim",
        "Pitch CV trim",
        (-1.0, 1.0, 1.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_CV_RESONANCE,
        "filter.resonance_cv_trim",
        "Resonance CV trim",
        (-1.0, 1.0, 1.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_CV_LEVEL,
        "master.level_cv_trim",
        "Level CV trim",
        (-1.0, 1.0, 1.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_CV_PAN,
        "pan_cv_trim",
        "Pan CV trim",
        (-1.0, 1.0, 1.0),
        Lin,
        Unit::None,
        false,
    ),
];

const NOTES_IN: SocketId = SocketId(0);
const CUTOFF_CV_IN: SocketId = SocketId(1);
const PITCH_CV_IN: SocketId = SocketId(2);
const POSITION_CV_IN: SocketId = SocketId(3);
const RESONANCE_CV_IN: SocketId = SocketId(4);
const LEVEL_CV_IN: SocketId = SocketId(5);
const PAN_CV_IN: SocketId = SocketId(6);
const OUT_L: SocketId = SocketId(7);
const OUT_R: SocketId = SocketId(8);
const LFO_CV_OUT: SocketId = SocketId(9);

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

static SOCKETS: [SocketDescriptor; 10] = [
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
        POSITION_CV_IN,
        "position_cv_in",
        "Position CV",
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

/// Static description of the wavetable synth.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.wavetable-synth",
    name: "Wavetable Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`WavetableSynth`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(WavetableSynth::new()),
    }
}

/// Samples between control-rate updates (pitch, position, cutoff) of a voice.
const CONTROL_INTERVAL: u32 = 16;
/// Fixed range of the pitch CV in semitones (times `pitch_cv_trim`).
const CV_PITCH_RANGE: f32 = 24.0;
/// Fixed range of the level CV in dB (times `master.level_cv_trim`).
const CV_LEVEL_RANGE_DB: f32 = 24.0;
/// Pitch bend range in semitones each way.
const BEND_RANGE: f32 = 2.0;
/// Extra LFO depth in semitones at a full mod wheel (CC 1).
const WHEEL_VIBRATO: f32 = 1.0;
/// Per-voice output gain before the master stage.
const VOICE_GAIN: f32 = 0.5;
/// Glide time for velocity changes on retrigger.
const VELOCITY_GLIDE_MS: f32 = 2.0;
/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Highest filter cutoff as a fraction of the sample rate.
const MAX_CUTOFF_RATIO: f32 = 0.45;
/// Highest oscillator frequency as a fraction of the sample rate (a phase step of at most 0.45).
const MAX_FREQ_RATIO: f32 = 0.45;

/// Shared (per-device) synthesis parameters, as plain values.
#[derive(Debug, Clone, Copy)]
struct Shared {
    sample_rate: f32,
    bank: &'static WavetableBank,
    tables: [usize; 2],
    base_position: [f32; 2],
    pitch_offset: [f32; 2],
    levels: [f32; 2],
    sub: f32,
    noise: f32,
    /// Unison copies per oscillator.
    unison: usize,
    /// Frequency ratio of each copy.
    uni_ratio: [f32; MAX_UNISON],
    /// Ratio of the widest copy (the highest frequency of the oscillator).
    uni_top: f32,
    /// Left and right gain of each copy, normalisation included.
    uni_gain: [(f32, f32); MAX_UNISON],
    filter_mode: FilterMode,
    cutoff: f32,
    k: f32,
    /// Normalised resonance (the base of the resonance CV).
    resonance: f32,
    env_amount: f32,
    key_track: f32,
    env_to_position: [f32; 2],
    env_to_pitch: f32,
    lfo_to_pitch: f32,
    lfo_to_cutoff: f32,
    lfo_to_position: [f32; 2],
    /// Pitch bend in semitones, `-BEND_RANGE..=BEND_RANGE`.
    bend: f32,
    /// Mod wheel (CC 1), `0..=1`.
    wheel: f32,
    velocity_amount: f32,
    cv_cutoff_amount: f32,
    cv_position_amount: f32,
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
    position: f32,
    resonance: f32,
}

/// Per-sample modulation sources of one render segment (all aligned with the output slices).
struct Mods<'a> {
    lfo: &'a [f32],
    cutoff: &'a [f32],
    pitch: &'a [f32],
    position: &'a [f32],
    resonance: &'a [f32],
}

/// One preallocated voice.
#[derive(Debug, Clone)]
struct Voice {
    key: u8,
    order: u64,
    pending: Option<(u8, f32)>,
    /// Phase (cycles) of each unison copy of each oscillator.
    phase: [[f64; MAX_UNISON]; 2],
    /// Phase step per sample of each copy, set at the control rate.
    step: [[f64; MAX_UNISON]; 2],
    mip: [usize; 2],
    position: [f32; 2],
    position_target: [f32; 2],
    position_step: [f32; 2],
    sub_phase: f64,
    sub_step: f64,
    filter: [Svf; 2],
    amp: Adsr,
    menv: Adsr,
    noise: Rng,
    phases: Rng,
    vel_gain: Smoother,
    countdown: u32,
    g: f32,
    g_target: f32,
    g_step: f32,
    snap: bool,
    /// Filter damping while the resonance CV is cabled (`k` ramps like `g`).
    k: f32,
    k_target: f32,
    k_step: f32,
}

impl Voice {
    fn new(seed: u64, index: u64) -> Self {
        Self {
            key: 0,
            order: 0,
            pending: None,
            phase: [[0.0; MAX_UNISON]; 2],
            step: [[0.0; MAX_UNISON]; 2],
            mip: [0; 2],
            position: [0.0; 2],
            position_target: [0.0; 2],
            position_step: [0.0; 2],
            sub_phase: 0.0,
            sub_step: 0.0,
            filter: [Svf::new(); 2],
            amp: Adsr::new(48_000.0),
            menv: Adsr::new(48_000.0),
            noise: Rng::new(derive_seed(seed, index + 1)),
            phases: Rng::new(derive_seed(seed, index + 1 + MAX_VOICES as u64)),
            vel_gain: Smoother::new(0.0, VELOCITY_GLIDE_MS, 48_000.0),
            countdown: 0,
            g: 0.0,
            g_target: 0.0,
            g_step: 0.0,
            snap: true,
            k: 1.0,
            k_target: 1.0,
            k_step: 0.0,
        }
    }

    /// Re-seeds the voice's random streams (as `prepare` and `reset` do).
    fn reseed(&mut self, seed: u64, index: u64) {
        self.noise = Rng::new(derive_seed(seed, index + 1));
        self.phases = Rng::new(derive_seed(seed, index + 1 + MAX_VOICES as u64));
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

    /// Starts a note from silence (fresh phases and filters): the first copy of an oscillator
    /// starts at phase 0, the others at seeded random phases.
    fn start(&mut self, key: u8, velocity: f32, p: &Shared) {
        self.key = key;
        self.pending = None;
        for osc in &mut self.phase {
            for (u, ph) in osc.iter_mut().enumerate() {
                *ph = if u == 0 {
                    0.0
                } else {
                    f64::from(self.phases.next_f32())
                };
            }
        }
        self.sub_phase = 0.0;
        for f in &mut self.filter {
            f.reset();
        }
        self.amp.reset();
        self.menv.reset();
        self.amp.gate_on();
        self.menv.gate_on();
        self.vel_gain
            .set_immediate(Self::velocity_gain(p, velocity));
        self.countdown = 0;
        self.snap = true;
    }

    /// Same-key retrigger: envelopes restart from their current levels.
    fn retrigger(&mut self, velocity: f32, p: &Shared) {
        self.amp.gate_on();
        self.menv.gate_on();
        self.vel_gain.set_target(Self::velocity_gain(p, velocity));
    }

    fn release(&mut self) {
        self.pending = None;
        self.amp.gate_off();
        self.menv.gate_off();
    }

    /// Control-rate update: oscillator pitch, mip level and frame position, and the filter
    /// coefficient target.
    fn control(&mut self, c: Ctl, p: &Shared) {
        let key = f32::from(self.key);
        let env = self.menv.level();
        let pitch = c.lfo * (p.lfo_to_pitch + p.wheel * WHEEL_VIBRATO)
            + p.bend
            + env * p.env_to_pitch
            + p.cv_pitch_amount * clean(c.pitch);
        let max_hz = MAX_FREQ_RATIO * p.sample_rate;
        for o in 0..2 {
            let hz = note_to_hz(key + p.pitch_offset[o] + pitch, 0.0).min(max_hz);
            self.mip[o] = atomik_dsp::wavetable::level_for(hz * p.uni_top, p.sample_rate);
            for u in 0..p.unison {
                let step = (hz * p.uni_ratio[u]).min(max_hz) / p.sample_rate;
                self.step[o][u] = f64::from(step);
            }
            let target = (p.base_position[o]
                + env * p.env_to_position[o]
                + c.lfo * p.lfo_to_position[o]
                + p.cv_position_amount * clean(c.position))
            .clamp(0.0, 1.0);
            // Land exactly on the previous target (no drift), then ramp towards the new one.
            self.position[o] = if self.snap {
                target
            } else {
                self.position_target[o]
            };
            self.position_target[o] = target;
            self.position_step[o] = (target - self.position[o]) / CONTROL_INTERVAL as f32;
        }
        self.sub_step = f64::from(note_to_hz(key + pitch - 12.0, 0.0).min(max_hz) / p.sample_rate);

        let octaves = p.env_amount * 6.0 * env
            + p.key_track * (key - 60.0) / 12.0
            + p.lfo_to_cutoff * c.lfo
            + p.cv_cutoff_amount * clean(c.cutoff);
        let cutoff = (p.cutoff * octaves.exp2()).clamp(20.0, MAX_CUTOFF_RATIO * p.sample_rate);
        let g = Svf::cutoff_to_g(cutoff, p.sample_rate);
        self.g = if self.snap { g } else { self.g_target };
        self.g_target = g;
        self.g_step = (g - self.g) / CONTROL_INTERVAL as f32;
        if p.cv_resonance_on {
            let r = (p.resonance + p.cv_resonance_amount * clean(c.resonance)).clamp(0.0, 1.0);
            let k = 1.0 / Svf::resonance_to_q(r);
            self.k = if self.snap { k } else { self.k_target };
            self.k_target = k;
            self.k_step = (k - self.k) / CONTROL_INTERVAL as f32;
        } else {
            self.k = p.k;
            self.k_target = p.k;
            self.k_step = 0.0;
        }
        if self.snap || self.g_step == 0.0 {
            let k = self.k_now(p);
            for f in &mut self.filter {
                f.set_coefficients(self.g, k);
            }
            self.snap = false;
        }
    }

    /// The filter damping in use: the base value, or the ramped value under resonance CV.
    #[inline]
    fn k_now(&self, p: &Shared) -> f32 {
        if p.cv_resonance_on { self.k } else { p.k }
    }

    /// One sample of the two oscillators and their unison copies, as a stereo pair.
    #[inline]
    fn oscillators(&mut self, tables: &[&Wavetable; 2], p: &Shared) -> (f32, f32) {
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for (o, table) in tables.iter().enumerate() {
            let position = self.position[o];
            self.position[o] += self.position_step[o];
            let level = p.levels[o];
            if level <= 0.0 {
                continue;
            }
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for u in 0..p.unison {
                let s = table.read(self.mip[o], position, self.phase[o][u]);
                let ph = self.phase[o][u] + self.step[o][u];
                self.phase[o][u] = ph - ph.floor();
                l += s * p.uni_gain[u].0;
                r += s * p.uni_gain[u].1;
            }
            left += level * l;
            right += level * r;
        }
        (left, right)
    }

    /// Adds this voice into `left` and `right`. The modulation slices are aligned with them.
    fn render(&mut self, left: &mut [f32], right: &mut [f32], m: &Mods<'_>, p: &Shared) {
        let tables = [p.bank.table(p.tables[0]), p.bank.table(p.tables[1])];
        for (n, (lo, ro)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            if self.countdown == 0 {
                let c = Ctl {
                    lfo: m.lfo[n],
                    cutoff: m.cutoff[n],
                    pitch: m.pitch[n],
                    position: m.position[n],
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
                let k = self.k_now(p);
                for f in &mut self.filter {
                    f.set_coefficients(self.g, k);
                }
            }
            let (mut xl, mut xr) = self.oscillators(&tables, p);
            if p.sub > 0.0 {
                let s = p.sub * sin_cycles(self.sub_phase) as f32;
                self.sub_phase += self.sub_step;
                self.sub_phase -= self.sub_phase.floor();
                xl += s;
                xr += s;
            }
            if p.noise > 0.0 {
                let s = p.noise * self.noise.next_bipolar();
                xl += s;
                xr += s;
            }
            let mut yl = self.filter[0].process_mode(xl, p.filter_mode);
            let mut yr = self.filter[1].process_mode(xr, p.filter_mode);
            if !(yl.is_finite() && yr.is_finite()) {
                for f in &mut self.filter {
                    f.reset();
                }
                (yl, yr) = (0.0, 0.0);
            }
            self.menv.next_sample();
            let gain = self.amp.next_sample() * self.vel_gain.next_value() * VOICE_GAIN;
            *lo += yl * gain;
            *ro += yr * gain;
            if !self.amp.is_active() {
                match self.pending.take() {
                    Some((key, velocity)) => self.start(key, velocity, p),
                    None => return,
                }
            }
        }
    }
}

/// The wavetable synth device.
#[derive(Debug)]
pub struct WavetableSynth {
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

impl Default for WavetableSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl WavetableSynth {
    /// A synth with default params (call `prepare` before processing). Builds the shared
    /// wavetable bank on the first call in the process, so create it off the audio thread.
    pub fn new() -> Self {
        let mut s = Self {
            values: PARAMS.map(|p| p.default),
            shared: Shared {
                sample_rate: 48_000.0,
                bank: factory_bank(),
                tables: [0; 2],
                base_position: [0.0; 2],
                pitch_offset: [0.0; 2],
                levels: [0.0; 2],
                sub: 0.0,
                noise: 0.0,
                unison: 1,
                uni_ratio: [1.0; MAX_UNISON],
                uni_top: 1.0,
                uni_gain: [(1.0, 1.0); MAX_UNISON],
                filter_mode: FilterMode::Lowpass,
                cutoff: 8_000.0,
                k: 1.0,
                resonance: 0.2,
                env_amount: 0.0,
                key_track: 0.0,
                env_to_position: [0.0; 2],
                env_to_pitch: 0.0,
                lfo_to_pitch: 0.0,
                lfo_to_cutoff: 0.0,
                lfo_to_position: [0.0; 2],
                bend: 0.0,
                wheel: 0.0,
                velocity_amount: 0.5,
                cv_cutoff_amount: 2.0,
                cv_position_amount: 0.5,
                cv_pitch_amount: CV_PITCH_RANGE,
                cv_resonance_amount: 1.0,
                cv_resonance_on: false,
            },
            voices: std::array::from_fn(|i| Voice::new(0, i as u64)),
            voice_limit: MAX_VOICES,
            next_order: 0,
            lfo: Lfo::new(derive_seed(0, 0)),
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            cv_level_db: CV_LEVEL_RANGE_DB,
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

    /// Recomputes the unison copies: detune ratios, stereo gains and the level normalisation.
    fn apply_unison(&mut self) {
        let n = (self.values[P_UNISON_VOICES as usize].round().max(1.0) as usize).min(MAX_UNISON);
        let (detune, spread) = (
            self.values[P_UNISON_DETUNE as usize],
            self.values[P_UNISON_SPREAD as usize],
        );
        let norm = 1.0 / (n as f32).sqrt();
        let s = &mut self.shared;
        s.unison = n;
        for u in 0..MAX_UNISON {
            // -1..=1 across the copies (0 for a lone copy).
            let x = if n > 1 && u < n {
                2.0 * u as f32 / (n - 1) as f32 - 1.0
            } else {
                0.0
            };
            s.uni_ratio[u] = (detune * x / 1_200.0).exp2();
            let (gl, gr) = balance_gains(spread * x);
            s.uni_gain[u] = (gl * norm, gr * norm);
        }
        s.uni_top = (detune / 1_200.0).exp2().max(1.0);
        if n == 1 {
            s.uni_top = 1.0;
        }
    }

    /// Applies one stored plain value. RT-safe (no allocation).
    fn apply(&mut self, id: u32, v: f32) {
        let sr = self.shared.sample_rate;
        let choice = |v: f32| v.max(0.0) as usize;
        let voices = &mut self.voices;
        match id {
            P_OSC1_TABLE | P_OSC2_TABLE => {
                let i = usize::from(id == P_OSC2_TABLE);
                self.shared.tables[i] = choice(v).min(TABLE_COUNT - 1);
            }
            P_OSC1_POSITION => self.shared.base_position[0] = v,
            P_OSC2_POSITION => self.shared.base_position[1] = v,
            P_OSC1_OCTAVE | P_OSC1_SEMI | P_OSC1_FINE => {
                self.shared.pitch_offset[0] = 12.0 * self.values[P_OSC1_OCTAVE as usize]
                    + self.values[P_OSC1_SEMI as usize]
                    + 0.01 * self.values[P_OSC1_FINE as usize];
            }
            P_OSC2_OCTAVE | P_OSC2_SEMI | P_OSC2_FINE => {
                self.shared.pitch_offset[1] = 12.0 * self.values[P_OSC2_OCTAVE as usize]
                    + self.values[P_OSC2_SEMI as usize]
                    + 0.01 * self.values[P_OSC2_FINE as usize];
            }
            P_OSC1_LEVEL => self.shared.levels[0] = v,
            P_OSC2_LEVEL => self.shared.levels[1] = v,
            P_SUB => self.shared.sub = v,
            P_NOISE => self.shared.noise = v,
            P_UNISON_VOICES | P_UNISON_DETUNE | P_UNISON_SPREAD => self.apply_unison(),
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
            P_AMP_S => voices.iter_mut().for_each(|vc| vc.amp.set_sustain(v)),
            P_AMP_R => voices.iter_mut().for_each(|vc| vc.amp.set_release_ms(v)),
            P_MOD_A => voices.iter_mut().for_each(|vc| vc.menv.set_attack_ms(v)),
            P_MOD_D => voices.iter_mut().for_each(|vc| vc.menv.set_decay_ms(v)),
            P_MOD_S => voices.iter_mut().for_each(|vc| vc.menv.set_sustain(v)),
            P_MOD_R => voices.iter_mut().for_each(|vc| vc.menv.set_release_ms(v)),
            P_ENV_POS1 => self.shared.env_to_position[0] = v,
            P_ENV_POS2 => self.shared.env_to_position[1] = v,
            P_ENV_PITCH => self.shared.env_to_pitch = v,
            P_LFO_WAVE => self.lfo.set_shape(LfoShape::from_index(choice(v))),
            P_LFO_RATE => self.lfo.set_rate(v, sr),
            P_LFO_PITCH => self.shared.lfo_to_pitch = v,
            P_LFO_CUTOFF => self.shared.lfo_to_cutoff = v,
            P_LFO_POS1 => self.shared.lfo_to_position[0] = v,
            P_LFO_POS2 => self.shared.lfo_to_position[1] = v,
            P_VEL_AMP => self.shared.velocity_amount = v,
            P_CV_CUTOFF => self.shared.cv_cutoff_amount = v,
            P_CV_POSITION => self.shared.cv_position_amount = v,
            P_CV_PITCH => self.shared.cv_pitch_amount = v * CV_PITCH_RANGE,
            P_CV_RESONANCE => self.shared.cv_resonance_amount = v,
            P_CV_LEVEL => self.cv_level_db = v * CV_LEVEL_RANGE_DB,
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
            NoteKind::PitchBend { value } => self.shared.bend = value.clamp(-1.0, 1.0) * BEND_RANGE,
            NoteKind::Cc {
                controller: 1,
                value,
            } => self.shared.wheel = value.clamp(0.0, 1.0),
            // The other controllers, pressure and expressions are ignored.
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

impl Device for WavetableSynth {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.shared.sample_rate = sr;
        for v in &mut self.voices {
            v.amp.set_sample_rate(sr);
            v.menv.set_sample_rate(sr);
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
            v.menv.reset();
            for f in &mut v.filter {
                f.reset();
            }
            v.pending = None;
            v.reseed(self.seed, i as u64);
        }
        self.next_order = 0;
        self.shared.bend = 0.0;
        self.shared.wheel = 0.0;
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
        let cv_position = *ctx.input(POSITION_CV_IN);
        let cv_res = *ctx.input(RESONANCE_CV_IN);
        self.shared.cv_resonance_on = ctx.is_connected(RESONANCE_CV_IN);
        let (mut left, mut right) = ([0.0f32; BATCH], [0.0f32; BATCH]);
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
                position: &cv_position[start..end],
                resonance: &cv_res[start..end],
            };
            for v in self.voices.iter_mut().filter(|v| v.is_active()) {
                v.render(&mut left[start..end], &mut right[start..end], &m, &p);
            }
            start = end;
        }
        let level_on = ctx.is_connected(LEVEL_CV_IN);
        let cv_level = *ctx.input(LEVEL_CV_IN);
        for (n, &lv) in cv_level.iter().enumerate() {
            let mut g = self.master.next_value();
            if level_on {
                g *= db_gain(self.cv_level_db, lv);
            }
            left[n] = soft_limit(left[n] * g);
            right[n] = soft_limit(right[n] * g);
        }
        if ctx.is_connected(PAN_CV_IN) {
            // The stereo pair balanced by the pan CV (unity at the centre).
            let cv_pan = *ctx.input(PAN_CV_IN);
            for n in 0..BATCH {
                let (gl, gr) = balance_gains(self.cv_pan_amount * clean(cv_pan[n]));
                left[n] *= gl;
                right[n] *= gr;
            }
        }
        *ctx.output(OUT_L) = left;
        *ctx.output(OUT_R) = right;
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
    use atomik_testkit::signal::{db, dominant_frequency, magnitude_spectrum, max_step, peak, rms};
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
    /// The table that starts as a sine and ends as a saw (`sine>saw`).
    const SINE_SAW: f32 = 2.0;

    fn synth() -> (DeviceHarness, Arc<VoiceProbe>) {
        let probe = Arc::new(VoiceProbe::default());
        let mut s = WavetableSynth::new();
        s.probe = Some(probe.clone());
        (harness(Box::new(s)), probe)
    }

    /// Oscillator 1 on the sine frame alone, filter wide open and static, no modulation.
    fn pure_sine(h: &mut DeviceHarness) {
        h.set_param("osc1.table", SINE_SAW);
        h.set_param("osc1.position", 0.0);
        h.set_param("osc1.level", 1.0);
        h.set_param("osc2.level", 0.0);
        h.set_param("env.mod.to_pos1", 0.0);
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

    fn push(h: &mut DeviceHarness, offset: u16, kind: NoteKind) {
        h.push_note("notes_in", NoteEvent { offset, kind });
    }

    #[test]
    fn descriptor_validates() {
        assert_keys(
            &DESCRIPTOR,
            &[
                "osc1.table",
                "osc1.position",
                "osc1.octave",
                "osc1.semi",
                "osc1.fine",
                "osc1.level",
                "osc2.table",
                "osc2.position",
                "osc2.octave",
                "osc2.semi",
                "osc2.fine",
                "osc2.level",
                "mix.sub",
                "mix.noise",
                "unison.voices",
                "unison.detune",
                "unison.spread",
                "filter.mode",
                "filter.cutoff",
                "filter.resonance",
                "filter.env_amount",
                "filter.key_track",
                "env.amp.attack",
                "env.amp.decay",
                "env.amp.sustain",
                "env.amp.release",
                "env.mod.attack",
                "env.mod.decay",
                "env.mod.sustain",
                "env.mod.release",
                "env.mod.to_pos1",
                "env.mod.to_pos2",
                "env.mod.to_pitch",
                "lfo.wave",
                "lfo.rate",
                "lfo.to_pitch",
                "lfo.to_cutoff",
                "lfo.to_pos1",
                "lfo.to_pos2",
                "velocity.amp",
                "cv.cutoff_amount",
                "cv.position_amount",
                "voices",
                "master.level",
                "pitch_cv_trim",
                "filter.resonance_cv_trim",
                "master.level_cv_trim",
                "pan_cv_trim",
            ],
            &[
                "notes_in",
                "cutoff_cv_in",
                "pitch_cv_in",
                "position_cv_in",
                "resonance_cv_in",
                "level_cv_in",
                "pan_cv_in",
                "out_l",
                "out_r",
                "lfo_cv_out",
            ],
        );
        assert_eq!(DESCRIPTOR.type_id, "atomik.wavetable-synth");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
        let t = DESCRIPTOR.param_by_key("osc1.table").unwrap();
        assert_eq!(t.unit, Unit::Choice(&TABLE_NAMES));
        assert_eq!(t.max, (TABLE_COUNT - 1) as f32);
        let c = DESCRIPTOR.param_by_key("filter.cutoff").unwrap();
        assert_eq!((c.taper, c.unit), (Taper::Exponential, Unit::Hz));
        assert!((DESCRIPTOR.param_by_key("osc2.fine").unwrap().default - 7.0).abs() < 1e-6);
        // The ids are the indices.
        for (i, p) in DESCRIPTOR.params.iter().enumerate() {
            assert_eq!(p.id.0 as usize, i, "{}", p.key);
        }
    }

    #[test]
    fn sine_frame_at_key_69_is_440_hz() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.push_note("notes_in", on(0, 69, 1.0));
        let o = h.render(seconds(1.0), &OUT);
        let f = dominant_frequency(&o[0][..48_000], SR);
        assert!((f - 440.0).abs() < 1.0, "{f} Hz");
        assert!(rms(&o[0]) > 0.05, "audible: {}", rms(&o[0]));
        assert_eq!(o[0], o[1], "one unison copy: L == R");
    }

    #[test]
    fn default_patch_sounds_and_stays_bounded() {
        let (mut h, _) = synth();
        for (i, k) in [48u8, 55, 60, 64].iter().enumerate() {
            h.push_note("notes_in", on(i as u16, *k, 0.9));
        }
        let o = h.render(seconds(0.5), &OUT);
        assert!(rms(&o[0]) > 0.02);
        assert!(peak(&o[0]) < 1.5, "peak {}", peak(&o[0]));
    }

    #[test]
    fn seventeen_notes_steal_the_oldest_voice() {
        let (mut h, probe) = synth();
        h.set_param("voices", 16.0);
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
    fn voices_param_limits_polyphony() {
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
    fn lowering_voices_fades_out_the_extra_voices() {
        let (mut h, probe) = synth();
        for (i, key) in [60u8, 64, 67, 71].iter().enumerate() {
            h.push_note("notes_in", on(i as u16, *key, 0.8));
        }
        h.render(4, &OUT);
        assert_eq!(probe.active(), 4);
        h.set_param("voices", 2.0);
        h.render(4, &OUT);
        assert_eq!(probe.active(), 2);
    }

    #[test]
    fn same_key_retriggers_same_voice_continuously() {
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
    fn steal_fades_over_2ms_without_click() {
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
    fn release_decays_below_minus_90_dbfs_and_voice_goes_idle() {
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
    fn all_off_releases_every_voice() {
        let (mut h, probe) = synth();
        for (i, k) in [48u8, 52, 55].iter().enumerate() {
            h.push_note("notes_in", on(i as u16, *k, 1.0));
        }
        h.render(seconds(0.1), &OUT);
        assert_eq!(probe.active(), 3);
        push(&mut h, 0, NoteKind::AllOff);
        h.render(seconds(0.5), &OUT);
        assert_eq!(probe.active(), 0);
    }

    #[test]
    fn note_on_is_sample_accurate() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.position", 1.0); // the saw frame starts away from zero
        h.push_note("notes_in", on(37, 60, 1.0));
        let o = h.render(1, &OUT);
        assert!(o[0][..37].iter().all(|s| s.to_bits() == 0));
        assert!(o[0][38..].iter().any(|s| s.abs() > 1e-4));
    }

    #[test]
    fn velocity_amount_scales_amplitude() {
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
    fn writes_lfo_to_cv_out_every_sample() {
        let (mut h, _) = synth();
        h.set_param("lfo.rate", 2.0);
        let o = h.render(seconds(1.0), &["lfo_cv_out"]).remove(0);
        assert!(o.iter().all(|v| (-1.0..=1.0).contains(v)));
        // Sine LFO at 2 Hz: +1 at 0.125 s, -1 at 0.375 s.
        assert!((o[6_000] - 1.0).abs() < 1e-3 && (o[18_000] + 1.0).abs() < 1e-3);
        assert!(max_step(&o) < 0.001);
    }

    #[test]
    fn lfo_to_pitch_shifts_by_semitones() {
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
    fn pitch_bend_moves_every_voice_by_up_to_two_semitones() {
        let freq = |bend: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            push(&mut h, 0, NoteKind::PitchBend { value: bend });
            h.push_note("notes_in", on(1, 57, 1.0)); // 220 Hz
            let o = h.render(seconds(0.5), &OUT).remove(0);
            dominant_frequency(&o[4_800..24_000], SR)
        };
        assert!((freq(0.0) - 220.0).abs() < 1.0);
        let up = 220.0 * 2f64.powf(2.0 / 12.0);
        let down = 220.0 * 2f64.powf(-1.0 / 12.0);
        assert!((freq(1.0) - up).abs() < 1.5, "{}", freq(1.0));
        assert!((freq(-0.5) - down).abs() < 1.0, "{}", freq(-0.5));
        assert!((freq(5.0) - up).abs() < 1.5, "out of range is clamped");
    }

    #[test]
    fn bend_survives_all_off_and_reset_clears_it() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        push(&mut h, 0, NoteKind::PitchBend { value: 1.0 });
        push(&mut h, 1, NoteKind::AllOff);
        h.push_note("notes_in", on(2, 57, 1.0));
        let o = h.render(seconds(0.5), &OUT).remove(0);
        let bent = dominant_frequency(&o[4_800..24_000], SR);
        assert!(bent > 240.0, "{bent}");
        let mut s = WavetableSynth::new();
        s.handle(NoteKind::PitchBend { value: -1.0 });
        s.handle(NoteKind::Cc {
            controller: 1,
            value: 1.0,
        });
        assert_eq!((s.shared.bend, s.shared.wheel), (-BEND_RANGE, 1.0));
        s.reset();
        assert_eq!((s.shared.bend, s.shared.wheel), (0.0, 0.0));
    }

    #[test]
    fn mod_wheel_adds_lfo_vibrato_and_other_ccs_do_nothing() {
        let spread = |kind: Option<NoteKind>| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("lfo.wave", 3.0); // square: +1 first half, -1 second
            h.set_param("lfo.rate", 0.5);
            if let Some(kind) = kind {
                push(&mut h, 0, kind);
            }
            h.push_note("notes_in", on(1, 57, 1.0));
            let o = h.render(seconds(2.0), &OUT).remove(0);
            let up = dominant_frequency(&o[4_800..43_200], SR);
            let down = dominant_frequency(&o[52_800..91_200], SR);
            up - down
        };
        let cc = |controller, value| Some(NoteKind::Cc { controller, value });
        assert!(spread(None).abs() < 1.0);
        assert!(spread(cc(7, 1.0)).abs() < 1.0, "volume is ignored");
        // A full wheel: +1 and -1 semitone around 220 Hz, about 25 Hz apart.
        let s = spread(cc(1, 1.0));
        assert!(s > 20.0 && s < 30.0, "{s}");
    }

    // ------------------------------------------------------------------ wavetable behaviour

    /// Brightness of a held note at a fixed frame position of the sine-to-saw table.
    fn brightness_at(position: f32) -> f32 {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.position", position);
        h.push_note("notes_in", on(0, 45, 1.0));
        brightness(&h.render(seconds(0.4), &OUT)[0][4_800..])
    }

    #[test]
    fn position_picks_the_frame() {
        let (b0, b1) = (brightness_at(0.0), brightness_at(1.0));
        assert!(b1 > 4.0 * b0, "sine {b0} vs saw {b1}");
        // Monotone along the way (each frame adds a harmonic).
        let mut prev = b0;
        for i in 1..=6 {
            let b = brightness_at(i as f32 / 6.0);
            assert!(b > prev, "{i}: {b} <= {prev}");
            prev = b;
        }
    }

    #[test]
    fn tables_sound_different() {
        let frame = |table: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("osc1.table", table);
            h.set_param("osc1.position", 0.5);
            h.push_note("notes_in", on(0, 48, 1.0));
            h.render(seconds(0.2), &OUT).remove(0)
        };
        let all: Vec<Vec<f32>> = (0..TABLE_COUNT).map(|t| frame(t as f32)).collect();
        for t in 0..TABLE_COUNT {
            assert!(rms(&all[t]) > 0.02, "table {t} is silent");
            for u in 0..t {
                assert_ne!(all[t], all[u], "tables {t} and {u}");
            }
        }
    }

    #[test]
    fn position_morph_is_continuous() {
        // A held note whose frame position is swept across the whole table every 100 ms:
        // the output has no click. The sine-to-saw table changes by a harmonic per frame.
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.octave", -2.0);
        h.set_param("env.amp.sustain", 1.0);
        h.push_note("notes_in", on(0, 57, 0.8));
        let mut out = h.render(seconds(0.1), &OUT).remove(0);
        for i in 0..=100 {
            h.set_param("osc1.position", i as f32 / 100.0);
            out.extend(h.render(1, &OUT).remove(0));
        }
        out.extend(h.render(seconds(0.1), &OUT).remove(0));
        let early = max_step(&out[..4_000]);
        // A saw has a jump per cycle: compare against the steady-state saw step instead.
        let steady = {
            let (mut h2, _) = synth();
            pure_sine(&mut h2);
            h2.set_param("osc1.octave", -2.0);
            h2.set_param("osc1.position", 1.0);
            h2.set_param("env.amp.sustain", 1.0);
            h2.push_note("notes_in", on(0, 57, 0.8));
            let o = h2.render(seconds(0.3), &OUT).remove(0);
            max_step(&o[9_600..])
        };
        assert!(early < 0.05);
        assert!(
            max_step(&out[4_800..]) <= steady * 1.5 + 0.01,
            "{} vs {steady}",
            max_step(&out[4_800..])
        );
    }

    #[test]
    fn mod_envelope_sweeps_the_position() {
        let bright = |amount: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("env.mod.to_pos1", amount);
            h.set_param("env.mod.attack", 0.5);
            h.set_param("env.mod.decay", 200.0);
            h.set_param("env.mod.sustain", 0.0);
            h.set_param("env.amp.sustain", 1.0);
            h.push_note("notes_in", on(0, 45, 1.0));
            let o = h.render(seconds(1.0), &OUT).remove(0);
            // Early (envelope high) and late (envelope spent) brightness.
            (brightness(&o[100..2_000]), brightness(&o[40_000..]))
        };
        let (early, late) = bright(1.0);
        assert!(early > 2.0 * late, "{early} vs {late}");
        let (early0, late0) = bright(0.0);
        assert!(
            (early0 - late0).abs() < 0.05 * late0 + 1e-4,
            "{early0} {late0}"
        );
    }

    #[test]
    fn lfo_sweeps_the_position() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.position", 0.5);
        h.set_param("lfo.wave", 3.0); // square: +1 then -1
        h.set_param("lfo.rate", 0.5);
        h.set_param("lfo.to_pos1", 0.5);
        h.set_param("env.amp.sustain", 1.0);
        h.push_note("notes_in", on(0, 45, 1.0));
        let o = h.render(seconds(2.0), &OUT).remove(0);
        let high = brightness(&o[4_800..43_200]); // lfo +1: position 1
        let low = brightness(&o[52_800..91_200]); // lfo -1: position 0
        assert!(high > 4.0 * low, "{high} vs {low}");
    }

    #[test]
    fn position_cv_moves_the_frame() {
        let level = |cv: Option<f32>, amount: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("osc1.position", 0.5);
            h.set_param("cv.position_amount", amount);
            if let Some(cv) = cv {
                *h.input_mut("position_cv_in") = [cv; BATCH];
            }
            h.push_note("notes_in", on(0, 45, 1.0));
            brightness(&h.render(seconds(0.4), &OUT)[0][4_800..])
        };
        let none = level(None, 0.5);
        assert!((none - level(Some(0.0), 0.5)).abs() < 1e-6);
        assert!(level(Some(1.0), 0.5) > none * 1.2);
        assert!(level(Some(-1.0), 0.5) < none * 0.8);
        assert!(
            (level(Some(1.0), 0.0) - none).abs() < 1e-6,
            "amount 0 cancels"
        );
        assert!(
            (level(Some(9.0), 0.5) - level(Some(1.0), 0.5)).abs() < 1e-6,
            "clamped"
        );
        assert!(
            level(Some(1.0), -0.5) < none * 0.8,
            "negative amount inverts"
        );
    }

    #[test]
    fn env_to_pitch_bends_the_note() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("env.mod.to_pitch", 12.0);
        h.set_param("env.mod.attack", 0.5);
        h.set_param("env.mod.decay", 200.0);
        h.set_param("env.mod.sustain", 0.0);
        h.set_param("env.amp.sustain", 1.0);
        h.push_note("notes_in", on(0, 57, 1.0)); // 220 Hz, an octave up at first
        let o = h.render(seconds(1.0), &OUT).remove(0);
        let early = dominant_frequency(&o[100..1_000], SR);
        let late = dominant_frequency(&o[40_000..], SR);
        assert!(early > 280.0, "{early}");
        assert!((late - 220.0).abs() < 2.0, "{late}");
    }

    #[test]
    fn osc_tuning_octave_semi_fine() {
        let freq = |setup: &dyn Fn(&mut DeviceHarness)| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            setup(&mut h);
            h.push_note("notes_in", on(0, 57, 1.0));
            let o = h.render(seconds(0.5), &OUT).remove(0);
            dominant_frequency(&o[4_800..], SR)
        };
        assert!((freq(&|_| {}) - 220.0).abs() < 1.0);
        assert!((freq(&|h| h.set_param("osc1.octave", 1.0)) - 440.0).abs() < 2.0);
        let up7 = 220.0 * 2f64.powf(7.0 / 12.0);
        assert!((freq(&|h| h.set_param("osc1.semi", 7.0)) - up7).abs() < 2.0);
        let cents = 220.0 * 2f64.powf(0.5 / 12.0);
        assert!((freq(&|h| h.set_param("osc1.fine", 50.0)) - cents).abs() < 1.0);
    }

    #[test]
    fn sub_adds_an_octave_below_and_noise_adds_energy() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.level", 0.0);
        h.set_param("mix.sub", 1.0);
        h.push_note("notes_in", on(0, 69, 1.0));
        let o = h.render(seconds(0.5), &OUT).remove(0);
        let f = dominant_frequency(&o[4_800..], SR);
        assert!((f - 220.0).abs() < 1.0, "sub one octave below 440: {f}");
        assert!(rms(&o[4_800..]) > 0.05);

        let silent = |noise: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("osc1.level", 0.0);
            h.set_param("mix.noise", noise);
            h.push_note("notes_in", on(0, 69, 1.0));
            rms(&h.render(seconds(0.3), &OUT)[0][4_800..])
        };
        assert_eq!(silent(0.0), 0.0);
        assert!(silent(0.5) > 0.02);
    }

    #[test]
    fn osc2_mixes_with_osc1_by_level() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.level", 0.0);
        h.set_param("osc2.table", SINE_SAW);
        h.set_param("osc2.position", 0.0);
        h.set_param("osc2.fine", 0.0);
        h.set_param("osc2.octave", 1.0);
        h.set_param("osc2.level", 1.0);
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(0.5), &OUT).remove(0);
        let f = dominant_frequency(&o[4_800..], SR);
        assert!((f - 440.0).abs() < 2.0, "{f}");
    }

    // ----------------------------------------------------------------------------- unison

    #[test]
    fn unison_spreads_across_the_stereo_field_and_keeps_level() {
        let render = |voices: f32, spread: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("osc1.position", 1.0);
            h.set_param("unison.voices", voices);
            h.set_param("unison.spread", spread);
            h.set_param("unison.detune", 15.0);
            h.push_note("notes_in", on(0, 48, 1.0));
            h.render(seconds(0.5), &OUT)
        };
        let single = render(1.0, 1.0);
        assert_eq!(single[0], single[1], "one copy is centred");
        let wide = render(5.0, 1.0);
        assert_ne!(wide[0], wide[1], "spread");
        let mono = render(5.0, 0.0);
        assert_eq!(mono[0], mono[1], "spread 0 keeps the copies in the centre");
        // The 1/sqrt(n) scaling keeps the level of n decorrelated copies near that of one.
        let (a, b) = (rms(&single[0][9_600..]), rms(&mono[0][9_600..]));
        assert!(db(b / a).abs() < 3.0, "{} dB", db(b / a));
    }

    #[test]
    fn unison_detune_widens_the_peak() {
        let beating = |detune: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("unison.voices", 3.0);
            h.set_param("unison.spread", 0.0);
            h.set_param("unison.detune", detune);
            h.set_param("env.amp.sustain", 1.0);
            h.push_note("notes_in", on(0, 69, 1.0));
            let o = h.render(seconds(1.0), &OUT).remove(0);
            // Amplitude swing of the sum: copies in phase add, out of phase cancel.
            let env: Vec<f32> = o[9_600..].chunks(480).map(peak).collect();
            let hi = env.iter().cloned().fold(0.0f32, f32::max);
            let lo = env.iter().cloned().fold(f32::MAX, f32::min);
            (hi - lo) / hi
        };
        assert!(beating(0.0) < 0.05);
        assert!(beating(20.0) > 0.2);
    }

    #[test]
    fn unison_copies_start_at_seeded_phases() {
        let first = |seed: u64| {
            let mut h =
                DeviceHarness::with_options(Box::new(WavetableSynth::new()), SR, 120.0, seed);
            pure_sine(&mut h);
            h.set_param("unison.voices", 4.0);
            h.set_param("unison.detune", 0.0);
            h.set_param("unison.spread", 0.0);
            h.push_note("notes_in", on(0, 60, 1.0));
            h.render(2, &OUT).remove(0)
        };
        assert_eq!(first(3), first(3));
        assert_ne!(
            first(3),
            first(4),
            "a different patch seed, different phases"
        );
    }

    // ------------------------------------------------------------------------ band limiting

    /// Highest level (dB below the fundamental) among the bins that are not near a harmonic of
    /// `f0`, for a Hann-windowed second of `x` (starting after the attack).
    fn aliasing_db(x: &[f32], f0: f64) -> f64 {
        let n = 1 << 15;
        let mags = magnitude_spectrum(&x[..n]);
        let bin_hz = SR / n as f64;
        let near_harmonic = |k: usize| {
            let h = (k as f64 * bin_hz / f0).round();
            h >= 1.0 && (k as f64 * bin_hz - h * f0).abs() < 12.0 * bin_hz
        };
        let fundamental = mags
            .iter()
            .enumerate()
            .filter(|(k, _)| (*k as f64 * bin_hz - f0).abs() < 3.0 * bin_hz)
            .fold(0.0f64, |m, (_, v)| m.max(*v));
        // Skip the lowest bins (DC leakage) and look at everything else.
        let worst = mags
            .iter()
            .enumerate()
            .skip(8)
            .filter(|(k, _)| !near_harmonic(*k))
            .fold(0.0f64, |m, (_, v)| m.max(*v));
        20.0 * (worst / fundamental).log10()
    }

    #[test]
    fn high_notes_do_not_alias() {
        // A saw frame played at pitches whose harmonics run well past Nyquist: the table's mip
        // levels drop them, so nothing folds back below -70 dB.
        for key in [84u8, 96, 105, 112] {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("osc1.table", 0.0); // saw>square, frame 0: a saw
            h.set_param("env.amp.sustain", 1.0);
            h.push_note("notes_in", on(0, key, 1.0));
            let o = h.render(seconds(1.0), &OUT).remove(0);
            let f0 = 440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0);
            let alias = aliasing_db(&o[9_600..], f0);
            assert!(alias < -70.0, "key {key} ({f0:.0} Hz): alias {alias:.1} dB");
        }
    }

    #[test]
    fn the_alias_meter_catches_a_naive_saw() {
        // The same measurement on a non-band-limited saw at 3.5 kHz sits far above the limit.
        let f0 = 3_520.0f64;
        let x: Vec<f32> = (0..48_000)
            .map(|n| {
                let p = f0 * n as f64 / SR;
                (1.0 - 2.0 * (p - p.floor())) as f32
            })
            .collect();
        let alias = aliasing_db(&x, f0);
        assert!(alias > -40.0, "{alias} dB");
    }

    #[test]
    fn pitch_modulation_and_unison_detune_stay_band_limited() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.table", 0.0);
        h.set_param("unison.voices", 7.0);
        h.set_param("unison.detune", 100.0);
        h.set_param("unison.spread", 0.0);
        h.set_param("env.mod.to_pitch", 12.0);
        h.set_param("env.amp.sustain", 1.0);
        h.push_note("notes_in", on(0, 100, 1.0));
        let o = h.render(seconds(1.0), &OUT).remove(0);
        // The widest copy runs at about 4.2 kHz at most: the top is cut for it, so there is
        // nothing above Nyquist to fold; the output is finite and bounded.
        assert!(o.iter().all(|s| s.is_finite() && s.abs() < 4.0));
    }

    // --------------------------------------------------------------------------- determinism

    #[test]
    fn two_renders_are_identical() {
        let render = || {
            let mut h = DeviceHarness::with_options(Box::new(WavetableSynth::new()), SR, 120.0, 77);
            h.set_param("unison.voices", 5.0);
            h.set_param("osc1.table", 5.0);
            h.set_param("osc2.table", 9.0);
            h.set_param("lfo.wave", 4.0);
            h.set_param("lfo.rate", 7.0);
            h.set_param("lfo.to_pos1", 0.5);
            h.set_param("lfo.to_pos2", 0.5);
            h.set_param("lfo.to_cutoff", 1.0);
            h.set_param("mix.noise", 0.2);
            h.set_param("mix.sub", 0.3);
            for (i, k) in [40u8, 47, 52, 59].iter().enumerate() {
                h.push_note("notes_in", on(i as u16 * 5, *k, 0.7 + 0.05 * i as f32));
            }
            let a = h.render(seconds(0.3), &OUT);
            h.push_note("notes_in", off(10, 47));
            h.push_note("notes_in", on(20, 76, 0.9));
            let b = h.render(seconds(0.3), &OUT);
            (a, b)
        };
        assert_eq!(render(), render());
    }

    #[test]
    fn reset_replays_the_same_audio() {
        let mut h = DeviceHarness::with_options(Box::new(WavetableSynth::new()), SR, 120.0, 5);
        h.set_param("unison.voices", 3.0);
        h.set_param("mix.noise", 0.3);
        let play = |h: &mut DeviceHarness| {
            h.push_note("notes_in", on(0, 55, 0.8));
            h.render(seconds(0.2), &OUT)
        };
        let first = play(&mut h);
        h.device_mut().reset();
        assert_eq!(play(&mut h), first);
    }

    #[test]
    fn every_param_at_min_max_and_default_stays_finite() {
        let (mut h, _) = synth();
        for (i, k) in [36u8, 60, 84, 108].iter().enumerate() {
            h.push_note("notes_in", on(i as u16, *k, 1.0));
        }
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }

    #[test]
    fn output_is_bounded_with_everything_open() {
        let (mut h, _) = synth();
        h.set_param("unison.voices", 7.0);
        h.set_param("osc1.level", 1.0);
        h.set_param("osc2.level", 1.0);
        h.set_param("mix.sub", 1.0);
        h.set_param("mix.noise", 1.0);
        h.set_param("filter.resonance", 1.0);
        h.set_param("master.level", 6.0);
        h.set_param("voices", 16.0);
        for (i, k) in (36u8..52).enumerate() {
            h.push_note("notes_in", on(i as u16, k, 1.0));
        }
        let o = h.render(seconds(1.0), &OUT);
        let bad = o
            .iter()
            .flatten()
            .find(|s| !(s.is_finite() && s.abs() <= 4.0));
        assert!(bad.is_none(), "{bad:?}");
    }

    #[test]
    fn filter_modes_and_cutoff_cv_shape_the_sound() {
        let level = |mode: f32, cv: Option<f32>| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("osc1.position", 1.0);
            h.set_param("filter.mode", mode);
            h.set_param("filter.cutoff", 300.0);
            if let Some(cv) = cv {
                *h.input_mut("cutoff_cv_in") = [cv; BATCH];
            }
            h.push_note("notes_in", on(0, 36, 1.0));
            brightness(&h.render(seconds(0.4), &OUT)[0][4_800..])
        };
        assert!(
            level(2.0, None) > level(0.0, None) * 2.0,
            "HP is brighter than LP"
        );
        assert!(
            level(0.0, Some(1.0)) > level(0.0, None) * 2.0,
            "CV opens the filter"
        );
    }

    #[test]
    fn pan_cv_balances_the_stereo_pair() {
        let sides = |cv: Option<f32>| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            if let Some(cv) = cv {
                *h.input_mut("pan_cv_in") = [cv; BATCH];
            }
            h.push_note("notes_in", on(0, 57, 1.0));
            let o = h.render(seconds(0.4), &OUT);
            (rms(&o[0][9_600..]), rms(&o[1][9_600..]))
        };
        let (l0, r0) = sides(None);
        assert_eq!(l0.to_bits(), r0.to_bits(), "unconnected: dual mono");
        let (l, r) = sides(Some(1.0));
        assert!(l < r0 * 1e-3 && (r - r0).abs() < r0 * 1e-3, "{l} {r}");
    }
}
