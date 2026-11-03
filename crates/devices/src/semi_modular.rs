//! `atomik.semi-modular` ("Semi-Modular"): a synthesizer built from slots. Every voice has three
//! oscillator slots, three filter slots and a shaper; the modulation bus ([`MOD_TABLES`], eight
//! slots) joins any source to any of fifteen targets; a sixteen-step sequencer plays the voices
//! from the keys.
//!
//! # Signal flow
//!
//! ```text
//! per voice:   osc 1 + osc 2 + osc 3 (each: type, tuning, level, shape)
//!              -> filter slots 1..3 (routing: series, parallel, or 1 then 2 + 3)
//!              -> shaper (type, drive, mix) -> amp envelope . velocity . level -> key pan
//! all voices:  sum . level (5 ms smoothing) . soft limiter . volume and expression -> out L/R
//! ```
//!
//! * **Oscillator slots.** `osc1.*` to `osc3.*`. `type` is one of `saw`, `pulse`, `triangle`,
//!   `sine` (band-limited by PolyBLEP/PolyBLAMP or exact), `wave` (one of the 32 additive waves
//!   of [`atomik_dsp::wavetable::wave_bank`], picked with `wave`), `table` (one of the 12 factory
//!   wavetables, `wave` is the table and `shape` the position in it), `fm` (a sine whose phase a
//!   second sine at the ratio `0.5 + wave / 4` moves by `shape` times 6 radians) and `noise`.
//!   `shape` is also the narrowing of `pulse` (a square at 0, 5 % wide at 1). The tuning is
//!   `octave`, `coarse` (semitones) and `fine` (cents); `level` is the slot's share of the mix.
//!   The wave and table types read the mip level that keeps every harmonic under Nyquist; `fm`
//!   scales its index down on a high key so the highest sideband stays under 0.4 of the sample
//!   rate. Oscillators restart at phase 0 with every note that starts a voice.
//! * **Filter slots.** `filter1.*` to `filter3.*`. `mode` is `off`, `LP12`, `BP12`, `HP12`,
//!   `notch` (state-variable), `LP24` (the ladder) or `comb` (resonance is its feedback, up to
//!   0.9; the cutoff is `1 / delay`). The cutoff moves by `env` octaves times the filter
//!   envelope, `track` octaves per octave of key (around middle C), the mod wheel by
//!   `wheel.to_cutoff` octaves, the cutoff CV by `cv.cutoff_amount` octaves and the matrix by up
//!   to 6 octaves. `mix` blends the slot with its input. `filter.routing` is `series`
//!   (1, 2, 3), `parallel` (the average of the slots that are on) or `split` (1, then the average
//!   of 2 and 3). A slot that is off passes its input.
//! * **Shaper.** `shaper.type` (`off`, `soft`, `asymmetric`, `hard`, `fold`) with `shaper.drive`
//!   in dB and `shaper.mix`, anti-aliased (first-order antiderivative) after the filters.
//! * **Envelopes and LFOs.** `env.amp.*`, `env.filter.*` and `env.mod.*` (ADSR) and two LFOs
//!   (`lfo1.*`, `lfo2.*`; the first is written to `lfo_cv_out`). The filter envelope drives the
//!   `env` amounts of the filters; the others reach their targets through the matrix.
//! * **Modulation bus.** Sources: the three envelopes, both LFOs, velocity, key, mod wheel,
//!   pressure, the current step of the sequencer (its pitch, +-24 semitones as +-1) and a random
//!   value drawn at every note start. Targets: pitch (24 semitones at full amount), the level and
//!   shape of each oscillator slot, the cutoff (6 octaves) and resonance of each filter slot, the
//!   shaper drive (24 dB) and the voice level. Slots are summed at the control rate (every 16
//!   samples) in the slot order, see [`atomik_device_sdk::ModBus`].
//! * **Play.** `play.mode`: `poly` (up to `voices`, default 4, at most 8; a note without a free
//!   voice steals the oldest, which fades out over 2 ms), `mono` (one voice, every key retriggers
//!   the envelopes and a released key returns to the key still held) and `legato` (as mono, but a
//!   key played while another is held changes the pitch only). `play.glide` is the time in ms a
//!   pitch takes to cover most of the way to the new key in the mono modes. With one voice the
//!   cost is that of one voice; the cost of the poly mode is `voices` times that.
//! * **Sequencer.** With `seq.run` on, the keys no longer play notes: they start and transpose
//!   the pattern. The last key pressed is the root. The pattern starts at step 1 when the first
//!   key goes down, runs on its own clock at the tempo of the transport (`seq.rate` is the step
//!   length, `seq.swing` delays odd steps by up to 75 % of a step) and stops when the last key is
//!   released. `seq.length` steps (1 to 16) repeat;
//!   step `n` plays when `seq.on.n` is set, `seq.pitch.n` semitones above the root with
//!   `seq.vel.n`. `seq.gate` is how long a note lasts as a share of the step; 100 % ties it to
//!   the next one (the mono modes glide, the poly mode overlaps). `seq_gate_cv_out` and
//!   `seq_pitch_cv_out` carry the gate and the pitch (`(key - 60) / 24`) of the sounding step
//!   to other devices.
//! * **Controllers.** Pitch bend (`bend.range`, RPN 0), mod wheel (CC 1, `wheel.to_cutoff` and a
//!   matrix source), sustain, volume, expression and pressure as for the other instruments
//!   ([`ControllerState`]). The sustain pedal does not hold sequencer notes.
//! * **CV.** `pitch_cv_in` (`cv.pitch_amount` semitones at +1), `cutoff_cv_in`
//!   (`cv.cutoff_amount` octaves) and `level_cv_in` (`cv.level_amount` times 24 dB).
//! * **Realtime.** Nothing allocates, locks or blocks in `process`, `reset` and `set_param`.
//!   Deterministic: the only randomness is the noise oscillator, the random source and the LFO's
//!   sample and hold, seeded by the patch seed; the output is the same bit for bit however the
//!   host splits the render. Every output is bounded by 4 (the poly synth's soft limiter).

use atomik_core::{BATCH, NoteKind, PPQ, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, MOD_SLOT_PARAMS, ModBus,
    ModSource, ModTables, ModTarget, PrepareInfo, ProcessContext, SocketDescriptor, SocketId,
    SocketKind, join_params, mod_slot_params,
};
use atomik_dsp::wavetable::{TABLE_COUNT, WAVE_COUNT, factory_bank, level_for, wave_bank};
use atomik_dsp::{
    Adsr, Comb, FilterMode, Ladder, Lfo, LfoShape, PolyBlepOsc, Rng, Shaper, ShaperKind, Smoother,
    Svf, Waveform, balance_gains, exp2_det, note_to_hz, sin_cycles,
};

use crate::controller_state::{ControllerState, Effect};
use crate::cv_in::{clean, db_gain};
use crate::note_clock::Grid;
use crate::poly_synth::{derive_seed, soft_limit};

/// Preallocated voices.
pub const MAX_VOICES: usize = 8;
/// Oscillator slots and filter slots per voice.
pub const SLOTS: usize = 3;
/// Steps of the sequencer.
pub const STEPS: usize = 16;

/// Params of one oscillator slot.
const OSC_FIELDS: usize = 7;
/// Params of one filter slot.
const FILTER_FIELDS: usize = 6;

const P_VOICES: u32 = 0;
const P_MODE: u32 = 1;
const P_GLIDE: u32 = 2;
const P_BEND_RANGE: u32 = 3;
const P_LEVEL: u32 = 4;
const P_SPREAD: u32 = 5;
const P_WHEEL_CUTOFF: u32 = 6;
const OSC_BASE: u32 = 7;
const F_TYPE: usize = 0;
const F_WAVE: usize = 1;
const F_OCTAVE: usize = 2;
const F_COARSE: usize = 3;
const F_FINE: usize = 4;
const F_LEVEL: usize = 5;
const F_SHAPE: usize = 6;
const FILTER_BASE: u32 = OSC_BASE + (SLOTS * OSC_FIELDS) as u32;
const G_MODE: usize = 0;
const G_CUTOFF: usize = 1;
const G_RESONANCE: usize = 2;
const G_ENV: usize = 3;
const G_TRACK: usize = 4;
const G_MIX: usize = 5;
const P_ROUTING: u32 = FILTER_BASE + (SLOTS * FILTER_FIELDS) as u32;
const P_SHAPER_TYPE: u32 = P_ROUTING + 1;
const P_SHAPER_DRIVE: u32 = P_ROUTING + 2;
const P_SHAPER_MIX: u32 = P_ROUTING + 3;
const ENV_BASE: u32 = P_ROUTING + 4;
const P_LFO1_WAVE: u32 = ENV_BASE + 12;
const P_LFO1_RATE: u32 = P_LFO1_WAVE + 1;
const P_LFO2_WAVE: u32 = P_LFO1_WAVE + 2;
const P_LFO2_RATE: u32 = P_LFO1_WAVE + 3;
const P_SEQ_RUN: u32 = P_LFO1_WAVE + 4;
const P_SEQ_RATE: u32 = P_SEQ_RUN + 1;
const P_SEQ_LENGTH: u32 = P_SEQ_RUN + 2;
const P_SEQ_SWING: u32 = P_SEQ_RUN + 3;
const P_SEQ_GATE: u32 = P_SEQ_RUN + 4;
const P_STEP_ON: u32 = P_SEQ_RUN + 5;
const P_STEP_PITCH: u32 = P_STEP_ON + STEPS as u32;
const P_STEP_VEL: u32 = P_STEP_PITCH + STEPS as u32;
const P_CV_PITCH: u32 = P_STEP_VEL + STEPS as u32;
const P_CV_CUTOFF: u32 = P_CV_PITCH + 1;
const P_CV_LEVEL: u32 = P_CV_PITCH + 2;
/// The first param of the modulation slot block.
pub const P_MOD_FIRST: u32 = P_CV_PITCH + 3;
const SYNTH_PARAM_COUNT: usize = P_MOD_FIRST as usize;
const PARAM_COUNT: usize = SYNTH_PARAM_COUNT + MOD_SLOT_PARAMS;

macro_rules! per_slot {
    ($keys:ident, $names:ident, $kp:literal, $ks:literal, $np:literal, $ns:literal) => {
        const $keys: [&str; SLOTS] = [
            concat!($kp, "1", $ks),
            concat!($kp, "2", $ks),
            concat!($kp, "3", $ks),
        ];
        const $names: [&str; SLOTS] = [
            concat!($np, "1", $ns),
            concat!($np, "2", $ns),
            concat!($np, "3", $ns),
        ];
    };
}

macro_rules! per_step {
    ($keys:ident, $names:ident, $kp:literal, $np:literal, $ns:literal) => {
        const $keys: [&str; STEPS] = [
            concat!($kp, "1"),
            concat!($kp, "2"),
            concat!($kp, "3"),
            concat!($kp, "4"),
            concat!($kp, "5"),
            concat!($kp, "6"),
            concat!($kp, "7"),
            concat!($kp, "8"),
            concat!($kp, "9"),
            concat!($kp, "10"),
            concat!($kp, "11"),
            concat!($kp, "12"),
            concat!($kp, "13"),
            concat!($kp, "14"),
            concat!($kp, "15"),
            concat!($kp, "16"),
        ];
        const $names: [&str; STEPS] = [
            concat!($np, "1", $ns),
            concat!($np, "2", $ns),
            concat!($np, "3", $ns),
            concat!($np, "4", $ns),
            concat!($np, "5", $ns),
            concat!($np, "6", $ns),
            concat!($np, "7", $ns),
            concat!($np, "8", $ns),
            concat!($np, "9", $ns),
            concat!($np, "10", $ns),
            concat!($np, "11", $ns),
            concat!($np, "12", $ns),
            concat!($np, "13", $ns),
            concat!($np, "14", $ns),
            concat!($np, "15", $ns),
            concat!($np, "16", $ns),
        ];
    };
}

per_slot!(
    OSC_TYPE_KEYS,
    OSC_TYPE_NAMES,
    "osc",
    ".type",
    "Osc ",
    " type"
);
per_slot!(
    OSC_WAVE_KEYS,
    OSC_WAVE_NAMES,
    "osc",
    ".wave",
    "Osc ",
    " wave"
);
per_slot!(
    OSC_OCT_KEYS,
    OSC_OCT_NAMES,
    "osc",
    ".octave",
    "Osc ",
    " octave"
);
per_slot!(
    OSC_COARSE_KEYS,
    OSC_COARSE_NAMES,
    "osc",
    ".coarse",
    "Osc ",
    " coarse"
);
per_slot!(
    OSC_FINE_KEYS,
    OSC_FINE_NAMES,
    "osc",
    ".fine",
    "Osc ",
    " fine"
);
per_slot!(
    OSC_LEVEL_KEYS,
    OSC_LEVEL_NAMES,
    "osc",
    ".level",
    "Osc ",
    " level"
);
per_slot!(
    OSC_SHAPE_KEYS,
    OSC_SHAPE_NAMES,
    "osc",
    ".shape",
    "Osc ",
    " shape"
);
per_slot!(
    FLT_MODE_KEYS,
    FLT_MODE_NAMES,
    "filter",
    ".mode",
    "Filter ",
    " mode"
);
per_slot!(
    FLT_CUTOFF_KEYS,
    FLT_CUTOFF_NAMES,
    "filter",
    ".cutoff",
    "Filter ",
    " cutoff"
);
per_slot!(
    FLT_RES_KEYS,
    FLT_RES_NAMES,
    "filter",
    ".resonance",
    "Filter ",
    " resonance"
);
per_slot!(
    FLT_ENV_KEYS,
    FLT_ENV_NAMES,
    "filter",
    ".env",
    "Filter ",
    " envelope"
);
per_slot!(
    FLT_TRACK_KEYS,
    FLT_TRACK_NAMES,
    "filter",
    ".track",
    "Filter ",
    " key track"
);
per_slot!(
    FLT_MIX_KEYS,
    FLT_MIX_NAMES,
    "filter",
    ".mix",
    "Filter ",
    " mix"
);
per_step!(STEP_ON_KEYS, STEP_ON_NAMES, "seq.on.", "Step ", " on");
per_step!(
    STEP_PITCH_KEYS,
    STEP_PITCH_NAMES,
    "seq.pitch.",
    "Step ",
    " pitch"
);
per_step!(
    STEP_VEL_KEYS,
    STEP_VEL_NAMES,
    "seq.vel.",
    "Step ",
    " velocity"
);

/// The pattern a new device holds: semitones above the root.
const STEP_PITCHES: [f32; STEPS] = [
    0.0, 0.0, 12.0, 0.0, 7.0, 0.0, 10.0, 12.0, 0.0, 0.0, 12.0, 0.0, 5.0, 3.0, 7.0, 0.0,
];
/// Velocity of each step of the default pattern (accents on the beats).
const STEP_VELS: [f32; STEPS] = [
    1.0, 0.6, 0.8, 0.6, 1.0, 0.6, 0.8, 0.7, 1.0, 0.6, 0.8, 0.6, 1.0, 0.7, 0.8, 0.7,
];

const OSC_TYPES: Unit = Unit::Choice(&[
    "saw", "pulse", "triangle", "sine", "wave", "table", "fm", "noise",
]);
const FILTER_MODES: Unit = Unit::Choice(&["off", "LP12", "BP12", "HP12", "notch", "LP24", "comb"]);
const ROUTINGS: Unit = Unit::Choice(&["series", "parallel", "split"]);
const SHAPER_TYPES: Unit = Unit::Choice(&["off", "soft", "asymmetric", "hard", "fold"]);
const LFO_WAVES: Unit = Unit::Choice(&["sine", "triangle", "saw", "square", "sample&hold"]);
const PLAY_MODES: Unit = Unit::Choice(&["poly", "mono", "legato"]);
const SEQ_RATES: Unit = Unit::Choice(&["1/4", "1/8", "1/8T", "1/16", "1/16T", "1/32"]);
const ENV_MS: (f32, f32) = (0.5, 10_000.0);

/// The length of each `seq.rate` in ticks.
const RATE_TICKS: [u32; 6] = [
    (PPQ) as u32,
    (PPQ / 2) as u32,
    (PPQ / 3) as u32,
    (PPQ / 4) as u32,
    (PPQ / 6) as u32,
    (PPQ / 8) as u32,
];

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

use Taper::{Decibel as Db, Exponential as Exp, Linear as Lin};

/// Types, waves, levels and tuning each oscillator slot starts with.
const OSC_DEFAULT_TYPE: [f32; SLOTS] = [0.0, 0.0, 3.0];
const OSC_DEFAULT_LEVEL: [f32; SLOTS] = [0.7, 0.5, 0.0];
const OSC_DEFAULT_FINE: [f32; SLOTS] = [0.0, 7.0, 0.0];
const OSC_DEFAULT_OCTAVE: [f32; SLOTS] = [0.0, 0.0, -1.0];
/// Mode, cutoff, mix of each filter slot at the start (the first is a low-pass).
const FLT_DEFAULT_MODE: [f32; SLOTS] = [1.0, 0.0, 0.0];
const FLT_DEFAULT_CUTOFF: [f32; SLOTS] = [3_000.0, 1_000.0, 500.0];

const fn build_params() -> [ParamDescriptor; SYNTH_PARAM_COUNT] {
    let blank = param(0, "", "", (0.0, 1.0, 0.0), Lin, Unit::None, false);
    let mut o = [blank; SYNTH_PARAM_COUNT];
    o[P_VOICES as usize] = param(
        P_VOICES,
        "voices",
        "Voices",
        (1.0, MAX_VOICES as f32, 4.0),
        Lin,
        Unit::None,
        true,
    );
    o[P_MODE as usize] = param(
        P_MODE,
        "play.mode",
        "Play mode",
        (0.0, 2.0, 0.0),
        Lin,
        PLAY_MODES,
        true,
    );
    o[P_GLIDE as usize] = param(
        P_GLIDE,
        "play.glide",
        "Glide",
        (0.0, 2_000.0, 0.0),
        Lin,
        Unit::Ms,
        false,
    );
    o[P_BEND_RANGE as usize] = param(
        P_BEND_RANGE,
        "bend.range",
        "Bend range",
        (0.0, 24.0, 2.0),
        Lin,
        Unit::Semitones,
        false,
    );
    o[P_LEVEL as usize] = param(
        P_LEVEL,
        "master.level",
        "Level",
        (-60.0, 6.0, -6.0),
        Db,
        Unit::Db,
        false,
    );
    o[P_SPREAD as usize] = param(
        P_SPREAD,
        "master.spread",
        "Spread",
        (0.0, 1.0, 0.25),
        Lin,
        Unit::Percent,
        false,
    );
    o[P_WHEEL_CUTOFF as usize] = param(
        P_WHEEL_CUTOFF,
        "wheel.to_cutoff",
        "Wheel to cutoff",
        (0.0, 4.0, 1.5),
        Lin,
        Unit::Octaves,
        false,
    );
    let mut s = 0;
    while s < SLOTS {
        let b = OSC_BASE as usize + s * OSC_FIELDS;
        o[b + F_TYPE] = param(
            b as u32 + F_TYPE as u32,
            OSC_TYPE_KEYS[s],
            OSC_TYPE_NAMES[s],
            (0.0, 7.0, OSC_DEFAULT_TYPE[s]),
            Lin,
            OSC_TYPES,
            true,
        );
        o[b + F_WAVE] = param(
            b as u32 + F_WAVE as u32,
            OSC_WAVE_KEYS[s],
            OSC_WAVE_NAMES[s],
            (0.0, (WAVE_COUNT - 1) as f32, 0.0),
            Lin,
            Unit::None,
            true,
        );
        o[b + F_OCTAVE] = param(
            b as u32 + F_OCTAVE as u32,
            OSC_OCT_KEYS[s],
            OSC_OCT_NAMES[s],
            (-3.0, 3.0, OSC_DEFAULT_OCTAVE[s]),
            Lin,
            Unit::Octaves,
            true,
        );
        o[b + F_COARSE] = param(
            b as u32 + F_COARSE as u32,
            OSC_COARSE_KEYS[s],
            OSC_COARSE_NAMES[s],
            (-12.0, 12.0, 0.0),
            Lin,
            Unit::Semitones,
            true,
        );
        o[b + F_FINE] = param(
            b as u32 + F_FINE as u32,
            OSC_FINE_KEYS[s],
            OSC_FINE_NAMES[s],
            (-50.0, 50.0, OSC_DEFAULT_FINE[s]),
            Lin,
            Unit::Cents,
            false,
        );
        o[b + F_LEVEL] = param(
            b as u32 + F_LEVEL as u32,
            OSC_LEVEL_KEYS[s],
            OSC_LEVEL_NAMES[s],
            (0.0, 1.0, OSC_DEFAULT_LEVEL[s]),
            Lin,
            Unit::Percent,
            false,
        );
        o[b + F_SHAPE] = param(
            b as u32 + F_SHAPE as u32,
            OSC_SHAPE_KEYS[s],
            OSC_SHAPE_NAMES[s],
            (0.0, 1.0, 0.0),
            Lin,
            Unit::Percent,
            false,
        );
        let g = FILTER_BASE as usize + s * FILTER_FIELDS;
        o[g + G_MODE] = param(
            g as u32 + G_MODE as u32,
            FLT_MODE_KEYS[s],
            FLT_MODE_NAMES[s],
            (0.0, 6.0, FLT_DEFAULT_MODE[s]),
            Lin,
            FILTER_MODES,
            true,
        );
        o[g + G_CUTOFF] = param(
            g as u32 + G_CUTOFF as u32,
            FLT_CUTOFF_KEYS[s],
            FLT_CUTOFF_NAMES[s],
            (20.0, 20_000.0, FLT_DEFAULT_CUTOFF[s]),
            Exp,
            Unit::Hz,
            false,
        );
        o[g + G_RESONANCE] = param(
            g as u32 + G_RESONANCE as u32,
            FLT_RES_KEYS[s],
            FLT_RES_NAMES[s],
            (0.0, 1.0, 0.2),
            Lin,
            Unit::Percent,
            false,
        );
        o[g + G_ENV] = param(
            g as u32 + G_ENV as u32,
            FLT_ENV_KEYS[s],
            FLT_ENV_NAMES[s],
            (-6.0, 6.0, if s == 0 { 2.0 } else { 0.0 }),
            Lin,
            Unit::Octaves,
            false,
        );
        o[g + G_TRACK] = param(
            g as u32 + G_TRACK as u32,
            FLT_TRACK_KEYS[s],
            FLT_TRACK_NAMES[s],
            (0.0, 1.0, 0.5),
            Lin,
            Unit::Percent,
            false,
        );
        o[g + G_MIX] = param(
            g as u32 + G_MIX as u32,
            FLT_MIX_KEYS[s],
            FLT_MIX_NAMES[s],
            (0.0, 1.0, 1.0),
            Lin,
            Unit::Percent,
            false,
        );
        s += 1;
    }
    o[P_ROUTING as usize] = param(
        P_ROUTING,
        "filter.routing",
        "Filter routing",
        (0.0, 2.0, 0.0),
        Lin,
        ROUTINGS,
        true,
    );
    o[P_SHAPER_TYPE as usize] = param(
        P_SHAPER_TYPE,
        "shaper.type",
        "Shaper type",
        (0.0, 4.0, 0.0),
        Lin,
        SHAPER_TYPES,
        true,
    );
    o[P_SHAPER_DRIVE as usize] = param(
        P_SHAPER_DRIVE,
        "shaper.drive",
        "Shaper drive",
        (0.0, 36.0, 6.0),
        Lin,
        Unit::Db,
        false,
    );
    o[P_SHAPER_MIX as usize] = param(
        P_SHAPER_MIX,
        "shaper.mix",
        "Shaper mix",
        (0.0, 1.0, 1.0),
        Lin,
        Unit::Percent,
        false,
    );
    // The three envelopes: amp, filter, mod (attack, decay, sustain, release each).
    const ENV_KEYS: [[&str; 4]; 3] = [
        [
            "env.amp.attack",
            "env.amp.decay",
            "env.amp.sustain",
            "env.amp.release",
        ],
        [
            "env.filter.attack",
            "env.filter.decay",
            "env.filter.sustain",
            "env.filter.release",
        ],
        [
            "env.mod.attack",
            "env.mod.decay",
            "env.mod.sustain",
            "env.mod.release",
        ],
    ];
    const ENV_NAMES: [[&str; 4]; 3] = [
        ["Amp attack", "Amp decay", "Amp sustain", "Amp release"],
        [
            "Filter attack",
            "Filter decay",
            "Filter sustain",
            "Filter release",
        ],
        ["Mod attack", "Mod decay", "Mod sustain", "Mod release"],
    ];
    const ENV_DEFAULTS: [[f32; 4]; 3] = [
        [2.0, 300.0, 0.8, 250.0],
        [2.0, 400.0, 0.2, 300.0],
        [5.0, 600.0, 0.5, 300.0],
    ];
    let mut e = 0;
    while e < 3 {
        let mut f = 0;
        while f < 4 {
            let id = ENV_BASE + (e * 4 + f) as u32;
            let sustain = f == 2;
            o[id as usize] = param(
                id,
                ENV_KEYS[e][f],
                ENV_NAMES[e][f],
                if sustain {
                    (0.0, 1.0, ENV_DEFAULTS[e][f])
                } else {
                    (ENV_MS.0, ENV_MS.1, ENV_DEFAULTS[e][f])
                },
                if sustain { Lin } else { Exp },
                if sustain { Unit::Percent } else { Unit::Ms },
                false,
            );
            f += 1;
        }
        e += 1;
    }
    o[P_LFO1_WAVE as usize] = param(
        P_LFO1_WAVE,
        "lfo1.wave",
        "LFO 1 wave",
        (0.0, 4.0, 0.0),
        Lin,
        LFO_WAVES,
        true,
    );
    o[P_LFO1_RATE as usize] = param(
        P_LFO1_RATE,
        "lfo1.rate",
        "LFO 1 rate",
        (0.05, 30.0, 2.0),
        Exp,
        Unit::Hz,
        false,
    );
    o[P_LFO2_WAVE as usize] = param(
        P_LFO2_WAVE,
        "lfo2.wave",
        "LFO 2 wave",
        (0.0, 4.0, 1.0),
        Lin,
        LFO_WAVES,
        true,
    );
    o[P_LFO2_RATE as usize] = param(
        P_LFO2_RATE,
        "lfo2.rate",
        "LFO 2 rate",
        (0.05, 30.0, 0.5),
        Exp,
        Unit::Hz,
        false,
    );
    o[P_SEQ_RUN as usize] = param(
        P_SEQ_RUN,
        "seq.run",
        "Sequencer",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Toggle,
        true,
    );
    o[P_SEQ_RATE as usize] = param(
        P_SEQ_RATE,
        "seq.rate",
        "Step length",
        (0.0, 5.0, 3.0),
        Lin,
        SEQ_RATES,
        true,
    );
    o[P_SEQ_LENGTH as usize] = param(
        P_SEQ_LENGTH,
        "seq.length",
        "Steps",
        (1.0, STEPS as f32, STEPS as f32),
        Lin,
        Unit::None,
        true,
    );
    o[P_SEQ_SWING as usize] = param(
        P_SEQ_SWING,
        "seq.swing",
        "Swing",
        (0.0, 75.0, 0.0),
        Lin,
        Unit::None,
        true,
    );
    o[P_SEQ_GATE as usize] = param(
        P_SEQ_GATE,
        "seq.gate",
        "Gate",
        (5.0, 100.0, 60.0),
        Lin,
        Unit::None,
        true,
    );
    let mut n = 0;
    while n < STEPS {
        o[P_STEP_ON as usize + n] = param(
            P_STEP_ON + n as u32,
            STEP_ON_KEYS[n],
            STEP_ON_NAMES[n],
            (0.0, 1.0, 1.0),
            Lin,
            Unit::Toggle,
            true,
        );
        o[P_STEP_PITCH as usize + n] = param(
            P_STEP_PITCH + n as u32,
            STEP_PITCH_KEYS[n],
            STEP_PITCH_NAMES[n],
            (-24.0, 24.0, STEP_PITCHES[n]),
            Lin,
            Unit::Semitones,
            true,
        );
        o[P_STEP_VEL as usize + n] = param(
            P_STEP_VEL + n as u32,
            STEP_VEL_KEYS[n],
            STEP_VEL_NAMES[n],
            (0.0, 1.0, STEP_VELS[n]),
            Lin,
            Unit::Percent,
            false,
        );
        n += 1;
    }
    o[P_CV_PITCH as usize] = param(
        P_CV_PITCH,
        "cv.pitch_amount",
        "Pitch CV amount",
        (0.0, 1.0, 1.0),
        Lin,
        Unit::Percent,
        false,
    );
    o[P_CV_CUTOFF as usize] = param(
        P_CV_CUTOFF,
        "cv.cutoff_amount",
        "Cutoff CV amount",
        (0.0, 4.0, 2.0),
        Lin,
        Unit::Octaves,
        false,
    );
    o[P_CV_LEVEL as usize] = param(
        P_CV_LEVEL,
        "cv.level_amount",
        "Level CV amount",
        (0.0, 1.0, 1.0),
        Lin,
        Unit::Percent,
        false,
    );
    o
}

const SYNTH_PARAMS: [ParamDescriptor; SYNTH_PARAM_COUNT] = build_params();

/// The device's params: its own, then the modulation slot block.
static PARAMS: [ParamDescriptor; PARAM_COUNT] =
    join_params(SYNTH_PARAMS, mod_slot_params(P_MOD_FIRST));

/// Number of modulation sources.
const SOURCE_COUNT: usize = 11;
const SRC_AMP_ENV: usize = 0;
const SRC_FILTER_ENV: usize = 1;
const SRC_MOD_ENV: usize = 2;
const SRC_LFO1: usize = 3;
const SRC_LFO2: usize = 4;
const SRC_VELOCITY: usize = 5;
const SRC_KEY: usize = 6;
const SRC_WHEEL: usize = 7;
const SRC_PRESSURE: usize = 8;
const SRC_STEP: usize = 9;
const SRC_RANDOM: usize = 10;

const fn mod_source(id: u16, key: &'static str, name: &'static str, bipolar: bool) -> ModSource {
    ModSource {
        id,
        key,
        name,
        bipolar,
    }
}

/// Modulation sources, in id order (ids are stored in projects: append only).
pub static MOD_SOURCES: [ModSource; SOURCE_COUNT] = [
    mod_source(0, "amp_env", "Amp envelope", false),
    mod_source(1, "filter_env", "Filter envelope", false),
    mod_source(2, "mod_env", "Mod envelope", false),
    mod_source(3, "lfo1", "LFO 1", true),
    mod_source(4, "lfo2", "LFO 2", true),
    mod_source(5, "velocity", "Velocity", false),
    mod_source(6, "key", "Key (C4 is 0)", true),
    mod_source(7, "mod_wheel", "Mod wheel", false),
    mod_source(8, "pressure", "Pressure", false),
    mod_source(9, "step", "Sequencer step", true),
    mod_source(10, "random", "Random per note", true),
];

const fn mod_target(id: u16, key: &'static str, name: &'static str, param: u32) -> ModTarget {
    ModTarget {
        id,
        key,
        name,
        param: ParamId(param),
    }
}

const T_PITCH: u16 = 0;
const T_OSC_LEVEL: u16 = 1;
const T_OSC_SHAPE: u16 = 4;
const T_CUTOFF: u16 = 7;
const T_RESONANCE: u16 = 10;
const T_DRIVE: u16 = 13;
const T_LEVEL: u16 = 14;

/// Modulation targets, in id order. Each is shown against the param it moves; a slot never
/// changes the stored param.
pub static MOD_TARGETS: [ModTarget; 15] = [
    mod_target(0, "pitch", "Pitch", P_CV_PITCH),
    mod_target(1, "osc1_level", "Osc 1 level", OSC_BASE + F_LEVEL as u32),
    mod_target(
        2,
        "osc2_level",
        "Osc 2 level",
        OSC_BASE + (OSC_FIELDS + F_LEVEL) as u32,
    ),
    mod_target(
        3,
        "osc3_level",
        "Osc 3 level",
        OSC_BASE + (2 * OSC_FIELDS + F_LEVEL) as u32,
    ),
    mod_target(4, "osc1_shape", "Osc 1 shape", OSC_BASE + F_SHAPE as u32),
    mod_target(
        5,
        "osc2_shape",
        "Osc 2 shape",
        OSC_BASE + (OSC_FIELDS + F_SHAPE) as u32,
    ),
    mod_target(
        6,
        "osc3_shape",
        "Osc 3 shape",
        OSC_BASE + (2 * OSC_FIELDS + F_SHAPE) as u32,
    ),
    mod_target(
        7,
        "filter1_cutoff",
        "Filter 1 cutoff",
        FILTER_BASE + G_CUTOFF as u32,
    ),
    mod_target(
        8,
        "filter2_cutoff",
        "Filter 2 cutoff",
        FILTER_BASE + (FILTER_FIELDS + G_CUTOFF) as u32,
    ),
    mod_target(
        9,
        "filter3_cutoff",
        "Filter 3 cutoff",
        FILTER_BASE + (2 * FILTER_FIELDS + G_CUTOFF) as u32,
    ),
    mod_target(
        10,
        "filter1_resonance",
        "Filter 1 resonance",
        FILTER_BASE + G_RESONANCE as u32,
    ),
    mod_target(
        11,
        "filter2_resonance",
        "Filter 2 resonance",
        FILTER_BASE + (FILTER_FIELDS + G_RESONANCE) as u32,
    ),
    mod_target(
        12,
        "filter3_resonance",
        "Filter 3 resonance",
        FILTER_BASE + (2 * FILTER_FIELDS + G_RESONANCE) as u32,
    ),
    mod_target(13, "shaper_drive", "Shaper drive", P_SHAPER_DRIVE),
    mod_target(14, "level", "Level", P_LEVEL),
];

/// Semitones of pitch a full slot adds.
const MOD_PITCH_SEMITONES: f32 = 24.0;
/// Octaves of cutoff a full slot adds.
const MOD_CUTOFF_OCTAVES: f32 = 6.0;
/// Decibels of drive a full slot adds.
const MOD_DRIVE_DB: f32 = 24.0;

/// The matrix of the device.
pub static MOD_TABLES: ModTables = ModTables {
    sources: &MOD_SOURCES,
    targets: &MOD_TARGETS,
    first_param: P_MOD_FIRST,
};

const NOTES_IN: SocketId = SocketId(0);
const PITCH_CV_IN: SocketId = SocketId(1);
const CUTOFF_CV_IN: SocketId = SocketId(2);
const LEVEL_CV_IN: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const LFO_CV_OUT: SocketId = SocketId(6);
const SEQ_GATE_CV_OUT: SocketId = SocketId(7);
const SEQ_PITCH_CV_OUT: SocketId = SocketId(8);

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
        PITCH_CV_IN,
        "pitch_cv_in",
        "Pitch CV",
        SocketKind::Cv,
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
        LEVEL_CV_IN,
        "level_cv_in",
        "Level CV",
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
    socket(
        SEQ_GATE_CV_OUT,
        "seq_gate_cv_out",
        "Seq gate CV",
        SocketKind::Cv,
        Direction::Output,
    ),
    socket(
        SEQ_PITCH_CV_OUT,
        "seq_pitch_cv_out",
        "Seq pitch CV",
        SocketKind::Cv,
        Direction::Output,
    ),
];

/// Static description of the semi-modular synth.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.semi-modular",
    name: "Semi-Modular",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`SemiModular`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(SemiModular::new()),
    }
}

/// Samples between control-rate updates of a voice.
const CONTROL_INTERVAL: u32 = 16;
/// Per-voice output gain before the master stage.
const VOICE_GAIN: f32 = 0.35;
/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Highest oscillator and filter frequency as a fraction of the sample rate.
const MAX_FREQ_RATIO: f32 = 0.45;
/// The highest FM sideband the index limiter allows, as a fraction of the sample rate.
const FM_BANDWIDTH_RATIO: f32 = 0.4;
/// Phase deviation in radians of the `fm` type at `shape` = 1.
const FM_MAX_INDEX_RAD: f32 = 6.0;
/// Largest feedback of the comb filter slot (at resonance 1).
const COMB_FEEDBACK: f32 = 0.9;
/// The longest comb delay in ms (a 20 Hz cutoff).
const COMB_MAX_MS: f32 = 52.0;
/// How far the keys pan at full `master.spread`: middle C is centre, this many semitones away
/// is the edge.
const PAN_SPAN: f32 = 36.0;
/// Level of the noise oscillator type.
const NOISE_LEVEL: f32 = 0.5;
/// Most sequencer actions handled at one frame (a safety net, never reached in practice).
const MAX_FIRES: usize = 16;

/// How the voices are played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayMode {
    Poly,
    Mono,
    Legato,
}

/// The oscillator types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OscKind {
    Saw,
    Pulse,
    Triangle,
    Sine,
    Wave,
    Table,
    Fm,
    Noise,
}

impl OscKind {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Saw,
            1 => Self::Pulse,
            2 => Self::Triangle,
            3 => Self::Sine,
            4 => Self::Wave,
            5 => Self::Table,
            6 => Self::Fm,
            _ => Self::Noise,
        }
    }
}

/// The filter slot modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterKind {
    Off,
    Lp12,
    Bp12,
    Hp12,
    Notch,
    Lp24,
    Comb,
}

impl FilterKind {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Off,
            1 => Self::Lp12,
            2 => Self::Bp12,
            3 => Self::Hp12,
            4 => Self::Notch,
            5 => Self::Lp24,
            _ => Self::Comb,
        }
    }
}

/// How the filter slots are wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Routing {
    Series,
    Parallel,
    Split,
}

/// Plain values of one oscillator slot.
#[derive(Debug, Clone, Copy)]
struct OscValues {
    kind: OscKind,
    wave: usize,
    /// Octave, coarse and fine together, in semitones.
    semis: f32,
    level: f32,
    shape: f32,
}

/// Plain values of one filter slot.
#[derive(Debug, Clone, Copy)]
struct FilterValues {
    kind: FilterKind,
    cutoff: f32,
    resonance: f32,
    env: f32,
    track: f32,
    mix: f32,
}

/// Shared (per-device) synthesis parameters, as plain values.
#[derive(Debug, Clone, Copy)]
struct Shared {
    sample_rate: f32,
    osc: [OscValues; SLOTS],
    filt: [FilterValues; SLOTS],
    routing: Routing,
    shaper: Option<ShaperKind>,
    drive_db: f32,
    shaper_mix: f32,
    mode: PlayMode,
    /// Share of the way to the new pitch covered per control step (1 is no glide).
    glide_coef: f32,
    spread: f32,
    wheel_cutoff: f32,
    /// Pitch bend in semitones.
    bend: f32,
    tune_cents: f32,
    wheel: f32,
    cv_pitch_amount: f32,
    cv_cutoff_amount: f32,
    mods: ModBus,
    /// The pitch of the sounding step of the sequencer over 24 semitones.
    step_value: f32,
}

/// Control-rate values read by a voice.
#[derive(Debug, Clone, Copy)]
struct Ctl {
    lfo1: f32,
    lfo2: f32,
    pitch: f32,
    cutoff: f32,
    pressure: f32,
}

/// Per-sample modulation of one render segment (aligned with the output slices).
struct Mods<'a> {
    lfo1: &'a [f32],
    lfo2: &'a [f32],
    pitch: &'a [f32],
    cutoff: &'a [f32],
}

/// A value that ramps linearly to its target over one control step.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    v: f32,
    target: f32,
    step: f32,
}

impl Ramp {
    const fn new(v: f32) -> Self {
        Self {
            v,
            target: v,
            step: 0.0,
        }
    }

    /// Aims at `target`: from the previous target (no drift), or at once when `snap`.
    fn aim(&mut self, target: f32, snap: bool) {
        self.v = if snap { target } else { self.target };
        self.target = target;
        self.step = (target - self.v) / CONTROL_INTERVAL as f32;
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let v = self.v;
        self.v += self.step;
        v
    }

    /// Nothing to hear: at zero and aimed at zero.
    fn is_silent(&self) -> bool {
        self.v == 0.0 && self.target == 0.0
    }
}

/// One oscillator slot of a voice.
#[derive(Debug, Clone)]
struct OscState {
    a: PolyBlepOsc,
    /// The second saw of the `pulse` type.
    b: PolyBlepOsc,
    /// Phase of the FM modulator.
    mphase: f64,
    mstep: f64,
    mip: usize,
    level: Ramp,
    /// The shape; for `fm` the phase deviation in cycles.
    shape: Ramp,
}

impl OscState {
    fn new() -> Self {
        Self {
            a: PolyBlepOsc::new(Waveform::Saw),
            b: PolyBlepOsc::new(Waveform::Saw),
            mphase: 0.0,
            mstep: 0.0,
            mip: 0,
            level: Ramp::new(0.0),
            shape: Ramp::new(0.0),
        }
    }
}

/// One preallocated voice.
#[derive(Debug)]
struct Voice {
    key: u8,
    order: u64,
    pending: Option<(u8, f32)>,
    velocity: f32,
    /// The pitch in semitones (a MIDI key) now, and the one it glides to.
    pitch: f32,
    target: f32,
    gated: bool,
    fading: bool,
    osc: [OscState; SLOTS],
    svf: [Svf; SLOTS],
    ladder: [Ladder; SLOTS],
    comb: [Comb; SLOTS],
    comb_gain: [f32; SLOTS],
    shaper: Shaper,
    amp: Adsr,
    fenv: Adsr,
    menv: Adsr,
    rng: Rng,
    /// The random source: drawn at every note start.
    random: f32,
    gain: Ramp,
    pan: (f32, f32),
    countdown: u32,
    snap: bool,
}

impl Voice {
    fn new(sr: f32, seed: u64) -> Self {
        Self {
            key: 0,
            order: 0,
            pending: None,
            velocity: 0.0,
            pitch: 0.0,
            target: 0.0,
            gated: false,
            fading: false,
            osc: std::array::from_fn(|_| OscState::new()),
            svf: [Svf::new(); SLOTS],
            ladder: std::array::from_fn(|_| Ladder::new()),
            comb: std::array::from_fn(|_| Comb::new(sr, COMB_MAX_MS)),
            comb_gain: [1.0; SLOTS],
            shaper: Shaper::new(sr),
            amp: Adsr::new(sr),
            fenv: Adsr::new(sr),
            menv: Adsr::new(sr),
            rng: Rng::new(seed),
            random: 0.0,
            gain: Ramp::new(0.0),
            pan: (1.0, 1.0),
            countdown: 0,
            snap: true,
        }
    }

    /// Non-RT: a new sample rate and seed (the comb lines depend on the rate).
    fn prepare(&mut self, sr: f32, seed: u64) {
        self.comb = std::array::from_fn(|_| Comb::new(sr, COMB_MAX_MS));
        self.shaper = Shaper::new(sr);
        for e in [&mut self.amp, &mut self.fenv, &mut self.menv] {
            e.set_sample_rate(sr);
        }
        self.rng = Rng::new(seed);
    }

    fn is_active(&self) -> bool {
        self.amp.is_active()
    }

    /// The key this voice is playing (or will play after its steal fade), if any.
    fn sounding_key(&self) -> Option<u8> {
        if let Some((k, _)) = self.pending {
            Some(k)
        } else if self.is_active() && !self.fading {
            Some(self.key)
        } else {
            None
        }
    }

    fn clear_filters(&mut self) {
        for i in 0..SLOTS {
            self.svf[i].reset();
            self.ladder[i].reset();
            self.comb[i].reset();
        }
        self.shaper.reset();
    }

    fn gate_on(&mut self) {
        self.amp.gate_on();
        self.fenv.gate_on();
        self.menv.gate_on();
        self.gated = true;
        self.fading = false;
    }

    /// Starts a note from silence (fresh phases, filters and envelopes). `from` is the pitch to
    /// glide from, when there is one.
    fn start(&mut self, key: u8, velocity: f32, from: Option<f32>) {
        self.key = key;
        self.pending = None;
        self.velocity = velocity.clamp(0.0, 1.0);
        self.target = f32::from(key);
        self.pitch = from.unwrap_or(self.target);
        for o in &mut self.osc {
            o.a.set_phase(0.0);
            o.b.set_phase(0.0);
            o.mphase = 0.0;
        }
        self.clear_filters();
        for e in [&mut self.amp, &mut self.fenv, &mut self.menv] {
            e.reset();
        }
        self.random = self.rng.next_bipolar();
        self.gate_on();
        self.countdown = 0;
        self.snap = true;
    }

    /// Same-key retrigger: the envelopes restart from their current levels.
    fn retrigger(&mut self, key: u8, velocity: f32) {
        self.key = key;
        self.velocity = velocity.clamp(0.0, 1.0);
        self.target = f32::from(key);
        self.random = self.rng.next_bipolar();
        self.gate_on();
    }

    fn release(&mut self) {
        self.pending = None;
        self.gated = false;
        self.amp.gate_off();
        self.fenv.gate_off();
        self.menv.gate_off();
    }

    /// The steal fade: the envelopes fall to nothing over 2 ms.
    fn steal(&mut self) {
        self.gated = false;
        self.fading = true;
        self.amp.steal_fade();
        self.fenv.steal_fade();
        self.menv.steal_fade();
    }

    /// Control-rate update: pitches, oscillator and filter coefficients, shaper and gain.
    fn control(&mut self, c: Ctl, p: &Shared) {
        let snap = self.snap;
        let sr = p.sample_rate;
        if p.glide_coef >= 1.0 {
            self.pitch = self.target;
        } else {
            self.pitch += (self.target - self.pitch) * p.glide_coef;
        }
        let key = self.pitch;
        let mut src = [0.0f32; SOURCE_COUNT];
        let live = p.mods.is_active();
        if live {
            src[SRC_AMP_ENV] = self.amp.level();
            src[SRC_FILTER_ENV] = self.fenv.level();
            src[SRC_MOD_ENV] = self.menv.level();
            src[SRC_LFO1] = c.lfo1;
            src[SRC_LFO2] = c.lfo2;
            src[SRC_VELOCITY] = self.velocity;
            src[SRC_KEY] = (key - 60.0) / 60.0;
            src[SRC_WHEEL] = p.wheel;
            src[SRC_PRESSURE] = c.pressure;
            src[SRC_STEP] = p.step_value;
            src[SRC_RANDOM] = self.random;
        }
        let off = |target: u16| {
            if live {
                p.mods.offset(target, &src)
            } else {
                0.0
            }
        };
        let max_hz = MAX_FREQ_RATIO * sr;
        let semis =
            key + p.bend + p.cv_pitch_amount * clean(c.pitch) + off(T_PITCH) * MOD_PITCH_SEMITONES;
        for (i, (o, st)) in p.osc.iter().zip(&mut self.osc).enumerate() {
            let hz = note_to_hz(semis + o.semis, p.tune_cents).clamp(1.0, max_hz);
            let wave = match o.kind {
                OscKind::Triangle => Waveform::Triangle,
                _ => Waveform::Saw,
            };
            st.a.set_waveform(wave);
            st.a.set_frequency(hz, sr);
            st.b.set_frequency(hz, sr);
            st.mip = level_for(hz, sr);
            let level = (o.level + off(T_OSC_LEVEL + i as u16)).clamp(0.0, 1.0);
            let shape = (o.shape + off(T_OSC_SHAPE + i as u16)).clamp(0.0, 1.0);
            st.level.aim(level, snap);
            let shape = if o.kind == OscKind::Fm {
                // The index in radians, then what the bandwidth allows of it: the highest
                // sideband is about `hz * (1 + ratio * (index + 1))`.
                let ratio = fm_ratio(o.wave);
                st.mstep = f64::from(hz * ratio / sr);
                let room = (FM_BANDWIDTH_RATIO * sr / hz - 1.0) / ratio - 1.0;
                let index = (shape * FM_MAX_INDEX_RAD).min(room.max(0.0));
                index * (1.0 / std::f32::consts::TAU)
            } else {
                shape
            };
            st.shape.aim(shape, snap);
        }
        let common = p.wheel * p.wheel_cutoff + p.cv_cutoff_amount * clean(c.cutoff);
        for (i, f) in p.filt.iter().enumerate() {
            if f.kind == FilterKind::Off {
                continue;
            }
            let octaves = f.env * self.fenv.level()
                + (key - 60.0) * (1.0 / 12.0) * f.track
                + common
                + off(T_CUTOFF + i as u16) * MOD_CUTOFF_OCTAVES;
            let cutoff = (f.cutoff * exp2_det(octaves.clamp(-12.0, 12.0))).clamp(20.0, max_hz);
            let res = (f.resonance + off(T_RESONANCE + i as u16)).clamp(0.0, 1.0);
            match f.kind {
                FilterKind::Off => {}
                FilterKind::Lp24 => self.ladder[i]
                    .set_coefficients(Ladder::cutoff_to_g(cutoff, sr), Ladder::resonance_to_k(res)),
                FilterKind::Comb => {
                    self.comb[i].set_delay_samples(sr / cutoff);
                    let fb = res * COMB_FEEDBACK;
                    self.comb[i].set_feedback(fb);
                    // Unity gain at the peaks.
                    self.comb_gain[i] = 1.0 - fb;
                }
                _ => self.svf[i].set_coefficients(
                    Svf::cutoff_to_g_det(cutoff, sr),
                    Svf::resonance_to_k_det(res),
                ),
            }
        }
        if let Some(kind) = p.shaper {
            self.shaper.set_kind(kind);
            self.shaper
                .set_drive_db(p.drive_db + off(T_DRIVE) * MOD_DRIVE_DB);
        }
        let level = (1.0 + off(T_LEVEL)).clamp(0.0, 2.0);
        self.gain.aim((0.4 + 0.6 * self.velocity) * level, snap);
        self.pan = balance_gains(p.spread * ((self.target - 60.0) / PAN_SPAN).clamp(-1.0, 1.0));
        self.snap = false;
    }

    /// One sample of an oscillator slot.
    #[inline]
    fn oscillator(
        st: &mut OscState,
        o: &OscValues,
        rng: &mut Rng,
        waves: &atomik_dsp::wavetable::WaveBank,
        tables: &atomik_dsp::wavetable::WavetableBank,
    ) -> f32 {
        let shape = st.shape.next();
        match o.kind {
            OscKind::Saw | OscKind::Triangle => st.a.next_sample(),
            OscKind::Pulse => {
                // The difference of two saws a pulse width apart: a pulse without DC.
                let width = 0.5 - 0.45 * shape;
                let p0 = st.a.phase();
                st.b.set_phase(p0 + 1.0 - f64::from(width));
                0.5 * (st.a.next_sample() - st.b.next_sample())
            }
            OscKind::Sine => {
                let ph = st.a.phase();
                st.a.advance();
                sin_cycles(ph) as f32
            }
            OscKind::Wave => {
                let ph = st.a.phase();
                st.a.advance();
                waves.read(o.wave, st.mip, ph)
            }
            OscKind::Table => {
                let ph = st.a.phase();
                st.a.advance();
                tables
                    .table(o.wave.min(TABLE_COUNT - 1))
                    .read(st.mip, shape, ph)
            }
            OscKind::Fm => {
                let ph = st.a.phase();
                st.a.advance();
                let m = sin_cycles(st.mphase) as f32;
                let next = st.mphase + st.mstep;
                st.mphase = next - next.floor();
                sin_cycles(ph + f64::from(shape * m)) as f32
            }
            OscKind::Noise => rng.next_bipolar() * NOISE_LEVEL,
        }
    }

    /// One filter slot: the filter, blended with its input by `mix`.
    #[inline]
    fn slot(&mut self, i: usize, f: &FilterValues, x: f32) -> f32 {
        let y = match f.kind {
            FilterKind::Off => return x,
            FilterKind::Lp12 => self.svf[i].process_mode(x, FilterMode::Lowpass),
            FilterKind::Bp12 => self.svf[i].process_mode(x, FilterMode::Bandpass),
            FilterKind::Hp12 => self.svf[i].process_mode(x, FilterMode::Highpass),
            FilterKind::Notch => {
                let o = self.svf[i].process(x);
                o.lp + o.hp
            }
            FilterKind::Lp24 => self.ladder[i].process(x),
            FilterKind::Comb => self.comb[i].process(x) * self.comb_gain[i],
        };
        x + (y - x) * f.mix
    }

    /// The average of the slots of `from..SLOTS` that are on, or `x` when none is.
    #[inline]
    fn parallel(&mut self, p: &Shared, from: usize, x: f32) -> f32 {
        let (mut sum, mut n) = (0.0f32, 0u32);
        for i in from..SLOTS {
            if p.filt[i].kind != FilterKind::Off {
                sum += self.slot(i, &p.filt[i], x);
                n += 1;
            }
        }
        if n == 0 { x } else { sum / n as f32 }
    }

    #[inline]
    fn filters(&mut self, p: &Shared, x: f32) -> f32 {
        match p.routing {
            Routing::Series => {
                let mut y = x;
                for i in 0..SLOTS {
                    y = self.slot(i, &p.filt[i], y);
                }
                y
            }
            Routing::Parallel => self.parallel(p, 0, x),
            Routing::Split => {
                let y = self.slot(0, &p.filt[0], x);
                self.parallel(p, 1, y)
            }
        }
    }

    /// Adds this voice into `left` and `right`. The modulation slices are aligned with them.
    fn render(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        m: &Mods<'_>,
        p: &Shared,
        ctl: &ControllerState,
    ) {
        let waves = wave_bank();
        let tables = factory_bank();
        for (n, (lo, ro)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            if self.countdown == 0 {
                let c = Ctl {
                    lfo1: m.lfo1[n],
                    lfo2: m.lfo2[n],
                    pitch: m.pitch[n],
                    cutoff: m.cutoff[n],
                    pressure: ctl.pressure(self.key),
                };
                self.control(c, p);
                self.countdown = CONTROL_INTERVAL;
            }
            self.countdown -= 1;
            let env = self.amp.next_sample();
            self.fenv.next_sample();
            self.menv.next_sample();
            let mut x = 0.0f32;
            for i in 0..SLOTS {
                if self.osc[i].level.is_silent() {
                    continue;
                }
                let level = self.osc[i].level.next();
                let o = &p.osc[i];
                x += level * Self::oscillator(&mut self.osc[i], o, &mut self.rng, waves, tables);
            }
            let mut y = self.filters(p, x);
            if p.shaper.is_some() {
                let w = self.shaper.process(y);
                y += (w - y) * p.shaper_mix;
            }
            let out = y * env * self.gain.next() * VOICE_GAIN;
            *lo += out * self.pan.0;
            *ro += out * self.pan.1;
            if !self.is_active() {
                match self.pending.take() {
                    Some((key, velocity)) => self.start(key, velocity, None),
                    None => return,
                }
            }
        }
    }
}

/// The FM modulator ratio of a `wave` value: `0.5 + wave / 4`.
fn fm_ratio(wave: usize) -> f32 {
    0.5 + wave as f32 * 0.25
}

/// A held key, for the mono modes.
#[derive(Debug, Clone, Copy)]
struct Held {
    key: u8,
    velocity: f32,
}

/// The first frame at or after `x` frames into the batch, with a tolerance for the rounding of
/// the accumulated step lengths; [`BATCH`] when it is not in this batch.
fn frame_of(x: f64) -> usize {
    let f = (x - 1e-6).ceil();
    if f <= 0.0 {
        0
    } else if f >= BATCH as f64 {
        BATCH
    } else {
        f as usize
    }
}

/// The step sequencer. Frame counts are relative to the first frame of the batch being
/// processed and drop by [`BATCH`] when it ends.
#[derive(Debug, Clone)]
struct Sequencer {
    run: bool,
    ticks: u32,
    swing: u8,
    length: usize,
    gate_pct: f64,
    on: [bool; STEPS],
    pitch: [f32; STEPS],
    vel: [f32; STEPS],
    /// The keys held, the last pressed on top.
    keys: [u8; MAX_VOICES * 2],
    nkeys: usize,
    running: bool,
    /// The next step to play.
    step: i64,
    /// Frames until the next step starts, and until the sounding note ends (negative: none).
    step_left: f64,
    gate_left: f64,
    /// The key sounding.
    sounding: Option<u8>,
    /// The key of the last step played, for the pitch CV.
    last_key: u8,
}

impl Sequencer {
    fn new() -> Self {
        Self {
            run: false,
            ticks: RATE_TICKS[3],
            swing: 0,
            length: STEPS,
            gate_pct: 60.0,
            on: [true; STEPS],
            pitch: STEP_PITCHES,
            vel: STEP_VELS,
            keys: [0; MAX_VOICES * 2],
            nkeys: 0,
            running: false,
            step: 0,
            step_left: 0.0,
            gate_left: -1.0,
            sounding: None,
            last_key: 60,
        }
    }

    fn root(&self) -> Option<u8> {
        self.keys[..self.nkeys].last().copied()
    }

    fn press(&mut self, key: u8) {
        self.release_key(key);
        if self.nkeys == self.keys.len() {
            self.keys.copy_within(1.., 0);
            self.nkeys -= 1;
        }
        self.keys[self.nkeys] = key;
        self.nkeys += 1;
    }

    fn release_key(&mut self, key: u8) {
        if let Some(i) = self.keys[..self.nkeys].iter().position(|k| *k == key) {
            self.keys.copy_within(i + 1..self.nkeys, i);
            self.nkeys -= 1;
        }
    }

    fn grid(&self) -> Grid {
        Grid {
            s: self.ticks,
            swing: self.swing,
        }
    }

    /// The frame of the next action inside the batch, or [`BATCH`].
    fn next_frame(&self) -> usize {
        if !self.running {
            return BATCH;
        }
        let mut f = frame_of(self.step_left);
        if self.gate_left >= 0.0 {
            f = f.min(frame_of(self.gate_left));
        }
        f
    }

    /// A batch ends.
    fn end_batch(&mut self) {
        if self.running {
            self.step_left -= BATCH as f64;
            if self.gate_left >= 0.0 {
                self.gate_left -= BATCH as f64;
            }
        }
    }
}

/// The semi-modular device.
#[derive(Debug)]
pub struct SemiModular {
    values: [f32; PARAM_COUNT],
    shared: Shared,
    voices: [Voice; MAX_VOICES],
    voice_limit: usize,
    next_order: u64,
    /// The keys held in the mono modes, oldest first (at most one per key).
    held: [Held; MAX_VOICES * 2],
    held_len: usize,
    /// The pitch the mono voice last sounded at, to glide from.
    last_pitch: Option<f32>,
    lfo1: Lfo,
    lfo2: Lfo,
    master: Smoother,
    cv_level_db: f32,
    ctl: ControllerState,
    seq: Sequencer,
    /// Set by `prepare`/`reset`: the next `process` snaps the master gain to its target.
    fresh: bool,
}

impl Default for SemiModular {
    fn default() -> Self {
        Self::new()
    }
}

impl SemiModular {
    /// A synth with default params (call `prepare` before processing).
    pub fn new() -> Self {
        let osc = OscValues {
            kind: OscKind::Saw,
            wave: 0,
            semis: 0.0,
            level: 0.0,
            shape: 0.0,
        };
        let filt = FilterValues {
            kind: FilterKind::Off,
            cutoff: 1_000.0,
            resonance: 0.0,
            env: 0.0,
            track: 0.0,
            mix: 1.0,
        };
        let mut s = Self {
            values: PARAMS.map(|p| p.default),
            shared: Shared {
                sample_rate: 48_000.0,
                osc: [osc; SLOTS],
                filt: [filt; SLOTS],
                routing: Routing::Series,
                shaper: None,
                drive_db: 0.0,
                shaper_mix: 1.0,
                mode: PlayMode::Poly,
                glide_coef: 1.0,
                spread: 0.0,
                wheel_cutoff: 0.0,
                bend: 0.0,
                tune_cents: 0.0,
                wheel: 0.0,
                cv_pitch_amount: 24.0,
                cv_cutoff_amount: 0.0,
                mods: ModBus::new(P_MOD_FIRST),
                step_value: 0.0,
            },
            voices: std::array::from_fn(|n| Voice::new(48_000.0, derive_seed(0, n as u64 + 1))),
            voice_limit: MAX_VOICES,
            next_order: 0,
            held: [Held {
                key: 0,
                velocity: 0.0,
            }; MAX_VOICES * 2],
            held_len: 0,
            last_pitch: None,
            lfo1: Lfo::new(derive_seed(0, 0)),
            lfo2: Lfo::new(derive_seed(0, 100)),
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            cv_level_db: 24.0,
            ctl: ControllerState::new(),
            seq: Sequencer::new(),
            fresh: true,
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
        if self.shared.mods.set_param(id, v) {
            return;
        }
        if (OSC_BASE..FILTER_BASE).contains(&id) {
            let (slot, field) = (
                (id - OSC_BASE) as usize / OSC_FIELDS,
                (id - OSC_BASE) as usize % OSC_FIELDS,
            );
            let base = OSC_BASE as usize + slot * OSC_FIELDS;
            let get = |f: usize| self.values[base + f];
            // Octave, coarse and fine make one tuning (the value is already stored).
            let semis = get(F_OCTAVE).round() * 12.0 + get(F_COARSE).round() + get(F_FINE) * 0.01;
            let o = &mut self.shared.osc[slot];
            match field {
                F_TYPE => o.kind = OscKind::from_index(choice(v)),
                F_WAVE => o.wave = choice(v).min(WAVE_COUNT - 1),
                F_LEVEL => o.level = v,
                F_SHAPE => o.shape = v,
                _ => o.semis = semis,
            }
            return;
        }
        if (FILTER_BASE..P_ROUTING).contains(&id) {
            let (slot, field) = (
                (id - FILTER_BASE) as usize / FILTER_FIELDS,
                (id - FILTER_BASE) as usize % FILTER_FIELDS,
            );
            let f = &mut self.shared.filt[slot];
            match field {
                G_MODE => {
                    let kind = FilterKind::from_index(choice(v));
                    // A new mode starts from a clean state.
                    if kind != f.kind {
                        f.kind = kind;
                        for vc in &mut self.voices {
                            vc.svf[slot].reset();
                            vc.ladder[slot].reset();
                            vc.comb[slot].reset();
                        }
                    }
                }
                G_CUTOFF => f.cutoff = v,
                G_RESONANCE => f.resonance = v,
                G_ENV => f.env = v,
                G_TRACK => f.track = v,
                _ => f.mix = v,
            }
            return;
        }
        if (ENV_BASE..P_LFO1_WAVE).contains(&id) {
            let (env, field) = ((id - ENV_BASE) as usize / 4, (id - ENV_BASE) as usize % 4);
            for vc in &mut self.voices {
                let e = match env {
                    0 => &mut vc.amp,
                    1 => &mut vc.fenv,
                    _ => &mut vc.menv,
                };
                match field {
                    0 => e.set_attack_ms(v),
                    1 => e.set_decay_ms(v),
                    2 => e.set_sustain(v),
                    _ => e.set_release_ms(v),
                }
            }
            return;
        }
        if (P_STEP_ON..P_CV_PITCH).contains(&id) {
            let n = (id - P_STEP_ON) as usize % STEPS;
            match (id - P_STEP_ON) as usize / STEPS {
                0 => self.seq.on[n] = v >= 0.5,
                1 => self.seq.pitch[n] = v.round(),
                _ => self.seq.vel[n] = v,
            }
            return;
        }
        match id {
            P_ROUTING => {
                self.shared.routing = match choice(v) {
                    0 => Routing::Series,
                    1 => Routing::Parallel,
                    _ => Routing::Split,
                }
            }
            P_SHAPER_TYPE => {
                self.shared.shaper = match choice(v) {
                    1 => Some(ShaperKind::SoftClip),
                    2 => Some(ShaperKind::Asymmetric),
                    3 => Some(ShaperKind::Hard),
                    4 => Some(ShaperKind::Fold),
                    _ => None,
                };
                if self.shared.shaper.is_none() {
                    for vc in &mut self.voices {
                        vc.shaper.reset();
                    }
                } else {
                    for vc in &mut self.voices {
                        vc.shaper.set_adaa(true);
                    }
                }
            }
            P_SHAPER_DRIVE => self.shared.drive_db = v,
            P_SHAPER_MIX => self.shared.shaper_mix = v,
            P_LFO1_WAVE => self.lfo1.set_shape(LfoShape::from_index(choice(v))),
            P_LFO1_RATE => self.lfo1.set_rate(v, sr),
            P_LFO2_WAVE => self.lfo2.set_shape(LfoShape::from_index(choice(v))),
            P_LFO2_RATE => self.lfo2.set_rate(v, sr),
            P_MODE => {
                let mode = match choice(v) {
                    0 => PlayMode::Poly,
                    1 => PlayMode::Mono,
                    _ => PlayMode::Legato,
                };
                // Only a real change resets the held stack: the same value again (automation
                // sends it every batch) must not drop the keys still held.
                if mode != self.shared.mode {
                    self.shared.mode = mode;
                    self.held_len = 0;
                    if mode != PlayMode::Poly {
                        for vc in &mut self.voices[1..] {
                            vc.release();
                        }
                    }
                }
            }
            P_GLIDE => {
                self.shared.glide_coef = if v <= 0.0 {
                    1.0
                } else {
                    let tau = (v * 0.001 * sr).max(1.0);
                    1.0 - (-(CONTROL_INTERVAL as f32) / tau).exp()
                };
            }
            P_SPREAD => self.shared.spread = v,
            P_WHEEL_CUTOFF => self.shared.wheel_cutoff = v,
            P_BEND_RANGE => {
                self.ctl.set_bend_range(v);
                self.shared.bend = self.ctl.bend_semitones();
            }
            P_VOICES => {
                self.voice_limit = (v.round().max(1.0) as usize).min(MAX_VOICES);
                for vc in &mut self.voices[self.voice_limit..] {
                    vc.pending = None;
                    vc.steal();
                }
            }
            P_LEVEL => self
                .master
                .set_target(PARAMS[P_LEVEL as usize].db_param_gain(v)),
            P_SEQ_RUN => {
                let run = v >= 0.5;
                if run != self.seq.run {
                    self.seq.run = run;
                    // Keys held while the sequencer was off sound no more, and the notes it
                    // played end with it.
                    self.seq_stop();
                    self.seq.nkeys = 0;
                }
            }
            P_SEQ_RATE => self.seq.ticks = RATE_TICKS[choice(v).min(RATE_TICKS.len() - 1)],
            P_SEQ_LENGTH => self.seq.length = (choice(v)).clamp(1, STEPS),
            P_SEQ_SWING => self.seq.swing = v.round().clamp(0.0, 75.0) as u8,
            P_SEQ_GATE => self.seq.gate_pct = f64::from(v.clamp(5.0, 100.0)),
            P_CV_PITCH => self.shared.cv_pitch_amount = v * 24.0,
            P_CV_CUTOFF => self.shared.cv_cutoff_amount = v,
            P_CV_LEVEL => self.cv_level_db = v * 24.0,
            _ => {}
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32) {
        self.ctl.note_on(key);
        let p = self.shared;
        if p.mode != PlayMode::Poly {
            return self.mono_note_on(key, velocity);
        }
        let order = self.next_order;
        self.next_order += 1;
        let voices = &mut self.voices[..self.voice_limit];
        // 1. Same key sounding (or pending after a steal): retrigger that voice.
        if let Some(v) = voices.iter_mut().find(|v| v.sounding_key() == Some(key)) {
            v.order = order;
            if v.pending.is_some() {
                v.pending = Some((key, velocity));
            } else {
                v.retrigger(key, velocity);
            }
            return;
        }
        // 2. A free voice.
        if let Some(v) = voices.iter_mut().find(|v| !v.is_active()) {
            v.order = order;
            v.start(key, velocity, None);
            return;
        }
        // 3. Steal the oldest started voice: 2 ms fade, then the new note.
        if let Some(v) = voices.iter_mut().min_by_key(|v| v.order) {
            v.order = order;
            v.pending = Some((key, velocity));
            v.steal();
        }
    }

    /// A key in the mono modes: the one voice plays it (the last key wins).
    fn mono_note_on(&mut self, key: u8, velocity: f32) {
        let mode = self.shared.mode;
        self.hold(key, velocity);
        let from = self.last_pitch;
        let v = &mut self.voices[0];
        if v.gated && mode == PlayMode::Legato {
            // A legato key under a held one: the pitch glides, the envelopes run on.
            v.key = key;
            v.target = f32::from(key);
            v.velocity = velocity.clamp(0.0, 1.0);
        } else if v.is_active() && !v.fading {
            v.retrigger(key, velocity);
        } else {
            v.start(key, velocity, from);
        }
        self.last_pitch = Some(f32::from(key));
    }

    /// Puts `key` on top of the held keys of the mono modes.
    fn hold(&mut self, key: u8, velocity: f32) {
        self.unhold(key);
        if self.held_len < self.held.len() {
            self.held[self.held_len] = Held { key, velocity };
            self.held_len += 1;
        } else {
            let last = self.held.len() - 1;
            self.held.copy_within(1.., 0);
            self.held[last] = Held { key, velocity };
        }
    }

    /// Takes `key` off the held keys.
    fn unhold(&mut self, key: u8) {
        if let Some(i) = self.held[..self.held_len].iter().position(|h| h.key == key) {
            self.held.copy_within(i + 1..self.held_len, i);
            self.held_len -= 1;
        }
    }

    /// A key released in the mono modes: back to the key still held, or release.
    fn mono_note_off(&mut self, key: u8) {
        let mode = self.shared.mode;
        self.unhold(key);
        let v = &mut self.voices[0];
        if v.key != key || !v.gated {
            return;
        }
        if self.held_len == 0 {
            v.release();
            return;
        }
        let back = self.held[self.held_len - 1];
        if mode == PlayMode::Legato {
            v.key = back.key;
            v.target = f32::from(back.key);
        } else {
            v.retrigger(back.key, back.velocity);
        }
        self.last_pitch = Some(f32::from(back.key));
    }

    /// The key is let go (or its held-back release comes due): its voice releases, or in the
    /// mono modes the voice returns to the key still held.
    fn release_key(&mut self, key: u8) {
        if self.shared.mode == PlayMode::Poly {
            self.voices
                .iter_mut()
                .filter(|v| v.sounding_key() == Some(key))
                .for_each(Voice::release);
        } else {
            self.mono_note_off(key);
        }
    }

    /// The sequencer stops: its note ends and the clock rests.
    fn seq_stop(&mut self) {
        if let Some(key) = self.seq.sounding.take() {
            self.release_key(key);
        }
        self.seq.running = false;
        self.seq.gate_left = -1.0;
    }

    /// Plays the actions of the sequencer that are due at or before frame `at`.
    fn seq_fire(&mut self, at: usize, tpf: f64) {
        for _ in 0..MAX_FIRES {
            if !self.seq.running {
                return;
            }
            let step_due = frame_of(self.seq.step_left) <= at;
            let gate_due = self.seq.gate_left >= 0.0 && frame_of(self.seq.gate_left) <= at;
            if gate_due && (!step_due || self.seq.gate_left <= self.seq.step_left) {
                if let Some(key) = self.seq.sounding.take() {
                    self.release_key(key);
                }
                self.seq.gate_left = -1.0;
            } else if step_due {
                self.seq_step(tpf);
            } else {
                return;
            }
        }
    }

    /// Plays the step that is due.
    fn seq_step(&mut self, tpf: f64) {
        let k = self.seq.step;
        let grid = self.seq.grid();
        let frames = ((grid.start(k + 1) - grid.start(k)) / tpf).max(1.0);
        let start = self.seq.step_left;
        self.seq.step_left += frames;
        self.seq.step += 1;
        let n = (k % self.seq.length as i64) as usize;
        let old = self.seq.sounding.take();
        let pitch = self.seq.pitch[n];
        self.shared.step_value = (pitch / 24.0).clamp(-1.0, 1.0);
        self.seq.gate_left = -1.0;
        let root = self.seq.root().unwrap_or(60);
        if self.seq.on[n] {
            let key = (i32::from(root) + pitch as i32).clamp(0, 127) as u8;
            self.note_on(key, self.seq.vel[n]);
            self.seq.sounding = Some(key);
            self.seq.last_key = key;
            if self.seq.gate_pct < 100.0 {
                self.seq.gate_left = start + frames * self.seq.gate_pct / 100.0;
            }
        }
        // A tied note (or a rest) ends with the step; the same key stays for the retrigger.
        if let Some(o) = old
            && self.seq.sounding != Some(o)
        {
            self.release_key(o);
        }
    }

    fn handle(&mut self, kind: NoteKind, frame: usize) {
        match kind {
            NoteKind::On { key, .. } if self.seq.run => {
                self.seq.press(key);
                if !self.seq.running {
                    self.seq.running = true;
                    self.seq.step = 0;
                    self.seq.step_left = frame as f64;
                    self.seq.gate_left = -1.0;
                }
            }
            NoteKind::Off { key } if self.seq.run => {
                self.seq.release_key(key);
                if self.seq.nkeys == 0 {
                    self.seq_stop();
                }
            }
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
            NoteKind::Off { key } => {
                if self.ctl.note_off(key) {
                    self.release_key(key);
                }
            }
            NoteKind::AllOff => {
                self.ctl.all_off();
                self.held_len = 0;
                self.seq.nkeys = 0;
                self.seq_stop();
                self.voices.iter_mut().for_each(Voice::release);
            }
            NoteKind::Cc {
                controller: 1,
                value,
            } => self.shared.wheel = value.clamp(0.0, 1.0),
            // Pedal, volume, expression, pressure, bend and its range.
            _ => {
                if self.ctl.handle(kind) == Effect::ReleaseHeld {
                    let mut keys = [0u8; 128];
                    let mut n = 0;
                    self.ctl.release_held(|key| {
                        keys[n] = key;
                        n += 1;
                    });
                    for &key in &keys[..n] {
                        self.release_key(key);
                    }
                }
                self.shared.bend = self.ctl.bend_semitones();
            }
        }
    }

    /// The pitch CV of a key, `(key - 60) / 24` clamped to +-1.
    fn pitch_cv(&self) -> f32 {
        ((f32::from(self.seq.last_key) - 60.0) / 24.0).clamp(-1.0, 1.0)
    }
}

impl Device for SemiModular {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.shared.sample_rate = sr;
        for (n, v) in self.voices.iter_mut().enumerate() {
            v.prepare(sr, derive_seed(info.seed, n as u64 + 1));
        }
        // The wave banks are built on first use; do it here, off the audio thread.
        let _ = (wave_bank(), factory_bank());
        self.master.set_time(MASTER_SMOOTH_MS, sr);
        self.ctl.set_sample_rate(sr);
        self.lfo1 = Lfo::new(derive_seed(info.seed, 0));
        self.lfo2 = Lfo::new(derive_seed(info.seed, 100));
        self.apply_all();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            for e in [&mut v.amp, &mut v.fenv, &mut v.menv] {
                e.reset();
            }
            v.clear_filters();
            v.pending = None;
            v.gated = false;
            v.fading = false;
            v.gain = Ramp::new(0.0);
            for o in &mut v.osc {
                o.level = Ramp::new(0.0);
                o.shape = Ramp::new(0.0);
            }
        }
        self.next_order = 0;
        self.held_len = 0;
        self.last_pitch = None;
        self.ctl.reset();
        self.shared.bend = 0.0;
        self.shared.wheel = 0.0;
        self.shared.step_value = 0.0;
        self.seq.running = false;
        self.seq.nkeys = 0;
        self.seq.sounding = None;
        self.seq.gate_left = -1.0;
        self.seq.step = 0;
        self.seq.step_left = 0.0;
        self.lfo1.reset();
        self.lfo2.reset();
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
        self.shared.tune_cents = ctx.time.tune_cents;
        let tpf = ctx.time.ticks_per_frame;
        let tpf = if tpf.is_finite() && tpf > 0.0 {
            tpf
        } else {
            1.0
        };
        let (mut lfo1, mut lfo2) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        for n in 0..BATCH {
            lfo1[n] = self.lfo1.next_sample();
            lfo2[n] = self.lfo2.next_sample();
        }
        let cv_pitch = *ctx.input(PITCH_CV_IN);
        let cv_cutoff = *ctx.input(CUTOFF_CV_IN);
        let (mut left, mut right) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        let (mut gate_cv, mut pitch_cv) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        let mut ctl_gain = [1.0f32; BATCH];
        let events = ctx.notes_in(NOTES_IN);
        let mut next = 0;
        let mut start = 0;
        // Render in segments between note events and sequencer steps, so both are
        // sample-accurate and the output does not depend on how the batches fall.
        while start < BATCH {
            while next < events.len() && (events[next].offset as usize).min(BATCH - 1) <= start {
                self.handle(events[next].kind, start);
                next += 1;
            }
            self.seq_fire(start, tpf);
            let mut end = events
                .get(next)
                .map_or(BATCH, |e| (e.offset as usize).clamp(start + 1, BATCH));
            end = end.min(self.seq.next_frame().clamp(start + 1, BATCH));
            let p = self.shared;
            let m = Mods {
                lfo1: &lfo1[start..end],
                lfo2: &lfo2[start..end],
                pitch: &cv_pitch[start..end],
                cutoff: &cv_cutoff[start..end],
            };
            for v in self.voices.iter_mut().filter(|v| v.is_active()) {
                v.render(
                    &mut left[start..end],
                    &mut right[start..end],
                    &m,
                    &p,
                    &self.ctl,
                );
            }
            gate_cv[start..end].fill(if self.seq.sounding.is_some() {
                1.0
            } else {
                0.0
            });
            let pv = self.pitch_cv();
            pitch_cv[start..end].fill(pv);
            self.ctl.fill_gain(&mut ctl_gain[start..end]);
            start = end;
        }
        self.seq.end_batch();
        let level_on = ctx.is_connected(LEVEL_CV_IN);
        let cv_level = *ctx.input(LEVEL_CV_IN);
        for (n, &lv) in cv_level.iter().enumerate() {
            let mut g = self.master.next_value();
            if level_on {
                g *= db_gain(self.cv_level_db, lv);
            }
            left[n] = soft_limit(left[n] * g) * ctl_gain[n];
            right[n] = soft_limit(right[n] * g) * ctl_gain[n];
        }
        *ctx.output(OUT_L) = left;
        *ctx.output(OUT_R) = right;
        *ctx.output(LFO_CV_OUT) = lfo1;
        *ctx.output(SEQ_GATE_CV_OUT) = gate_cv;
        *ctx.output(SEQ_PITCH_CV_OUT) = pitch_cv;
    }
}

#[cfg(test)]
mod tests;
