//! `atomik.granular` ("Granular Sampler"): a sample played as a cloud of short grains. An original
//! design from the textbook of granular synthesis; the blocks that are not the voice (windows,
//! grain pool, clock, anti-aliased read, built-in source) are in [`atomik_dsp::grain`].
//!
//! # Signal flow
//!
//! ```text
//! per voice:   clock --density--> grain --> pool of up to 64 grains --> sum
//!              (position + scan + spray, size, pitch, direction, window, pan)
//!              sum . filter . amp envelope . velocity -> voices summed . master level . soft limiter
//! ```
//!
//! * **Sample.** The device plays the sample map the app installs with `swap_samples`, exactly as
//!   the sampler does (the pool, the browser, a drop on the panel, the sample editor, relink): the
//!   first zone that matches the note (key and velocity range, in list order) gives the audio
//!   and the **window** `start..end` the grains read from; the zone's root key, transpose, tune,
//!   gain and pan apply, its loop and trigger fields are ignored (the grains loop the window by
//!   themselves, and a note-off always releases). A mono sample is panned per grain with the
//!   constant-power law, a stereo one balanced. **Without a map** the device plays its built-in
//!   source, a sung vowel pad ([`atomik_dsp::grain::default_source`], three seconds at middle C,
//!   made in code at the sample rate), so a new device sounds at once; with a map that has no zone
//!   for a note, that note is silent.
//! * **Grains.** Each voice has a clock that starts `grain.density` grains per second (up to
//!   `grain.timing` of randomness in the intervals, the mean unchanged). A grain lasts
//!   `grain.size` ms, shaped by `grain.window` (Hann, Tukey, percussive). Its start in the window
//!   is `pos.position` plus the **scan** plus a random **spray**:
//!   * `pos.scan` moves the position through the window at that many times real time (0 freezes,
//!     negative runs backwards) and wraps round the window; it restarts at each note.
//!   * `pos.spray` is the width, as a fraction of the window, of the uniform random offset added
//!     to every grain (centred on the position and reflected at the ends of the window).
//!   * A grain that fits the window is kept inside it, so there is no splice in a grain: a start
//!     near an end is moved in (reversed grains read downwards from their start).
//! * **Pitch.** A grain reads the window at `2^(semitones/12)` source frames per output frame:
//!   `pitch.key_track` of the distance between the key and the zone's root key (100 % follows the
//!   keyboard, 0 plays every key at the recorded pitch), `pitch.transpose` and `pitch.fine`, the
//!   zone's transpose and tune, the pitch bend (two semitones), the pitch CV, the LFO and the mod
//!   envelope, and, per grain, a random offset of up to `pitch.random` cents either way. The ratio
//!   is limited to 1/32 to 8 and read with a four-point Hermite; above a step of 1 the read
//!   low-passes by a triangular multi-tap average so a high transposition does not alias
//!   ([`atomik_dsp::grain`] gives the response and the test measures it).
//! * **Direction and stereo.** A fraction `grain.reverse` of the grains read backwards (0 none,
//!   1 all, in between at random). `grain.spread` pans each grain at random by up to that much
//!   either side of the zone's pan.
//! * **Level.** The grains of a voice are scaled by `1/sqrt(overlap)` where the overlap (grains
//!   sounding at once: density times size, scaled by how much power the window carries, at most
//!   `grain.limit`) is at least 1, so density and size change the texture, not the loudness much.
//!   Then the amp ADSR, `1 - velocity.amp (1 - velocity)`, the master level (5 ms smoothing) and
//!   the poly synth's soft limiter (|y| < 4).
//! * **Cap and stealing.** A voice holds at most `grain.limit` (1 to 64) grains; when they are all
//!   sounding the one closest to the end of its window is replaced, a deterministic cut at the
//!   quietest moment. Notes: up to `voices` (default 8, at most 16); a note that finds no free
//!   voice steals the oldest, which fades out over 2 ms.
//! * **Modulation.** One LFO (`lfo.*`) and one mod envelope (`env.mod.*`) per voice move the
//!   position (a fraction of the window), the size and the density (octaves) and the pitch
//!   (semitones); the LFO and the filter envelope move the cutoff (octaves). The mod wheel (CC 1)
//!   adds `wheel.to_position` to the position and `wheel.to_spray` to the spray (default: it
//!   opens the spray). The CV inputs (pitch, position, density and cutoff) add to those values,
//!   scaled by their `cv.*` amounts; the level CV scales the master level.
//! * **Filter.** Off by default; when `filter.enabled` a stereo pair of 12 dB state-variable
//!   filters per voice, cutoff `filter.cutoff * 2^(env_amount * 6 * mod envelope + key_track *
//!   (key - 60)/12 + lfo.to_cutoff * lfo + cv.cutoff_amount * cv)` as in the sampler.
//! * **Sounds.** `sound` selects a factory sound ([`sounds`]). The device ignores it; the app
//!   turns a change of `sound` into a batch of param changes (one undo step).
//! * **Swapping samples.** As the sampler: the device holds at most two maps (`current` and
//!   `draining`) and never drops one, it hands one back. A voice whose zone survives the swap with
//!   the same buffer moves to the new zone (the window follows); any other voice on the replaced
//!   map fades out in 5 ms and then its grains are gone, which frees the old map. A voice playing
//!   the built-in source is left alone.
//! * **Realtime.** Nothing allocates, locks or blocks in `process`, `reset`, `set_param` and
//!   `swap_samples`: every voice owns its grain pool and the window tables are built at
//!   construction. Deterministic: each note seeds its own random stream from the patch seed and
//!   the note's start number (which `reset` restarts), so two renders of the same notes are the
//!   same samples. Output is bounded by 4.
//! * **Display.** The device publishes 31 values (`meter_values`) for the panel: the source
//!   (0 built-in, else the zone number plus 1), the newest voice's position and spray (`-1` when
//!   no voice sounds), the number of sounding grains and voices, then the last 13 started grains
//!   as (position in the window, how far through its life) pairs (`-1, -1` for none).

pub mod sounds;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit, db_to_gain};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, LoopMode, PrepareInfo,
    ProcessContext, SampleBuffer, SampleMap, SampleMapRef, SocketDescriptor, SocketId, SocketKind,
    TriggerMode, Zone, velocity_to_7bit,
};
use atomik_dsp::grain::{
    self, GrainClock, GrainParams, GrainPool, GrainSource, MAX_GRAINS, MAX_STEP, Window,
    WindowBank, fit_start, fold01,
};
use atomik_dsp::rng::{Rng, hash64};
use atomik_dsp::{
    Adsr, FilterMode, Lfo, LfoShape, Smoother, Svf, balance_gains, exp2_det, flush_denormal,
    pan_gains_mono,
};

use crate::cv_in::{clean, db_gain};
use crate::poly_synth::{derive_seed, soft_limit};

/// Number of preallocated voices (the highest value of `voices`).
pub const MAX_VOICES: usize = 16;
/// Number of values the device publishes for its panel (`meter_values`).
pub const METER_COUNT: usize = 31;
/// How many started grains the display values list.
pub const TRAIL_GRAINS: usize = 13;
/// The value published for "none".
pub const NONE: f32 = -1.0;

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
const WINDOWS: Unit = Unit::Choice(&["Hann", "Tukey", "Percussive"]);
const FILTER_MODES: Unit = Unit::Choice(&["LP12", "BP12", "HP12"]);
const LFO_WAVES: Unit = Unit::Choice(&["sine", "triangle", "saw", "square", "sample&hold"]);
const ENV_MS: (f32, f32) = (0.5, 10_000.0);

const P_SOUND: u32 = 0;
const P_VOICES: u32 = 1;
const P_MASTER: u32 = 2;
const P_DENSITY: u32 = 3;
const P_SIZE: u32 = 4;
const P_WINDOW: u32 = 5;
const P_TIMING: u32 = 6;
const P_REVERSE: u32 = 7;
const P_SPREAD: u32 = 8;
const P_LIMIT: u32 = 9;
const P_POSITION: u32 = 10;
const P_SPRAY: u32 = 11;
const P_SCAN: u32 = 12;
const P_TRANSPOSE: u32 = 13;
const P_FINE: u32 = 14;
const P_RANDOM: u32 = 15;
const P_KEY_TRACK: u32 = 16;
const P_AMP_A: u32 = 17;
const P_AMP_D: u32 = 18;
const P_AMP_S: u32 = 19;
const P_AMP_R: u32 = 20;
const P_VEL_AMP: u32 = 21;
const P_FILTER_ON: u32 = 22;
const P_FILTER_MODE: u32 = 23;
const P_CUTOFF: u32 = 24;
const P_RESONANCE: u32 = 25;
const P_FILTER_KEY: u32 = 26;
const P_FILTER_ENV: u32 = 27;
const P_MOD_A: u32 = 28;
const P_MOD_D: u32 = 29;
const P_MOD_S: u32 = 30;
const P_MOD_R: u32 = 31;
const P_MOD_POSITION: u32 = 32;
const P_MOD_SIZE: u32 = 33;
const P_MOD_DENSITY: u32 = 34;
const P_MOD_PITCH: u32 = 35;
const P_LFO_WAVE: u32 = 36;
const P_LFO_RATE: u32 = 37;
const P_LFO_POSITION: u32 = 38;
const P_LFO_SIZE: u32 = 39;
const P_LFO_DENSITY: u32 = 40;
const P_LFO_PITCH: u32 = 41;
const P_LFO_CUTOFF: u32 = 42;
const P_WHEEL_POSITION: u32 = 43;
const P_WHEEL_SPRAY: u32 = 44;
const P_CV_PITCH: u32 = 45;
const P_CV_POSITION: u32 = 46;
const P_CV_DENSITY: u32 = 47;
const P_CV_CUTOFF: u32 = 48;
const P_CV_LEVEL: u32 = 49;
const PARAM_COUNT: usize = 50;

static PARAMS: [ParamDescriptor; PARAM_COUNT] = [
    param(
        P_SOUND,
        "sound",
        "Sound",
        (0.0, (sounds::SOUND_NAMES.len() - 1) as f32, 0.0),
        Lin,
        SOUNDS,
        true,
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
        P_DENSITY,
        "grain.density",
        "Grain density",
        (1.0, 200.0, 24.0),
        Exp,
        Unit::Hz,
        false,
    ),
    param(
        P_SIZE,
        "grain.size",
        "Grain size",
        (5.0, 500.0, 90.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_WINDOW,
        "grain.window",
        "Grain window",
        (0.0, 2.0, 0.0),
        Lin,
        WINDOWS,
        true,
    ),
    param(
        P_TIMING,
        "grain.timing",
        "Grain timing",
        (0.0, 1.0, 0.3),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_REVERSE,
        "grain.reverse",
        "Grain reverse",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_SPREAD,
        "grain.spread",
        "Grain spread",
        (0.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_LIMIT,
        "grain.limit",
        "Grain limit",
        (1.0, MAX_GRAINS as f32, 48.0),
        Lin,
        Unit::None,
        true,
    ),
    param(
        P_POSITION,
        "pos.position",
        "Position",
        (0.0, 1.0, 0.2),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_SPRAY,
        "pos.spray",
        "Spray",
        (0.0, 1.0, 0.08),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_SCAN,
        "pos.scan",
        "Scan speed",
        (-4.0, 4.0, 0.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_TRANSPOSE,
        "pitch.transpose",
        "Transpose",
        (-24.0, 24.0, 0.0),
        Lin,
        Unit::Semitones,
        true,
    ),
    param(
        P_FINE,
        "pitch.fine",
        "Fine tune",
        (-100.0, 100.0, 0.0),
        Lin,
        Unit::Cents,
        false,
    ),
    param(
        P_RANDOM,
        "pitch.random",
        "Pitch random",
        (0.0, 1_200.0, 0.0),
        Lin,
        Unit::Cents,
        false,
    ),
    param(
        P_KEY_TRACK,
        "pitch.key_track",
        "Key tracking",
        (0.0, 1.0, 1.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_AMP_A,
        "env.amp.attack",
        "Amp attack",
        (ENV_MS.0, ENV_MS.1, 30.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_AMP_D,
        "env.amp.decay",
        "Amp decay",
        (ENV_MS.0, ENV_MS.1, 400.0),
        Exp,
        Unit::Ms,
        false,
    ),
    param(
        P_AMP_S,
        "env.amp.sustain",
        "Amp sustain",
        (0.0, 1.0, 1.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_AMP_R,
        "env.amp.release",
        "Amp release",
        (ENV_MS.0, ENV_MS.1, 400.0),
        Exp,
        Unit::Ms,
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
        P_FILTER_ON,
        "filter.enabled",
        "Filter on",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Toggle,
        true,
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
        P_FILTER_KEY,
        "filter.key_track",
        "Key tracking",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_FILTER_ENV,
        "filter.env_amount",
        "Filter env amount",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_MOD_A,
        "env.mod.attack",
        "Mod attack",
        (ENV_MS.0, ENV_MS.1, 5.0),
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
        P_MOD_POSITION,
        "env.mod.to_position",
        "Mod env to position",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_MOD_SIZE,
        "env.mod.to_size",
        "Mod env to size",
        (-2.0, 2.0, 0.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_MOD_DENSITY,
        "env.mod.to_density",
        "Mod env to density",
        (-2.0, 2.0, 0.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_MOD_PITCH,
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
        (0.05, 30.0, 0.5),
        Exp,
        Unit::Hz,
        false,
    ),
    param(
        P_LFO_POSITION,
        "lfo.to_position",
        "LFO to position",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_LFO_SIZE,
        "lfo.to_size",
        "LFO to size",
        (-2.0, 2.0, 0.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_LFO_DENSITY,
        "lfo.to_density",
        "LFO to density",
        (-2.0, 2.0, 0.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_LFO_PITCH,
        "lfo.to_pitch",
        "LFO to pitch",
        (-12.0, 12.0, 0.0),
        Lin,
        Unit::Semitones,
        false,
    ),
    param(
        P_LFO_CUTOFF,
        "lfo.to_cutoff",
        "LFO to cutoff",
        (-4.0, 4.0, 0.0),
        Lin,
        Unit::Octaves,
        false,
    ),
    param(
        P_WHEEL_POSITION,
        "wheel.to_position",
        "Wheel to position",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_WHEEL_SPRAY,
        "wheel.to_spray",
        "Wheel to spray",
        (0.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_CV_PITCH,
        "cv.pitch_amount",
        "CV to pitch",
        (0.0, 24.0, 12.0),
        Lin,
        Unit::Semitones,
        false,
    ),
    param(
        P_CV_POSITION,
        "cv.position_amount",
        "CV to position",
        (0.0, 1.0, 0.5),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_CV_DENSITY,
        "cv.density_amount",
        "CV to density",
        (0.0, 4.0, 2.0),
        Lin,
        Unit::Octaves,
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
        P_CV_LEVEL,
        "master.level_cv_trim",
        "Level CV trim",
        (-1.0, 1.0, 1.0),
        Lin,
        Unit::None,
        false,
    ),
];

const NOTES_IN: SocketId = SocketId(0);
const PITCH_CV_IN: SocketId = SocketId(1);
const POSITION_CV_IN: SocketId = SocketId(2);
const DENSITY_CV_IN: SocketId = SocketId(3);
const CUTOFF_CV_IN: SocketId = SocketId(4);
const LEVEL_CV_IN: SocketId = SocketId(5);
const OUT_L: SocketId = SocketId(6);
const OUT_R: SocketId = SocketId(7);

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
        POSITION_CV_IN,
        "position_cv_in",
        "Position CV",
        SocketKind::Cv,
        Direction::Input,
    ),
    socket(
        DENSITY_CV_IN,
        "density_cv_in",
        "Density CV",
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
];

/// Static description of the granular sampler.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.granular",
    name: "Granular Sampler",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Granular`] (register it with `register_with_samples`).
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Granular::new()),
    }
}

/// Samples between control-rate updates of a voice.
const CONTROL_INTERVAL: u32 = 16;
/// Pitch bend range in semitones each way.
const BEND_RANGE: f32 = 2.0;
/// Fixed range of the level CV in dB (times `master.level_cv_trim`).
const CV_LEVEL_RANGE_DB: f32 = 24.0;
/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Fade-out of a voice whose map is draining.
const DRAIN_FADE_MS: f32 = 5.0;
/// The shortest and longest grain after modulation, in ms.
const MIN_SIZE_MS: f32 = 1.0;
const MAX_SIZE_MS: f32 = 2_000.0;
/// The lowest and highest grain density after modulation, in grains per second.
const MIN_DENSITY: f32 = 0.25;
const MAX_DENSITY: f32 = 400.0;
/// The slowest grain read (a step of 1/32 is five octaves down).
const MIN_STEP: f64 = 1.0 / 32.0;
/// The power of a Hann window, the reference of the overlap scaling.
const HANN_POWER: f32 = 0.375;
/// Highest filter cutoff as a fraction of the sample rate.
const MAX_CUTOFF_RATIO: f32 = 0.45;

/// Shared (per-device) parameters, as plain values.
#[derive(Debug, Clone, Copy)]
struct Shared {
    sample_rate: f32,
    density: f32,
    size_ms: f32,
    window: Window,
    timing: f32,
    reverse: f32,
    spread: f32,
    limit: usize,
    position: f32,
    spray: f32,
    scan: f32,
    transpose: f32,
    fine: f32,
    random_cents: f32,
    key_track: f32,
    velocity_amount: f32,
    filter_on: bool,
    filter_mode: FilterMode,
    cutoff: f32,
    k: f32,
    filter_key: f32,
    filter_env: f32,
    env_position: f32,
    env_size: f32,
    env_density: f32,
    env_pitch: f32,
    lfo_position: f32,
    lfo_size: f32,
    lfo_density: f32,
    lfo_pitch: f32,
    lfo_cutoff: f32,
    wheel_position: f32,
    wheel_spray: f32,
    cv_pitch: f32,
    cv_position: f32,
    cv_density: f32,
    cv_cutoff: f32,
    cv_level_db: f32,
    /// Pitch bend in semitones.
    bend: f32,
    /// Mod wheel (CC 1), `0..=1`.
    wheel: f32,
}

impl Shared {
    fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            density: 24.0,
            size_ms: 90.0,
            window: Window::Hann,
            timing: 0.3,
            reverse: 0.0,
            spread: 0.5,
            limit: 48,
            position: 0.2,
            spray: 0.08,
            scan: 0.0,
            transpose: 0.0,
            fine: 0.0,
            random_cents: 0.0,
            key_track: 1.0,
            velocity_amount: 0.5,
            filter_on: false,
            filter_mode: FilterMode::Lowpass,
            cutoff: 8_000.0,
            k: 1.0,
            filter_key: 0.0,
            filter_env: 0.0,
            env_position: 0.0,
            env_size: 0.0,
            env_density: 0.0,
            env_pitch: 0.0,
            lfo_position: 0.0,
            lfo_size: 0.0,
            lfo_density: 0.0,
            lfo_pitch: 0.0,
            lfo_cutoff: 0.0,
            wheel_position: 0.0,
            wheel_spray: 0.5,
            cv_pitch: 12.0,
            cv_position: 0.5,
            cv_density: 2.0,
            cv_cutoff: 2.0,
            cv_level_db: CV_LEVEL_RANGE_DB,
            bend: 0.0,
            wheel: 0.0,
        }
    }
}

/// Per-sample modulation sources of one render segment (aligned with the output slices).
struct Mods<'a> {
    lfo: &'a [f32],
    pitch: &'a [f32],
    position: &'a [f32],
    density: &'a [f32],
    cutoff: &'a [f32],
}

/// Which map a voice reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Current,
    Draining,
}

/// Where a voice's audio is: the device's built-in source, or a buffer of a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SrcRef {
    Builtin,
    Map(u32),
}

/// A zone chosen for a note, or the built-in source's whole-buffer zone.
#[derive(Debug, Clone, Copy)]
struct Selection {
    src: SrcRef,
    zone_idx: u32,
    zone: Zone,
}

/// The zone of the built-in source: the whole buffer on every key, rooted at middle C.
fn builtin_zone(frames: u64) -> Zone {
    Zone {
        buffer: 0,
        key_lo: 0,
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root_key: grain::DEFAULT_SOURCE_ROOT,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: 0.0,
        pan: 0.0,
        start: 0,
        end: frames,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::Gate,
        reverse: false,
    }
}

/// The audio a render segment can read.
#[derive(Clone, Copy)]
struct Banks<'a> {
    current: Option<&'a SampleMap>,
    draining: Option<&'a SampleMap>,
    builtin: &'a SampleBuffer,
}

impl<'a> Banks<'a> {
    /// The zone a note plays: the first match of the current map, or the built-in source when
    /// there is no map (a map without a match is silent).
    fn select(&self, key: u8, vel7: u8) -> Option<Selection> {
        match self.current {
            None => Some(Selection {
                src: SrcRef::Builtin,
                zone_idx: 0,
                zone: builtin_zone(self.builtin.frames),
            }),
            Some(map) => map
                .zones()
                .iter()
                .enumerate()
                .find(|(_, z)| z.contains(key, vel7))
                .map(|(i, z)| Selection {
                    src: SrcRef::Map(z.buffer),
                    zone_idx: i as u32,
                    zone: *z,
                }),
        }
    }

    /// The planar audio of `src` in `slot`.
    fn data(&self, src: SrcRef, slot: Slot) -> Option<&'a [Vec<f32>]> {
        match src {
            SrcRef::Builtin => Some(&self.builtin.data),
            SrcRef::Map(b) => {
                let map = match slot {
                    Slot::Current => self.current,
                    Slot::Draining => self.draining,
                }?;
                map.buffers().get(b as usize).map(|buf| &buf.data[..])
            }
        }
    }
}

/// A note waiting for its voice's steal fade to end.
#[derive(Debug, Clone, Copy)]
struct Pending {
    key: u8,
    velocity: f32,
}

/// The modulated values of a voice, refreshed every [`CONTROL_INTERVAL`] samples.
#[derive(Debug, Clone, Copy)]
struct Control {
    /// Grains per sample.
    rate: f64,
    /// Grain length in samples.
    len: u32,
    /// The position before the scan, `0..=1`.
    position: f32,
    /// The spray width, `0..=1`.
    spray: f32,
    /// The pitch of a grain in semitones, before its random offset.
    semis: f32,
    /// The level of one grain.
    norm: f32,
}

/// The most recent grain starts, for the panel.
#[derive(Debug, Clone, Copy)]
struct TrailEntry {
    /// The device frame the grain started at (`u64::MAX`: no grain).
    born: u64,
    len: u32,
    /// Start and speed as fractions of the window (per frame).
    start: f32,
    step: f32,
}

const NO_GRAIN: TrailEntry = TrailEntry {
    born: u64::MAX,
    len: 0,
    start: 0.0,
    step: 0.0,
};

#[derive(Debug, Clone)]
struct Trail {
    entries: [TrailEntry; TRAIL_GRAINS],
    head: usize,
}

impl Trail {
    fn new() -> Self {
        Self {
            entries: [NO_GRAIN; TRAIL_GRAINS],
            head: 0,
        }
    }

    fn clear(&mut self) {
        *self = Self::new();
    }

    fn push(&mut self, e: TrailEntry) {
        self.entries[self.head] = e;
        self.head = (self.head + 1) % TRAIL_GRAINS;
    }
}

/// The newest voice's view for the panel.
#[derive(Debug, Clone, Copy)]
struct Display {
    valid: bool,
    order: u64,
    position: f32,
    spray: f32,
    /// 0 for the built-in source, else the zone number plus 1.
    source: f32,
}

/// What a render segment hands every voice besides the audio.
struct Frame<'a> {
    windows: &'a WindowBank,
    trail: &'a mut Trail,
    display: &'a mut Display,
    /// The device frame of the first sample of the segment.
    frame0: u64,
    seed: u64,
    #[cfg(test)]
    probe: Option<&'a tests::Probe>,
}

/// One preallocated voice: one note.
#[derive(Debug, Clone)]
struct Voice {
    key: u8,
    order: u64,
    slot: Slot,
    sel: Selection,
    stereo: bool,
    velocity: f32,
    vel_gain: f32,
    zone_gain: f32,
    pending: Option<Pending>,
    pool: GrainPool,
    clock: GrainClock,
    rng: Rng,
    /// The scan, as a fraction of the window moved since the note began.
    scan_phase: f64,
    scan_step: f64,
    ctl: Control,
    filters: [Svf; 2],
    filter_ready: bool,
    amp: Adsr,
    menv: Adsr,
    countdown: u32,
    g: f32,
    g_target: f32,
    g_step: f32,
    /// Samples left of the drain fade (0: not draining) and its length.
    fade_left: u32,
    fade_len: f32,
}

impl Voice {
    fn new() -> Self {
        Self {
            key: 0,
            order: 0,
            slot: Slot::Current,
            sel: Selection {
                src: SrcRef::Builtin,
                zone_idx: 0,
                zone: builtin_zone(0),
            },
            stereo: false,
            velocity: 1.0,
            vel_gain: 1.0,
            zone_gain: 1.0,
            pending: None,
            pool: GrainPool::new(),
            clock: GrainClock::new(),
            rng: Rng::new(0),
            scan_phase: 0.0,
            scan_step: 0.0,
            ctl: Control {
                rate: 0.0,
                len: 1,
                position: 0.0,
                spray: 0.0,
                semis: 0.0,
                norm: 1.0,
            },
            filters: [Svf::new(); 2],
            filter_ready: false,
            amp: Adsr::new(48_000.0),
            menv: Adsr::new(48_000.0),
            countdown: 0,
            g: 0.0,
            g_target: 0.0,
            g_step: 0.0,
            fade_left: 0,
            fade_len: 1.0,
        }
    }

    fn is_active(&self) -> bool {
        self.amp.is_active()
    }

    /// Stops the voice at once (no fade) and drops a pending note and its grains.
    fn kill(&mut self) {
        self.amp.reset();
        self.menv.reset();
        self.pending = None;
        self.fade_left = 0;
        self.pool.clear();
    }

    /// Takes the zone of `sel`: its gain and the window.
    fn take(&mut self, sel: Selection) {
        self.sel = sel;
        self.zone_gain = db_to_gain(sel.zone.gain_db);
    }

    /// Starts a note from silence (fresh grains, filters and envelopes).
    fn start(&mut self, sel: Selection, key: u8, velocity: f32, p: &Shared, seed: u64) {
        self.take(sel);
        let velocity = if velocity.is_nan() {
            0.0
        } else {
            velocity.clamp(0.0, 1.0)
        };
        self.key = key;
        self.slot = Slot::Current;
        self.pending = None;
        self.velocity = velocity;
        self.vel_gain = 1.0 - p.velocity_amount * (1.0 - velocity);
        self.pool.clear();
        self.pool.set_limit(p.limit);
        self.clock.reset();
        self.rng = Rng::new(hash64(seed, self.order, 0x6772_6169));
        self.scan_phase = 0.0;
        for flt in &mut self.filters {
            flt.reset();
        }
        self.amp.reset();
        self.menv.reset();
        self.amp.gate_on();
        self.menv.gate_on();
        self.filter_ready = false;
        self.countdown = 0;
        self.fade_left = 0;
    }

    /// Moves the voice onto the zone `sel` of a changed map that shares its buffer.
    fn rebind(&mut self, sel: Selection) {
        self.take(sel);
    }

    /// Starts the 5 ms fade-out of a voice that keeps playing from a draining map.
    fn start_drain(&mut self, sample_rate: f32) {
        let n = (DRAIN_FADE_MS * 0.001 * sample_rate).round().max(1.0);
        self.slot = Slot::Draining;
        self.fade_len = n;
        self.fade_left = n as u32;
    }

    fn release(&mut self) {
        self.pending = None;
        self.amp.gate_off();
        self.menv.gate_off();
    }

    /// Starts the 2 ms steal fade; the new note starts when it ends.
    fn steal(&mut self, pending: Pending) {
        self.pending = Some(pending);
        self.amp.steal_fade();
    }

    /// The window of the zone, in frames of its buffer.
    fn window(&self) -> (u64, u64) {
        (self.sel.zone.start, self.sel.zone.end)
    }

    /// The position grains centre on: the position plus the scan, wrapped round the window.
    fn centre(&self) -> f64 {
        let c = f64::from(self.ctl.position) + self.scan_phase;
        if (0.0..=1.0).contains(&c) {
            c
        } else {
            c.rem_euclid(1.0)
        }
    }

    /// Control-rate update: the grain rate, size, pitch and level, the filter, the panel view.
    fn control(&mut self, m: &Mods<'_>, n: usize, p: &Shared, f: &mut Frame<'_>) {
        let lfo = m.lfo[n];
        let menv = self.menv.level();
        let z = &self.sel.zone;
        let position = (p.position
            + lfo * p.lfo_position
            + menv * p.env_position
            + p.wheel * p.wheel_position
            + clean(m.position[n]) * p.cv_position)
            .clamp(0.0, 1.0);
        let spray = (p.spray + p.wheel * p.wheel_spray).clamp(0.0, 1.0);
        let size_ms = (p.size_ms * exp2_det(lfo * p.lfo_size + menv * p.env_size))
            .clamp(MIN_SIZE_MS, MAX_SIZE_MS);
        let density = (p.density
            * exp2_det(
                lfo * p.lfo_density + menv * p.env_density + clean(m.density[n]) * p.cv_density,
            ))
        .clamp(MIN_DENSITY, MAX_DENSITY);
        let semis = p.key_track * (f32::from(self.key) - f32::from(z.root_key))
            + f32::from(z.transpose)
            + z.tune_cents * 0.01
            + p.transpose
            + p.fine * 0.01
            + p.bend
            + lfo * p.lfo_pitch
            + menv * p.env_pitch
            + clean(m.pitch[n]) * p.cv_pitch;
        let sr = p.sample_rate;
        let overlap = (density * size_ms * 0.001 * f.windows.mean_square(p.window) / HANN_POWER)
            .min(p.limit as f32);
        self.ctl = Control {
            rate: f64::from(density) / f64::from(sr),
            len: ((size_ms * 0.001 * sr).round() as u32).max(1),
            position,
            spray,
            semis,
            norm: 1.0 / overlap.max(1.0).sqrt(),
        };
        let (start, end) = self.window();
        let frames = end.saturating_sub(start).max(1) as f64;
        self.scan_step = f64::from(p.scan) / frames;
        self.scan_phase = self.scan_phase.rem_euclid(1.0);
        if !f.display.valid || self.order >= f.display.order {
            *f.display = Display {
                valid: true,
                order: self.order,
                position: self.centre() as f32,
                spray,
                source: match self.sel.src {
                    SrcRef::Builtin => 0.0,
                    SrcRef::Map(_) => (self.sel.zone_idx + 1) as f32,
                },
            };
        }
        self.filter_control(lfo, menv, clean(m.cutoff[n]), p);
    }

    /// The filter coefficient target.
    fn filter_control(&mut self, lfo: f32, menv: f32, cv_cutoff: f32, p: &Shared) {
        if !p.filter_on {
            self.filter_ready = false;
            self.g_step = 0.0;
            return;
        }
        let octaves = p.filter_env * 6.0 * menv
            + p.filter_key * (f32::from(self.key) - 60.0) / 12.0
            + p.lfo_cutoff * lfo
            + p.cv_cutoff * cv_cutoff;
        let cutoff = (p.cutoff * exp2_det(octaves)).clamp(20.0, MAX_CUTOFF_RATIO * p.sample_rate);
        let g = Svf::cutoff_to_g(cutoff, p.sample_rate);
        let snap = !self.filter_ready;
        self.filter_ready = true;
        if snap {
            for flt in &mut self.filters {
                flt.reset();
            }
        }
        // Land exactly on the previous target (no drift), then ramp towards the new one.
        self.g = if snap { g } else { self.g_target };
        self.g_target = g;
        self.g_step = (g - self.g) / CONTROL_INTERVAL as f32;
        if snap || self.g_step == 0.0 {
            for flt in &mut self.filters {
                flt.set_coefficients(self.g, p.k);
            }
        }
    }

    /// Starts one grain from the window `src` at output sample `n` of the segment.
    fn spawn(&mut self, src: &GrainSource<'_>, n: usize, p: &Shared, f: &mut Frame<'_>) {
        let window = src.len() as f64;
        if window < 1.0 {
            return;
        }
        let c = self.ctl;
        let spray = f64::from(self.rng.next_bipolar()) * 0.5 * f64::from(c.spray);
        let random = self.rng.next_bipolar() * p.random_cents;
        let reverse = self.rng.next_f32() < p.reverse;
        let pan = (self.sel.zone.pan + p.spread * self.rng.next_bipolar()).clamp(-1.0, 1.0);
        let fraction = fold01(self.centre() + spray);
        let ratio = f64::from(exp2_det((c.semis + random * 0.01) / 12.0)).clamp(MIN_STEP, MAX_STEP);
        let start = fit_start(fraction * window, f64::from(c.len) * ratio, window, reverse);
        let step = if reverse { -ratio } else { ratio };
        let (gl, gr) = if self.stereo {
            balance_gains(pan)
        } else {
            pan_gains_mono(pan)
        };
        let level = c.norm * self.zone_gain;
        #[cfg(test)]
        let stolen_before = self.pool.stolen();
        self.pool.spawn(GrainParams {
            pos: start,
            step,
            len: c.len,
            window: p.window,
            gain: [gl * level, gr * level],
        });
        f.trail.push(TrailEntry {
            born: f.frame0 + n as u64,
            len: c.len,
            start: (start / window) as f32,
            step: (step / window) as f32,
        });
        #[cfg(test)]
        if let Some(probe) = f.probe {
            probe.on_spawn(
                fraction as f32,
                c.len,
                c.semis + random * 0.01,
                reverse,
                self.pool.stolen() > stolen_before,
            );
        }
    }

    /// Adds this voice into `out` (two slices of the segment's length, aligned with `m`).
    fn render(
        &mut self,
        banks: &Banks<'_>,
        out: [&mut [f32]; 2],
        m: &Mods<'_>,
        p: &Shared,
        f: &mut Frame<'_>,
    ) {
        let [out_l, out_r] = out;
        let Some(mut data) = banks.data(self.sel.src, self.slot) else {
            self.kill();
            return;
        };
        self.stereo = data.len() >= 2;
        let mut src = source_of(data, &self.sel.zone);
        for n in 0..out_l.len() {
            if self.countdown == 0 {
                self.control(m, n, p, f);
                self.countdown = CONTROL_INTERVAL;
            }
            self.countdown -= 1;
            if p.filter_on && self.g_step != 0.0 {
                self.g += self.g_step;
                for flt in &mut self.filters {
                    flt.set_coefficients(self.g, p.k);
                }
            }
            self.scan_phase += self.scan_step;
            if self.clock.tick(self.ctl.rate, p.timing, &mut self.rng) {
                self.spawn(&src, n, p, f);
            }
            let (xl, xr) = self.pool.process(f.windows, &src);
            let (mut yl, mut yr) = (flush_denormal(xl), flush_denormal(xr));
            if p.filter_on {
                yl = self.filters[0].process_mode(yl, p.filter_mode);
                yr = self.filters[1].process_mode(yr, p.filter_mode);
                if !(yl.is_finite() && yr.is_finite()) {
                    self.filters.iter_mut().for_each(Svf::reset);
                    (yl, yr) = (0.0, 0.0);
                }
            }
            self.menv.next_sample();
            let amp = self.amp.next_sample();
            let fade = if self.fade_left > 0 {
                self.fade_left as f32 / self.fade_len
            } else {
                1.0
            };
            let g = amp * self.vel_gain * fade;
            out_l[n] += yl * g;
            out_r[n] += yr * g;
            if self.fade_left > 0 {
                self.fade_left -= 1;
                if self.fade_left == 0 {
                    self.amp.reset();
                }
            }
            if !self.amp.is_active() {
                let started = self.pending.take().and_then(|pe| {
                    banks
                        .select(pe.key, velocity_to_7bit(pe.velocity))
                        .map(|sel| (sel, pe))
                });
                match started {
                    Some((sel, pe)) => {
                        self.start(sel, pe.key, pe.velocity, p, f.seed);
                        match banks.data(sel.src, Slot::Current) {
                            Some(d) => {
                                data = d;
                                self.stereo = data.len() >= 2;
                                src = source_of(data, &self.sel.zone);
                            }
                            None => {
                                self.kill();
                                return;
                            }
                        }
                    }
                    None => {
                        self.kill();
                        return;
                    }
                }
            }
        }
    }
}

/// The grain source over the window of `zone` in `data`.
fn source_of<'a>(data: &'a [Vec<f32>], zone: &Zone) -> GrainSource<'a> {
    match data {
        [l, r, ..] => GrainSource::new(l, r, zone.start, zone.end),
        [m] => GrainSource::mono(m, zone.start, zone.end),
        [] => GrainSource::mono(&[], 0, 0),
    }
}

/// The built-in source at `sample_rate`, made once per rate (non-real-time).
fn builtin_buffer(sample_rate: f32) -> Arc<SampleBuffer> {
    static CACHE: OnceLock<Mutex<BTreeMap<u32, Arc<SampleBuffer>>>> = OnceLock::new();
    let key = sample_rate.round().max(8_000.0) as u32;
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    Arc::clone(cache.entry(key).or_insert_with(|| {
        let mono = grain::default_source(key as f32);
        let data: Arc<[Vec<f32>]> = Arc::from(vec![mono]);
        // A single channel is never ragged or empty.
        Arc::new(SampleBuffer {
            frames: data[0].len() as u64,
            channels: 1,
            data,
        })
    }))
}

/// The granular sampler device.
#[derive(Debug)]
pub struct Granular {
    values: [f32; PARAM_COUNT],
    shared: Shared,
    voices: Vec<Voice>,
    voice_limit: usize,
    next_order: u64,
    lfo: Lfo,
    master: Smoother,
    /// Set by `prepare`/`reset`: the next `process` snaps the master gain to its target.
    fresh: bool,
    seed: u64,
    windows: WindowBank,
    builtin: Arc<SampleBuffer>,
    current: Option<SampleMapRef>,
    draining: Option<SampleMapRef>,
    trail: Trail,
    display: Display,
    /// Frames processed since `reset`.
    frames: u64,
    grains_active: u32,
    voices_active: u32,
    #[cfg(test)]
    probe: Option<Arc<tests::Probe>>,
}

impl Default for Granular {
    fn default() -> Self {
        Self::new()
    }
}

impl Granular {
    /// A granular sampler with default params and no map (call `prepare` before processing).
    pub fn new() -> Self {
        let mut s = Self {
            values: PARAMS.map(|p| p.default),
            shared: Shared::new(),
            voices: vec![Voice::new(); MAX_VOICES],
            voice_limit: 8,
            next_order: 0,
            lfo: Lfo::new(derive_seed(0, 0)),
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            fresh: true,
            seed: 0,
            windows: WindowBank::new(),
            builtin: builtin_buffer(48_000.0),
            current: None,
            draining: None,
            trail: Trail::new(),
            display: Display {
                valid: false,
                order: 0,
                position: NONE,
                spray: NONE,
                source: 0.0,
            },
            frames: 0,
            grains_active: 0,
            voices_active: 0,
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
        let s = &mut self.shared;
        match id {
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
            P_DENSITY => s.density = v,
            P_SIZE => s.size_ms = v,
            P_WINDOW => s.window = Window::from_index(choice(v)),
            P_TIMING => s.timing = v,
            P_REVERSE => s.reverse = v,
            P_SPREAD => s.spread = v,
            P_LIMIT => {
                s.limit = (v.round().max(1.0) as usize).min(MAX_GRAINS);
                voices.iter_mut().for_each(|vc| vc.pool.set_limit(s.limit));
            }
            P_POSITION => s.position = v,
            P_SPRAY => s.spray = v,
            P_SCAN => s.scan = v,
            P_TRANSPOSE => s.transpose = v,
            P_FINE => s.fine = v,
            P_RANDOM => s.random_cents = v,
            P_KEY_TRACK => s.key_track = v,
            P_AMP_A => voices.iter_mut().for_each(|vc| vc.amp.set_attack_ms(v)),
            P_AMP_D => voices.iter_mut().for_each(|vc| vc.amp.set_decay_ms(v)),
            P_AMP_S => voices.iter_mut().for_each(|vc| vc.amp.set_sustain(v)),
            P_AMP_R => voices.iter_mut().for_each(|vc| vc.amp.set_release_ms(v)),
            P_VEL_AMP => s.velocity_amount = v,
            P_FILTER_ON => s.filter_on = v >= 0.5,
            P_FILTER_MODE => s.filter_mode = FilterMode::from_index(choice(v)),
            P_CUTOFF => s.cutoff = v,
            P_RESONANCE => s.k = 1.0 / Svf::resonance_to_q(v),
            P_FILTER_KEY => s.filter_key = v,
            P_FILTER_ENV => s.filter_env = v,
            P_MOD_A => voices.iter_mut().for_each(|vc| vc.menv.set_attack_ms(v)),
            P_MOD_D => voices.iter_mut().for_each(|vc| vc.menv.set_decay_ms(v)),
            P_MOD_S => voices.iter_mut().for_each(|vc| vc.menv.set_sustain(v)),
            P_MOD_R => voices.iter_mut().for_each(|vc| vc.menv.set_release_ms(v)),
            P_MOD_POSITION => s.env_position = v,
            P_MOD_SIZE => s.env_size = v,
            P_MOD_DENSITY => s.env_density = v,
            P_MOD_PITCH => s.env_pitch = v,
            P_LFO_WAVE => self.lfo.set_shape(LfoShape::from_index(choice(v))),
            P_LFO_RATE => self.lfo.set_rate(v, sr),
            P_LFO_POSITION => s.lfo_position = v,
            P_LFO_SIZE => s.lfo_size = v,
            P_LFO_DENSITY => s.lfo_density = v,
            P_LFO_PITCH => s.lfo_pitch = v,
            P_LFO_CUTOFF => s.lfo_cutoff = v,
            P_WHEEL_POSITION => s.wheel_position = v,
            P_WHEEL_SPRAY => s.wheel_spray = v,
            P_CV_PITCH => s.cv_pitch = v,
            P_CV_POSITION => s.cv_position = v,
            P_CV_DENSITY => s.cv_density = v,
            P_CV_CUTOFF => s.cv_cutoff = v,
            P_CV_LEVEL => s.cv_level_db = v * CV_LEVEL_RANGE_DB,
            _ => {}
        }
    }

    /// Starts a voice for the first zone that matches the note (or the built-in source).
    fn note_on(&mut self, key: u8, velocity: f32) {
        let banks = Banks {
            current: self.current.as_deref(),
            draining: None,
            builtin: &self.builtin,
        };
        let Some(sel) = banks.select(key, velocity_to_7bit(velocity)) else {
            return;
        };
        let p = self.shared;
        let order = self.next_order;
        self.next_order += 1;
        let voices = &mut self.voices[..self.voice_limit];
        // A free voice, else steal the oldest started voice (2 ms fade, then the note).
        if let Some(v) = voices.iter_mut().find(|v| !v.is_active()) {
            v.order = order;
            v.start(sel, key, velocity, &p, self.seed);
        } else if let Some(v) = voices.iter_mut().min_by_key(|v| v.order) {
            v.order = order;
            v.steal(Pending { key, velocity });
        }
    }

    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.is_active()) {
            if v.pending.is_some_and(|pe| pe.key == key) {
                v.pending = None;
            } else if v.pending.is_none() && v.key == key {
                v.release();
            }
        }
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
            NoteKind::Off { key } => self.note_off(key),
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
}

impl Device for Granular {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.shared.sample_rate = sr;
        self.builtin = builtin_buffer(sr);
        if self.voices.len() < MAX_VOICES {
            self.voices.resize(MAX_VOICES, Voice::new());
        }
        for v in &mut self.voices {
            v.amp.set_sample_rate(sr);
            v.menv.set_sample_rate(sr);
        }
        self.master.set_time(MASTER_SMOOTH_MS, sr);
        self.lfo = Lfo::new(derive_seed(info.seed, 0));
        self.apply_all();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.kill();
            for f in &mut v.filters {
                f.reset();
            }
            v.filter_ready = false;
        }
        self.next_order = 0;
        self.lfo.reset();
        let m = self.master.target();
        self.master.set_immediate(m);
        self.fresh = true;
        self.trail.clear();
        self.display.valid = false;
        self.frames = 0;
        self.grains_active = 0;
        self.voices_active = 0;
        self.shared.bend = 0.0;
        self.shared.wheel = 0.0;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let i = id.0 as usize;
        if i < PARAM_COUNT {
            self.values[i] = plain;
            self.apply(id.0, plain);
        }
    }

    fn swap_samples(&mut self, map: Option<SampleMapRef>) -> Option<SampleMapRef> {
        let sr = self.shared.sample_rate;
        // 1. Voices still draining from the draining map are cut, which frees that map.
        for v in &mut self.voices {
            if v.is_active() && v.slot == Slot::Draining {
                v.kill();
            }
        }
        let stale = self.draining.take();
        // 2. Voices on the old current map move to the new map or start draining.
        let old = self.current.take();
        let mut drains = false;
        if let Some(old_map) = old.as_deref() {
            for v in self.voices.iter_mut().filter(|v| v.is_active()) {
                let SrcRef::Map(buffer) = v.sel.src else {
                    continue;
                };
                if v.slot != Slot::Current {
                    continue;
                }
                let target = map.as_deref().and_then(|new_map| {
                    let nz = new_map.zones().get(v.sel.zone_idx as usize)?;
                    let old_buf = old_map.buffers().get(buffer as usize)?;
                    Arc::ptr_eq(old_buf, new_map.buffer_of(nz)).then_some(Selection {
                        src: SrcRef::Map(nz.buffer),
                        zone_idx: v.sel.zone_idx,
                        zone: *nz,
                    })
                });
                match target {
                    Some(sel) => v.rebind(sel),
                    None => {
                        v.start_drain(sr);
                        drains = true;
                    }
                }
            }
        }
        self.current = map;
        // 3./4. Keep at most one extra map; hand back exactly one (or none).
        if drains || stale.is_some() {
            self.draining = old;
            stale
        } else {
            old
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            let m = self.master.target();
            self.master.set_immediate(m);
            self.fresh = false;
        }
        self.display.valid = false;
        let mut lfo = [0.0f32; BATCH];
        for s in &mut lfo {
            *s = self.lfo.next_sample();
        }
        let cv_pitch = *ctx.input(PITCH_CV_IN);
        let cv_position = *ctx.input(POSITION_CV_IN);
        let cv_density = *ctx.input(DENSITY_CV_IN);
        let cv_cutoff = *ctx.input(CUTOFF_CV_IN);
        let mut mix = [[0.0f32; BATCH]; 2];
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
                position: &cv_position[start..end],
                density: &cv_density[start..end],
                cutoff: &cv_cutoff[start..end],
            };
            let Self {
                voices,
                current,
                draining,
                builtin,
                windows,
                trail,
                display,
                frames,
                seed,
                #[cfg(test)]
                probe,
                ..
            } = self;
            let banks = Banks {
                current: current.as_deref(),
                draining: draining.as_deref(),
                builtin,
            };
            let mut f = Frame {
                windows,
                trail,
                display,
                frame0: *frames + start as u64,
                seed: *seed,
                #[cfg(test)]
                probe: probe.as_deref(),
            };
            let [mix_l, mix_r] = &mut mix;
            for v in voices.iter_mut().filter(|v| v.is_active()) {
                v.render(
                    &banks,
                    [&mut mix_l[start..end], &mut mix_r[start..end]],
                    &m,
                    &p,
                    &mut f,
                );
            }
            start = end;
        }
        let level_on = ctx.is_connected(LEVEL_CV_IN);
        let cv_level = *ctx.input(LEVEL_CV_IN);
        let [mix_l, mix_r] = &mut mix;
        for (n, (l, r)) in mix_l.iter_mut().zip(mix_r.iter_mut()).enumerate() {
            let mut m = self.master.next_value();
            if level_on {
                m *= db_gain(self.shared.cv_level_db, cv_level[n]);
            }
            *l = soft_limit(*l * m);
            *r = soft_limit(*r * m);
        }
        *ctx.output(OUT_L) = mix[0];
        *ctx.output(OUT_R) = mix[1];
        self.frames += BATCH as u64;
        let (mut grains, mut voices) = (0u32, 0u32);
        for v in self.voices.iter().filter(|v| v.is_active()) {
            voices += 1;
            grains += v.pool.active() as u32;
        }
        self.grains_active = grains;
        self.voices_active = voices;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = METER_COUNT.min(out.len());
        let mut v = [NONE; METER_COUNT];
        v[0] = if self.display.valid {
            self.display.source
        } else {
            f32::from(self.current.is_some())
        };
        if self.display.valid {
            v[1] = self.display.position;
            v[2] = self.display.spray;
        }
        v[3] = self.grains_active as f32;
        v[4] = self.voices_active as f32;
        for (i, e) in self.trail.entries.iter().enumerate() {
            let age = self.frames.saturating_sub(e.born);
            if e.born == u64::MAX || e.born > self.frames || age >= u64::from(e.len) {
                continue;
            }
            let pos = f64::from(e.start) + f64::from(e.step) * age as f64;
            v[5 + 2 * i] = pos.rem_euclid(1.0) as f32;
            v[6 + 2 * i] = (age as f32 / e.len as f32).clamp(0.0, 1.0);
        }
        out[..n].copy_from_slice(&v[..n]);
        n
    }
}

#[cfg(test)]
mod tests;
