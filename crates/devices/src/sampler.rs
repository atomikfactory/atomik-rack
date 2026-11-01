//! `atomik.sampler`: multi-zone sampler on the shared sample pool.
//!
//! The device plays a [`SampleMap`] (shared buffers plus zones) that the non-RT side installs
//! with [`Device::swap_samples`]. Without a map, or without a zone that matches the note, it is
//! silent.
//!
//! * **Zones and voices.** A note-on starts one voice per matching zone (key and velocity
//!   range), so overlapping zones layer. Voices (1–64, preallocated at the maximum) play at the
//!   ratio `2^((key − root + transpose + tune.semi)/12 + (tune_cents + tune.fine)/1200 +
//!   (lfo·lfo.to_pitch + cv·cv.pitch_amount)/12)`, read with cubic Hermite interpolation from
//!   the playhead of `atomik_dsp::Playhead` (loops, ping-pong, crossfade and edge clamping are
//!   specified there). With the loop off, the last frames before `end` fade linearly to zero
//!   (see `end_fade_len`), so the voice reaches silence at `end` and then goes idle.
//! * **Performance controllers.** Pitch bend adds up to `bend.range` semitones (default 2) to the
//!   pitch of every voice; the sustain pedal holds back the note-off of gate zones; volume and
//!   expression scale the output (see [`crate::controller_state`]).
//! * **Reverse.** A zone with `reverse` plays from `end - 1` down to `start` through the same
//!   playhead (no loop; the end fade of 64 frames or 1.5 ms is at `start`).
//! * **Trigger modes.** Gate zones release on note-off; one-shot zones ignore note-off and play
//!   to `end` (or until stolen or `AllOff`). `AllOff` releases every voice. A note-off for a key
//!   whose note is still pending after a steal cancels that note (gate zones only).
//! * **Stealing.** When every allowed voice is busy, the oldest started voice is stolen: it fades
//!   out over 2 ms, then the new note starts from silence (as the poly-synth does). Voices are
//!   numbered by a start counter, one per zone started, so stealing is deterministic.
//! * **Level.** Voice gain = zone gain · amp ADSR · `(1 − velocity.amp·(1 − velocity))` ·
//!   tremolo `(1 − lfo.to_amp·(1 − lfo)/2)`. A mono sample goes through a constant-power pan
//!   (−3 dB at the centre), a stereo sample is balanced by the zone pan (unity at the centre).
//!   The sum is scaled by `master.level` (5 ms smoothing) and soft limited like the poly-synth.
//! * **Filter.** Off by default (`filter.enabled`); then it costs nothing and the signal is
//!   untouched. When on, each voice has a stereo pair of 12 dB state-variable filters. Cutoff =
//!   `filter.cutoff · 2^(env_amount·6·filter env + key_track·(key − 60)/12 + lfo.to_cutoff·lfo +
//!   cv.cutoff_amount·cv + 4·filter.velocity·(velocity − 1))`, updated every 16 samples with the
//!   coefficient interpolated in between (as the poly-synth).
//! * **Groups, crossfades and round-robin.** A zone belongs to a group (0..32). With a velocity
//!   crossfade it fades in over the first `vel_xfade` velocities above `vel_lo` and out over the
//!   last below `vel_hi` on an equal-power curve (neighbours that overlap by the same width sum
//!   to constant power; a zone at a silent edge starts no voice). A round-robin zone (`rr_len`
//!   above one) sounds when its group-and-key note counter modulo `rr_len` is its `rr_pos`; the counter counts
//!   one per note-on of the key that reaches a round-robin zone of the group and is cleared by `reset`, so
//!   the cycle is the same on every bounce and after every locate.
//! * **Own envelope and filter.** A zone's `env` replaces the amp envelope for its voices, its
//!   `filter` (mode, cutoff, resonance, key tracking, envelope amount; the device filter
//!   envelope) replaces the device filter settings and runs even when `filter.enabled` is off.
//!   A voice takes both when a note starts on a zone or the voice moves onto another zone after
//!   a map swap; the filter state restarts whenever the zone's filter changes, so no state
//!   crosses from one zone to another through stealing or swaps.
//! * **Sample-accurate starts.** Notes start at their event offset.
//!
//! **Swapping maps.** The device holds at most two maps: `current` and `draining`. It
//! never drops a map; [`Sampler::swap_samples`] hands one back instead. On a swap:
//! 1. Voices still playing from the draining map are cut at once (counted in
//!    `cut_voices`), so the draining map is free.
//! 2. A voice on the old current map whose zone index exists in the new map and whose buffer
//!    `Arc` is pointer-equal moves to the new map's zone (it adopts that zone's parameters and
//!    keeps its position). Every other voice keeps playing from the old map with a 5 ms
//!    fade-out ("drains"). A note that is pending after a steal is kept only if the new map
//!    still has a zone at that index that matches it.
//! 3. The old current map becomes the new draining map if a voice drains, or if a stale draining
//!    map has to be returned in the same call (so exactly one map is ever returned). Otherwise
//!    the old current map itself is returned.
//! 4. The returned map is the previous draining map when the old current map becomes draining;
//!    in the remaining case (nobody drains, nothing was draining) it is the replaced map.
//!
//! So a stale draining map (all its voices ended) stays in the device until the next swap hands
//! it back; a device being removed frees its maps off the audio thread with the device itself.

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit, db_to_gain};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, LoopMode, MAX_ZONE_GROUPS,
    PrepareInfo, ProcessContext, SampleMap, SampleMapRef, SocketDescriptor, SocketId, SocketKind,
    TriggerMode, Zone, velocity_to_7bit,
};
use atomik_dsp::{
    Adsr, FilterMode, Lfo, LfoShape, LoopKind, PlayRegion, Playhead, Smoother, Svf, balance_gains,
    pan_gains_mono,
};
use std::sync::Arc;

use crate::controller_state::{ControllerState, DEFAULT_BEND_RANGE, Effect, MAX_BEND_RANGE};
use crate::cv_in::{clean, db_gain};
use crate::poly_synth::{derive_seed, soft_limit};

/// Number of preallocated voices (the highest value of `voices`).
pub const MAX_VOICES: usize = 64;

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

const FILTER_MODES: Unit = Unit::Choice(&["LP12", "BP12", "HP12"]);
const LFO_WAVES: Unit = Unit::Choice(&["sine", "triangle", "saw", "square", "sample&hold"]);
const ENV_MS: (f32, f32) = (0.5, 10_000.0);

const P_VOICES: u32 = 0;
const P_MASTER: u32 = 1;
const P_TUNE_SEMI: u32 = 2;
const P_TUNE_FINE: u32 = 3;
const P_AMP_A: u32 = 4;
const P_AMP_D: u32 = 5;
const P_AMP_S: u32 = 6;
const P_AMP_R: u32 = 7;
const P_VEL_AMP: u32 = 8;
const P_FILTER_ON: u32 = 9;
const P_FILTER_MODE: u32 = 10;
const P_CUTOFF: u32 = 11;
const P_RESONANCE: u32 = 12;
const P_ENV_AMOUNT: u32 = 13;
const P_KEY_TRACK: u32 = 14;
const P_FILTER_VEL: u32 = 15;
const P_FLT_A: u32 = 16;
const P_FLT_D: u32 = 17;
const P_FLT_S: u32 = 18;
const P_FLT_R: u32 = 19;
const P_LFO_WAVE: u32 = 20;
const P_LFO_RATE: u32 = 21;
const P_LFO_PITCH: u32 = 22;
const P_LFO_CUTOFF: u32 = 23;
const P_LFO_AMP: u32 = 24;
const P_CV_PITCH: u32 = 25;
const P_CV_CUTOFF: u32 = 26;
const P_CV_RESONANCE: u32 = 27;
const P_CV_LEVEL: u32 = 28;
const P_CV_PAN: u32 = 29;
const P_BEND_RANGE: u32 = 30;
const PARAM_COUNT: usize = 31;

use Taper::{Decibel as Db, Exponential as Exp, Linear as Lin};

static PARAMS: [ParamDescriptor; PARAM_COUNT] = [
    param(
        P_VOICES,
        "voices",
        "Voices",
        (1.0, MAX_VOICES as f32, 32.0),
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
        P_TUNE_SEMI,
        "tune.semi",
        "Tune semi",
        (-24.0, 24.0, 0.0),
        Lin,
        Unit::Semitones,
        true,
    ),
    param(
        P_TUNE_FINE,
        "tune.fine",
        "Tune fine",
        (-100.0, 100.0, 0.0),
        Lin,
        Unit::Cents,
        false,
    ),
    param(
        P_AMP_A,
        "env.amp.attack",
        "Amp attack",
        (ENV_MS.0, ENV_MS.1, 1.0),
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
        (0.0, 1.0, 1.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_AMP_R,
        "env.amp.release",
        "Amp release",
        (ENV_MS.0, ENV_MS.1, 150.0),
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
        P_ENV_AMOUNT,
        "filter.env_amount",
        "Filter env amount",
        (-1.0, 1.0, 0.0),
        Lin,
        Unit::None,
        false,
    ),
    param(
        P_KEY_TRACK,
        "filter.key_track",
        "Key tracking",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
        false,
    ),
    param(
        P_FILTER_VEL,
        "filter.velocity",
        "Velocity to cutoff",
        (0.0, 1.0, 0.0),
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
        P_FLT_S,
        "env.filter.sustain",
        "Filter sustain",
        (0.0, 1.0, 0.0),
        Lin,
        Unit::Percent,
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
        P_LFO_AMP,
        "lfo.to_amp",
        "LFO to amp",
        (0.0, 1.0, 0.0),
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
        P_CV_CUTOFF,
        "cv.cutoff_amount",
        "CV to cutoff",
        (0.0, 4.0, 2.0),
        Lin,
        Unit::Octaves,
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
];

const NOTES_IN: SocketId = SocketId(0);
const PITCH_CV_IN: SocketId = SocketId(1);
const CUTOFF_CV_IN: SocketId = SocketId(2);
const RESONANCE_CV_IN: SocketId = SocketId(3);
const LEVEL_CV_IN: SocketId = SocketId(4);
const PAN_CV_IN: SocketId = SocketId(5);
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
        CUTOFF_CV_IN,
        "cutoff_cv_in",
        "Cutoff CV",
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
];

/// Static description of the sampler.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.sampler",
    name: "Sampler",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Sampler`] (register it with `register_with_samples`).
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Sampler::new()),
    }
}

/// Samples between control-rate updates (pitch, cutoff) of a voice.
const CONTROL_INTERVAL: u32 = 16;
/// Fixed range of the level CV in dB (times `master.level_cv_trim`).
const CV_LEVEL_RANGE_DB: f32 = 24.0;
/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Fade-out of a voice whose map is draining.
const DRAIN_FADE_MS: f32 = 5.0;
/// Longest end-of-sample fade in frames, and in milliseconds (the smaller one wins).
const END_FADE_FRAMES: u64 = 64;
const END_FADE_MS: f32 = 1.5;

/// Length in source frames of the linear fade-out before a zone's `end` when the loop is off:
/// 64 frames or 1.5 ms (whichever is fewer), never longer than the zone (at least 1 frame).
pub(crate) fn end_fade_len(start: u64, end: u64, sample_rate: f32) -> u64 {
    let ms = (END_FADE_MS * 0.001 * sample_rate).round().max(1.0) as u64;
    END_FADE_FRAMES
        .min(ms)
        .min(end.saturating_sub(start))
        .max(1)
}

/// Gain of the end-of-sample fade at the read position `pos + frac`: 1 up to `fade` frames
/// before `end`, then falling linearly to 0 at `end`.
pub(crate) fn end_fade_gain(pos: u64, frac: f64, end: u64, fade: u64) -> f32 {
    let left = end as f64 - (pos as f64 + frac);
    (left / fade as f64).clamp(0.0, 1.0) as f32
}

/// Gain of the end fade of a reversed zone at the read position `pos + frac`: 1 from `fade`
/// frames above `start`, then falling linearly to 0 one frame below `start`.
pub(crate) fn start_fade_gain(pos: u64, frac: f64, start: u64, fade: u64) -> f32 {
    let left = (pos as f64 + frac) - start as f64 + 1.0;
    (left / fade as f64).clamp(0.0, 1.0) as f32
}

/// The playable region of `zone` over a buffer of `frames` (the shared playhead rule: a
/// reversed zone plays `end - 1` down to `start` and never loops).
pub(crate) fn zone_region(z: &Zone, frames: u64) -> PlayRegion {
    if z.reverse {
        return PlayRegion::reversed(z.start, z.end, frames);
    }
    PlayRegion::new(
        z.start,
        z.end,
        loop_kind(z.loop_mode),
        z.loop_start,
        z.loop_end,
        z.crossfade,
        frames,
    )
}

/// The end-of-sample fade gain of a voice playing `z` at `head` (1 while looping): the fade
/// before `end`, or for a reversed zone before `start`.
pub(crate) fn tail_gain(head: &Playhead, z: &Zone, sample_rate: f32) -> f32 {
    let (pos, frac) = head.position();
    let fade = end_fade_len(z.start, z.end, sample_rate);
    if head.region().reverse() {
        start_fade_gain(pos, frac, z.start, fade)
    } else if head.region().kind() == LoopKind::Off {
        end_fade_gain(pos, frac, z.end, fade)
    } else {
        1.0
    }
}
/// Highest filter cutoff as a fraction of the sample rate.
const MAX_CUTOFF_RATIO: f32 = 0.45;
/// Octaves the cutoff drops at velocity 0 when `filter.velocity` is 1.
const VELOCITY_CUTOFF_OCTAVES: f32 = 4.0;

/// Shared (per-device) parameters, as plain values.
#[derive(Debug, Clone, Copy)]
struct Shared {
    sample_rate: f32,
    tune_semi: f32,
    tune_fine: f32,
    /// Pitch bend in semitones, `-range..=range` (the controller state's range).
    bend: f32,
    velocity_amount: f32,
    filter_on: bool,
    filter_mode: FilterMode,
    cutoff: f32,
    k: f32,
    env_amount: f32,
    key_track: f32,
    filter_velocity: f32,
    lfo_to_pitch: f32,
    lfo_to_cutoff: f32,
    lfo_to_amp: f32,
    cv_pitch: f32,
    cv_cutoff: f32,
    /// Normalised resonance (the base of the resonance CV).
    resonance: f32,
    cv_resonance: f32,
    cv_pan: f32,
    cv_level_db: f32,
    /// Set per batch: the resonance / pan CV sockets are cabled.
    cv_resonance_on: bool,
    cv_pan_on: bool,
    /// The device amp envelope: attack, decay (ms), sustain, release (ms).
    amp_env: [f32; 4],
}

/// Per-sample modulation sources of one render segment (aligned with the output slices).
struct Mods<'a> {
    lfo: &'a [f32],
    pitch: &'a [f32],
    cutoff: &'a [f32],
    resonance: &'a [f32],
    pan: &'a [f32],
}

/// Which map a voice reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Current,
    Draining,
}

/// A note waiting for its voice's steal fade to end.
#[derive(Debug, Clone, Copy)]
struct Pending {
    key: u8,
    velocity: f32,
    zone: u32,
}

/// Placeholder zone of an idle voice.
const IDLE_ZONE: Zone = Zone {
    buffer: 0,
    key_lo: 0,
    key_hi: 0,
    vel_lo: 1,
    vel_hi: 1,
    root_key: 60,
    transpose: 0,
    tune_cents: 0.0,
    gain_db: 0.0,
    pan: 0.0,
    start: 0,
    end: 0,
    loop_mode: LoopMode::Off,
    loop_start: 0,
    loop_end: 0,
    crossfade: 0,
    trigger: TriggerMode::Gate,
    reverse: false,
    ext: atomik_device_sdk::ZoneExt::NONE,
};

fn loop_kind(mode: LoopMode) -> LoopKind {
    match mode {
        LoopMode::Off => LoopKind::Off,
        LoopMode::Forward => LoopKind::Forward,
        LoopMode::PingPong => LoopKind::PingPong,
    }
}

/// One preallocated voice: one zone of one note.
#[derive(Debug, Clone)]
struct Voice {
    key: u8,
    order: u64,
    slot: Slot,
    zone_idx: u32,
    zone: Zone,
    head: Playhead,
    stereo: bool,
    zone_gain: f32,
    vel_gain: f32,
    velocity: f32,
    pan_l: f32,
    pan_r: f32,
    pending: Option<Pending>,
    filters: [Svf; 2],
    amp: Adsr,
    fenv: Adsr,
    ratio: f64,
    countdown: u32,
    g: f32,
    g_target: f32,
    g_step: f32,
    /// Filter damping while the resonance CV is cabled (`k` ramps like `g`).
    k: f32,
    k_target: f32,
    k_step: f32,
    filter_ready: bool,
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
            zone_idx: 0,
            zone: IDLE_ZONE,
            head: Playhead::new(PlayRegion::new(0, 0, LoopKind::Off, 0, 0, 0, 0)),
            stereo: false,
            zone_gain: 1.0,
            vel_gain: 1.0,
            velocity: 1.0,
            pan_l: 1.0,
            pan_r: 1.0,
            pending: None,
            filters: [Svf::new(); 2],
            amp: Adsr::new(48_000.0),
            fenv: Adsr::new(48_000.0),
            ratio: 1.0,
            countdown: 0,
            g: 0.0,
            g_target: 0.0,
            g_step: 0.0,
            k: 1.0,
            k_target: 1.0,
            k_step: 0.0,
            filter_ready: false,
            fade_left: 0,
            fade_len: 1.0,
        }
    }

    fn is_active(&self) -> bool {
        self.amp.is_active()
    }

    /// Stops the voice at once (no fade) and drops a pending note.
    fn kill(&mut self) {
        self.amp.reset();
        self.fenv.reset();
        self.pending = None;
        self.fade_left = 0;
    }

    /// Takes zone `zi` of `map`: its parameters and a region over its buffer.
    ///
    /// The zone's own envelope, if it has one, replaces the device's (`p.amp_env`); the
    /// velocity crossfade gain is taken at the voice's velocity. A change of the zone's filter
    /// restarts the filter from a clean state, so no stale state crosses from one zone to
    /// another (a stolen voice, a rebind after a map swap).
    fn take_zone(&mut self, map: &SampleMap, zi: usize, p: &Shared) -> bool {
        let Some(&zone) = map.zones().get(zi) else {
            return false;
        };
        let buf = map.buffer_of(&zone);
        if self.zone.ext.filter != zone.ext.filter {
            self.filter_ready = false;
        }
        self.zone = zone;
        self.zone_idx = zi as u32;
        self.stereo = buf.data.len() >= 2;
        let e = zone.ext.env.map_or(p.amp_env, |e| {
            [e.attack_ms, e.decay_ms, e.sustain, e.release_ms]
        });
        self.amp.set_attack_ms(e[0]);
        self.amp.set_decay_ms(e[1]);
        self.amp.set_sustain(e[2]);
        self.amp.set_release_ms(e[3]);
        self.zone_gain =
            db_to_gain(zone.gain_db) * zone.velocity_gain(velocity_to_7bit(self.velocity));
        (self.pan_l, self.pan_r) = if self.stereo {
            balance_gains(zone.pan)
        } else {
            pan_gains_mono(zone.pan)
        };
        true
    }

    fn region(&self, map: &SampleMap) -> PlayRegion {
        let z = &self.zone;
        zone_region(z, map.buffer_of(z).frames)
    }

    /// Starts a note on zone `zi` from silence (fresh filters and envelopes).
    fn start(&mut self, map: &SampleMap, zi: usize, key: u8, velocity: f32, p: &Shared) {
        let velocity = if velocity.is_nan() {
            0.0
        } else {
            velocity.clamp(0.0, 1.0)
        };
        self.velocity = velocity;
        if !self.take_zone(map, zi, p) {
            self.kill();
            return;
        }
        self.key = key;
        self.slot = Slot::Current;
        self.pending = None;
        self.velocity = velocity;
        self.vel_gain = 1.0 - p.velocity_amount * (1.0 - velocity);
        self.head = Playhead::new(self.region(map));
        for f in &mut self.filters {
            f.reset();
        }
        self.amp.reset();
        self.fenv.reset();
        self.amp.gate_on();
        self.fenv.gate_on();
        self.filter_ready = false;
        self.countdown = 0;
        self.fade_left = 0;
    }

    /// Moves the voice onto zone `zi` of `map` (a changed map that shares the buffer): the
    /// voice adopts the zone's parameters and keeps its position.
    fn rebind(&mut self, map: &SampleMap, zi: usize, p: &Shared) {
        if !self.take_zone(map, zi, p) {
            self.kill();
            return;
        }
        let (pos, frac) = self.head.position();
        self.head = Playhead::at(self.region(map), pos, frac);
        if self.head.finished() {
            self.kill();
        }
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
        self.fenv.gate_off();
    }

    /// Starts the 2 ms steal fade; the new note starts when it ends.
    fn steal(&mut self, pending: Pending) {
        self.pending = Some(pending);
        self.amp.steal_fade();
    }

    /// Control-rate update: playback ratio and the filter coefficient target.
    fn control(&mut self, m: &Mods<'_>, n: usize, p: &Shared) {
        let (lfo, cv_pitch, cv_cutoff) = (m.lfo[n], m.pitch[n], m.cutoff[n]);
        let z = &self.zone;
        let semis = (i32::from(self.key) - i32::from(z.root_key) + i32::from(z.transpose)) as f32;
        let exponent = (semis + p.tune_semi + p.bend) / 12.0
            + (z.tune_cents + p.tune_fine) / 1200.0
            + (lfo * p.lfo_to_pitch + cv_pitch.clamp(-1.0, 1.0) * p.cv_pitch) / 12.0;
        self.ratio = f64::from(exponent.exp2());
        if !p.filter_on {
            self.filter_ready = false;
            self.g_step = 0.0;
            return;
        }
        let octaves = p.env_amount * 6.0 * self.fenv.level()
            + p.key_track * (f32::from(self.key) - 60.0) / 12.0
            + p.lfo_to_cutoff * lfo
            + p.cv_cutoff * cv_cutoff.clamp(-1.0, 1.0)
            + VELOCITY_CUTOFF_OCTAVES * p.filter_velocity * (self.velocity - 1.0);
        let cutoff = (p.cutoff * octaves.exp2()).clamp(20.0, MAX_CUTOFF_RATIO * p.sample_rate);
        let g = Svf::cutoff_to_g(cutoff, p.sample_rate);
        let snap = !self.filter_ready;
        self.filter_ready = true;
        if snap {
            for f in &mut self.filters {
                f.reset();
            }
        }
        // Land exactly on the previous target (no drift), then ramp towards the new one.
        self.g = if snap { g } else { self.g_target };
        self.g_target = g;
        self.g_step = (g - self.g) / CONTROL_INTERVAL as f32;
        if p.cv_resonance_on {
            let r = (p.resonance + p.cv_resonance * clean(m.resonance[n])).clamp(0.0, 1.0);
            let k = 1.0 / Svf::resonance_to_q(r);
            self.k = if snap { k } else { self.k_target };
            self.k_target = k;
            self.k_step = (k - self.k) / CONTROL_INTERVAL as f32;
        } else {
            self.k = p.k;
            self.k_target = p.k;
            self.k_step = 0.0;
        }
        if snap || self.g_step == 0.0 {
            let k = self.k_now(p);
            for f in &mut self.filters {
                f.set_coefficients(self.g, k);
            }
        }
    }

    /// The filter damping in use: the base value, or the ramped value under resonance CV.
    #[inline]
    fn k_now(&self, p: &Shared) -> f32 {
        if p.cv_resonance_on { self.k } else { p.k }
    }

    /// `base` with the filter fields of this voice's zone filter, when it has one: the filter
    /// runs whatever the device's `filter.enabled` says, with the zone's mode, cutoff,
    /// resonance, key tracking and envelope amount (and no resonance CV).
    fn effective(&self, base: &Shared) -> Shared {
        let Some(f) = self.zone.ext.filter else {
            return *base;
        };
        Shared {
            filter_on: true,
            filter_mode: FilterMode::from_index(usize::from(f.mode)),
            cutoff: f.cutoff,
            resonance: f.resonance,
            k: 1.0 / Svf::resonance_to_q(f.resonance),
            env_amount: f.env_amount,
            key_track: f.key_track,
            cv_resonance_on: false,
            ..*base
        }
    }

    /// Adds this voice into `out_l`/`out_r`. `own` is the map the voice plays from, `current`
    /// the device's current map (where a pending note starts). The slices are aligned.
    fn render(
        &mut self,
        own: &SampleMap,
        current: Option<&SampleMap>,
        out: [&mut [f32]; 2],
        m: &Mods<'_>,
        base: &Shared,
    ) {
        // A zone with its own filter overrides the device's filter settings for its voices.
        let mut eff = self.effective(base);
        let lfo = m.lfo;
        let [out_l, out_r] = out;
        let mut map = own;
        for n in 0..out_l.len() {
            let p = &eff;
            if self.countdown == 0 {
                self.control(m, n, p);
                self.countdown = CONTROL_INTERVAL;
            }
            self.countdown -= 1;
            if p.filter_on && p.cv_resonance_on {
                self.k += self.k_step;
            }
            if p.filter_on && (self.g_step != 0.0 || (p.cv_resonance_on && self.k_step != 0.0)) {
                self.g += self.g_step;
                let k = self.k_now(p);
                for f in &mut self.filters {
                    f.set_coefficients(self.g, k);
                }
            }
            let data = &map.buffer_of(&self.zone).data;
            let xl = self.head.read(&data[0]);
            let xr = if self.stereo {
                self.head.read(&data[1])
            } else {
                xl
            };
            let (mut yl, mut yr) = (xl, xr);
            if p.filter_on {
                yl = self.filters[0].process_mode(xl, p.filter_mode);
                yr = self.filters[1].process_mode(xr, p.filter_mode);
                if !(yl.is_finite() && yr.is_finite()) {
                    self.filters.iter_mut().for_each(Svf::reset);
                    (yl, yr) = (0.0, 0.0);
                }
            }
            self.fenv.next_sample();
            let amp = self.amp.next_sample();
            let tremolo = 1.0 - p.lfo_to_amp * 0.5 * (1.0 - lfo[n]);
            let fade = if self.fade_left > 0 {
                self.fade_left as f32 / self.fade_len
            } else {
                1.0
            };
            let tail = tail_gain(&self.head, &self.zone, p.sample_rate);
            let g = amp * self.zone_gain * self.vel_gain * tremolo * fade * tail;
            let (pan_l, pan_r) = if p.cv_pan_on {
                // The zone pan plus the pan CV, through the same law as the zone pan.
                let pan = self.zone.pan + p.cv_pan * clean(m.pan[n]);
                if self.stereo {
                    balance_gains(pan)
                } else {
                    pan_gains_mono(pan)
                }
            } else {
                (self.pan_l, self.pan_r)
            };
            out_l[n] += yl * pan_l * g;
            out_r[n] += yr * pan_r * g;
            self.head.advance(self.ratio);
            if self.head.finished() {
                self.amp.reset();
            }
            if self.fade_left > 0 {
                self.fade_left -= 1;
                if self.fade_left == 0 {
                    self.amp.reset();
                }
            }
            if !self.amp.is_active() {
                let started = self.pending.take().and_then(|pe| {
                    let m = current?;
                    let z = m.zones().get(pe.zone as usize)?;
                    z.contains(pe.key, velocity_to_7bit(pe.velocity))
                        .then_some((m, pe))
                });
                match started {
                    Some((m, pe)) => {
                        self.start(m, pe.zone as usize, pe.key, pe.velocity, base);
                        eff = self.effective(base);
                        map = m;
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

/// The sampler device.
#[derive(Debug)]
pub struct Sampler {
    values: [f32; PARAM_COUNT],
    shared: Shared,
    voices: Vec<Voice>,
    voice_limit: usize,
    next_order: u64,
    /// Notes counted per zone group and key for round-robin (see `note_on`); cleared by `reset`.
    round_robin: Vec<u32>,
    lfo: Lfo,
    master: Smoother,
    /// Sustain, volume, expression, pressure and the bend range.
    ctl: ControllerState,
    /// Set by `prepare`/`reset`: the next `process` snaps the master gain to its target.
    fresh: bool,
    seed: u64,
    current: Option<SampleMapRef>,
    draining: Option<SampleMapRef>,
    /// Voices cut because a second swap arrived while they were still draining (read by the
    /// tests' probe).
    #[cfg_attr(not(test), allow(dead_code))]
    cut_voices: u64,
    #[cfg(test)]
    probe: Option<Arc<tests::Probe>>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    /// A sampler with default params and no map (call `prepare` before processing).
    pub fn new() -> Self {
        let mut s = Self {
            values: PARAMS.map(|p| p.default),
            shared: Shared {
                sample_rate: 48_000.0,
                tune_semi: 0.0,
                tune_fine: 0.0,
                bend: 0.0,
                velocity_amount: 0.5,
                filter_on: false,
                filter_mode: FilterMode::Lowpass,
                cutoff: 8_000.0,
                k: 1.0,
                env_amount: 0.0,
                key_track: 0.0,
                filter_velocity: 0.0,
                lfo_to_pitch: 0.0,
                lfo_to_cutoff: 0.0,
                lfo_to_amp: 0.0,
                cv_pitch: 12.0,
                cv_cutoff: 2.0,
                resonance: 0.2,
                cv_resonance: 1.0,
                cv_pan: 1.0,
                cv_level_db: CV_LEVEL_RANGE_DB,
                cv_resonance_on: false,
                cv_pan_on: false,
                amp_env: [1.0, 200.0, 1.0, 150.0],
            },
            voices: vec![Voice::new(); MAX_VOICES],
            voice_limit: 32,
            next_order: 0,
            round_robin: vec![0; MAX_ZONE_GROUPS * 128],
            lfo: Lfo::new(derive_seed(0, 0)),
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            ctl: ControllerState::new(),
            fresh: true,
            seed: 0,
            current: None,
            draining: None,
            cut_voices: 0,
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
            P_TUNE_SEMI => s.tune_semi = v,
            P_TUNE_FINE => s.tune_fine = v,
            P_AMP_A => {
                s.amp_env[0] = v;
                voices
                    .iter_mut()
                    .filter(|vc| vc.zone.ext.env.is_none())
                    .for_each(|vc| vc.amp.set_attack_ms(v));
            }
            P_AMP_D => {
                s.amp_env[1] = v;
                voices
                    .iter_mut()
                    .filter(|vc| vc.zone.ext.env.is_none())
                    .for_each(|vc| vc.amp.set_decay_ms(v));
            }
            P_AMP_S => {
                s.amp_env[2] = v;
                voices
                    .iter_mut()
                    .filter(|vc| vc.zone.ext.env.is_none())
                    .for_each(|vc| vc.amp.set_sustain(v));
            }
            P_AMP_R => {
                s.amp_env[3] = v;
                voices
                    .iter_mut()
                    .filter(|vc| vc.zone.ext.env.is_none())
                    .for_each(|vc| vc.amp.set_release_ms(v));
            }
            P_VEL_AMP => s.velocity_amount = v,
            P_FILTER_ON => s.filter_on = v >= 0.5,
            P_FILTER_MODE => s.filter_mode = FilterMode::from_index(choice(v)),
            P_CUTOFF => s.cutoff = v,
            P_RESONANCE => {
                s.k = 1.0 / Svf::resonance_to_q(v);
                s.resonance = v;
            }
            P_ENV_AMOUNT => s.env_amount = v,
            P_KEY_TRACK => s.key_track = v,
            P_FILTER_VEL => s.filter_velocity = v,
            P_FLT_A => voices.iter_mut().for_each(|vc| vc.fenv.set_attack_ms(v)),
            P_FLT_D => voices.iter_mut().for_each(|vc| vc.fenv.set_decay_ms(v)),
            P_FLT_S => voices.iter_mut().for_each(|vc| vc.fenv.set_sustain(v)),
            P_FLT_R => voices.iter_mut().for_each(|vc| vc.fenv.set_release_ms(v)),
            P_LFO_WAVE => self.lfo.set_shape(LfoShape::from_index(choice(v))),
            P_LFO_RATE => self.lfo.set_rate(v, sr),
            P_LFO_PITCH => s.lfo_to_pitch = v,
            P_LFO_CUTOFF => s.lfo_to_cutoff = v,
            P_LFO_AMP => s.lfo_to_amp = v,
            P_CV_PITCH => s.cv_pitch = v,
            P_CV_CUTOFF => s.cv_cutoff = v,
            P_CV_RESONANCE => s.cv_resonance = v,
            P_CV_LEVEL => s.cv_level_db = v * CV_LEVEL_RANGE_DB,
            P_CV_PAN => s.cv_pan = v,
            P_BEND_RANGE => {
                self.ctl.set_bend_range(v);
                s.bend = self.ctl.bend_semitones();
            }
            _ => {}
        }
    }

    /// Starts one voice per sounding zone of the current map.
    ///
    /// A zone sounds when `key` and velocity are in its ranges, its round-robin slot is the one
    /// its group's note counter selects and its velocity crossfade gain is above zero. Every
    /// group that has a round-robin zone in range then counts one note, whichever zone sounded,
    /// so the cycle is a pure function of the notes played since the last reset.
    fn note_on(&mut self, key: u8, velocity: f32) {
        self.ctl.note_on(key);
        let Some(map) = self.current.as_deref() else {
            return;
        };
        let p = self.shared;
        let vel7 = velocity_to_7bit(velocity);
        let mut counted = 0u32;
        for (zi, zone) in map.zones().iter().enumerate() {
            if !zone.contains(key, vel7) {
                continue;
            }
            if zone.ext.round_robin() {
                let g = usize::from(zone.ext.group).min(MAX_ZONE_GROUPS - 1);
                counted |= 1 << g;
                if !zone.rr_turn(self.round_robin[g * 128 + usize::from(key & 127)]) {
                    continue;
                }
            }
            if zone.velocity_gain(vel7) <= 0.0 {
                continue;
            }
            let order = self.next_order;
            self.next_order += 1;
            let voices = &mut self.voices[..self.voice_limit];
            // A free voice, else steal the oldest started voice (2 ms fade, then the note).
            if let Some(v) = voices.iter_mut().find(|v| !v.is_active()) {
                v.order = order;
                v.start(map, zi, key, velocity, &p);
            } else if let Some(v) = voices.iter_mut().min_by_key(|v| v.order) {
                v.order = order;
                v.steal(Pending {
                    key,
                    velocity,
                    zone: zi as u32,
                });
            }
        }
        for g in 0..MAX_ZONE_GROUPS {
            if counted >> g & 1 == 1 {
                let n = &mut self.round_robin[g * 128 + usize::from(key & 127)];
                *n = n.wrapping_add(1);
            }
        }
    }

    fn note_off(&mut self, key: u8) {
        let current = self.current.as_deref();
        for v in self.voices.iter_mut().filter(|v| v.is_active()) {
            if let Some(pe) = v.pending {
                let gate = current
                    .and_then(|m| m.zones().get(pe.zone as usize))
                    .is_none_or(|z| z.trigger == TriggerMode::Gate);
                if pe.key == key && gate {
                    v.pending = None;
                }
            } else if v.key == key && v.zone.trigger == TriggerMode::Gate {
                v.release();
            }
        }
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
            NoteKind::Off { key } => {
                if self.ctl.note_off(key) {
                    self.note_off(key);
                }
            }
            NoteKind::AllOff => {
                self.ctl.all_off();
                self.voices.iter_mut().for_each(Voice::release);
            }
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
                        self.note_off(key);
                    }
                }
                self.shared.bend = self.ctl.bend_semitones();
            }
        }
    }

    #[cfg(test)]
    fn update_probe(&self) {
        use std::sync::atomic::Ordering;
        if let Some(probe) = &self.probe {
            let (mut active, mut draining) = (0, 0);
            for (slot, v) in probe.keys.iter().zip(&self.voices) {
                let k = if v.is_active() {
                    active += 1;
                    draining += usize::from(v.slot == Slot::Draining);
                    u32::from(v.pending.map_or(v.key, |p| p.key)) + 1
                } else {
                    0
                };
                slot.store(k, Ordering::Relaxed);
            }
            probe.active.store(active, Ordering::Relaxed);
            probe.draining.store(draining, Ordering::Relaxed);
            probe.cuts.store(self.cut_voices, Ordering::Relaxed);
        }
    }
}

impl Device for Sampler {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.shared.sample_rate = sr;
        if self.voices.len() < MAX_VOICES {
            self.voices.resize(MAX_VOICES, Voice::new());
        }
        for v in &mut self.voices {
            v.amp.set_sample_rate(sr);
            v.fenv.set_sample_rate(sr);
        }
        self.master.set_time(MASTER_SMOOTH_MS, sr);
        self.ctl.set_sample_rate(sr);
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
        self.round_robin.fill(0);
        self.ctl.reset();
        self.shared.bend = 0.0;
        self.lfo.reset();
        let m = self.master.target();
        self.master.set_immediate(m);
        self.fresh = true;
        #[cfg(test)]
        self.update_probe();
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
        let shared = self.shared;
        // 1. Voices still draining from the draining map are cut, which frees that map.
        for v in &mut self.voices {
            if v.is_active() && v.slot == Slot::Draining {
                v.kill();
                self.cut_voices += 1;
            }
        }
        let stale = self.draining.take();
        // 2. Voices on the old current map move to the new map or start draining.
        let old = self.current.take();
        let mut drains = false;
        if let Some(old_map) = old.as_deref() {
            for v in self.voices.iter_mut().filter(|v| v.is_active()) {
                if v.slot != Slot::Current {
                    continue;
                }
                let target = map.as_deref().and_then(|new_map| {
                    let nz = new_map.zones().get(v.zone_idx as usize)?;
                    Arc::ptr_eq(old_map.buffer_of(&v.zone), new_map.buffer_of(nz))
                        .then_some((new_map, v.zone_idx as usize))
                });
                match target {
                    Some((new_map, zi)) => v.rebind(new_map, zi, &shared),
                    None => {
                        v.start_drain(sr);
                        drains = true;
                    }
                }
            }
        }
        // Pending notes survive only if the new map has a matching zone at their index.
        for v in self.voices.iter_mut().filter(|v| v.is_active()) {
            if let Some(pe) = v.pending {
                let ok = map
                    .as_deref()
                    .and_then(|m| m.zones().get(pe.zone as usize))
                    .is_some_and(|z| z.contains(pe.key, velocity_to_7bit(pe.velocity)));
                if !ok {
                    v.pending = None;
                }
            }
        }
        self.current = map;
        // 3./4. Keep at most one extra map; hand back exactly one (or none).
        let ret = if drains || stale.is_some() {
            self.draining = old;
            stale
        } else {
            old
        };
        #[cfg(test)]
        self.update_probe();
        ret
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
        let cv_cutoff = *ctx.input(CUTOFF_CV_IN);
        let cv_res = *ctx.input(RESONANCE_CV_IN);
        let cv_pan = *ctx.input(PAN_CV_IN);
        self.shared.cv_resonance_on = ctx.is_connected(RESONANCE_CV_IN);
        self.shared.cv_pan_on = ctx.is_connected(PAN_CV_IN);
        let mut mix = [[0.0f32; BATCH]; 2];
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
                cutoff: &cv_cutoff[start..end],
                resonance: &cv_res[start..end],
                pan: &cv_pan[start..end],
            };
            let Self {
                voices,
                current,
                draining,
                ..
            } = self;
            let [mix_l, mix_r] = &mut mix;
            for v in voices.iter_mut().filter(|v| v.is_active()) {
                let own = match v.slot {
                    Slot::Current => current.as_deref(),
                    Slot::Draining => draining.as_deref(),
                };
                let Some(own) = own else {
                    v.kill();
                    continue;
                };
                v.render(
                    own,
                    current.as_deref(),
                    [&mut mix_l[start..end], &mut mix_r[start..end]],
                    &m,
                    &p,
                );
            }
            self.ctl.fill_gain(&mut ctl_gain[start..end]);
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
            *l *= ctl_gain[n];
            *r *= ctl_gain[n];
        }
        *ctx.output(OUT_L) = mix[0];
        *ctx.output(OUT_R) = mix[1];
        #[cfg(test)]
        self.update_probe();
    }
}

#[cfg(test)]
mod tests;
