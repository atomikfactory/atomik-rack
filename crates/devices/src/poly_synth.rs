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
//!   combination. With `spread` 0 (the default) `out_l == out_r`.
//! * Channel messages: pitch bend moves every voice by up to `bend.range` semitones (default 2;
//!   RPN 0 changes it; at the 16-sample control rate) and the mod wheel (CC 1) adds up to one
//!   semitone of LFO vibrato ([`WHEEL_VIBRATO`]) on top of `lfo.to_pitch`. Both are device state
//!   that `AllOff` leaves alone (as a hardware synth does) and `reset` clears. The sustain pedal
//!   (CC 64), volume (CC 7), expression (CC 11) and pressure are kept by a
//!   [`ControllerState`]; other controllers are ignored.
//! * Randomness (noise, sample & hold LFO, `drift`) comes from `Rng`s derived from
//!   `PrepareInfo::seed`.
//!
//! The appended params (every default leaves the sound above bit for bit as it was):
//!
//! * **Wave bank.** `osc1.table` / `osc2.table` pick one of the 32 additive waves of
//!   [`atomik_dsp::wavetable::wave_bank`] (0 keeps the PolyBLEP `wave`); the read is band-limited
//!   by mip level.
//! * **Oscillator 2 mode.** `osc2.mode`: `normal`; `ring` (oscillator 2 is the product of the two
//!   oscillators); `fm` (oscillator 1 modulates the frequency of oscillator 2 by `osc2.amount`,
//!   up to twice its pitch each way); `sync` (oscillator 2 restarts with every cycle of
//!   oscillator 1, `osc2.amount` raising its pitch by up to two octaves; the reset step is
//!   smoothed with a one-sided BLEP).
//! * **Filters.** `filter.type` picks the 12 dB state-variable filter or the 24 dB ladder
//!   (lowpass, `filter.mode` ignored) with `filter.drive` (anti-aliased clipping in front of the
//!   loop). `filter2.mode` (off, LP, BP, HP) adds a second 12 dB filter, in `series` or
//!   `parallel` (the two outputs averaged); its cutoff is `filter2.cutoff` times the share
//!   `filter.link` of the first filter's modulation (envelope, key tracking, LFOs, CV).
//! * **LFO 2** (`lfo2.*`) is a second global LFO to pitch, cutoff and level (tremolo).
//! * **Mod envelope.** `env.mod.*` is a third ADSR per voice; `env.mod.amount` (bipolar) scales it
//!   onto `env.mod.dest`: pitch (24 semitones), osc 2 pitch, osc mix, osc 2 amount or filter 2
//!   cutoff (4 octaves).
//! * **Play.** `glide` is the time in ms a pitch takes to cover most of the way to a new key
//!   (from the last key played). `legato` makes the synth monophonic: a key played while another
//!   is held changes the pitch only (envelopes run on), releasing a key returns to the one still
//!   held. `spread` pans the voices by key (low left, high right, middle C centred) and makes
//!   the output a true stereo pair. `drift` detunes every voice by a seeded random amount (up to
//!   20 cents) that wanders slowly: an ageing analogue voice.
//! * **Effects.** `chorus.*` (a stereo chorus) and `delay.*` (a stereo ping-pong echo) run on
//!   the output after the pan; a mix of 0 is exactly the dry signal.
//!
//! The same voice code runs the mono bass ([`crate::mono_bass`]) and the dual layer
//! ([`crate::dual_layer`]), which are devices of their own with other defaults and sockets.

use atomik_core::NoteEvent;
use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::chorus::Chorus;
use atomik_dsp::wavetable::{WAVE_COUNT, WaveBank, level_for, wave_bank};
use atomik_dsp::{
    Adsr, AdsrStage, DelayLine, FilterMode, Ladder, Lfo, LfoShape, PolyBlepOsc, Rng, Smoother, Svf,
    Waveform, balance_gains, exp2_det, note_to_hz,
};

use crate::controller_state::{ControllerState, DEFAULT_BEND_RANGE, Effect, MAX_BEND_RANGE};
use crate::cv_in::{clean, db_gain};

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

pub(crate) const P_OSC1_WAVE: u32 = 0;
pub(crate) const P_OSC2_WAVE: u32 = 1;
pub(crate) const P_OSC1_OCTAVE: u32 = 2;
pub(crate) const P_OSC2_OCTAVE: u32 = 3;
pub(crate) const P_OSC1_SEMI: u32 = 4;
pub(crate) const P_OSC2_SEMI: u32 = 5;
pub(crate) const P_OSC1_FINE: u32 = 6;
pub(crate) const P_OSC2_FINE: u32 = 7;
pub(crate) const P_OSC_MIX: u32 = 8;
pub(crate) const P_NOISE: u32 = 9;
pub(crate) const P_FILTER_MODE: u32 = 10;
pub(crate) const P_CUTOFF: u32 = 11;
pub(crate) const P_RESONANCE: u32 = 12;
pub(crate) const P_ENV_AMOUNT: u32 = 13;
pub(crate) const P_KEY_TRACK: u32 = 14;
pub(crate) const P_AMP_A: u32 = 15;
pub(crate) const P_AMP_D: u32 = 16;
pub(crate) const P_AMP_R: u32 = 17;
pub(crate) const P_AMP_S: u32 = 18;
pub(crate) const P_FLT_A: u32 = 19;
pub(crate) const P_FLT_D: u32 = 20;
pub(crate) const P_FLT_R: u32 = 21;
pub(crate) const P_FLT_S: u32 = 22;
pub(crate) const P_LFO_WAVE: u32 = 23;
pub(crate) const P_LFO_RATE: u32 = 24;
pub(crate) const P_LFO_PITCH: u32 = 25;
pub(crate) const P_LFO_CUTOFF: u32 = 26;
pub(crate) const P_VEL_AMP: u32 = 27;
pub(crate) const P_CV_AMOUNT: u32 = 28;
pub(crate) const P_VOICES: u32 = 29;
pub(crate) const P_MASTER: u32 = 30;
pub(crate) const P_CV_PITCH: u32 = 31;
pub(crate) const P_CV_RESONANCE: u32 = 32;
pub(crate) const P_CV_LEVEL: u32 = 33;
pub(crate) const P_CV_PAN: u32 = 34;
pub(crate) const P_BEND_RANGE: u32 = 35;
pub(crate) const P_OSC1_TABLE: u32 = 36;
pub(crate) const P_OSC2_TABLE: u32 = 37;
pub(crate) const P_OSC2_MODE: u32 = 38;
pub(crate) const P_OSC2_AMOUNT: u32 = 39;
pub(crate) const P_FILTER_TYPE: u32 = 40;
pub(crate) const P_DRIVE: u32 = 41;
pub(crate) const P_F2_MODE: u32 = 42;
pub(crate) const P_F2_CUTOFF: u32 = 43;
pub(crate) const P_F2_RESONANCE: u32 = 44;
pub(crate) const P_F2_LINK: u32 = 45;
pub(crate) const P_F2_ROUTING: u32 = 46;
pub(crate) const P_LFO2_WAVE: u32 = 47;
pub(crate) const P_LFO2_RATE: u32 = 48;
pub(crate) const P_LFO2_PITCH: u32 = 49;
pub(crate) const P_LFO2_CUTOFF: u32 = 50;
pub(crate) const P_LFO2_LEVEL: u32 = 51;
pub(crate) const P_MENV_A: u32 = 52;
pub(crate) const P_MENV_D: u32 = 53;
pub(crate) const P_MENV_S: u32 = 54;
pub(crate) const P_MENV_R: u32 = 55;
pub(crate) const P_MENV_AMOUNT: u32 = 56;
pub(crate) const P_MENV_DEST: u32 = 57;
pub(crate) const P_GLIDE: u32 = 58;
pub(crate) const P_LEGATO: u32 = 59;
pub(crate) const P_SPREAD: u32 = 60;
pub(crate) const P_DRIFT: u32 = 61;
pub(crate) const P_CHORUS_MIX: u32 = 62;
pub(crate) const P_CHORUS_RATE: u32 = 63;
pub(crate) const P_CHORUS_DEPTH: u32 = 64;
pub(crate) const P_DELAY_MIX: u32 = 65;
pub(crate) const P_DELAY_TIME: u32 = 66;
pub(crate) const P_DELAY_FEEDBACK: u32 = 67;
pub(crate) const PARAM_COUNT: usize = 68;

/// `osc.table` choices: the PolyBLEP wave, then the additive bank.
const WAVE_CHOICES: Unit = Unit::Choice(&[
    "wave",
    "sine",
    "saw",
    "square",
    "triangle",
    "pulse 25",
    "pulse 10",
    "octave saw",
    "soft saw",
    "soft square",
    "bright saw",
    "hollow",
    "organ 1",
    "organ 2",
    "organ 3",
    "sine 2nd",
    "sine 3rd",
    "vowel a",
    "vowel e",
    "vowel i",
    "vowel o",
    "vowel u",
    "peak 4",
    "peak 8",
    "peak 16",
    "triple",
    "pairs",
    "bell",
    "random a",
    "random b",
    "random c",
    "random d",
    "buzz",
]);
const OSC2_MODES: Unit = Unit::Choice(&["normal", "ring", "fm", "sync"]);
const FILTER_TYPES: Unit = Unit::Choice(&["svf", "ladder"]);
const FILTER2_MODES: Unit = Unit::Choice(&["off", "LP12", "BP12", "HP12"]);
const FILTER2_ROUTINGS: Unit = Unit::Choice(&["series", "parallel"]);
const MENV_DESTS: Unit = Unit::Choice(&[
    "pitch",
    "osc 2 pitch",
    "osc mix",
    "osc 2 amount",
    "filter 2 cutoff",
]);
const OFF_ON: Unit = Unit::Choice(&["off", "on"]);

use Taper::{Decibel as Db, Exponential as Exp, Linear as Lin};

pub(crate) const BASE_PARAMS: [ParamDescriptor; PARAM_COUNT] = [
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
    param(
        P_BEND_RANGE,
        "bend.range",
        "Bend range",
        (0.0, MAX_BEND_RANGE, DEFAULT_BEND_RANGE),
        Lin,
        Unit::Semitones,
        false,
    ),
    param(
        P_OSC1_TABLE,
        "osc1.table",
        "Osc 1 table",
        (0.0, WAVE_COUNT as f32, 0.0),
        Lin,
        WAVE_CHOICES,
        true,
    ),
    param(
        P_OSC2_TABLE,
        "osc2.table",
        "Osc 2 table",
        (0.0, WAVE_COUNT as f32, 0.0),
        Lin,
        WAVE_CHOICES,
        true,
    ),
    param(
        P_OSC2_MODE,
        "osc2.mode",
        "Osc 2 mode",
        (0.0, 3.0, 0.0),
        Lin,
        OSC2_MODES,
        true,
    ),
    param(
        P_OSC2_AMOUNT,
        "osc2.amount",
        "Osc 2 amount",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_FILTER_TYPE,
        "filter.type",
        "Filter type",
        (0.0, 1.0, 0.0),
        Lin,
        FILTER_TYPES,
        true,
    ),
    param(
        P_DRIVE,
        "filter.drive",
        "Filter drive",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_F2_MODE,
        "filter2.mode",
        "Filter 2 mode",
        (0.0, 3.0, 0.0),
        Lin,
        FILTER2_MODES,
        true,
    ),
    param(
        P_F2_CUTOFF,
        "filter2.cutoff",
        "Filter 2 cutoff",
        (20.0, 20_000.0, 4_000.0),
        Exp,
        Unit::Hz,
        false,
    ),
    param(
        P_F2_RESONANCE,
        "filter2.resonance",
        "Filter 2 resonance",
        (0.0, 1.0, 0.2),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_F2_LINK,
        "filter.link",
        "Filter link",
        (0.0, 1.0, 1.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_F2_ROUTING,
        "filter2.routing",
        "Filter 2 routing",
        (0.0, 1.0, 0.0),
        Lin,
        FILTER2_ROUTINGS,
        true,
    ),
    param(
        P_LFO2_WAVE,
        "lfo2.wave",
        "LFO 2 wave",
        (0.0, 4.0, 0.0),
        Lin,
        LFO_WAVES,
        true,
    ),
    param(
        P_LFO2_RATE,
        "lfo2.rate",
        "LFO 2 rate",
        (0.05, 30.0, 0.5),
        Exp,
        Unit::Hz,
        false,
    ),
    param(
        P_LFO2_PITCH,
        "lfo2.to_pitch",
        "LFO 2 to pitch",
        (0.0, 12.0, 0.0),
        Lin,
        Unit::Semitones,
        false,
    ),
    param(
        P_LFO2_CUTOFF,
        "lfo2.to_cutoff",
        "LFO 2 to cutoff",
        (0.0, 4.0, 0.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_LFO2_LEVEL,
        "lfo2.to_level",
        "LFO 2 to level",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_MENV_A,
        "env.mod.attack",
        "Mod attack",
        (ENV_MS.0, ENV_MS.1, 2.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_MENV_D,
        "env.mod.decay",
        "Mod decay",
        (ENV_MS.0, ENV_MS.1, 400.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_MENV_S,
        "env.mod.sustain",
        "Mod sustain",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_MENV_R,
        "env.mod.release",
        "Mod release",
        (ENV_MS.0, ENV_MS.1, 300.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_MENV_AMOUNT,
        "env.mod.amount",
        "Mod env amount",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_MENV_DEST,
        "env.mod.dest",
        "Mod env destination",
        (0.0, 4.0, 0.0),
        Lin,
        MENV_DESTS,
        true,
    ),
    param(
        P_GLIDE,
        "glide",
        "Glide",
        (0.0, 2_000.0, 0.0),
        Lin,
        Unit::Ms,
        false,
    ),
    param(
        P_LEGATO,
        "legato",
        "Legato",
        (0.0, 1.0, 0.0),
        Lin,
        OFF_ON,
        true,
    ),
    param(
        P_SPREAD,
        "spread",
        "Stereo spread",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_DRIFT,
        "drift",
        "Drift",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_CHORUS_MIX,
        "chorus.mix",
        "Chorus mix",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_CHORUS_RATE,
        "chorus.rate",
        "Chorus rate",
        (0.05, 5.0, 0.6),
        Exp,
        Unit::Hz,
        false,
    ),
    param(
        P_CHORUS_DEPTH,
        "chorus.depth",
        "Chorus depth",
        (0.0, 1.0, 0.4),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_DELAY_MIX,
        "delay.mix",
        "Delay mix",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_DELAY_TIME,
        "delay.time",
        "Delay time",
        (10.0, 1_000.0, 300.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_DELAY_FEEDBACK,
        "delay.feedback",
        "Delay feedback",
        (0.0, 0.95, 0.35),
        Lin,
        Unit::Percent,
        false,
    ),
];

/// The poly synth's params.
static PARAMS: [ParamDescriptor; PARAM_COUNT] = BASE_PARAMS;

pub(crate) const NOTES_IN: SocketId = SocketId(0);
pub(crate) const CUTOFF_CV_IN: SocketId = SocketId(1);
pub(crate) const PITCH_CV_IN: SocketId = SocketId(2);
pub(crate) const RESONANCE_CV_IN: SocketId = SocketId(3);
pub(crate) const LEVEL_CV_IN: SocketId = SocketId(4);
pub(crate) const PAN_CV_IN: SocketId = SocketId(5);
pub(crate) const OUT_L: SocketId = SocketId(6);
pub(crate) const OUT_R: SocketId = SocketId(7);
pub(crate) const LFO_CV_OUT: SocketId = SocketId(8);

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

pub(crate) static SOCKETS: [SocketDescriptor; 9] = [
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
/// Fixed range of the pitch CV in semitones (times `pitch_cv_trim`).
const CV_PITCH_RANGE: f32 = 24.0;
/// Fixed range of the level CV in dB (times `master.level_cv_trim`).
const CV_LEVEL_RANGE_DB: f32 = 24.0;
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
/// How far the keys pan at full `spread`: middle C is centre, this many semitones away is the edge.
const PAN_SPAN: f32 = 36.0;
/// Semitones of the mod envelope at full amount when it targets a pitch.
const MENV_PITCH_RANGE: f32 = 24.0;
/// Octaves of the mod envelope at full amount when it targets filter 2.
const MENV_CUTOFF_RANGE: f32 = 4.0;
/// Half the osc mix range the mod envelope sweeps at full amount.
const MENV_MIX_RANGE: f32 = 0.5;
/// Semitones oscillator 2 rises at full `osc2.amount` in `sync` mode.
const SYNC_RANGE: f32 = 24.0;
/// Frequency deviation of oscillator 2 at full `osc2.amount` in `fm` mode (times its pitch).
const FM_DEPTH: f32 = 2.0;
/// Detune of a voice at full `drift`, in cents.
const DRIFT_CENTS: f32 = 20.0;
/// How often a drifting voice picks a new random target, in ms.
const DRIFT_HOLD_MS: f32 = 600.0;
/// How slowly it moves to it (time constant in ms).
const DRIFT_SLEW_MS: f32 = 400.0;
/// The log2 of e, for `exp(-x)` through the deterministic `2^x`.
const LOG2_E: f32 = std::f32::consts::LOG2_E;
/// Longest chorus delay line in ms.
const CHORUS_MAX_MS: f32 = 40.0;
/// Centre delay of the chorus in ms and its swing at full `chorus.depth`.
const CHORUS_CENTER_MS: f32 = 14.0;
const CHORUS_DEPTH_MS: f32 = 6.0;
/// Longest echo in seconds (the delay lines' size).
const DELAY_MAX_S: f32 = 1.0;

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

/// How oscillator 2 relates to oscillator 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Osc2Mode {
    Normal,
    Ring,
    Fm,
    Sync,
}

/// What the mod envelope moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenvDest {
    Pitch,
    Osc2Pitch,
    OscMix,
    Osc2Amount,
    Filter2Cutoff,
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
    /// Pitch bend in semitones, `-range..=range` (the controller state's range).
    bend: f32,
    /// The master tune in cents for this batch (`TimeInfo::tune_cents`).
    tune_cents: f32,
    /// Mod wheel (CC 1), `0..=1`.
    wheel: f32,
    lfo_to_cutoff: f32,
    velocity_amount: f32,
    cv_amount: f32,
    cv_pitch_amount: f32,
    cv_resonance_amount: f32,
    /// Set per batch: the resonance CV socket is cabled.
    cv_resonance_on: bool,
    /// The additive bank the `osc.table`s read.
    bank: &'static WaveBank,
    /// Per oscillator: 0 for the PolyBLEP `wave`, else 1 + the additive wave.
    tables: [usize; 2],
    osc2_mode: Osc2Mode,
    osc2_amount: f32,
    /// A table or a mode other than `normal` is in use: the voice takes the longer oscillator path.
    osc_advanced: bool,
    /// The ladder replaces the state-variable filter.
    ladder: bool,
    /// The loop gain of the ladder for the base resonance.
    k_ladder: f32,
    f2_mode: Option<FilterMode>,
    f2_cutoff: f32,
    f2_k: f32,
    f2_link: f32,
    f2_parallel: bool,
    lfo2_to_pitch: f32,
    lfo2_to_cutoff: f32,
    lfo2_to_level: f32,
    menv_amount: f32,
    menv_dest: MenvDest,
    /// Share of the way to a new pitch covered per control step (1 is no glide).
    glide_coef: f32,
    legato: bool,
    spread: f32,
    drift: f32,
    /// Control steps between new random targets of a drifting voice, and the share it moves
    /// towards the target per step.
    drift_hold: u32,
    drift_coef: f32,
}

/// Control-rate modulation values read by a voice.
#[derive(Debug, Clone, Copy)]
struct Ctl {
    lfo: f32,
    lfo2: f32,
    cutoff: f32,
    pitch: f32,
    resonance: f32,
}

/// Per-sample modulation sources of one render segment (all aligned with the output slice).
struct Mods<'a> {
    lfo: &'a [f32],
    lfo2: &'a [f32],
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
    ladder: Ladder,
    filter2: Svf,
    amp: Adsr,
    fenv: Adsr,
    menv: Adsr,
    noise: Rng,
    vel_gain: Smoother,
    countdown: u32,
    g: f32,
    g_target: f32,
    g_step: f32,
    snap_g: bool,
    /// Filter damping (state-variable) or loop gain (ladder) while the resonance CV is cabled
    /// (`k` ramps like `g`).
    k: f32,
    k_target: f32,
    k_step: f32,
    /// Filter 2's integrator gain ramps like `g`; `f2_cold` marks a stale target.
    g2: f32,
    g2_target: f32,
    g2_step: f32,
    f2_cold: bool,
    /// The pitch in semitones (a MIDI key) now, and the one it glides to.
    pitch: f32,
    target: f32,
    /// Oscillator 1 wrapped on its last step (for the hard sync of oscillator 2), and the
    /// phase increment oscillator 2 has without FM.
    wrapped: bool,
    inc2: f64,
    /// The mip level each table oscillator reads at its current pitch.
    level: [usize; 2],
    /// Osc mix and osc 2 amount after the mod envelope, set at the control rate.
    mix: f32,
    amount2: f32,
    /// The level of LFO 2's tremolo, ramped like `g`.
    trem: f32,
    trem_target: f32,
    trem_step: f32,
    /// Drift in `-1..=1` (times the drift amount), its random target and the control steps left.
    drift: f32,
    drift_target: f32,
    drift_ticks: u32,
    drift_rng: Rng,
    /// Output gains of the left and right channel (the key spread).
    gl: f32,
    gr: f32,
}

impl Voice {
    fn new(seed: u64) -> Self {
        Self {
            key: 0,
            order: 0,
            pending: None,
            osc: [PolyBlepOsc::new(Waveform::Saw); 2],
            filter: Svf::new(),
            ladder: Ladder::new(),
            filter2: Svf::new(),
            amp: Adsr::new(48_000.0),
            fenv: Adsr::new(48_000.0),
            menv: Adsr::new(48_000.0),
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
            g2: 0.0,
            g2_target: 0.0,
            g2_step: 0.0,
            f2_cold: true,
            pitch: 0.0,
            target: 0.0,
            wrapped: false,
            inc2: 0.0,
            level: [0; 2],
            mix: 0.5,
            amount2: 0.0,
            trem: 1.0,
            trem_target: 1.0,
            trem_step: 0.0,
            drift: 0.0,
            drift_target: 0.0,
            drift_ticks: 0,
            drift_rng: Rng::new(derive_seed(seed, 1_000)),
            gl: 1.0,
            gr: 1.0,
        }
    }

    fn is_active(&self) -> bool {
        self.amp.is_active()
    }

    /// Whether the key is held (the amp envelope is before its release).
    fn is_gated(&self) -> bool {
        matches!(
            self.amp.stage(),
            AdsrStage::Attack | AdsrStage::Decay | AdsrStage::Sustain
        )
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

    /// Starts a note from silence (fresh oscillators and filters). With a glide set the pitch
    /// starts at `from` (a key in semitones) when there is one.
    fn start(&mut self, key: u8, velocity: f32, from: Option<f32>, p: &Shared) {
        self.key = key;
        self.target = f32::from(key);
        self.pitch = match from {
            Some(f) if p.glide_coef < 1.0 => f,
            _ => self.target,
        };
        self.pending = None;
        for o in &mut self.osc {
            o.set_phase(0.0);
        }
        self.wrapped = false;
        self.filter.reset();
        self.ladder.reset();
        self.filter2.reset();
        self.f2_cold = true;
        self.amp.reset();
        self.fenv.reset();
        self.menv.reset();
        self.amp.gate_on();
        self.fenv.gate_on();
        self.menv.gate_on();
        self.vel_gain
            .set_immediate(Self::velocity_gain(p, velocity));
        if p.drift > 0.0 {
            self.drift = self.drift_rng.next_bipolar();
            self.drift_target = self.drift;
            self.drift_ticks = p.drift_hold;
        }
        self.countdown = 0;
        self.snap_g = true;
    }

    /// Same-key retrigger: envelopes restart from their current levels.
    fn retrigger(&mut self, velocity: f32, p: &Shared) {
        self.amp.gate_on();
        self.fenv.gate_on();
        self.menv.gate_on();
        self.vel_gain.set_target(Self::velocity_gain(p, velocity));
    }

    fn release(&mut self) {
        self.pending = None;
        self.amp.gate_off();
        self.fenv.gate_off();
        self.menv.gate_off();
    }

    /// Control-rate update: oscillator pitch and the filter coefficient targets.
    fn control(&mut self, c: Ctl, p: &Shared) {
        let (lfo, cv) = (c.lfo, c.cutoff);
        let snap = self.snap_g;
        if p.glide_coef >= 1.0 {
            self.pitch = self.target;
        } else {
            self.pitch += (self.target - self.pitch) * p.glide_coef;
        }
        let key = self.pitch;
        let cv_pitch = p.cv_pitch_amount * clean(c.pitch);
        // The mod envelope, LFO 2 and drift, each zero at its default.
        let menv = self.menv.level() * p.menv_amount;
        let mut extra = c.lfo2 * p.lfo2_to_pitch;
        let mut extra2 = 0.0f32;
        let mut mix = p.osc_mix;
        let mut amount2 = p.osc2_amount;
        let mut f2_octaves = 0.0f32;
        match p.menv_dest {
            MenvDest::Pitch => extra += menv * MENV_PITCH_RANGE,
            MenvDest::Osc2Pitch => extra2 += menv * MENV_PITCH_RANGE,
            MenvDest::OscMix => mix = (mix + menv * MENV_MIX_RANGE).clamp(0.0, 1.0),
            MenvDest::Osc2Amount => amount2 = (amount2 + menv).clamp(0.0, 1.0),
            MenvDest::Filter2Cutoff => f2_octaves = menv * MENV_CUTOFF_RANGE,
        }
        self.mix = mix;
        self.amount2 = amount2;
        if p.osc2_mode == Osc2Mode::Sync {
            extra2 += amount2 * SYNC_RANGE;
        }
        if p.drift > 0.0 {
            if self.drift_ticks == 0 {
                self.drift_target = self.drift_rng.next_bipolar();
                self.drift_ticks = p.drift_hold;
            } else {
                self.drift_ticks -= 1;
            }
            self.drift += (self.drift_target - self.drift) * p.drift_coef;
        }
        let drift = self.drift * p.drift * (DRIFT_CENTS * 0.01);
        let fm_reach = if p.osc2_mode == Osc2Mode::Fm {
            1.0 + FM_DEPTH * amount2
        } else {
            1.0
        };
        for (i, (o, offset)) in self.osc.iter_mut().zip(p.pitch_offset).enumerate() {
            let own = if i == 1 { extra2 } else { 0.0 };
            let hz = note_to_hz(
                key + offset
                    + lfo * (p.lfo_to_pitch + p.wheel * WHEEL_VIBRATO)
                    + p.bend
                    + cv_pitch
                    + extra
                    + drift
                    + own,
                p.tune_cents,
            );
            o.set_frequency(hz, p.sample_rate);
            if p.tables[i] > 0 {
                let reach = if i == 1 { fm_reach } else { 1.0 };
                self.level[i] = level_for(hz * reach, p.sample_rate);
            }
        }
        self.inc2 = self.osc[1].increment();
        let octaves = p.env_amount * 6.0 * self.fenv.level()
            + p.key_track * (key - 60.0) / 12.0
            + p.lfo_to_cutoff * lfo
            + p.cv_amount * cv.clamp(-1.0, 1.0)
            + p.lfo2_to_cutoff * c.lfo2;
        let scale = if p.ladder {
            exp2_det(octaves)
        } else {
            octaves.exp2()
        };
        let cutoff = (p.cutoff * scale).clamp(20.0, MAX_CUTOFF_RATIO * p.sample_rate);
        let g = if p.ladder {
            Ladder::cutoff_to_g(cutoff, p.sample_rate)
        } else {
            Svf::cutoff_to_g(cutoff, p.sample_rate)
        };
        // Land exactly on the previous target (no drift), then ramp towards the new one.
        self.g = if self.snap_g { g } else { self.g_target };
        self.g_target = g;
        self.g_step = (g - self.g) / CONTROL_INTERVAL as f32;
        let base_k = if p.ladder { p.k_ladder } else { p.k };
        if p.cv_resonance_on {
            let r = (p.resonance + p.cv_resonance_amount * clean(c.resonance)).clamp(0.0, 1.0);
            let k = if p.ladder {
                Ladder::resonance_to_k(r)
            } else {
                1.0 / Svf::resonance_to_q(r)
            };
            self.k = if self.snap_g { k } else { self.k_target };
            self.k_target = k;
            self.k_step = (k - self.k) / CONTROL_INTERVAL as f32;
        } else {
            self.k = base_k;
            self.k_target = base_k;
            self.k_step = 0.0;
        }
        if self.snap_g || self.g_step == 0.0 {
            self.apply_coefficients(self.g, self.k_now(p, base_k), p);
            self.snap_g = false;
        }
        // Filter 2: cutoff from its own setting and its share of the first filter's modulation.
        if p.f2_mode.is_some() {
            let octaves2 = p.f2_link * octaves + f2_octaves;
            let cutoff2 =
                (p.f2_cutoff * exp2_det(octaves2)).clamp(20.0, MAX_CUTOFF_RATIO * p.sample_rate);
            let g2 = Svf::cutoff_to_g_det(cutoff2, p.sample_rate);
            self.g2 = if snap || self.f2_cold {
                g2
            } else {
                self.g2_target
            };
            self.g2_target = g2;
            self.g2_step = (g2 - self.g2) / CONTROL_INTERVAL as f32;
            if snap || self.f2_cold || self.g2_step == 0.0 {
                self.filter2.set_coefficients(self.g2, p.f2_k);
            }
            self.f2_cold = false;
        } else {
            self.f2_cold = true;
        }
        // LFO 2 tremolo: unity at its top, `1 - to_level` at its bottom.
        let trem = 1.0 - p.lfo2_to_level * (0.5 - 0.5 * c.lfo2);
        self.trem = if snap { trem } else { self.trem_target };
        self.trem_target = trem;
        self.trem_step = (trem - self.trem) / CONTROL_INTERVAL as f32;
        let (gl, gr) = if p.spread > 0.0 {
            balance_gains(p.spread * ((self.target - 60.0) / PAN_SPAN).clamp(-1.0, 1.0))
        } else {
            (1.0, 1.0)
        };
        self.gl = gl;
        self.gr = gr;
    }

    /// Sends `g` and `k` to the filter in use.
    #[inline]
    fn apply_coefficients(&mut self, g: f32, k: f32, p: &Shared) {
        if p.ladder {
            self.ladder.set_coefficients(g, k);
        } else {
            self.filter.set_coefficients(g, k);
        }
    }

    /// The filter damping in use: the base value, or the ramped value under resonance CV.
    #[inline]
    fn k_now(&self, p: &Shared, base_k: f32) -> f32 {
        if p.cv_resonance_on { self.k } else { base_k }
    }

    /// The oscillators with a table or a mode other than `normal`: returns the output of
    /// oscillator 1 and of oscillator 2.
    fn oscillators(&mut self, p: &Shared) -> (f32, f32) {
        let sync_wrap = self.wrapped;
        let p1 = self.osc[0].phase();
        let inc1 = self.osc[0].increment();
        let o1 = if p.tables[0] > 0 {
            let v = p.bank.read(p.tables[0] - 1, self.level[0], p1);
            self.wrapped = self.osc[0].advance();
            v
        } else {
            let v = self.osc[0].next_sample();
            self.wrapped = self.osc[0].phase() < p1;
            v
        };
        let mut extra = 0.0;
        if p.osc2_mode == Osc2Mode::Sync && sync_wrap && inc1 > 0.0 {
            // Oscillator 1 wrapped `frac` of a sample ago: oscillator 2 restarts then. The jump
            // that makes is softened where the waveform's own correction does not cover it.
            let frac = (p1 / inc1).clamp(0.0, 1.0) as f32;
            let inc2 = self.osc[1].increment();
            let before = self.osc[1].phase() - f64::from(frac) * inc2;
            let value = |v: &Self, phase: f64| {
                if p.tables[1] > 0 {
                    p.bank.read(p.tables[1] - 1, v.level[1], phase)
                } else {
                    v.osc[1].naive_at(phase)
                }
            };
            let step = value(self, 0.0) - value(self, before);
            let assumed = if p.tables[1] > 0 {
                0.0
            } else {
                match self.osc[1].waveform() {
                    Waveform::Saw => -2.0,
                    Waveform::Square => 2.0,
                    _ => 0.0,
                }
            };
            extra = PolyBlepOsc::step_residual(step - assumed, frac);
            self.osc[1].set_phase(f64::from(frac) * inc2);
        }
        if p.osc2_mode == Osc2Mode::Fm {
            let depth = f64::from(FM_DEPTH * self.amount2);
            self.osc[1].set_increment(self.inc2 * (1.0 + depth * f64::from(o1)));
        }
        let raw = (if p.tables[1] > 0 {
            let v = p
                .bank
                .read(p.tables[1] - 1, self.level[1], self.osc[1].phase());
            self.osc[1].advance();
            v
        } else {
            self.osc[1].next_sample()
        }) + extra;
        let o2 = if p.osc2_mode == Osc2Mode::Ring {
            o1 * raw
        } else {
            raw
        };
        (o1, o2)
    }

    /// Adds this voice into `l` (and, with a stereo spread, `r`). The modulation slices are
    /// aligned with the outputs.
    fn render(&mut self, l: &mut [f32], r: &mut [f32], m: &Mods<'_>, p: &Shared) {
        let base_k = if p.ladder { p.k_ladder } else { p.k };
        for (n, (ol, or)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            if self.countdown == 0 {
                let c = Ctl {
                    lfo: m.lfo[n],
                    lfo2: m.lfo2[n],
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
                self.apply_coefficients(self.g, self.k_now(p, base_k), p);
            }
            let (o1, o2) = if p.osc_advanced {
                self.oscillators(p)
            } else {
                (self.osc[0].next_sample(), self.osc[1].next_sample())
            };
            let mut x = o1 + (o2 - o1) * self.mix;
            if p.noise > 0.0 {
                x += p.noise * self.noise.next_bipolar();
            }
            let mut y = if p.ladder {
                self.ladder.process(x)
            } else {
                self.filter.process_mode(x, p.filter_mode)
            };
            if let Some(mode) = p.f2_mode {
                if self.g2_step != 0.0 {
                    self.g2 += self.g2_step;
                    self.filter2.set_coefficients(self.g2, p.f2_k);
                }
                if p.f2_parallel {
                    y = 0.5 * (y + self.filter2.process_mode(x, mode));
                } else {
                    y = self.filter2.process_mode(y, mode);
                }
            }
            if !y.is_finite() {
                self.filter.reset();
                self.ladder.reset();
                self.filter2.reset();
                y = 0.0;
            }
            self.fenv.next_sample();
            if p.menv_amount != 0.0 {
                self.menv.next_sample();
            }
            self.trem += self.trem_step;
            let amp = self.amp.next_sample();
            let v = y * amp * self.vel_gain.next_value() * self.trem * VOICE_GAIN;
            *ol += v * self.gl;
            if p.spread > 0.0 {
                *or += v * self.gr;
            }
            if !self.amp.is_active() {
                match self.pending.take() {
                    Some((key, velocity)) => self.start(key, velocity, Some(self.pitch), p),
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

/// Control steps between random drift targets and the share of the way to a target covered per
/// step, at `sr`.
fn drift_timing(sr: f32) -> (u32, f32) {
    let step_ms = CONTROL_INTERVAL as f32 * 1_000.0 / sr;
    (
        (DRIFT_HOLD_MS / step_ms).round().max(1.0) as u32,
        (step_ms / DRIFT_SLEW_MS).min(1.0),
    )
}

/// The effect chorus for `sr` (non-RT: allocates its lines).
fn make_chorus(sr: f32) -> Chorus {
    let mut c = Chorus::new(sr, CHORUS_MAX_MS);
    c.set_center_ms(CHORUS_CENTER_MS);
    c
}

/// A key held in the legato mode.
#[derive(Debug, Clone, Copy, Default)]
struct Held {
    key: u8,
    velocity: f32,
}

/// What one batch reads: the notes and the CV inputs (silent arrays when not cabled).
pub(crate) struct BatchIn<'a> {
    pub events: &'a [NoteEvent],
    pub cutoff: &'a [f32; BATCH],
    pub pitch: &'a [f32; BATCH],
    pub resonance: &'a [f32; BATCH],
    pub level: &'a [f32; BATCH],
    pub pan: &'a [f32; BATCH],
    pub resonance_on: bool,
    pub level_on: bool,
    pub pan_on: bool,
    /// `TimeInfo::tune_cents`: the master tune.
    pub tune_cents: f32,
}

/// What one batch makes.
pub(crate) struct BatchOut {
    pub l: [f32; BATCH],
    pub r: [f32; BATCH],
    pub lfo: [f32; BATCH],
}

impl Default for BatchOut {
    fn default() -> Self {
        Self {
            l: [0.0; BATCH],
            r: [0.0; BATCH],
            lfo: [0.0; BATCH],
        }
    }
}

/// The poly synth device.
#[derive(Debug)]
pub struct PolySynth {
    desc: &'static DeviceDescriptor,
    values: [f32; PARAM_COUNT],
    shared: Shared,
    voices: [Voice; MAX_VOICES],
    voice_limit: usize,
    next_order: u64,
    lfo: Lfo,
    lfo2: Lfo,
    master: Smoother,
    cv_level_db: f32,
    cv_pan_amount: f32,
    /// Sustain, volume, expression, pressure and the bend range.
    ctl: ControllerState,
    /// The keys held in the legato mode (the last is on top) and the pitch last played.
    held: [Held; MAX_VOICES],
    held_len: usize,
    last_pitch: Option<f32>,
    chorus: Chorus,
    chorus_mix: f32,
    delay: [DelayLine; 2],
    delay_mix: f32,
    /// The echo lines may hold sound (they are cleared only then, so an unused echo never touches
    /// its memory).
    delay_used: bool,
    delay_samples: f32,
    delay_feedback: f32,
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
    /// A synth with default params (call `prepare` before processing). Builds the shared wave
    /// bank on the first call in the process, so create it off the audio thread.
    pub fn new() -> Self {
        Self::with_descriptor(&DESCRIPTOR)
    }

    /// A synth for `desc`, a descriptor with the params of [`DESCRIPTOR`] and other defaults
    /// (the mono bass).
    pub(crate) fn with_descriptor(desc: &'static DeviceDescriptor) -> Self {
        let mut s = Self {
            desc,
            values: std::array::from_fn(|i| desc.params[i].default),
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
                bend: 0.0,
                tune_cents: 0.0,
                wheel: 0.0,
                lfo_to_cutoff: 0.0,
                velocity_amount: 0.5,
                cv_amount: 2.0,
                cv_pitch_amount: CV_PITCH_RANGE,
                cv_resonance_amount: 1.0,
                cv_resonance_on: false,
                bank: wave_bank(),
                tables: [0; 2],
                osc2_mode: Osc2Mode::Normal,
                osc2_amount: 0.0,
                osc_advanced: false,
                ladder: false,
                k_ladder: 0.0,
                f2_mode: None,
                f2_cutoff: 4_000.0,
                f2_k: 1.0,
                f2_link: 1.0,
                f2_parallel: false,
                lfo2_to_pitch: 0.0,
                lfo2_to_cutoff: 0.0,
                lfo2_to_level: 0.0,
                menv_amount: 0.0,
                menv_dest: MenvDest::Pitch,
                glide_coef: 1.0,
                legato: false,
                spread: 0.0,
                drift: 0.0,
                drift_hold: drift_timing(48_000.0).0,
                drift_coef: drift_timing(48_000.0).1,
            },
            voices: std::array::from_fn(|i| Voice::new(derive_seed(0, i as u64 + 1))),
            voice_limit: MAX_VOICES,
            next_order: 0,
            lfo: Lfo::new(derive_seed(0, 0)),
            lfo2: Lfo::new(derive_seed(0, 2_000)),
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            cv_level_db: CV_LEVEL_RANGE_DB,
            cv_pan_amount: 1.0,
            ctl: ControllerState::new(),
            held: [Held::default(); MAX_VOICES],
            held_len: 0,
            last_pitch: None,
            chorus: make_chorus(48_000.0),
            chorus_mix: 0.0,
            delay: [DelayLine::new(48_000), DelayLine::new(48_000)],
            delay_mix: 0.0,
            delay_used: false,
            delay_samples: 14_400.0,
            delay_feedback: 0.35,
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

    /// Whether the oscillators need the longer path.
    fn update_advanced(&mut self) {
        let s = &mut self.shared;
        s.osc_advanced = s.tables != [0, 0] || s.osc2_mode != Osc2Mode::Normal;
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
                self.shared.k_ladder = Ladder::resonance_to_k(v);
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
            P_CV_PITCH => self.shared.cv_pitch_amount = v * CV_PITCH_RANGE,
            P_CV_RESONANCE => self.shared.cv_resonance_amount = v,
            P_CV_LEVEL => self.cv_level_db = v * CV_LEVEL_RANGE_DB,
            P_CV_PAN => self.cv_pan_amount = v,
            P_BEND_RANGE => {
                self.ctl.set_bend_range(v);
                self.shared.bend = self.ctl.bend_semitones();
            }
            P_VOICES => {
                self.voice_limit = (v.round().max(1.0) as usize).min(MAX_VOICES);
                for vc in &mut voices[self.voice_limit..] {
                    vc.pending = None;
                    vc.amp.steal_fade();
                }
            }
            P_MASTER => self
                .master
                .set_target(self.desc.params[P_MASTER as usize].db_param_gain(v)),
            P_OSC1_TABLE | P_OSC2_TABLE => {
                self.shared.tables[(id - P_OSC1_TABLE) as usize] = choice(v).min(WAVE_COUNT);
                self.update_advanced();
            }
            P_OSC2_MODE => {
                self.shared.osc2_mode = match choice(v) {
                    0 => Osc2Mode::Normal,
                    1 => Osc2Mode::Ring,
                    2 => Osc2Mode::Fm,
                    _ => Osc2Mode::Sync,
                };
                self.update_advanced();
            }
            P_OSC2_AMOUNT => self.shared.osc2_amount = v,
            P_FILTER_TYPE => {
                let ladder = choice(v) >= 1;
                if ladder != self.shared.ladder {
                    self.shared.ladder = ladder;
                    for vc in voices.iter_mut() {
                        vc.snap_g = true;
                        vc.filter.reset();
                        vc.ladder.reset();
                    }
                }
            }
            P_DRIVE => voices.iter_mut().for_each(|vc| vc.ladder.set_drive(v)),
            P_F2_MODE => {
                self.shared.f2_mode = match choice(v) {
                    0 => None,
                    1 => Some(FilterMode::Lowpass),
                    2 => Some(FilterMode::Bandpass),
                    _ => Some(FilterMode::Highpass),
                };
            }
            P_F2_CUTOFF => self.shared.f2_cutoff = v,
            P_F2_RESONANCE => self.shared.f2_k = Svf::resonance_to_k_det(v),
            P_F2_LINK => self.shared.f2_link = v,
            P_F2_ROUTING => self.shared.f2_parallel = choice(v) >= 1,
            P_LFO2_WAVE => self.lfo2.set_shape(LfoShape::from_index(choice(v))),
            P_LFO2_RATE => self.lfo2.set_rate(v, sr),
            P_LFO2_PITCH => self.shared.lfo2_to_pitch = v,
            P_LFO2_CUTOFF => self.shared.lfo2_to_cutoff = v,
            P_LFO2_LEVEL => self.shared.lfo2_to_level = v,
            P_MENV_A => voices.iter_mut().for_each(|vc| vc.menv.set_attack_ms(v)),
            P_MENV_D => voices.iter_mut().for_each(|vc| vc.menv.set_decay_ms(v)),
            P_MENV_S => voices.iter_mut().for_each(|vc| vc.menv.set_sustain(v)),
            P_MENV_R => voices.iter_mut().for_each(|vc| vc.menv.set_release_ms(v)),
            P_MENV_AMOUNT => self.shared.menv_amount = v,
            P_MENV_DEST => {
                self.shared.menv_dest = match choice(v) {
                    0 => MenvDest::Pitch,
                    1 => MenvDest::Osc2Pitch,
                    2 => MenvDest::OscMix,
                    3 => MenvDest::Osc2Amount,
                    _ => MenvDest::Filter2Cutoff,
                };
            }
            P_GLIDE => {
                self.shared.glide_coef = if v <= 0.0 {
                    1.0
                } else {
                    let tau = (v * 0.001 * sr).max(1.0);
                    1.0 - exp2_det(-(CONTROL_INTERVAL as f32) / tau * LOG2_E)
                };
            }
            P_LEGATO => {
                let on = v >= 0.5;
                if on != self.shared.legato {
                    self.shared.legato = on;
                    self.held_len = 0;
                    if on {
                        for vc in &mut voices[1..] {
                            vc.release();
                        }
                    }
                }
            }
            P_SPREAD => self.shared.spread = v,
            P_DRIFT => self.shared.drift = v,
            P_CHORUS_MIX => {
                if v <= 0.0 && self.chorus_mix > 0.0 {
                    self.chorus.reset();
                }
                self.chorus_mix = v;
                self.chorus.set_mix(v);
            }
            P_CHORUS_RATE => self.chorus.set_rate_hz(v),
            P_CHORUS_DEPTH => self.chorus.set_depth_ms(v * CHORUS_DEPTH_MS),
            P_DELAY_MIX => {
                if v <= 0.0 && self.delay_used {
                    self.delay.iter_mut().for_each(DelayLine::clear);
                    self.delay_used = false;
                }
                self.delay_mix = v;
            }
            P_DELAY_TIME => self.delay_samples = v * 0.001 * sr,
            P_DELAY_FEEDBACK => self.delay_feedback = v,
            _ => {}
        }
    }

    /// The values that follow from the sample rate: the drift timing, the effect lines.
    /// Allocates (the lines), so only `prepare` calls it.
    fn set_sample_rate(&mut self, sr: f32) {
        self.shared.sample_rate = sr;
        (self.shared.drift_hold, self.shared.drift_coef) = drift_timing(sr);
        self.chorus = make_chorus(sr);
        let lines = (sr * DELAY_MAX_S).ceil() as usize;
        for line in &mut self.delay {
            line.resize(lines);
        }
        self.delay_used = false;
    }

    fn note_on(&mut self, key: u8, velocity: f32) {
        self.ctl.note_on(key);
        if self.shared.legato {
            self.mono_note_on(key, velocity);
            return;
        }
        let p = self.shared;
        let order = self.next_order;
        self.next_order += 1;
        let from = self.last_pitch;
        self.last_pitch = Some(f32::from(key));
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
            v.start(key, velocity, from, &p);
            return;
        }
        // 3. Steal the oldest started voice: 2 ms fade, then the new note.
        if let Some(v) = voices.iter_mut().min_by_key(|v| v.order) {
            v.order = order;
            v.pending = Some((key, velocity));
            v.amp.steal_fade();
        }
    }

    /// A key in the legato mode: the one voice plays it (the last key wins). A key played while
    /// another is held changes the pitch only; the envelopes run on.
    fn mono_note_on(&mut self, key: u8, velocity: f32) {
        let p = self.shared;
        self.hold(key, velocity);
        let from = self.last_pitch;
        self.last_pitch = Some(f32::from(key));
        self.next_order += 1;
        let v = &mut self.voices[0];
        v.order = self.next_order;
        if v.is_gated() && v.pending.is_none() {
            v.key = key;
            v.target = f32::from(key);
            v.vel_gain.set_target(Voice::velocity_gain(&p, velocity));
        } else if v.is_active() && v.amp.stage() != AdsrStage::Fade {
            // Released but still sounding: restart the envelopes and glide on.
            v.key = key;
            v.target = f32::from(key);
            v.retrigger(velocity, &p);
        } else if v.is_active() {
            v.pending = Some((key, velocity));
        } else {
            v.start(key, velocity, from, &p);
        }
    }

    /// Puts `key` on top of the held keys of the legato mode.
    fn hold(&mut self, key: u8, velocity: f32) {
        self.unhold(key);
        if self.held_len < MAX_VOICES {
            self.held[self.held_len] = Held { key, velocity };
            self.held_len += 1;
        } else {
            self.held.copy_within(1.., 0);
            self.held[MAX_VOICES - 1] = Held { key, velocity };
        }
    }

    /// Takes `key` off the held keys.
    fn unhold(&mut self, key: u8) {
        if let Some(i) = self.held[..self.held_len].iter().position(|h| h.key == key) {
            self.held.copy_within(i + 1..self.held_len, i);
            self.held_len -= 1;
        }
    }

    /// A key released in the legato mode: back to the key still held, or release.
    fn mono_note_off(&mut self, key: u8) {
        self.unhold(key);
        let v = &mut self.voices[0];
        if v.key != key || !v.is_gated() {
            return;
        }
        if self.held_len == 0 {
            v.release();
            return;
        }
        let back = self.held[self.held_len - 1];
        v.key = back.key;
        v.target = f32::from(back.key);
        v.vel_gain
            .set_target(Voice::velocity_gain(&self.shared, back.velocity));
        self.last_pitch = Some(f32::from(back.key));
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
            NoteKind::Off { key } => {
                if self.shared.legato {
                    self.unhold(key);
                }
                if self.ctl.note_off(key) {
                    if self.shared.legato {
                        self.mono_note_off(key);
                    } else {
                        self.release_key(key);
                    }
                }
            }
            NoteKind::AllOff => {
                self.ctl.all_off();
                self.held_len = 0;
                self.voices.iter_mut().for_each(Voice::release);
            }
            NoteKind::Cc {
                controller: 1,
                value,
            } => self.shared.wheel = value.clamp(0.0, 1.0),
            // Pedal, volume, expression, pressure, bend and its range.
            _ => {
                if self.ctl.handle(kind) == Effect::ReleaseHeld {
                    let voices = &mut self.voices;
                    self.ctl.release_held(|key| {
                        voices
                            .iter_mut()
                            .filter(|v| v.sounding_key() == Some(key))
                            .for_each(Voice::release);
                    });
                }
                self.shared.bend = self.ctl.bend_semitones();
            }
        }
    }

    fn release_key(&mut self, key: u8) {
        self.voices
            .iter_mut()
            .filter(|v| v.sounding_key() == Some(key))
            .for_each(Voice::release);
    }

    /// Renders one batch: `limit` puts the soft limiter on the output (a layered device limits
    /// the sum of its layers itself).
    pub(crate) fn render_batch(&mut self, inp: &BatchIn<'_>, limit: bool, out: &mut BatchOut) {
        if self.fresh {
            let m = self.master.target();
            self.master.set_immediate(m);
            self.fresh = false;
        }
        let mut lfo = [0.0f32; BATCH];
        let mut lfo2 = [0.0f32; BATCH];
        for a in &mut lfo {
            *a = self.lfo.next_sample();
        }
        // LFO 2 runs only while it has a destination (its phase waits otherwise).
        let s = &self.shared;
        if s.lfo2_to_pitch != 0.0 || s.lfo2_to_cutoff != 0.0 || s.lfo2_to_level != 0.0 {
            for b in &mut lfo2 {
                *b = self.lfo2.next_sample();
            }
        }
        self.shared.cv_resonance_on = inp.resonance_on;
        self.shared.tune_cents = inp.tune_cents;
        let mut mix = [0.0f32; BATCH];
        let mut mix_r = [0.0f32; BATCH];
        let mut ctl_gain = [1.0f32; BATCH];
        let events = inp.events;
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
                lfo2: &lfo2[start..end],
                cutoff: &inp.cutoff[start..end],
                pitch: &inp.pitch[start..end],
                resonance: &inp.resonance[start..end],
            };
            for v in self.voices.iter_mut().filter(|v| v.is_active()) {
                v.render(&mut mix[start..end], &mut mix_r[start..end], &m, &p);
            }
            self.ctl.fill_gain(&mut ctl_gain[start..end]);
            start = end;
        }
        let spread_on = self.shared.spread > 0.0;
        let limited = |x: f32| if limit { soft_limit(x) } else { x };
        for n in 0..BATCH {
            let mut g = self.master.next_value();
            if inp.level_on {
                g *= db_gain(self.cv_level_db, inp.level[n]);
            }
            mix[n] = limited(mix[n] * g);
            if spread_on {
                mix_r[n] = limited(mix_r[n] * g);
            }
        }
        if !spread_on {
            mix_r = mix;
        }
        for ((l, r), g) in mix.iter_mut().zip(mix_r.iter_mut()).zip(&ctl_gain) {
            *l *= g;
            *r *= g;
        }
        if inp.pan_on {
            // The voice sum balanced by the pan CV (unity at the centre, like a stereo balance).
            for n in 0..BATCH {
                let (gl, gr) = balance_gains(self.cv_pan_amount * clean(inp.pan[n]));
                mix[n] *= gl;
                mix_r[n] *= gr;
            }
        }
        if self.delay_mix > 0.0 {
            self.delay_used = true;
        }
        if self.chorus_mix > 0.0 || self.delay_mix > 0.0 {
            for n in 0..BATCH {
                let (mut l, mut r) = (mix[n], mix_r[n]);
                if self.chorus_mix > 0.0 {
                    (l, r) = self.chorus.process(l, r);
                }
                if self.delay_mix > 0.0 {
                    // A ping-pong echo: the mono input enters the left line, every echo feeds the
                    // other side.
                    let (el, er) = (
                        self.delay[0].read(self.delay_samples),
                        self.delay[1].read(self.delay_samples),
                    );
                    self.delay[0].write(0.5 * (l + r) + self.delay_feedback * er);
                    self.delay[1].write(self.delay_feedback * el);
                    l += self.delay_mix * el;
                    r += self.delay_mix * er;
                }
                mix[n] = limited(l);
                mix_r[n] = limited(r);
            }
        }
        out.l = mix;
        out.r = mix_r;
        out.lfo = lfo;
        #[cfg(test)]
        self.update_probe();
    }

    /// The pressure on `key` (test window into the controller state).
    #[cfg(test)]
    pub(crate) fn pressure_on(&self, key: u8) -> f32 {
        self.ctl.pressure(key)
    }

    /// Prepares for `sr` and a seed (the shared part of `prepare`).
    pub(crate) fn prepare_with(&mut self, sr: f32, seed: u64) {
        self.seed = seed;
        self.set_sample_rate(sr);
        for v in &mut self.voices {
            v.amp.set_sample_rate(sr);
            v.fenv.set_sample_rate(sr);
            v.menv.set_sample_rate(sr);
            v.vel_gain.set_time(VELOCITY_GLIDE_MS, sr);
        }
        self.master.set_time(MASTER_SMOOTH_MS, sr);
        self.ctl.set_sample_rate(sr);
        self.lfo = Lfo::new(derive_seed(seed, 0));
        self.lfo2 = Lfo::new(derive_seed(seed, 2_000));
        self.apply_all();
        self.reset();
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
        self.desc
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.prepare_with(info.sample_rate.hz() as f32, info.seed);
    }

    fn reset(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            v.amp.reset();
            v.fenv.reset();
            v.menv.reset();
            v.filter.reset();
            v.ladder.reset();
            v.filter2.reset();
            v.pending = None;
            v.noise = Rng::new(derive_seed(self.seed, i as u64 + 1));
            v.drift_rng = Rng::new(derive_seed(self.seed, i as u64 + 1_001));
            v.drift = 0.0;
            v.drift_target = 0.0;
            v.drift_ticks = 0;
            v.trem = 1.0;
            v.trem_target = 1.0;
            v.trem_step = 0.0;
        }
        self.next_order = 0;
        self.ctl.reset();
        self.shared.bend = 0.0;
        self.shared.wheel = 0.0;
        self.held_len = 0;
        self.last_pitch = None;
        self.lfo.reset();
        self.lfo2.reset();
        self.chorus.reset();
        if self.delay_used {
            self.delay.iter_mut().for_each(DelayLine::clear);
            self.delay_used = false;
        }
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
        let cutoff = *ctx.input(CUTOFF_CV_IN);
        let pitch = *ctx.input(PITCH_CV_IN);
        let resonance = *ctx.input(RESONANCE_CV_IN);
        let level = *ctx.input(LEVEL_CV_IN);
        let pan = *ctx.input(PAN_CV_IN);
        let mut out = BatchOut::default();
        {
            let inp = BatchIn {
                events: ctx.notes_in(NOTES_IN),
                cutoff: &cutoff,
                pitch: &pitch,
                resonance: &resonance,
                level: &level,
                pan: &pan,
                resonance_on: ctx.is_connected(RESONANCE_CV_IN),
                level_on: ctx.is_connected(LEVEL_CV_IN),
                pan_on: ctx.is_connected(PAN_CV_IN),
                tune_cents: ctx.time.tune_cents,
            };
            self.render_batch(&inp, true, &mut out);
        }
        *ctx.output(OUT_L) = out.l;
        *ctx.output(OUT_R) = out.r;
        *ctx.output(LFO_CV_OUT) = out.lfo;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        SR, assert_bend_range, assert_keys, assert_sustain_pedal, assert_volume_and_expression,
        harness, off, on, sweep_params_min_max,
    };
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
                "pitch_cv_trim",
                "filter.resonance_cv_trim",
                "master.level_cv_trim",
                "pan_cv_trim",
                "bend.range",
                "osc1.table",
                "osc2.table",
                "osc2.mode",
                "osc2.amount",
                "filter.type",
                "filter.drive",
                "filter2.mode",
                "filter2.cutoff",
                "filter2.resonance",
                "filter.link",
                "filter2.routing",
                "lfo2.wave",
                "lfo2.rate",
                "lfo2.to_pitch",
                "lfo2.to_cutoff",
                "lfo2.to_level",
                "env.mod.attack",
                "env.mod.decay",
                "env.mod.sustain",
                "env.mod.release",
                "env.mod.amount",
                "env.mod.dest",
                "glide",
                "legato",
                "spread",
                "drift",
                "chorus.mix",
                "chorus.rate",
                "chorus.depth",
                "delay.mix",
                "delay.time",
                "delay.feedback",
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

    fn push(h: &mut DeviceHarness, offset: u16, kind: NoteKind) {
        h.push_note("notes_in", NoteEvent { offset, kind });
    }

    #[test]
    fn process_pitch_bend_moves_every_voice_by_up_to_two_semitones() {
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
        // Out-of-range input is clamped to the range.
        assert!((freq(5.0) - up).abs() < 1.5);
    }

    #[test]
    fn process_bend_survives_all_off_and_reset_clears_it() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        push(&mut h, 0, NoteKind::PitchBend { value: 1.0 });
        push(&mut h, 1, NoteKind::AllOff);
        h.push_note("notes_in", on(2, 57, 1.0));
        let o = h.render(seconds(0.5), &OUT).remove(0);
        let bent = dominant_frequency(&o[4_800..24_000], SR);
        assert!(bent > 240.0, "{bent}");
        let mut s = PolySynth::new();
        s.handle(NoteKind::PitchBend { value: -1.0 });
        s.handle(NoteKind::Cc {
            controller: 1,
            value: 1.0,
        });
        assert_eq!((s.shared.bend, s.shared.wheel), (-DEFAULT_BEND_RANGE, 1.0));
        s.reset();
        assert_eq!((s.shared.bend, s.shared.wheel), (0.0, 0.0));
    }

    /// A steady sine with a quick release.
    fn performance_synth() -> DeviceHarness {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("env.amp.release", 5.0);
        h
    }

    #[test]
    fn process_sustain_pedal_holds_the_release_until_pedal_up() {
        assert_sustain_pedal(performance_synth, &OUT, 57);
    }

    #[test]
    fn process_volume_and_expression_scale_the_output() {
        assert_volume_and_expression(performance_synth, &OUT, 57);
    }

    #[test]
    fn process_bend_range_param_and_rpn_zero() {
        assert_bend_range(performance_synth, &OUT, 57);
    }

    #[test]
    fn handle_pressure_reaches_the_modulation_source() {
        let mut s = PolySynth::new();
        s.handle(NoteKind::ChannelPressure { value: 0.5 });
        s.handle(NoteKind::PolyPressure {
            key: 60,
            value: 0.75,
        });
        assert_eq!((s.ctl.pressure(60), s.ctl.pressure(61)), (0.75, 0.5));
        s.reset();
        assert_eq!(s.ctl.pressure(60), 0.0);
    }

    #[test]
    fn process_mod_wheel_adds_lfo_vibrato_and_other_ccs_do_nothing() {
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
            let o = cv_render(pure_sine, "pitch_cv_in", "pitch_cv_trim", amount / 24.0, cv);
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
                "filter.resonance_cv_trim",
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
                "master.level_cv_trim",
                amount / 24.0,
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
            let o = cv_render(pure_sine, "pan_cv_in", "pan_cv_trim", amount, cv);
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
                    "pitch_cv_trim",
                    "filter.resonance_cv_trim",
                    "master.level_cv_trim",
                    "pan_cv_trim",
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
        let base = cv_render(pure_sine, "none", "pitch_cv_trim", 0.5, None);
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

    // ---------------------------------------------------------------------------------------
    // The appended params: wave bank, oscillator 2 modes, filters, LFO 2, mod envelope, play,
    // spread, drift and the effects.
    // ---------------------------------------------------------------------------------------

    /// Magnitude near `hz` in the Hann spectrum of `x`, in dB below the spectrum's peak.
    fn db_at(x: &[f32], hz: f64) -> f64 {
        let m = atomik_testkit::signal::magnitude_spectrum(x);
        let peak = m.iter().copied().fold(0.0, f64::max);
        let k = (hz * x.len() as f64 / SR).round() as usize;
        let v = m[k.saturating_sub(2)..=(k + 2).min(m.len() - 1)]
            .iter()
            .copied()
            .fold(0.0, f64::max);
        20.0 * (v / peak).log10()
    }

    /// A settled stretch of `n` samples of the left output after `skip`, with the notes on.
    fn steady(h: &mut DeviceHarness, key: u8, skip: usize, n: usize) -> Vec<f32> {
        h.push_note("notes_in", on(0, key, 1.0));
        let o = h.render((skip + n) / BATCH + 1, &OUT).remove(0);
        o[skip..skip + n].to_vec()
    }

    /// Two plain sine oscillators with the filter open and static, osc 2 on its own.
    fn two_sines(h: &mut DeviceHarness) {
        pure_sine(h);
        h.set_param("osc2.wave", 3.0);
        h.set_param("osc2.fine", 0.0);
        h.set_param("osc.mix", 1.0);
        h.set_param("env.amp.attack", 1.0);
        h.set_param("env.amp.sustain", 1.0);
    }

    #[test]
    fn the_table_choices_are_the_bank_names() {
        use atomik_dsp::wavetable::WAVE_NAMES;
        let p = DESCRIPTOR.param_by_key("osc1.table").unwrap();
        let Unit::Choice(names) = p.unit else {
            panic!("a choice")
        };
        assert_eq!(names[0], "wave");
        assert_eq!(&names[1..], &WAVE_NAMES[..]);
        assert_eq!(p.max as usize, WAVE_COUNT);
        assert_eq!(names.len(), WAVE_COUNT + 1);
    }

    #[test]
    fn process_a_table_plays_the_bank_wave_at_pitch() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.table", 1.0); // sine
        let o = steady(&mut h, 69, 4_800, 32_768);
        assert!((dominant_frequency(&o, SR) - 440.0).abs() < 1.0);
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.table", 2.0); // the additive saw: harmonics fall as 1/h
        let o = steady(&mut h, 57, 4_800, 32_768);
        assert!((dominant_frequency(&o, SR) - 220.0).abs() < 1.0);
        let second = db_at(&o, 440.0) - db_at(&o, 220.0);
        assert!((second + 6.0).abs() < 1.5, "second harmonic {second} dB");
        assert!(peak(&o) > 0.1 && peak(&o) < 4.0);
    }

    #[test]
    fn process_a_table_stays_band_limited_at_a_high_pitch() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("osc1.table", 3.0); // the additive square
        // Key 100 is 2637 Hz: its 11th harmonic (29 kHz) would fold to 18996 Hz.
        let o = steady(&mut h, 100, 4_800, 32_768);
        let alias = db_at(&o, 48_000.0 - 11.0 * 2_637.02);
        assert!(alias < -60.0, "alias {alias} dB");
        // Below Nyquist the odd harmonics are there.
        assert!(db_at(&o, 3.0 * 2_637.02) > -15.0);
    }

    #[test]
    fn process_the_ring_mode_multiplies_the_oscillators() {
        let (mut h, _) = synth();
        two_sines(&mut h);
        h.set_param("osc1.table", 1.0);
        h.set_param("osc2.table", 1.0);
        h.set_param("osc2.semi", 7.0);
        let f1 = 220.0;
        let f2 = f1 * 2.0f64.powf(7.0 / 12.0);
        let normal = steady(&mut h, 57, 4_800, 32_768);
        assert!(db_at(&normal, f2) > -1.0 && db_at(&normal, f2 - f1) < -50.0);
        let (mut h, _) = synth();
        two_sines(&mut h);
        h.set_param("osc1.table", 1.0);
        h.set_param("osc2.table", 1.0);
        h.set_param("osc2.semi", 7.0);
        h.set_param("osc2.mode", 1.0);
        let ring = steady(&mut h, 57, 4_800, 32_768);
        assert!(db_at(&ring, f2 - f1) > -1.0, "difference tone");
        assert!(db_at(&ring, f2 + f1) > -1.0, "sum tone");
        assert!(
            db_at(&ring, f1) < -40.0 && db_at(&ring, f2) < -40.0,
            "no carriers"
        );
    }

    #[test]
    fn process_the_fm_mode_adds_sidebands_with_the_amount() {
        let at = |amount: f32| {
            let (mut h, _) = synth();
            two_sines(&mut h);
            h.set_param("osc1.table", 1.0);
            h.set_param("osc2.table", 1.0);
            h.set_param("osc2.mode", 2.0);
            h.set_param("osc2.amount", amount);
            let o = steady(&mut h, 57, 4_800, 32_768);
            (db_at(&o, 440.0), db_at(&o, 660.0), peak(&o))
        };
        let (two0, three0, _) = at(0.0);
        assert!(
            two0 < -60.0 && three0 < -60.0,
            "no modulation, no sidebands"
        );
        let (two, three, pk) = at(0.3);
        assert!(two > -25.0 && three > -30.0, "{two} {three}");
        assert!(pk < 1.5);
    }

    #[test]
    fn process_the_sync_mode_locks_oscillator_2_to_oscillator_1() {
        let at = |mode: f32| {
            let (mut h, _) = synth();
            h.set_param("osc1.wave", 0.0);
            h.set_param("osc2.wave", 0.0);
            h.set_param("osc2.fine", 0.0);
            h.set_param("osc2.semi", 7.0); // not a multiple of osc 1
            h.set_param("osc.mix", 1.0);
            h.set_param("filter.cutoff", 20_000.0);
            h.set_param("filter.env_amount", 0.0);
            h.set_param("filter.key_track", 0.0);
            h.set_param("env.amp.sustain", 1.0);
            h.set_param("osc2.mode", mode);
            let o = steady(&mut h, 45, 4_800, 32_768); // 110 Hz
            (db_at(&o, 110.0), db_at(&o, 164.81))
        };
        let (own110, own165) = at(0.0);
        assert!(own110 < -40.0 && own165 > -3.0, "{own110} {own165}");
        let (sync110, sync165) = at(3.0);
        assert!(sync110 > -12.0, "the synced pitch is osc 1's: {sync110} dB");
        assert!(sync165 < sync110, "{sync165}");
    }

    /// The worst partial of `x` that is not a multiple of `f0`, in dB below the fundamental.
    fn alias_db(x: &[f32], f0: f64) -> f64 {
        let m = atomik_testkit::signal::magnitude_spectrum(x);
        let n = x.len() as f64;
        let bin = |hz: f64| (hz * n / SR).round() as usize;
        let fund = m[bin(f0) - 3..=bin(f0) + 3]
            .iter()
            .copied()
            .fold(0.0, f64::max);
        let mut masked = m.clone();
        let mut k = 1.0;
        while k * f0 < SR / 2.0 {
            let c = bin(k * f0);
            for v in masked
                .iter_mut()
                .take((c + 7).min(m.len()))
                .skip(c.saturating_sub(6))
            {
                *v = 0.0;
            }
            k += 1.0;
        }
        for v in masked.iter_mut().take(6) {
            *v = 0.0;
        }
        let worst = masked[..bin(20_000.0)]
            .iter()
            .copied()
            .fold(1e-300, f64::max);
        20.0 * (worst / fund).log10()
    }

    #[test]
    fn process_the_sync_reset_is_softened_against_aliasing() {
        use atomik_dsp::{PolyBlepOsc, Waveform};
        // Key 79 (784 Hz); osc 2 six semitones higher (amount 0.25 of 24).
        let f1 = 440.0 * 2.0f64.powf(10.0 / 12.0);
        let f2 = f1 * 2.0f64.powf(6.0 / 12.0);
        let (mut h, _) = synth();
        h.set_param("osc1.wave", 0.0);
        h.set_param("osc2.wave", 0.0);
        h.set_param("osc2.fine", 0.0);
        h.set_param("osc.mix", 1.0);
        h.set_param("filter.cutoff", 20_000.0);
        h.set_param("filter.env_amount", 0.0);
        h.set_param("filter.key_track", 0.0);
        h.set_param("env.amp.sustain", 1.0);
        h.set_param("osc2.mode", 3.0);
        h.set_param("osc2.amount", 0.25);
        let o = steady(&mut h, 79, 4_800, 1 << 15);
        let soft = alias_db(&o, f1);
        // The same sync with a plain reset at the first sample after oscillator 1 wraps.
        let (mut a, mut b) = (
            PolyBlepOsc::new(Waveform::Saw),
            PolyBlepOsc::new(Waveform::Saw),
        );
        a.set_frequency(f1 as f32, SR as f32);
        b.set_frequency(f2 as f32, SR as f32);
        let naive: Vec<f32> = (0..(1 << 15) + 4_800)
            .map(|_| {
                let before = a.phase();
                a.next_sample();
                let y = b.next_sample();
                if a.phase() < before {
                    b.set_phase(0.0);
                }
                y
            })
            .skip(4_800)
            .collect();
        let plain = alias_db(&naive, f1);
        assert!(
            soft < plain - 2.0,
            "softened {soft} dB vs plain reset {plain} dB"
        );
        assert!(soft < -20.0, "{soft}");
    }

    /// The first-difference rms (a brightness measure).
    fn bright(x: &[f32]) -> f32 {
        brightness(x)
    }

    #[test]
    fn process_the_second_filter_darkens_and_routes_in_series_or_parallel() {
        let level = |setup: &dyn Fn(&mut DeviceHarness)| {
            let (mut h, _) = synth();
            h.set_param("osc.mix", 0.0);
            h.set_param("filter.cutoff", 20_000.0);
            h.set_param("filter.env_amount", 0.0);
            h.set_param("filter.key_track", 0.0);
            h.set_param("filter.resonance", 0.0);
            h.set_param("env.amp.sustain", 1.0);
            setup(&mut h);
            let o = steady(&mut h, 36, 4_800, 24_000);
            bright(&o)
        };
        let off = level(&|_| {});
        let lp = |h: &mut DeviceHarness| {
            h.set_param("filter2.mode", 1.0);
            h.set_param("filter2.cutoff", 400.0);
            h.set_param("filter2.resonance", 0.0);
            h.set_param("filter.link", 0.0);
        };
        let series = level(&lp);
        assert!(series < off * 0.5, "{series} vs {off}");
        let parallel = level(&|h| {
            lp(h);
            h.set_param("filter2.routing", 1.0);
        });
        // In parallel the open first filter passes the highs: half of the dry brightness.
        assert!(parallel > series * 1.5 && parallel < off, "{parallel}");
        let hp = level(&|h| {
            lp(h);
            h.set_param("filter2.mode", 3.0);
        });
        assert!(hp > series, "a highpass keeps the edges");
    }

    #[test]
    fn process_the_link_makes_filter_2_follow_the_first_filters_modulation() {
        // Filter 2 lowpass at 300 Hz; the first filter is open but tracks the keyboard fully,
        // so linked, filter 2 moves with the key (2 octaves from key 60 to 84). Relative to
        // the unfiltered note, its attenuation stays the same linked and grows unlinked.
        let ratio = |key: u8, link: f32| {
            let render = |filter2: bool| {
                let (mut h, _) = synth();
                pure_sine(&mut h);
                h.set_param("osc1.wave", 0.0);
                h.set_param("filter.key_track", 1.0);
                h.set_param("env.amp.sustain", 1.0);
                if filter2 {
                    h.set_param("filter2.mode", 1.0);
                    h.set_param("filter2.cutoff", 300.0);
                    h.set_param("filter2.resonance", 0.0);
                    h.set_param("filter.link", link);
                }
                rms(&steady(&mut h, key, 9_600, 24_000))
            };
            render(true) / render(false)
        };
        let (low_l, high_l) = (ratio(60, 1.0), ratio(84, 1.0));
        assert!(
            (low_l / high_l - 1.0).abs() < 0.35,
            "linked {low_l} {high_l}"
        );
        let (low_u, high_u) = (ratio(60, 0.0), ratio(84, 0.0));
        assert!(
            high_u < high_l * 0.5,
            "unlinked {high_u} vs linked {high_l}"
        );
        assert!(
            (low_u / low_l - 1.0).abs() < 0.05,
            "middle C sits at the reference: {low_u} {low_l}"
        );
    }

    #[test]
    fn process_the_ladder_rolls_off_faster_and_drive_adds_harmonics() {
        let level = |ladder: f32, drive: f32| {
            let (mut h, _) = synth();
            h.set_param("osc.mix", 0.0);
            h.set_param("filter.cutoff", 900.0);
            h.set_param("filter.env_amount", 0.0);
            h.set_param("filter.key_track", 0.0);
            h.set_param("filter.resonance", 0.0);
            h.set_param("filter.type", ladder);
            h.set_param("filter.drive", drive);
            h.set_param("env.amp.sustain", 1.0);
            // The 55th harmonic of the saw (3.6 kHz), two octaves above the cutoff.
            db_at(&steady(&mut h, 36, 4_800, 32_768), 55.0 * 65.406)
        };
        let (svf, ladder) = (level(0.0, 0.0), level(1.0, 0.0));
        assert!(ladder < svf - 8.0, "24 dB vs 12 dB: {ladder} vs {svf}");
        // A sine through the open ladder: drive makes the odd harmonics.
        let harmonic = |drive: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("filter.type", 1.0);
            h.set_param("filter.drive", drive);
            h.set_param("env.amp.sustain", 1.0);
            let o = steady(&mut h, 57, 4_800, 32_768);
            db_at(&o, 660.0)
        };
        assert!(harmonic(0.0) < -60.0, "{}", harmonic(0.0));
        assert!(harmonic(1.0) > -25.0, "{}", harmonic(1.0));
    }

    #[test]
    fn process_the_ladder_is_stable_at_full_resonance_and_bounded() {
        for cutoff in [60.0, 800.0, 6_000.0, 20_000.0] {
            let (mut h, _) = synth();
            for (k, v) in [
                ("filter.type", 1.0),
                ("filter.resonance", 1.0),
                ("filter.cutoff", cutoff),
                ("filter.drive", 1.0),
                ("noise.level", 1.0),
                ("master.level", 6.0),
            ] {
                h.set_param(k, v);
            }
            for i in 0..8u8 {
                h.push_note("notes_in", on(u16::from(i), 30 + 9 * i, 1.0));
            }
            let o = h.render(seconds(1.0), &OUT);
            assert!(
                o[0].iter().all(|s| s.is_finite() && s.abs() < 4.0),
                "{cutoff}"
            );
        }
    }

    #[test]
    fn process_lfo_2_moves_pitch_and_level_like_lfo_1() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("lfo2.wave", 3.0);
        h.set_param("lfo2.rate", 0.5);
        h.set_param("lfo2.to_pitch", 12.0);
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(2.0), &OUT).remove(0);
        let up = dominant_frequency(&o[4_800..43_200], SR);
        let down = dominant_frequency(&o[52_800..91_200], SR);
        assert!(
            (up - 440.0).abs() < 2.0 && (down - 110.0).abs() < 2.0,
            "{up} {down}"
        );
        // To level: a square LFO at full depth is silent in its low half.
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("lfo2.wave", 3.0);
        h.set_param("lfo2.rate", 0.5);
        h.set_param("lfo2.to_level", 1.0);
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(2.0), &OUT).remove(0);
        let (high, low) = (rms(&o[4_800..43_200]), rms(&o[52_800..91_200]));
        assert!(high > 0.05 && low < high * 0.01, "{high} {low}");
        // To cutoff: it opens the filter in its high half.
        let (mut h, _) = synth();
        h.set_param("osc.mix", 0.0);
        h.set_param("filter.cutoff", 300.0);
        h.set_param("filter.env_amount", 0.0);
        h.set_param("filter.key_track", 0.0);
        h.set_param("lfo2.wave", 3.0);
        h.set_param("lfo2.rate", 0.5);
        h.set_param("lfo2.to_cutoff", 2.0);
        h.push_note("notes_in", on(0, 45, 1.0));
        let o = h.render(seconds(2.0), &OUT).remove(0);
        assert!(bright(&o[4_800..43_200]) > bright(&o[52_800..91_200]) * 2.0);
    }

    #[test]
    fn process_the_mod_envelope_moves_its_destination() {
        // Pitch: a held sustain of 1 at amount 0.5 is +12 semitones, a negative amount down.
        let freq = |amount: f32, dest: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("env.mod.sustain", 1.0);
            h.set_param("env.mod.amount", amount);
            h.set_param("env.mod.dest", dest);
            let o = steady(&mut h, 57, 9_600, 32_768);
            dominant_frequency(&o, SR)
        };
        assert!((freq(0.0, 0.0) - 220.0).abs() < 1.0);
        assert!((freq(0.5, 0.0) - 440.0).abs() < 2.0);
        assert!((freq(-0.5, 0.0) - 110.0).abs() < 1.0);
        // Osc 2 pitch: only oscillator 2 moves (osc 2 alone is audible here).
        let osc2 = |amount: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("osc2.wave", 3.0);
            h.set_param("osc2.fine", 0.0);
            h.set_param("osc.mix", 1.0);
            h.set_param("env.mod.sustain", 1.0);
            h.set_param("env.mod.amount", amount);
            h.set_param("env.mod.dest", 1.0);
            let o = steady(&mut h, 57, 9_600, 32_768);
            dominant_frequency(&o, SR)
        };
        assert!((osc2(0.5) - 440.0).abs() < 2.0 && (osc2(0.0) - 220.0).abs() < 1.0);
        // Osc mix: with the mix at oscillator 1 (a sine), the envelope fades the saw in.
        let mixed = |amount: f32| {
            let (mut h, _) = synth();
            h.set_param("osc1.wave", 3.0);
            h.set_param("osc2.wave", 0.0);
            h.set_param("osc2.fine", 0.0);
            h.set_param("osc.mix", 0.0);
            h.set_param("filter.cutoff", 20_000.0);
            h.set_param("filter.env_amount", 0.0);
            h.set_param("filter.key_track", 0.0);
            h.set_param("env.mod.sustain", 1.0);
            h.set_param("env.mod.amount", amount);
            h.set_param("env.mod.dest", 2.0);
            bright(&steady(&mut h, 45, 9_600, 24_000))
        };
        assert!(mixed(1.0) > mixed(0.0) * 2.0);
        // Osc 2 amount: the envelope opens the FM depth.
        let fm = |amount: f32| {
            let (mut h, _) = synth();
            two_sines(&mut h);
            h.set_param("osc2.mode", 2.0);
            h.set_param("env.mod.sustain", 1.0);
            h.set_param("env.mod.amount", amount);
            h.set_param("env.mod.dest", 3.0);
            let o = steady(&mut h, 57, 9_600, 32_768);
            db_at(&o, 440.0)
        };
        assert!(fm(0.0) < -60.0 && fm(0.5) > -30.0);
        // Filter 2 cutoff: a low lowpass opens by up to four octaves.
        let f2 = |amount: f32| {
            let (mut h, _) = synth();
            h.set_param("osc.mix", 0.0);
            h.set_param("filter.cutoff", 20_000.0);
            h.set_param("filter.env_amount", 0.0);
            h.set_param("filter.key_track", 0.0);
            h.set_param("filter2.mode", 1.0);
            h.set_param("filter2.cutoff", 200.0);
            h.set_param("filter2.resonance", 0.0);
            h.set_param("filter.link", 0.0);
            h.set_param("env.mod.sustain", 1.0);
            h.set_param("env.mod.amount", amount);
            h.set_param("env.mod.dest", 4.0);
            bright(&steady(&mut h, 36, 9_600, 24_000))
        };
        assert!(f2(1.0) > f2(0.0) * 3.0);
    }

    #[test]
    fn process_the_mod_envelope_follows_its_adsr() {
        // Attack 200 ms to full: the pitch rises over time and then holds.
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("env.mod.attack", 200.0);
        h.set_param("env.mod.sustain", 1.0);
        h.set_param("env.mod.amount", 0.5);
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(0.6), &OUT).remove(0);
        let early = dominant_frequency(&o[480..2_880], SR);
        let late = dominant_frequency(&o[24_000..28_800], SR);
        assert!(
            early < 300.0 && (late - 440.0).abs() < 3.0,
            "{early} {late}"
        );
    }

    #[test]
    fn process_glide_slides_a_stolen_voice_from_the_old_pitch() {
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("voices", 1.0);
        h.set_param("glide", 100.0);
        h.push_note("notes_in", on(0, 45, 1.0));
        h.render(seconds(0.4), &OUT);
        h.push_note("notes_in", on(0, 57, 1.0)); // steals the one voice
        let o = h.render(seconds(0.8), &OUT).remove(0);
        let early = dominant_frequency(&o[480..3_840], SR);
        let late = dominant_frequency(&o[28_800..38_400], SR);
        assert!(early < 190.0, "still gliding: {early}");
        assert!((late - 220.0).abs() < 3.0, "arrived: {late}");
    }

    #[test]
    fn process_legato_is_monophonic() {
        let (mut h, probe) = synth();
        pure_sine(&mut h);
        h.set_param("legato", 1.0);
        for (i, k) in [48u8, 52, 55].iter().enumerate() {
            h.push_note("notes_in", on(i as u16 * 9, *k, 1.0));
        }
        h.render(seconds(0.3), &OUT);
        assert_eq!(probe.active(), 1, "one voice whatever `voices` says");
        assert_eq!(probe.keys(), vec![55]);
        // Switching legato on released the rest.
        let (mut h, probe) = synth();
        for k in [48u8, 52, 55] {
            h.push_note("notes_in", on(0, k, 1.0));
        }
        h.render(2, &OUT);
        assert_eq!(probe.active(), 3);
        h.set_param("legato", 1.0);
        h.render(seconds(1.0), &OUT);
        assert_eq!(probe.active(), 1);
    }

    #[test]
    fn process_spread_pans_low_keys_left_and_high_keys_right() {
        let sides = |key: u8, spread: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("spread", spread);
            h.push_note("notes_in", on(0, key, 1.0));
            let o = h.render(seconds(0.4), &OUT);
            (rms(&o[0][9_600..]), rms(&o[1][9_600..]))
        };
        let (l, r) = sides(36, 0.0);
        assert_eq!(l.to_bits(), r.to_bits());
        let (l, r) = sides(36, 1.0);
        assert!(l > r * 1.4, "{l} {r}");
        let (l, r) = sides(84, 1.0);
        assert!(r > l * 1.4, "{l} {r}");
        let (l, r) = sides(60, 1.0);
        assert_eq!(l.to_bits(), r.to_bits(), "middle C is centred");
        // Half the spread is half the pan.
        let (l1, r1) = sides(36, 1.0);
        let (lh, rh) = sides(36, 0.5);
        assert!(rh / lh > r1 / l1 && rh / lh < 1.0);
    }

    #[test]
    fn process_drift_detunes_voices_slowly_and_deterministically() {
        let freqs = |drift: f32, seed: u64| {
            let mut h = DeviceHarness::with_options(Box::new(PolySynth::new()), SR, 120.0, seed);
            pure_sine(&mut h);
            h.set_param("drift", drift);
            h.set_param("env.amp.sustain", 1.0);
            h.push_note("notes_in", on(0, 57, 1.0));
            let o = h.render(seconds(2.4), &OUT).remove(0);
            (
                dominant_frequency(&o[4_800..28_800], SR),
                dominant_frequency(&o[91_200..115_200], SR),
            )
        };
        let cents = |f: f64| 1_200.0 * (f / 220.0).log2();
        let (a, b) = freqs(0.0, 3);
        assert!(cents(a).abs() < 1.0 && cents(b).abs() < 1.0);
        let mut moved = false;
        for seed in [3u64, 4, 5, 6] {
            let (a, b) = freqs(1.0, seed);
            assert!(
                cents(a).abs() < 21.0 && cents(b).abs() < 21.0,
                "{seed}: {a} {b}"
            );
            moved |= cents(a).abs() > 1.0 && (cents(a) - cents(b)).abs() > 0.5;
            assert_eq!(freqs(1.0, seed), (a, b), "deterministic");
        }
        assert!(moved, "some voice drifted away from pitch and wandered");
        // Half the drift is half the range.
        let (a, _) = freqs(1.0, 3);
        let (h, _) = freqs(0.5, 3);
        assert!((cents(h) - 0.5 * cents(a)).abs() < 1.0 + 0.1 * cents(a).abs());
    }

    #[test]
    fn process_the_chorus_and_the_echo_colour_the_output() {
        let render = |setup: &dyn Fn(&mut DeviceHarness)| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("env.amp.attack", 0.5);
            h.set_param("env.amp.release", 5.0);
            setup(&mut h);
            h.push_note("notes_in", on(0, 57, 1.0));
            h.push_note("notes_in", off(63, 57));
            h.render(seconds(0.5), &OUT)
        };
        let dry = render(&|_| {});
        // The note is one batch long: silence after it.
        assert!(rms(&dry[0][1_000..]) < 1e-4);
        let echo = render(&|h| {
            h.set_param("delay.mix", 0.8);
            h.set_param("delay.time", 100.0);
            h.set_param("delay.feedback", 0.0);
        });
        // The first echo lands 100 ms (4800 samples) after the note, on the left; the second
        // never comes.
        let (l1, r1) = (rms(&echo[0][4_800..5_100]), rms(&echo[1][4_800..5_100]));
        assert!(l1 > 0.01 && r1 < 1e-4, "{l1} {r1}");
        assert!(rms(&echo[0][9_700..12_000]) < 1e-4);
        // With feedback the echoes bounce to the other side and decay.
        let fb = render(&|h| {
            h.set_param("delay.mix", 0.8);
            h.set_param("delay.time", 100.0);
            h.set_param("delay.feedback", 0.5);
        });
        let (r2, l3) = (rms(&fb[1][9_600..9_900]), rms(&fb[0][14_400..14_700]));
        assert!(r2 > 0.2 * l1 && r2 < l1, "{l1} {r2}");
        assert!(l3 > 0.1 * r2 && l3 < r2, "{r2} {l3}");
        assert!(
            rms(&fb[1][4_800..5_100]) < 1e-4,
            "the first echo is left only"
        );
        // The chorus on a held tone: not the dry bits, same level class.
        let held = |mix: f32| {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.set_param("env.amp.sustain", 1.0);
            h.set_param("chorus.mix", mix);
            h.set_param("chorus.depth", 1.0);
            h.set_param("chorus.rate", 2.0);
            h.push_note("notes_in", on(0, 57, 1.0));
            h.render(seconds(0.5), &OUT)
        };
        let (c0, c1) = (held(0.0), held(0.5));
        assert_ne!(c0[0], c1[0]);
        assert!(c1[0].iter().all(|s| s.is_finite() && s.abs() < 4.0));
        assert!((rms(&c1[0][12_000..]) / rms(&c0[0][12_000..]) - 1.0).abs() < 0.5);
        assert_ne!(
            c1[0], c1[1],
            "the chorus sweeps the sides against each other"
        );
        // Turning the effect off again leaves nothing behind.
        let (mut h, _) = synth();
        pure_sine(&mut h);
        h.set_param("env.amp.release", 5.0);
        h.set_param("delay.mix", 0.8);
        h.push_note("notes_in", on(0, 57, 1.0));
        h.push_note("notes_in", off(10, 57));
        h.render(20, &OUT);
        h.set_param("delay.mix", 0.0);
        let o = h.render(seconds(0.3), &OUT);
        assert!(rms(&o[0]) < 1e-4);
    }

    /// Every appended param set to something audible, notes playing, params changing.
    fn everything_on(h: &mut DeviceHarness) {
        for (k, v) in [
            ("osc1.table", 5.0),
            ("osc2.table", 17.0),
            ("osc2.mode", 3.0),
            ("osc2.amount", 0.3),
            ("filter.type", 1.0),
            ("filter.drive", 0.5),
            ("filter2.mode", 2.0),
            ("filter2.cutoff", 1_500.0),
            ("filter2.resonance", 0.6),
            ("lfo2.wave", 4.0),
            ("lfo2.to_pitch", 0.5),
            ("lfo2.to_cutoff", 1.0),
            ("lfo2.to_level", 0.3),
            ("env.mod.amount", 0.4),
            ("env.mod.dest", 1.0),
            ("glide", 80.0),
            ("spread", 0.8),
            ("drift", 0.7),
            ("chorus.mix", 0.4),
            ("delay.mix", 0.3),
        ] {
            h.set_param(k, v);
        }
    }

    #[test]
    fn process_with_every_appended_param_on_is_allocation_free() {
        let (mut h, _) = synth();
        everything_on(&mut h);
        *h.input_mut("cutoff_cv_in") = [0.4; BATCH];
        *h.input_mut("resonance_cv_in") = [0.3; BATCH];
        for i in 0..300u32 {
            let key = 30 + (i * 7 % 60) as u8;
            h.push_note("notes_in", on((i % 64) as u16, key, 0.9));
            if i % 3 == 0 {
                h.push_note("notes_in", off(63, key));
            }
            h.set_param("legato", (i / 50 % 2) as f32);
            h.set_param("osc2.mode", (i % 4) as f32);
            h.set_param("osc1.table", (i % 33) as f32);
            h.set_param("filter.type", (i / 7 % 2) as f32);
            h.set_param("voices", (1 + i % 16) as f32);
            h.process();
        }
        h.device_mut().reset();
        h.process();
    }

    #[test]
    fn process_with_every_appended_param_on_is_deterministic_and_bounded() {
        let run = |seed: u64| {
            let mut h = DeviceHarness::with_options(Box::new(PolySynth::new()), SR, 120.0, seed);
            everything_on(&mut h);
            for (i, k) in [40u8, 47, 52, 59, 64].iter().enumerate() {
                h.push_note("notes_in", on(i as u16 * 11, *k, 0.9));
            }
            h.render(seconds(1.0), &OUT)
        };
        let a = run(8);
        assert_eq!(a, run(8));
        assert_ne!(a, run(9));
        assert!(a.iter().flatten().all(|s| s.is_finite() && s.abs() < 4.0));
        assert!(rms(&a[0]) > 0.005);
    }

    #[test]
    fn handle_pressure_reaches_the_mono_bass() {
        let mut s = PolySynth::with_descriptor(&crate::mono_bass::DESCRIPTOR);
        s.handle(NoteKind::ChannelPressure { value: 0.25 });
        assert_eq!(s.ctl.pressure(60), 0.25);
        assert_eq!(s.descriptor().type_id, "atomik.mono-bass");
        assert_eq!(
            s.values[P_LEGATO as usize],
            crate::mono_bass::DESCRIPTOR
                .param_by_key("legato")
                .unwrap()
                .default
        );
    }

    #[test]
    fn master_tune_shifts_the_pitch_by_the_cents_ratio() {
        for cents in [50.0f32, -100.0, 100.0] {
            let (mut h, _) = synth();
            pure_sine(&mut h);
            h.time_mut().tune_cents = cents;
            h.push_note("notes_in", on(0, 69, 1.0));
            let o = h.render(seconds(1.0), &OUT);
            let f = dominant_frequency(&o[0][..48_000], SR);
            let want = 440.0 * 2f64.powf(f64::from(cents) / 1200.0);
            assert!((f - want).abs() < 0.6, "{cents} cents: {f} vs {want}");
        }
    }
}
