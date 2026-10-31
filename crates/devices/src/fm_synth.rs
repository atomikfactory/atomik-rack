//! `atomik.fm-synth` ("FM Synth"): a four-operator FM synthesizer. An original design from the
//! textbook of phase-modulation synthesis (Chowning, 1973); the blocks that are not the voice
//! (the routings, the frequency rule and the bandwidth limiter) are in [`atomik_dsp::fm`].
//!
//! # Signal flow
//!
//! ```text
//! per voice:   op4 -> op3 -> op2 -> op1      (the routing, see ALGORITHMS: operator n is only
//!              each operator = sine . envelope . level   modulated by operators above n)
//!              carriers summed . 1/sqrt(carriers) . tremolo . key pan
//! all voices:  sum . master level (5 ms smoothing) . soft limiter -> out L/R
//! ```
//!
//! * **Operators.** Four sine operators, `op1.*` to `op4.*`. Operator `n` is a sine whose phase
//!   is moved by the outputs of the operators the algorithm routes into it. An operator that
//!   modulates another is a *modulator*, one that reaches the output a *carrier*; its `level` is
//!   the volume of a carrier and the modulation index (full level is [`fm::MAX_INDEX_RAD`] = 8
//!   radians at an index of 1) of a modulator. The phase accumulators are `f64` and wrap, so a
//!   note stays in tune for hours.
//! * **Frequency.** In ratio mode an operator runs at the played pitch times `coarse + fine`
//!   (coarse step 0 is the ratio 0.5), plus `detune` in cents; in fixed mode (`fixed` on) at
//!   `10^((coarse + fine) / 8)` Hz whatever the key ([`fm::op_frequency`]).
//! * **Envelope.** Each operator has its own ADSR (`attack`, `decay`, `sustain`, `release`), and
//!   a `delay` in ms before the attack starts, so an operator can swell in after the note. A
//!   voice sounds until the envelopes of its carriers are idle.
//! * **Velocity and key.** `velocity` is how much the key velocity scales the operator's level
//!   (0 none, 1 from silence up to full): on a modulator, a brighter tone for a harder hit. `key_scale`
//!   lowers the level on one side of the middle C (key 60): positive on the keys above, negative
//!   on the keys below, by 0.4 dB per semitone at full scale.
//! * **Algorithms.** `algorithm` picks one of twelve routings ([`fm::ALGORITHMS`]); the carriers
//!   are summed with the gain `1/sqrt(carriers)`. `feedback` feeds the average of the last two
//!   outputs of the algorithm's feedback operator (operator 4 in every routing) back into its own
//!   phase, up to [`fm::MAX_FEEDBACK_RAD`] = 1.8 radians: a sine at zero, a saw-like tone as it
//!   rises. A sine is bounded, so the loop cannot run away, and the range stops short of the
//!   point (about 2 radians) where a one-sample-late loop turns to noise.
//! * **Modulation index.** `index` scales every modulator at once; the mod wheel (CC 1,
//!   `wheel.to_index`), the LFO (`lfo.to_index`) and the index CV (`cv.index_amount`) add to it. The total is
//!   clamped to `0..=3`.
//! * **Aliasing.** A modulation index puts partials out to about `(index + 1)` times the
//!   modulator's frequency; what passes Nyquist folds back. Every control step (16 samples) the
//!   voice asks [`fm::limits`] for the scales that keep the highest partial of every operator under
//!   0.4 of the sample rate, and ramps the indices and the feedback to them: a bright patch plays
//!   duller on a high key instead of aliasing, continuously in the pitch. Operator frequencies
//!   are held under 0.45 of the sample rate.
//! * **Pitch.** `pitch.env_amount` semitones decay to nothing with `pitch.env_decay` ms after each
//!   note start; the LFO moves the pitch by `lfo.to_pitch` semitones; pitch bend moves it by up to
//!   `bend.range` semitones (default 2). The fixed-frequency operators ignore all of them.
//! * **LFO.** One global LFO (`lfo.*`: wave, rate; targets pitch, amplitude and index), written
//!   to `lfo_cv_out` every sample.
//! * **Play.** `play.mode`: `poly` (up to `voices`, default 8, at most 16: a note that finds no
//!   free voice steals the oldest, which fades out over 2 ms), `mono` (one voice; every key
//!   retriggers the envelopes and returns to the key still held when one is released) and
//!   `legato` (as mono, but a key played while another is held changes the pitch only).
//!   `play.glide` is the time in ms a pitch takes to cover most of the way to the new key in
//!   the mono modes. A same-key note-on retriggers that voice from its current level.
//! * **Stereo.** `spread` pans the voices by key (low left, high right, middle C centred); the
//!   pan CV balances the pair. The synth has no filter: an FM voice shapes its own spectrum.
//! * **Sounds.** `sound` selects a factory sound ([`sounds`]). The device ignores it; the app turns
//!   a change of `sound` into a batch of param changes (one undo step).
//! * **Realtime.** Nothing allocates, locks or blocks in `process`, `reset` and `set_param`.
//!   Deterministic: no randomness but the LFO's sample and hold, seeded by the patch seed. Every
//!   output is bounded by 4 (the poly synth's soft limiter).

pub mod sounds;

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::fm::{
    self, ALGORITHM_NAMES, Algorithm, MAX_FEEDBACK_RAD, MAX_INDEX_RAD, OPERATORS,
};
use atomik_dsp::{
    Adsr, Lfo, LfoShape, Smoother, balance_gains, flush_denormal, note_to_hz, sin_cycles,
};

use crate::controller_state::{ControllerState, DEFAULT_BEND_RANGE, Effect, MAX_BEND_RANGE};
use crate::cv_in::{clean, db_gain};
use crate::poly_synth::soft_limit;

/// Number of preallocated voices.
pub const MAX_VOICES: usize = 16;
/// The params of one operator (`op1.*`).
pub const OP_FIELDS: usize = 12;

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

const SOUNDS: Unit = Unit::Choice(&sounds::SOUND_NAMES);
const ALGORITHMS: Unit = Unit::Choice(&ALGORITHM_NAMES);
const LFO_WAVES: Unit = Unit::Choice(&["sine", "triangle", "saw", "square", "sample&hold"]);
const PLAY_MODES: Unit = Unit::Choice(&["poly", "mono", "legato"]);
const ENV_MS: (f32, f32) = (0.5, 10_000.0);

// The global params come first, then the four operators, then the rest.
const P_SOUND: u32 = 0;
const P_ALGORITHM: u32 = 1;
const P_FEEDBACK: u32 = 2;
const P_INDEX: u32 = 3;
const OP_BASE: u32 = 4;
const P_PITCH_ENV: u32 = OP_BASE + (OPERATORS * OP_FIELDS) as u32;
const P_PITCH_DECAY: u32 = P_PITCH_ENV + 1;
const P_LFO_WAVE: u32 = P_PITCH_ENV + 2;
const P_LFO_RATE: u32 = P_PITCH_ENV + 3;
const P_LFO_PITCH: u32 = P_PITCH_ENV + 4;
const P_LFO_AMP: u32 = P_PITCH_ENV + 5;
const P_LFO_INDEX: u32 = P_PITCH_ENV + 6;
const P_WHEEL_INDEX: u32 = P_PITCH_ENV + 7;
const P_MODE: u32 = P_PITCH_ENV + 8;
const P_GLIDE: u32 = P_PITCH_ENV + 9;
const P_VOICES: u32 = P_PITCH_ENV + 10;
const P_SPREAD: u32 = P_PITCH_ENV + 11;
const P_MASTER: u32 = P_PITCH_ENV + 12;
const P_CV_PITCH: u32 = P_PITCH_ENV + 13;
const P_CV_INDEX: u32 = P_PITCH_ENV + 14;
const P_CV_LEVEL: u32 = P_PITCH_ENV + 15;
const P_CV_PAN: u32 = P_PITCH_ENV + 16;
const P_BEND_RANGE: u32 = P_PITCH_ENV + 17;
const PARAM_COUNT: usize = P_BEND_RANGE as usize + 1;

/// The fields of an operator, in param order.
const F_FIXED: usize = 0;
const F_COARSE: usize = 1;
const F_FINE: usize = 2;
const F_DETUNE: usize = 3;
const F_LEVEL: usize = 4;
const F_VELOCITY: usize = 5;
const F_KEY_SCALE: usize = 6;
const F_DELAY: usize = 7;
const F_ATTACK: usize = 8;
const F_DECAY: usize = 9;
const F_SUSTAIN: usize = 10;
const F_RELEASE: usize = 11;

/// The keys of an operator's params: `$p` is `"op1"` and so on.
macro_rules! op_keys {
    ($p:literal) => {
        [
            concat!($p, ".fixed"),
            concat!($p, ".coarse"),
            concat!($p, ".fine"),
            concat!($p, ".detune"),
            concat!($p, ".level"),
            concat!($p, ".velocity"),
            concat!($p, ".key_scale"),
            concat!($p, ".delay"),
            concat!($p, ".attack"),
            concat!($p, ".decay"),
            concat!($p, ".sustain"),
            concat!($p, ".release"),
        ]
    };
}

/// The names of an operator's params: `$n` is `"Op 1"` and so on.
macro_rules! op_names {
    ($n:literal) => {
        [
            concat!($n, " fixed frequency"),
            concat!($n, " coarse"),
            concat!($n, " fine"),
            concat!($n, " detune"),
            concat!($n, " level"),
            concat!($n, " velocity"),
            concat!($n, " key scaling"),
            concat!($n, " delay"),
            concat!($n, " attack"),
            concat!($n, " decay"),
            concat!($n, " sustain"),
            concat!($n, " release"),
        ]
    };
}

const OP_KEYS: [[&str; OP_FIELDS]; OPERATORS] = [
    op_keys!("op1"),
    op_keys!("op2"),
    op_keys!("op3"),
    op_keys!("op4"),
];
const OP_NAMES: [[&str; OP_FIELDS]; OPERATORS] = [
    op_names!("Op 1"),
    op_names!("Op 2"),
    op_names!("Op 3"),
    op_names!("Op 4"),
];

/// The defaults of the four operators' levels and velocity sensitivities (the init sound: a
/// two-operator stack, operator 2 modulating operator 1).
const INIT_LEVEL: [f32; OPERATORS] = [0.8, 0.3, 0.0, 0.0];
const INIT_COARSE: [f32; OPERATORS] = [1.0, 2.0, 1.0, 1.0];
const INIT_VELOCITY: [f32; OPERATORS] = [0.4, 0.6, 0.0, 0.0];

/// The descriptor of field `f` of operator `op`.
const fn op_param(op: usize, f: usize) -> ParamDescriptor {
    let id = OP_BASE + (op * OP_FIELDS + f) as u32;
    let (key, name) = (OP_KEYS[op][f], OP_NAMES[op][f]);
    match f {
        F_FIXED => param(id, key, name, (0.0, 1.0, 0.0), Lin, Unit::Toggle, true),
        F_COARSE => param(
            id,
            key,
            name,
            (0.0, (fm::COARSE_STEPS - 1) as f32, INIT_COARSE[op]),
            Lin,
            Unit::None,
            true,
        ),
        F_FINE => param(id, key, name, (0.0, 0.99, 0.0), Lin, Unit::None, false),
        F_DETUNE => param(id, key, name, (-50.0, 50.0, 0.0), Lin, Unit::Cents, false),
        F_LEVEL => param(
            id,
            key,
            name,
            (0.0, 1.0, INIT_LEVEL[op]),
            Lin,
            Unit::Percent,
            false,
        ),
        F_VELOCITY => param(
            id,
            key,
            name,
            (0.0, 1.0, INIT_VELOCITY[op]),
            Lin,
            Unit::Percent,
            false,
        ),
        F_KEY_SCALE => param(id, key, name, (-1.0, 1.0, 0.0), Lin, Unit::None, false),
        F_DELAY => param(id, key, name, (0.0, 2_000.0, 0.0), Lin, Unit::Ms, false),
        F_ATTACK => param(
            id,
            key,
            name,
            (ENV_MS.0, ENV_MS.1, 2.0),
            Exp,
            Unit::Ms,
            false,
        ),
        F_DECAY => param(
            id,
            key,
            name,
            (ENV_MS.0, ENV_MS.1, 400.0),
            Exp,
            Unit::Ms,
            false,
        ),
        F_SUSTAIN => param(id, key, name, (0.0, 1.0, 0.7), Lin, Unit::Percent, false),
        _ => param(
            id,
            key,
            name,
            (ENV_MS.0, ENV_MS.1, 300.0),
            Exp,
            Unit::Ms,
            false,
        ),
    }
}

const fn build_params() -> [ParamDescriptor; PARAM_COUNT] {
    let mut out = [param(0, "", "", (0.0, 1.0, 0.0), Lin, Unit::None, false); PARAM_COUNT];
    out[0] = param(
        P_SOUND,
        "sound",
        "Sound",
        (0.0, (sounds::SOUND_NAMES.len() - 1) as f32, 0.0),
        Lin,
        SOUNDS,
        true,
    );
    out[1] = param(
        P_ALGORITHM,
        "algorithm",
        "Algorithm",
        (0.0, (fm::ALGORITHM_COUNT - 1) as f32, 0.0),
        Lin,
        ALGORITHMS,
        true,
    );
    out[2] = param(
        P_FEEDBACK,
        "feedback",
        "Feedback",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    );
    out[3] = param(
        P_INDEX,
        "index",
        "Modulation index",
        (0.0, 2.0, 1.0),
        Lin,
        Unit::None,
        false,
    );
    let mut op = 0;
    while op < OPERATORS {
        let mut f = 0;
        while f < OP_FIELDS {
            out[OP_BASE as usize + op * OP_FIELDS + f] = op_param(op, f);
            f += 1;
        }
        op += 1;
    }
    let rest = [
        param(
            P_PITCH_ENV,
            "pitch.env_amount",
            "Pitch envelope",
            (-48.0, 48.0, 0.0),
            Lin,
            Unit::Semitones,
            false,
        ),
        param(
            P_PITCH_DECAY,
            "pitch.env_decay",
            "Pitch envelope decay",
            (1.0, 5_000.0, 80.0),
            Exp,
            Unit::Ms,
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
            (0.05, 30.0, 5.0),
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
            P_LFO_AMP,
            "lfo.to_amp",
            "LFO to amplitude",
            (0.0, 1.0, 0.0),
            Lin,
            Unit::Percent,
            false,
        ),
        param(
            P_LFO_INDEX,
            "lfo.to_index",
            "LFO to index",
            (0.0, 1.0, 0.0),
            Lin,
            Unit::None,
            false,
        ),
        param(
            P_WHEEL_INDEX,
            "wheel.to_index",
            "Mod wheel to index",
            (0.0, 2.0, 0.5),
            Lin,
            Unit::None,
            false,
        ),
        param(
            P_MODE,
            "play.mode",
            "Play mode",
            (0.0, 2.0, 0.0),
            Lin,
            PLAY_MODES,
            true,
        ),
        param(
            P_GLIDE,
            "play.glide",
            "Glide",
            (0.0, 2_000.0, 0.0),
            Lin,
            Unit::Ms,
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
            P_SPREAD,
            "spread",
            "Stereo spread",
            (0.0, 1.0, 0.0),
            Lin,
            Unit::Percent,
            false,
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
            P_CV_INDEX,
            "cv.index_amount",
            "CV to index",
            (-2.0, 2.0, 1.0),
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
    ];
    let mut i = 0;
    while i < rest.len() {
        out[P_PITCH_ENV as usize + i] = rest[i];
        i += 1;
    }
    out
}

static PARAMS: [ParamDescriptor; PARAM_COUNT] = build_params();

const NOTES_IN: SocketId = SocketId(0);
const PITCH_CV_IN: SocketId = SocketId(1);
const INDEX_CV_IN: SocketId = SocketId(2);
const LEVEL_CV_IN: SocketId = SocketId(3);
const PAN_CV_IN: SocketId = SocketId(4);
const OUT_L: SocketId = SocketId(5);
const OUT_R: SocketId = SocketId(6);
const LFO_CV_OUT: SocketId = SocketId(7);

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

static SOCKETS: [SocketDescriptor; 8] = [
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
        INDEX_CV_IN,
        "index_cv_in",
        "Index CV",
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

/// Static description of the FM synth.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.fm-synth",
    name: "FM Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// The id of field `field` of operator `op` (0-based) among the params.
pub const fn op_param_id(op: usize, field: usize) -> u32 {
    OP_BASE + (op * OP_FIELDS + field) as u32
}

/// Factory registering [`FmSynth`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(FmSynth::new()),
    }
}

/// Samples between control-rate updates (pitch, indices, levels) of a voice.
const CONTROL_INTERVAL: u32 = 16;
/// Fixed range of the pitch CV in semitones (times `pitch_cv_trim`).
const CV_PITCH_RANGE: f32 = 24.0;
/// Fixed range of the level CV in dB (times `master.level_cv_trim`).
const CV_LEVEL_RANGE_DB: f32 = 24.0;
/// Per-voice output gain before the master stage.
const VOICE_GAIN: f32 = 0.5;
/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Highest operator frequency as a fraction of the sample rate.
const MAX_FREQ_RATIO: f32 = 0.45;
/// The highest partial the limiter allows, as a fraction of the sample rate.
const BANDWIDTH_RATIO: f32 = 0.4;
/// Key scaling in dB per semitone at full scale.
const KEY_SCALE_DB: f32 = 0.4;
/// The most the modulation index can be (the sum of `index` and its modulations).
const MAX_TOTAL_INDEX: f32 = 3.0;
/// How far the keys pan at full `spread`: middle C is centre, this many semitones away is the edge.
const PAN_SPAN: f32 = 36.0;
/// `-20 dB` per decade in a `2^x` exponent: `x = db * DB_TO_LOG2`.
const DB_TO_LOG2: f32 = std::f32::consts::LOG2_10 / 20.0;
/// One cycle in radians, to turn radians of phase deviation into cycles.
const RAD_TO_CYCLES: f32 = 1.0 / std::f32::consts::TAU;

/// How the voices are played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayMode {
    Poly,
    Mono,
    Legato,
}

/// Plain values of one operator.
#[derive(Debug, Clone, Copy)]
struct OpValues {
    fixed: bool,
    coarse: f32,
    fine: f32,
    /// `2^(detune / 1200)`.
    detune: f32,
    level: f32,
    velocity: f32,
    key_scale: f32,
    delay_samples: u32,
}

/// Shared (per-device) synthesis parameters, as plain values.
#[derive(Debug, Clone, Copy)]
struct Shared {
    sample_rate: f32,
    algorithm: &'static Algorithm,
    feedback: f32,
    index: f32,
    ops: [OpValues; OPERATORS],
    pitch_env: f32,
    /// Decay factor of the pitch envelope per control step.
    pitch_env_coef: f32,
    lfo_to_pitch: f32,
    lfo_to_amp: f32,
    lfo_to_index: f32,
    wheel_to_index: f32,
    mode: PlayMode,
    /// Share of the way to the new pitch covered per control step (1 is no glide).
    glide_coef: f32,
    spread: f32,
    /// Pitch bend in semitones, `-range..=range` (the controller state's range).
    bend: f32,
    /// Mod wheel (CC 1), `0..=1`.
    wheel: f32,
    cv_pitch_amount: f32,
    cv_index_amount: f32,
}

/// Control-rate modulation values read by a voice.
#[derive(Debug, Clone, Copy)]
struct Ctl {
    lfo: f32,
    pitch: f32,
    index: f32,
}

/// Per-sample modulation sources of one render segment (aligned with the output slices).
struct Mods<'a> {
    lfo: &'a [f32],
    pitch: &'a [f32],
    index: &'a [f32],
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
}

/// One preallocated voice.
#[derive(Debug, Clone)]
struct Voice {
    key: u8,
    order: u64,
    pending: Option<(u8, f32)>,
    velocity: f32,
    /// The pitch in semitones (a MIDI key) now, and the one it glides to.
    pitch: f32,
    target: f32,
    /// The pitch envelope's current offset in semitones.
    pitch_env: f32,
    /// The key is held (the envelopes were gated on and not yet released).
    gated: bool,
    /// The steal fade runs: the voice no longer counts as playing its key.
    fading: bool,
    phase: [f64; OPERATORS],
    step: [f64; OPERATORS],
    /// The last two outputs of the feedback operator.
    fb: [f32; 2],
    env: [Adsr; OPERATORS],
    delay_left: [u32; OPERATORS],
    level: [Ramp; OPERATORS],
    /// The phase deviation in cycles per unit of modulator output, of each operator's input.
    mscale: [Ramp; OPERATORS],
    fbscale: Ramp,
    trem: Ramp,
    pan: (f32, f32),
    countdown: u32,
    snap: bool,
}

impl Voice {
    fn new() -> Self {
        Self {
            key: 0,
            order: 0,
            pending: None,
            velocity: 0.0,
            pitch: 0.0,
            target: 0.0,
            pitch_env: 0.0,
            gated: false,
            fading: false,
            phase: [0.0; OPERATORS],
            step: [0.0; OPERATORS],
            fb: [0.0; 2],
            env: [Adsr::new(48_000.0); OPERATORS],
            delay_left: [0; OPERATORS],
            level: [Ramp::new(0.0); OPERATORS],
            mscale: [Ramp::new(0.0); OPERATORS],
            fbscale: Ramp::new(0.0),
            trem: Ramp::new(1.0),
            pan: (1.0, 1.0),
            countdown: 0,
            snap: true,
        }
    }

    /// Whether a carrier still sounds (or waits for its delay).
    fn is_active(&self, alg: &Algorithm) -> bool {
        (0..OPERATORS)
            .any(|op| alg.is_carrier(op) && (self.env[op].is_active() || self.delay_left[op] > 0))
    }

    /// The key this voice is playing (or will play after its steal fade), if any.
    fn sounding_key(&self, alg: &Algorithm) -> Option<u8> {
        if let Some((k, _)) = self.pending {
            Some(k)
        } else if self.is_active(alg) && !self.fading {
            Some(self.key)
        } else {
            None
        }
    }

    /// Gates every envelope on, after its operator's delay when `delayed`.
    fn gate_on(&mut self, p: &Shared, delayed: bool) {
        for (op, v) in p.ops.iter().enumerate() {
            if delayed && v.delay_samples > 0 {
                self.delay_left[op] = v.delay_samples;
            } else {
                self.delay_left[op] = 0;
                self.env[op].gate_on();
            }
        }
        self.gated = true;
        self.fading = false;
    }

    /// Starts a note from silence (fresh phases and envelopes). `from` is the pitch to glide
    /// from, when there is one.
    fn start(&mut self, key: u8, velocity: f32, from: Option<f32>, p: &Shared) {
        self.key = key;
        self.pending = None;
        self.velocity = velocity.clamp(0.0, 1.0);
        self.target = f32::from(key);
        self.pitch = from.unwrap_or(self.target);
        self.pitch_env = p.pitch_env;
        self.phase = [0.0; OPERATORS];
        self.fb = [0.0; 2];
        for e in &mut self.env {
            e.reset();
        }
        self.gate_on(p, true);
        self.countdown = 0;
        self.snap = true;
    }

    /// Same-key retrigger: the envelopes restart from their current levels.
    fn retrigger(&mut self, key: u8, velocity: f32, p: &Shared) {
        self.key = key;
        self.velocity = velocity.clamp(0.0, 1.0);
        self.target = f32::from(key);
        self.pitch_env = p.pitch_env;
        self.gate_on(p, false);
    }

    fn release(&mut self) {
        self.pending = None;
        self.gated = false;
        for (e, d) in self.env.iter_mut().zip(&mut self.delay_left) {
            *d = 0;
            e.gate_off();
        }
    }

    /// The steal fade: every envelope falls to nothing over 2 ms.
    fn steal(&mut self) {
        self.gated = false;
        self.fading = true;
        for (e, d) in self.env.iter_mut().zip(&mut self.delay_left) {
            *d = 0;
            e.steal_fade();
        }
    }

    /// The gain of a key's level scaling.
    fn key_scale_gain(scale: f32, key: f32) -> f32 {
        let d = key - 60.0;
        let steps = if scale >= 0.0 {
            d.max(0.0)
        } else {
            (-d).max(0.0)
        };
        (-scale.abs() * KEY_SCALE_DB * steps * DB_TO_LOG2).exp2()
    }

    /// Control-rate update: pitch, operator frequencies, levels, indices and feedback.
    fn control(&mut self, c: Ctl, p: &Shared) {
        let snap = self.snap;
        self.pitch += (self.target - self.pitch) * p.glide_coef;
        self.pitch_env *= p.pitch_env_coef;
        let semis = self.pitch
            + p.bend
            + self.pitch_env
            + c.lfo * p.lfo_to_pitch
            + p.cv_pitch_amount * clean(c.pitch);
        let max_hz = MAX_FREQ_RATIO * p.sample_rate;
        let key_hz = note_to_hz(semis, 0.0).min(max_hz);
        let mut freq = [0.0f32; OPERATORS];
        let mut level = [0.0f32; OPERATORS];
        for (op, v) in p.ops.iter().enumerate() {
            let hz = fm::op_frequency(v.fixed, v.coarse, v.fine, key_hz) * v.detune;
            freq[op] = hz.min(max_hz);
            self.step[op] = f64::from(freq[op] / p.sample_rate);
            let vel = 1.0 - v.velocity * (1.0 - self.velocity);
            level[op] = v.level * vel * Self::key_scale_gain(v.key_scale, self.target);
            self.level[op].aim(level[op], snap);
        }
        // The total modulation index, then what the bandwidth allows of it.
        let index = (p.index + p.wheel * p.wheel_to_index + c.lfo * p.lfo_to_index + c.index)
            .clamp(0.0, MAX_TOTAL_INDEX);
        let depth: [f32; OPERATORS] = std::array::from_fn(|op| level[op] * index * MAX_INDEX_RAD);
        let lim = fm::limits(
            p.algorithm,
            &freq,
            &depth,
            p.feedback * MAX_FEEDBACK_RAD,
            BANDWIDTH_RATIO * p.sample_rate,
        );
        for (op, m) in self.mscale.iter_mut().enumerate() {
            m.aim(lim.index[op] * index * MAX_INDEX_RAD * RAD_TO_CYCLES, snap);
        }
        self.fbscale.aim(
            lim.feedback * p.feedback * MAX_FEEDBACK_RAD * RAD_TO_CYCLES,
            snap,
        );
        self.trem.aim(
            1.0 - p.lfo_to_amp * 0.5 * (1.0 - c.lfo.clamp(-1.0, 1.0)),
            snap,
        );
        let (gl, gr) = balance_gains(p.spread * ((self.target - 60.0) / PAN_SPAN).clamp(-1.0, 1.0));
        self.pan = (gl, gr);
        self.snap = false;
    }

    /// One sample of the four operators through the algorithm: the mono carrier sum.
    #[inline]
    fn operators(&mut self, p: &Shared) -> f32 {
        let alg = p.algorithm;
        let fb_op = usize::from(alg.feedback);
        let fb_scale = self.fbscale.next();
        let mut y = [0.0f32; OPERATORS];
        let mut sum = 0.0f32;
        for op in (0..OPERATORS).rev() {
            let level = self.level[op].next();
            let mscale = self.mscale[op].next();
            let env = self.env[op].next_sample();
            let mut modulation = 0.0f32;
            let mask = alg.mods[op];
            if mask != 0 {
                for (j, yj) in y.iter().enumerate().skip(op + 1) {
                    if mask >> j & 1 == 1 {
                        modulation += yj;
                    }
                }
                modulation *= mscale;
            }
            if op == fb_op {
                // The average of the last two outputs: the damped feedback path.
                modulation += 0.5 * (self.fb[0] + self.fb[1]) * fb_scale;
            }
            let s = sin_cycles(self.phase[op] + f64::from(modulation)) as f32;
            y[op] = s * level * env;
            let ph = self.phase[op] + self.step[op];
            self.phase[op] = ph - ph.floor();
            if alg.is_carrier(op) {
                sum += y[op];
            }
        }
        self.fb[1] = self.fb[0];
        self.fb[0] = flush_denormal(y[fb_op]);
        sum * alg.carrier_gain()
    }

    /// Adds this voice into `left` and `right`. The modulation slices are aligned with them.
    fn render(&mut self, left: &mut [f32], right: &mut [f32], m: &Mods<'_>, p: &Shared) {
        for (n, (lo, ro)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            if self.countdown == 0 {
                let c = Ctl {
                    lfo: m.lfo[n],
                    pitch: m.pitch[n],
                    index: p.cv_index_amount * clean(m.index[n]),
                };
                self.control(c, p);
                self.countdown = CONTROL_INTERVAL;
            }
            self.countdown -= 1;
            for op in 0..OPERATORS {
                if self.delay_left[op] > 0 {
                    self.delay_left[op] -= 1;
                    if self.delay_left[op] == 0 && self.gated {
                        self.env[op].gate_on();
                    }
                }
            }
            let out = self.operators(p) * self.trem.next() * VOICE_GAIN;
            *lo += out * self.pan.0;
            *ro += out * self.pan.1;
            if !self.is_active(p.algorithm) {
                match self.pending.take() {
                    Some((key, velocity)) => self.start(key, velocity, None, p),
                    None => return,
                }
            }
        }
    }
}

/// A held key, for the mono modes.
#[derive(Debug, Clone, Copy)]
struct Held {
    key: u8,
    velocity: f32,
}

/// The FM synth device.
#[derive(Debug)]
pub struct FmSynth {
    values: [f32; PARAM_COUNT],
    shared: Shared,
    voices: [Voice; MAX_VOICES],
    voice_limit: usize,
    next_order: u64,
    /// The keys held in the mono modes, oldest first (at most one per key).
    held: [Held; MAX_VOICES],
    held_len: usize,
    /// The pitch the mono voice last sounded at, to glide from.
    last_pitch: Option<f32>,
    lfo: Lfo,
    master: Smoother,
    cv_level_db: f32,
    cv_pan_amount: f32,
    /// Sustain, volume, expression, pressure and the bend range.
    ctl: ControllerState,
    /// Set by `prepare`/`reset`: the next `process` snaps the master gain to its target.
    fresh: bool,
    seed: u64,
    #[cfg(test)]
    probe: Option<std::sync::Arc<tests::VoiceProbe>>,
}

impl Default for FmSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl FmSynth {
    /// A synth with default params (call `prepare` before processing).
    pub fn new() -> Self {
        let op = OpValues {
            fixed: false,
            coarse: 1.0,
            fine: 0.0,
            detune: 1.0,
            level: 0.0,
            velocity: 0.0,
            key_scale: 0.0,
            delay_samples: 0,
        };
        let mut s = Self {
            values: PARAMS.map(|p| p.default),
            shared: Shared {
                sample_rate: 48_000.0,
                algorithm: fm::algorithm(0),
                feedback: 0.0,
                index: 1.0,
                ops: [op; OPERATORS],
                pitch_env: 0.0,
                pitch_env_coef: 0.0,
                lfo_to_pitch: 0.0,
                lfo_to_amp: 0.0,
                lfo_to_index: 0.0,
                wheel_to_index: 0.5,
                mode: PlayMode::Poly,
                glide_coef: 1.0,
                spread: 0.0,
                bend: 0.0,
                wheel: 0.0,
                cv_pitch_amount: CV_PITCH_RANGE,
                cv_index_amount: 1.0,
            },
            voices: std::array::from_fn(|_| Voice::new()),
            voice_limit: MAX_VOICES,
            next_order: 0,
            held: [Held {
                key: 0,
                velocity: 0.0,
            }; MAX_VOICES],
            held_len: 0,
            last_pitch: None,
            lfo: Lfo::new(crate::poly_synth::derive_seed(0, 0)),
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            cv_level_db: CV_LEVEL_RANGE_DB,
            cv_pan_amount: 1.0,
            ctl: ControllerState::new(),
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
        if (OP_BASE..P_PITCH_ENV).contains(&id) {
            let (op, field) = (
                (id - OP_BASE) as usize / OP_FIELDS,
                (id - OP_BASE) as usize % OP_FIELDS,
            );
            let o = &mut self.shared.ops[op];
            let env = &mut self.voices;
            match field {
                F_FIXED => o.fixed = v >= 0.5,
                F_COARSE => o.coarse = v.round(),
                F_FINE => o.fine = v,
                F_DETUNE => o.detune = (v / 1_200.0).exp2(),
                F_LEVEL => o.level = v,
                F_VELOCITY => o.velocity = v,
                F_KEY_SCALE => o.key_scale = v,
                F_DELAY => o.delay_samples = (v * 0.001 * sr).round() as u32,
                F_ATTACK => env.iter_mut().for_each(|vc| vc.env[op].set_attack_ms(v)),
                F_DECAY => env.iter_mut().for_each(|vc| vc.env[op].set_decay_ms(v)),
                F_SUSTAIN => env.iter_mut().for_each(|vc| vc.env[op].set_sustain(v)),
                F_RELEASE => env.iter_mut().for_each(|vc| vc.env[op].set_release_ms(v)),
                _ => {}
            }
            return;
        }
        match id {
            P_ALGORITHM => self.shared.algorithm = fm::algorithm(choice(v)),
            P_FEEDBACK => self.shared.feedback = v,
            P_INDEX => self.shared.index = v,
            P_PITCH_ENV => self.shared.pitch_env = v,
            P_PITCH_DECAY => {
                let tau = (v * 0.001 * sr).max(1.0);
                self.shared.pitch_env_coef = (-(CONTROL_INTERVAL as f32) / tau).exp();
            }
            P_LFO_WAVE => self.lfo.set_shape(LfoShape::from_index(choice(v))),
            P_LFO_RATE => self.lfo.set_rate(v, sr),
            P_LFO_PITCH => self.shared.lfo_to_pitch = v,
            P_LFO_AMP => self.shared.lfo_to_amp = v,
            P_LFO_INDEX => self.shared.lfo_to_index = v,
            P_WHEEL_INDEX => self.shared.wheel_to_index = v,
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
            P_CV_PITCH => self.shared.cv_pitch_amount = v * CV_PITCH_RANGE,
            P_CV_INDEX => self.shared.cv_index_amount = v,
            P_CV_LEVEL => self.cv_level_db = v * CV_LEVEL_RANGE_DB,
            P_CV_PAN => self.cv_pan_amount = v,
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
            P_MASTER => self
                .master
                .set_target(PARAMS[P_MASTER as usize].db_param_gain(v)),
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
        let alg = p.algorithm;
        let voices = &mut self.voices[..self.voice_limit];
        // 1. Same key sounding (or pending after a steal): retrigger that voice.
        if let Some(v) = voices.iter_mut().find(|v| v.sounding_key(alg) == Some(key)) {
            v.order = order;
            if v.pending.is_some() {
                v.pending = Some((key, velocity));
            } else {
                v.retrigger(key, velocity, &p);
            }
            return;
        }
        // 2. A free voice.
        if let Some(v) = voices.iter_mut().find(|v| !v.is_active(alg)) {
            v.order = order;
            v.start(key, velocity, None, &p);
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
        let p = self.shared;
        self.hold(key, velocity);
        let from = self.last_pitch;
        let v = &mut self.voices[0];
        if v.gated && p.mode == PlayMode::Legato {
            // A legato key under a held one: the pitch glides, the envelopes run on.
            v.key = key;
            v.target = f32::from(key);
            v.velocity = velocity.clamp(0.0, 1.0);
        } else if v.is_active(p.algorithm) && !v.fading {
            v.retrigger(key, velocity, &p);
        } else {
            v.start(key, velocity, from, &p);
        }
        self.last_pitch = Some(f32::from(key));
    }

    /// Puts `key` on top of the held keys of the mono modes.
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

    /// A key released in the mono modes: back to the key still held, or release.
    fn mono_note_off(&mut self, key: u8) {
        let p = self.shared;
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
        if p.mode == PlayMode::Legato {
            v.key = back.key;
            v.target = f32::from(back.key);
        } else {
            v.retrigger(back.key, back.velocity, &p);
        }
        self.last_pitch = Some(f32::from(back.key));
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
            NoteKind::Off { key } => {
                if self.ctl.note_off(key) {
                    self.release_key(key);
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

    /// The key is let go (or its held-back release comes due): its voice releases, or in the
    /// mono modes the voice returns to the key still held.
    fn release_key(&mut self, key: u8) {
        if self.shared.mode == PlayMode::Poly {
            let alg = self.shared.algorithm;
            self.voices
                .iter_mut()
                .filter(|v| v.sounding_key(alg) == Some(key))
                .for_each(Voice::release);
        } else {
            self.mono_note_off(key);
        }
    }

    #[cfg(test)]
    fn update_probe(&self) {
        use std::sync::atomic::Ordering;
        if let Some(probe) = &self.probe {
            let alg = self.shared.algorithm;
            let mut active = 0;
            for (slot, v) in probe.keys.iter().zip(&self.voices) {
                let k = if v.is_active(alg) {
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

impl Device for FmSynth {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.shared.sample_rate = sr;
        for v in &mut self.voices {
            for e in &mut v.env {
                e.set_sample_rate(sr);
            }
        }
        self.master.set_time(MASTER_SMOOTH_MS, sr);
        self.ctl.set_sample_rate(sr);
        self.lfo = Lfo::new(crate::poly_synth::derive_seed(info.seed, 0));
        self.apply_all();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            for e in &mut v.env {
                e.reset();
            }
            v.delay_left = [0; OPERATORS];
            v.pending = None;
            v.gated = false;
            v.fading = false;
            v.fb = [0.0; 2];
        }
        self.next_order = 0;
        self.held_len = 0;
        self.last_pitch = None;
        self.ctl.reset();
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
        let cv_pitch = *ctx.input(PITCH_CV_IN);
        let cv_index = *ctx.input(INDEX_CV_IN);
        let (mut left, mut right) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        let mut ctl_gain = [1.0f32; BATCH];
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
                pitch: &cv_pitch[start..end],
                index: &cv_index[start..end],
            };
            for v in self.voices.iter_mut().filter(|v| v.is_active(p.algorithm)) {
                v.render(&mut left[start..end], &mut right[start..end], &m, &p);
            }
            self.ctl.fill_gain(&mut ctl_gain[start..end]);
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
            left[n] *= ctl_gain[n];
            right[n] *= ctl_gain[n];
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
mod tests;
