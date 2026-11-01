//! `atomik.drum-machine`: 16 sample pads on the shared sample pool.
//!
//! The device plays a [`SampleMap`] installed with [`Device::swap_samples`]. Pad `n` (1..=16)
//! sits on key `35 + n` (36..=51, GM). A pad's sound is the zones whose key range is exactly its
//! key (`key_lo == key_hi == pad key`); their velocity ranges make velocity layers, and
//! overlapping ranges sound together. Zones on other keys are ignored; notes on other keys are
//! ignored. Without a map, or without a zone for the hit's velocity, a note does nothing (it
//! does not choke or retrigger either).
//!
//! * **Pad zone lists.** `swap_samples` scans the map once and keeps, per pad, the indices of
//!   its first [`MAX_ZONES_PER_PAD`] (8) zones in map order in fixed arrays (no allocation).
//!   Zones past the eighth of a pad are never played.
//! * **Voices.** 32 voices, all preallocated. A hit starts one voice per matching zone. With no
//!   free voice the *oldest* started voice is stolen (a voice that is already fading out for a
//!   choke or retrigger is taken first, oldest first): it fades out over 2 ms, then the new hit
//!   starts from silence. Voices are numbered by a start counter, so stealing is deterministic.
//! * **Envelope.** Every voice has a 0.5 ms linear attack (click protection). Then, in *gate*
//!   mode, the amp holds at 1 while the key is down; note-off starts the decay. In *one-shot*
//!   mode (the default) the decay starts right after the attack and note-off is ignored. The
//!   decay is exponential, `10^(-3 t / decay)`, i.e. -60 dB after `padN.decay` ms; the voice
//!   goes idle below -80 dB. A sample that ends earlier ends the voice (the sampler's end
//!   fade, 64 frames or 1.5 ms, applies unless the zone loops). A looped zone loops until the
//!   decay ends (one-shot) or until released and decayed (gate). The zone's own `trigger` field
//!   is ignored: `padN.mode` decides. The mode is read at the hit; the other pad params are
//!   read live at every batch/event segment boundary without smoothing.
//! * **Pitch.** Ratio `2^((pad key - root + transpose + padN.tune + (padN.fine + zone cents)/100)/12)`,
//!   cubic Hermite from `atomik_dsp::Playhead` (loops, start/end as in the sampler). Pads
//!   normally have root = pad key, so the ratio is 1.
//! * **Level.** Voice gain = decay env x `10^(padN.level/20)` x zone gain x
//!   `(1 - padN.velocity x (1 - velocity))`. A mono sample goes through a constant-power pan
//!   (-3 dB centre), a stereo sample is balanced (unity centre); the pan is `padN.pan` plus the
//!   zone's pan, clamped.
//! * **Choke and retrigger.** A hit on pad *p* with choke group *g* > 0 fades every sounding
//!   voice (and cancels every pending hit) of the *other* pads in group *g* over 2 ms. The
//!   hit also retriggers its own pad: previous voices of pad *p* fade over 2 ms, the new voices
//!   start at once. Group 0 chokes nothing and is never choked. Groups are read live.
//! * **CV.** `level_cv_in` and `pitch_cv_in` (trims `master.level_cv_trim` and `pitch_cv_trim`;
//!   fixed ranges 24 dB and 24 semitones) act on all pads and on their individual outputs. Pitch
//!   acts on voices already sounding too: the batch mean of the CV sets a target semitone offset
//!   and the playback ratio glides to it linearly over each batch. Per pad, `padN_gate_cv_in`
//!   triggers the pad on a rise above 0.5 (velocity `(gate - 0.5) * 2`, so 1.0 is full velocity)
//!   and releases it on the fall; `padN_gate_cv_out` is `0.5 + velocity / 2` from every trigger
//!   (note or gate in) to its note-off (or the falling gate edge), else 0. A source that never
//!   sends a note-off keeps the gate high until all-notes-off or reset. Unconnected sockets
//!   change nothing.
//! * **Outputs.** A pad whose `padN_out_l` or `padN_out_r` is connected writes only to that
//!   pair; other pads go to `out_l`/`out_r`. `master.level` and the soft limiter
//!   (`poly_synth::soft_limit`) apply to the main mix only; individual outputs are unlimited and
//!   unscaled but non-finite values are replaced by 0. All-notes-off fades every voice over 2 ms.
//!
//! **Swapping maps** follow `atomik.sampler` exactly: voices on the draining
//! map are cut; a voice on the old map whose zone index exists in the new map with a
//! pointer-equal buffer moves to that zone (it keeps its pad and position and adopts the zone's
//! start/end/loop/gain/root, so it keeps playing the same buffer even if the pad's zone list
//! changed); all others drain with a 5 ms fade. A pending hit survives only if the new map has a
//! zone of its pad at the same index that still matches its velocity.

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SampleMap, SampleMapRef, SocketDescriptor, SocketId, SocketKind, Zone, velocity_to_7bit,
};
use atomik_dsp::Smoother;
use std::sync::Arc;

use crate::cv_in::{batch_mean, clean, db_gain};
use crate::poly_synth::soft_limit;

mod pad_fx;
mod voice;
use pad_fx::{HIGHCUT_MAX, LOWCUT_MIN, PadFx};
use voice::{Pending, Slot, Voice, VoiceCv};

/// Number of pads.
pub const PAD_COUNT: usize = 16;
/// Key of pad 1 (pad `n` is on key `PAD_KEY_BASE + n - 1`).
pub const PAD_KEY_BASE: u8 = 36;
/// Voices (fixed).
pub const VOICES: usize = 32;
/// Zones per pad that are used; further zones of a pad are ignored.
pub const MAX_ZONES_PER_PAD: usize = 8;

/// The key of the pad with zero-based index `pad`.
pub(crate) const fn pad_key(pad: u8) -> u8 {
    PAD_KEY_BASE + pad
}

/// Whether `zone` belongs to the pad on `key` (`key_lo == key_hi == key`).
pub(crate) fn is_pad_zone(zone: &Zone, key: u8) -> bool {
    zone.key_lo == key && zone.key_hi == key
}

const PARAMS_PER_PAD: usize = 8;
/// Params after the per-pad blocks: `master.level_cv_trim`, then `pitch_cv_trim`.
const CV_PARAM_BASE: usize = 1 + PAD_COUNT * PARAMS_PER_PAD;
const P_LEVEL_CV_TRIM: u32 = CV_PARAM_BASE as u32;
const P_PITCH_CV_TRIM: u32 = CV_PARAM_BASE as u32 + 1;
/// Per-pad effect params (tone, low cut, high cut, drive, two sends), appended after the CV trims.
const EXT_PER_PAD: usize = 6;
const EXT_BASE: usize = CV_PARAM_BASE + 2;
const PARAM_COUNT: usize = EXT_BASE + PAD_COUNT * EXT_PER_PAD;
/// Fixed range of the level CV in dB (times `master.level_cv_trim`).
const CV_LEVEL_RANGE_DB: f32 = 24.0;
/// Fixed range of the pitch CV in semitones (times `pitch_cv_trim`).
const CV_PITCH_RANGE_ST: f32 = 24.0;
/// A gate above this level is high.
const GATE_THRESHOLD: f32 = 0.5;
/// Lowest gate-out level of a held pad (velocity 1/127), so a held gate is always above the threshold.
const GATE_OUT_MIN: f32 = 0.5 + 0.5 / 127.0;
const SOCKET_COUNT: usize = 3 + PAD_COUNT * 2 + 2 + PAD_COUNT * 2 + 4;

const P_MASTER: u32 = 0;
const K_LEVEL: u32 = 0;
const K_PAN: u32 = 1;
const K_TUNE: u32 = 2;
const K_FINE: u32 = 3;
const K_DECAY: u32 = 4;
const K_MODE: u32 = 5;
const K_CHOKE: u32 = 6;
const K_VELOCITY: u32 = 7;

// Offsets inside a pad's block of effect params.
const X_TONE: u32 = 0;
const X_LOWCUT: u32 = 1;
const X_HIGHCUT: u32 = 2;
const X_DRIVE: u32 = 3;
const X_SEND1: u32 = 4;
const X_SEND2: u32 = 5;

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

const MODES: Unit = Unit::Choice(&["gate", "one-shot"]);

/// The eight params of pad `$n`, ids following `master.level`.
macro_rules! pad_params {
    ($($n:literal),*) => {
        [
            param(
                P_MASTER,
                "master.level",
                "Master level",
                (-60.0, 6.0, -6.0),
                Taper::Decibel,
                Unit::Db,
                false,
            ),
            $(
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_LEVEL,
                    concat!("pad", $n, ".level"),
                    concat!("Pad ", $n, " level"),
                    (-60.0, 6.0, 0.0),
                    Taper::Decibel,
                    Unit::Db,
                    false,
                ),
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_PAN,
                    concat!("pad", $n, ".pan"),
                    concat!("Pad ", $n, " pan"),
                    (-1.0, 1.0, 0.0),
                    Taper::Linear,
                    Unit::None,
                    false,
                ),
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_TUNE,
                    concat!("pad", $n, ".tune"),
                    concat!("Pad ", $n, " tune"),
                    (-24.0, 24.0, 0.0),
                    Taper::Linear,
                    Unit::Semitones,
                    true,
                ),
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_FINE,
                    concat!("pad", $n, ".fine"),
                    concat!("Pad ", $n, " fine"),
                    (-100.0, 100.0, 0.0),
                    Taper::Linear,
                    Unit::Cents,
                    false,
                ),
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_DECAY,
                    concat!("pad", $n, ".decay"),
                    concat!("Pad ", $n, " decay"),
                    (5.0, 10_000.0, 500.0),
                    Taper::Exponential,
                    Unit::Ms,
                    false,
                ),
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_MODE,
                    concat!("pad", $n, ".mode"),
                    concat!("Pad ", $n, " mode"),
                    (0.0, 1.0, 1.0),
                    Taper::Linear,
                    MODES,
                    true,
                ),
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_CHOKE,
                    concat!("pad", $n, ".choke"),
                    concat!("Pad ", $n, " choke group"),
                    (0.0, 4.0, 0.0),
                    Taper::Linear,
                    Unit::None,
                    true,
                ),
                param(
                    1 + ($n - 1) * PARAMS_PER_PAD as u32 + K_VELOCITY,
                    concat!("pad", $n, ".velocity"),
                    concat!("Pad ", $n, " velocity"),
                    (0.0, 1.0, 0.7),
                    Taper::Linear,
                    Unit::Percent,
                    false,
                ),
            )*
            param(
                P_LEVEL_CV_TRIM,
                "master.level_cv_trim",
                "Level CV trim",
                (-1.0, 1.0, 1.0),
                Taper::Linear,
                Unit::None,
                false,
            ),
            param(
                P_PITCH_CV_TRIM,
                "pitch_cv_trim",
                "Pitch CV trim",
                (-1.0, 1.0, 1.0),
                Taper::Linear,
                Unit::None,
                false,
            ),
            $(
                param(
                    EXT_BASE as u32 + ($n - 1) * EXT_PER_PAD as u32 + X_TONE,
                    concat!("pad", $n, ".tone"),
                    concat!("Pad ", $n, " tone"),
                    (-1.0, 1.0, 0.0),
                    Taper::Linear,
                    Unit::None,
                    false,
                ),
                param(
                    EXT_BASE as u32 + ($n - 1) * EXT_PER_PAD as u32 + X_LOWCUT,
                    concat!("pad", $n, ".lowcut"),
                    concat!("Pad ", $n, " low cut"),
                    (LOWCUT_MIN, 2_000.0, LOWCUT_MIN),
                    Taper::Exponential,
                    Unit::Hz,
                    false,
                ),
                param(
                    EXT_BASE as u32 + ($n - 1) * EXT_PER_PAD as u32 + X_HIGHCUT,
                    concat!("pad", $n, ".highcut"),
                    concat!("Pad ", $n, " high cut"),
                    (500.0, HIGHCUT_MAX, HIGHCUT_MAX),
                    Taper::Exponential,
                    Unit::Hz,
                    false,
                ),
                param(
                    EXT_BASE as u32 + ($n - 1) * EXT_PER_PAD as u32 + X_DRIVE,
                    concat!("pad", $n, ".drive"),
                    concat!("Pad ", $n, " drive"),
                    (0.0, 1.0, 0.0),
                    Taper::Linear,
                    Unit::Percent,
                    false,
                ),
                param(
                    EXT_BASE as u32 + ($n - 1) * EXT_PER_PAD as u32 + X_SEND1,
                    concat!("pad", $n, ".send1"),
                    concat!("Pad ", $n, " send 1"),
                    (0.0, 1.0, 0.0),
                    Taper::Linear,
                    Unit::Percent,
                    false,
                ),
                param(
                    EXT_BASE as u32 + ($n - 1) * EXT_PER_PAD as u32 + X_SEND2,
                    concat!("pad", $n, ".send2"),
                    concat!("Pad ", $n, " send 2"),
                    (0.0, 1.0, 0.0),
                    Taper::Linear,
                    Unit::Percent,
                    false,
                ),
            )*
        ]
    };
}

static PARAMS: [ParamDescriptor; PARAM_COUNT] =
    pad_params!(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16);

const NOTES_IN: SocketId = SocketId(0);
const OUT_L: SocketId = SocketId(1);
const OUT_R: SocketId = SocketId(2);

/// The individual output sockets of zero-based pad `pad`.
const fn pad_out(pad: usize) -> (SocketId, SocketId) {
    (SocketId(3 + 2 * pad as u16), SocketId(4 + 2 * pad as u16))
}

const LEVEL_CV_IN: SocketId = SocketId(3 + 2 * PAD_COUNT as u16);
const PITCH_CV_IN: SocketId = SocketId(4 + 2 * PAD_COUNT as u16);

/// The two shared send outputs (after the gate sockets).
const SEND1_L: SocketId = SocketId(5 + 4 * PAD_COUNT as u16);
const SEND1_R: SocketId = SocketId(6 + 4 * PAD_COUNT as u16);
const SEND2_L: SocketId = SocketId(7 + 4 * PAD_COUNT as u16);
const SEND2_R: SocketId = SocketId(8 + 4 * PAD_COUNT as u16);

/// The gate input and gate output sockets of zero-based pad `pad`.
const fn pad_gate(pad: usize) -> (SocketId, SocketId) {
    (
        SocketId(5 + 2 * PAD_COUNT as u16 + 2 * pad as u16),
        SocketId(6 + 2 * PAD_COUNT as u16 + 2 * pad as u16),
    )
}

const fn cv_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind: SocketKind::Cv,
        direction: Direction::Input,
    }
}

const fn cv_out(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind: SocketKind::Cv,
        direction: Direction::Output,
    }
}

const fn audio_out(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind: SocketKind::Audio,
        direction: Direction::Output,
    }
}

macro_rules! pad_sockets {
    ($($n:literal),*) => {
        [
            SocketDescriptor {
                id: NOTES_IN,
                key: "notes_in",
                name: "Notes in",
                kind: SocketKind::Notes,
                direction: Direction::Input,
            },
            audio_out(1, "out_l", "Out L"),
            audio_out(2, "out_r", "Out R"),
            $(
                audio_out(
                    3 + ($n - 1) * 2,
                    concat!("pad", $n, "_out_l"),
                    concat!("Pad ", $n, " out L"),
                ),
                audio_out(
                    4 + ($n - 1) * 2,
                    concat!("pad", $n, "_out_r"),
                    concat!("Pad ", $n, " out R"),
                ),
            )*
            cv_in(3 + 2 * PAD_COUNT as u16, "level_cv_in", "Level CV"),
            cv_in(4 + 2 * PAD_COUNT as u16, "pitch_cv_in", "Pitch CV"),
            $(
                cv_in(
                    5 + 2 * PAD_COUNT as u16 + ($n - 1) * 2,
                    concat!("pad", $n, "_gate_cv_in"),
                    concat!("Pad ", $n, " gate in"),
                ),
                cv_out(
                    6 + 2 * PAD_COUNT as u16 + ($n - 1) * 2,
                    concat!("pad", $n, "_gate_cv_out"),
                    concat!("Pad ", $n, " gate out"),
                ),
            )*
            audio_out(SEND1_L.0, "send1_l", "Send 1 L"),
            audio_out(SEND1_R.0, "send1_r", "Send 1 R"),
            audio_out(SEND2_L.0, "send2_l", "Send 2 L"),
            audio_out(SEND2_R.0, "send2_r", "Send 2 R"),
        ]
    };
}

static SOCKETS: [SocketDescriptor; SOCKET_COUNT] =
    pad_sockets!(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16);

/// Static description of the drum machine.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.drum-machine",
    name: "Drum machine",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`DrumMachine`] (register it with `register_with_samples`).
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(DrumMachine::new()),
    }
}

/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Fade-out of a stolen, choked or retriggered voice.
const FADE_MS: f32 = 2.0;
/// Fade-out of a voice whose map is draining.
const DRAIN_FADE_MS: f32 = 5.0;
/// Attack ramp length.
const ATTACK_MS: f32 = 0.5;

/// Per-pad parameters as plain values.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pad {
    /// Linear gain of `padN.level`.
    pub level: f32,
    pub pan: f32,
    pub tune: f32,
    pub fine: f32,
    pub decay_ms: f32,
    /// Per-sample decay multiplier for -60 dB over `decay_ms`.
    pub decay_coef: f32,
    pub gate: bool,
    pub choke: u8,
    pub velocity: f32,
}

impl Pad {
    fn new() -> Self {
        Self {
            level: 1.0,
            pan: 0.0,
            tune: 0.0,
            fine: 0.0,
            decay_ms: 500.0,
            decay_coef: 1.0,
            gate: false,
            choke: 0,
            velocity: 0.7,
        }
    }

    fn set_decay(&mut self, ms: f32, sample_rate: f32) {
        self.decay_ms = ms;
        let n = (ms * 0.001 * sample_rate).max(1.0);
        self.decay_coef = (-3.0 * std::f32::consts::LN_10 / n).exp();
    }
}

/// Values shared by all voices.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shared {
    pub sample_rate: f32,
    /// Attack length in samples (at least 1).
    pub attack: u32,
}

/// Indices of a pad's zones in the current map.
#[derive(Debug, Clone, Copy)]
struct PadZones {
    n: u8,
    idx: [u32; MAX_ZONES_PER_PAD],
}

impl PadZones {
    const EMPTY: Self = Self {
        n: 0,
        idx: [0; MAX_ZONES_PER_PAD],
    };
}

/// Scans `map` for the zones of every pad (no allocation).
fn scan_pads(map: Option<&SampleMap>) -> [PadZones; PAD_COUNT] {
    let mut out = [PadZones::EMPTY; PAD_COUNT];
    let Some(map) = map else {
        return out;
    };
    for (zi, z) in map.zones().iter().enumerate() {
        if z.key_lo != z.key_hi
            || !(PAD_KEY_BASE..PAD_KEY_BASE + PAD_COUNT as u8).contains(&z.key_lo)
        {
            continue;
        }
        let pz = &mut out[usize::from(z.key_lo - PAD_KEY_BASE)];
        if usize::from(pz.n) < MAX_ZONES_PER_PAD {
            pz.idx[usize::from(pz.n)] = zi as u32;
            pz.n += 1;
        }
    }
    out
}

/// The gate-out level of a hit at `velocity`: `0.5 + velocity / 2`, never below one MIDI step
/// above 0.5, so a held pad always reads as high.
fn gate_level(velocity: f32) -> f32 {
    let v = if velocity.is_nan() {
        0.0
    } else {
        velocity.clamp(0.0, 1.0)
    };
    (GATE_THRESHOLD + 0.5 * v).max(GATE_OUT_MIN)
}

/// The velocity a gate-in level stands for: the inverse of [`gate_level`]; 1.0 is full velocity.
fn gate_velocity(gate: f32) -> f32 {
    ((gate - GATE_THRESHOLD) * 2.0).clamp(0.0, 1.0)
}

/// The drum machine device.
#[derive(Debug)]
pub struct DrumMachine {
    values: [f32; PARAM_COUNT],
    pads: [Pad; PAD_COUNT],
    /// Per-pad tone, cuts, drive and sends.
    fx: [PadFx; PAD_COUNT],
    shared: Shared,
    master: Smoother,
    fresh: bool,
    /// `master.level_cv_trim` times the fixed range: dB per unit CV.
    cv_level_db: f32,
    /// `pitch_cv_trim` times the fixed range: semitones per unit CV.
    cv_pitch_st: f32,
    /// Pitch CV target of the previous batch in semitones (`None` until a batch has used it).
    pitch_prev: Option<f32>,
    /// Gate-out level per pad: 0 while released, else the encoded velocity of the hit.
    gate_out: [f32; PAD_COUNT],
    /// Gate-in state per pad (above the threshold at the last sample looked at).
    gate_high: [bool; PAD_COUNT],
    voices: Vec<Voice>,
    next_order: u64,
    zones: [PadZones; PAD_COUNT],
    current: Option<SampleMapRef>,
    draining: Option<SampleMapRef>,
    /// Voices cut because a second swap arrived while they were still draining (read by the
    /// tests' probe).
    #[cfg_attr(not(test), allow(dead_code))]
    cut_voices: u64,
    #[cfg(test)]
    probe: Option<Arc<tests::Probe>>,
}

impl Default for DrumMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl DrumMachine {
    /// A drum machine with default params and no map (call `prepare` before processing).
    pub fn new() -> Self {
        let mut d = Self {
            values: PARAMS.map(|p| p.default),
            pads: [Pad::new(); PAD_COUNT],
            fx: std::array::from_fn(|_| PadFx::new(48_000.0)),
            shared: Shared {
                sample_rate: 48_000.0,
                attack: 24,
            },
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            fresh: true,
            cv_level_db: CV_LEVEL_RANGE_DB,
            cv_pitch_st: CV_PITCH_RANGE_ST,
            pitch_prev: None,
            gate_out: [0.0; PAD_COUNT],
            gate_high: [false; PAD_COUNT],
            voices: vec![Voice::new(); VOICES],
            next_order: 0,
            zones: [PadZones::EMPTY; PAD_COUNT],
            current: None,
            draining: None,
            cut_voices: 0,
            #[cfg(test)]
            probe: None,
        };
        d.apply_all();
        let m = d.master.target();
        d.master.set_immediate(m);
        d
    }

    fn apply_all(&mut self) {
        for i in 0..PARAM_COUNT {
            self.apply(i as u32, self.values[i]);
        }
    }

    fn samples(&self, ms: f32) -> u32 {
        (ms * 0.001 * self.shared.sample_rate).round().max(1.0) as u32
    }

    /// Applies one stored plain value. RT-safe.
    fn apply(&mut self, id: u32, v: f32) {
        if id == P_MASTER {
            self.master
                .set_target(PARAMS[P_MASTER as usize].db_param_gain(v));
            return;
        }
        if id as usize >= EXT_BASE {
            let rel = id - EXT_BASE as u32;
            let pad = (rel / EXT_PER_PAD as u32) as usize;
            let mut p = self.fx[pad].params();
            match rel % EXT_PER_PAD as u32 {
                X_TONE => p.tone = v,
                X_LOWCUT => p.lowcut = v,
                X_HIGHCUT => p.highcut = v,
                X_DRIVE => p.drive = v,
                X_SEND1 => p.send1 = v,
                X_SEND2 => p.send2 = v,
                _ => {}
            }
            self.fx[pad].set(p);
            if !self.fx[pad].active() {
                self.fx[pad].reset();
            }
            return;
        }
        match id {
            P_LEVEL_CV_TRIM => {
                self.cv_level_db = v * CV_LEVEL_RANGE_DB;
                return;
            }
            P_PITCH_CV_TRIM => {
                self.cv_pitch_st = v * CV_PITCH_RANGE_ST;
                return;
            }
            _ => {}
        }
        let (pad, k) = (
            ((id - 1) / PARAMS_PER_PAD as u32) as usize,
            (id - 1) % PARAMS_PER_PAD as u32,
        );
        let sr = self.shared.sample_rate;
        let d = &PARAMS[id as usize];
        let p = &mut self.pads[pad];
        match k {
            K_LEVEL => p.level = d.db_param_gain(v),
            K_PAN => p.pan = v,
            K_TUNE => p.tune = v,
            K_FINE => p.fine = v,
            K_DECAY => p.set_decay(v, sr),
            K_MODE => p.gate = v < 0.5,
            K_CHOKE => p.choke = v.round().clamp(0.0, 4.0) as u8,
            K_VELOCITY => p.velocity = v,
            _ => {}
        }
    }

    /// A trigger of `pad` (zero-based): raises its gate out to the encoded velocity and plays
    /// the hit.
    fn trigger(&mut self, pad: u8, velocity: f32) {
        self.gate_out[usize::from(pad)] = gate_level(velocity);
        self.note_on(pad, velocity);
    }

    /// The end of a trigger of `pad`: lowers its gate out and releases a gate-mode pad.
    fn release(&mut self, pad: u8) {
        self.gate_out[usize::from(pad)] = 0.0;
        self.note_off(pad);
    }

    /// A hit on `pad` (zero-based) at `velocity`.
    fn note_on(&mut self, pad: u8, velocity: f32) {
        let Some(map) = self.current.as_deref() else {
            return;
        };
        let key = pad_key(pad);
        let vel7 = velocity_to_7bit(velocity);
        let list = self.zones[usize::from(pad)];
        let matches = |i: usize| {
            let zi = list.idx[i] as usize;
            map.zones()
                .get(zi)
                .is_some_and(|z| is_pad_zone(z, key) && z.contains(key, vel7))
        };
        if !(0..usize::from(list.n)).any(matches) {
            return;
        }
        // Choke the other pads of the group and retrigger this pad.
        let fade = self.samples(FADE_MS);
        let group = self.pads[usize::from(pad)].choke;
        let pads = self.pads;
        let hit = |p: u8| p == pad || (group != 0 && pads[usize::from(p)].choke == group);
        for v in self.voices.iter_mut().filter(|v| v.active) {
            if let Some(pe) = v.pending {
                if hit(pe.pad) {
                    v.pending = None;
                }
            } else if hit(v.pad) {
                v.start_fade(fade);
            }
        }
        for i in 0..usize::from(list.n) {
            if !matches(i) {
                continue;
            }
            let zi = list.idx[i] as usize;
            let order = self.next_order;
            self.next_order += 1;
            if let Some(v) = self.voices.iter_mut().find(|v| !v.active) {
                v.order = order;
                v.start(map, zi, pad, velocity, &pads);
                continue;
            }
            // Steal: a voice that is already fading out first, else the oldest.
            let victim = self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.is_fading() && v.pending.is_none())
                .min_by_key(|(_, v)| v.order)
                .or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.order))
                .map(|(i, _)| i);
            if let Some(v) = victim.map(|i| &mut self.voices[i]) {
                v.order = order;
                v.pending = Some(Pending {
                    pad,
                    velocity,
                    zone: zi as u32,
                });
                v.start_fade(fade);
            }
        }
    }

    fn note_off(&mut self, pad: u8) {
        let gate = self.pads[usize::from(pad)].gate;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            if let Some(pe) = v.pending {
                if pe.pad == pad && gate {
                    v.pending = None;
                }
            } else if v.pad == pad {
                v.release();
            }
        }
    }

    fn handle(&mut self, kind: NoteKind) {
        let pad_of = |key: u8| {
            (PAD_KEY_BASE..PAD_KEY_BASE + PAD_COUNT as u8)
                .contains(&key)
                .then(|| key - PAD_KEY_BASE)
        };
        match kind {
            NoteKind::On { key, velocity } => {
                if let Some(p) = pad_of(key) {
                    self.trigger(p, velocity);
                }
            }
            NoteKind::Off { key } => {
                if let Some(p) = pad_of(key) {
                    self.release(p);
                }
            }
            NoteKind::AllOff => {
                self.gate_out = [0.0; PAD_COUNT];
                let fade = self.samples(FADE_MS);
                for v in self.voices.iter_mut().filter(|v| v.active) {
                    v.pending = None;
                    v.start_fade(fade);
                }
            }
            // Channel messages and expressions are ignored.
            _ => {}
        }
    }

    #[cfg(test)]
    fn update_probe(&self) {
        use std::sync::atomic::Ordering;
        if let Some(probe) = &self.probe {
            let (mut active, mut draining) = (0, 0);
            for (slot, v) in probe.keys.iter().zip(&self.voices) {
                let k = if v.active {
                    active += 1;
                    draining += usize::from(v.slot == Slot::Draining);
                    u32::from(pad_key(v.pending.map_or(v.pad, |p| p.pad))) + 1
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

impl Device for DrumMachine {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.shared.sample_rate = sr;
        for fx in &mut self.fx {
            fx.set_sample_rate(sr);
        }
        self.shared.attack = (ATTACK_MS * 0.001 * sr).round().max(1.0) as u32;
        if self.voices.len() < VOICES {
            self.voices.resize(VOICES, Voice::new());
        }
        self.master.set_time(MASTER_SMOOTH_MS, sr);
        self.apply_all();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.kill();
        }
        self.next_order = 0;
        self.gate_out = [0.0; PAD_COUNT];
        self.gate_high = [false; PAD_COUNT];
        self.pitch_prev = None;
        for fx in &mut self.fx {
            fx.reset();
        }
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
        let drain = self.samples(DRAIN_FADE_MS);
        // 1. Voices still draining from the draining map are cut, which frees that map.
        for v in &mut self.voices {
            if v.active && v.slot == Slot::Draining {
                v.kill();
                self.cut_voices += 1;
            }
        }
        let stale = self.draining.take();
        // 2. Voices on the old current map move to the new map or start draining.
        let old = self.current.take();
        let mut drains = false;
        if let Some(old_map) = old.as_deref() {
            for v in self.voices.iter_mut().filter(|v| v.active) {
                if v.slot != Slot::Current {
                    continue;
                }
                let target = map.as_deref().and_then(|new_map| {
                    let nz = new_map.zones().get(v.zone_idx as usize)?;
                    Arc::ptr_eq(old_map.buffer_of(&v.zone), new_map.buffer_of(nz))
                        .then_some((new_map, v.zone_idx as usize))
                });
                match target {
                    Some((new_map, zi)) => v.rebind(new_map, zi),
                    None => {
                        v.start_drain(drain);
                        drains = true;
                    }
                }
            }
        }
        // Pending hits survive only if the new map has a matching zone of their pad there.
        for v in self.voices.iter_mut().filter(|v| v.active) {
            if let Some(pe) = v.pending {
                let key = pad_key(pe.pad);
                let ok = map
                    .as_deref()
                    .and_then(|m| m.zones().get(pe.zone as usize))
                    .is_some_and(|z| {
                        is_pad_zone(z, key) && z.contains(key, velocity_to_7bit(pe.velocity))
                    });
                if !ok {
                    v.pending = None;
                }
            }
        }
        self.zones = scan_pads(map.as_deref());
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
        let mut routed = [false; PAD_COUNT];
        let mut fx_on = [false; PAD_COUNT];
        for (p, r) in routed.iter_mut().enumerate() {
            let (l, rr) = pad_out(p);
            *r = ctx.is_connected(l) || ctx.is_connected(rr);
            fx_on[p] = self.fx[p].active();
        }
        let mut cv = VoiceCv::new();
        if ctx.is_connected(LEVEL_CV_IN) {
            cv.level_on = true;
            let src = *ctx.input(LEVEL_CV_IN);
            for (g, c) in cv.level.iter_mut().zip(&src) {
                *g = db_gain(self.cv_level_db, *c);
            }
        }
        if ctx.is_connected(PITCH_CV_IN) {
            // Per-batch control rate: the batch mean sets the target and the pitch glides there
            // linearly (in semitones) over the batch, for every sounding voice.
            cv.pitch_on = true;
            let target = self.cv_pitch_st * batch_mean(ctx.input(PITCH_CV_IN));
            let from = self.pitch_prev.unwrap_or(target);
            for (n, r) in cv.pitch.iter_mut().enumerate() {
                let st = from + (target - from) * ((n + 1) as f32 / BATCH as f32);
                *r = f64::from((st / 12.0).exp2());
            }
            self.pitch_prev = Some(target);
        } else {
            self.pitch_prev = None;
        }
        let mut gate_in: [Option<&[f32; BATCH]>; PAD_COUNT] = [None; PAD_COUNT];
        let mut gate_out_on = [false; PAD_COUNT];
        for p in 0..PAD_COUNT {
            let (gi, go) = pad_gate(p);
            if ctx.is_connected(gi) {
                gate_in[p] = Some(ctx.input(gi));
            } else if self.gate_high[p] {
                // Unplugged while high: counts as the falling edge.
                self.gate_high[p] = false;
                self.release(p as u8);
            }
            gate_out_on[p] = ctx.is_connected(go);
        }
        let mut gate_out = [[0.0f32; BATCH]; PAD_COUNT];
        let mut mix = [[0.0f32; BATCH]; 2];
        let mut pad_mix = [[[0.0f32; BATCH]; 2]; PAD_COUNT];
        let events = ctx.notes_in(NOTES_IN);
        let mut next = 0;
        let mut start = 0;
        // Render in segments between note events so events are sample-accurate.
        while start < BATCH {
            while next < events.len() && (events[next].offset as usize).min(BATCH - 1) <= start {
                self.handle(events[next].kind);
                next += 1;
            }
            // Gate-in edges at this sample (after the notes), then the next one ends the segment.
            for (p, g) in gate_in.iter().enumerate() {
                let Some(g) = g else { continue };
                let high = clean(g[start]) > GATE_THRESHOLD;
                if high != self.gate_high[p] {
                    self.gate_high[p] = high;
                    if high {
                        self.trigger(p as u8, gate_velocity(clean(g[start])));
                    } else {
                        self.release(p as u8);
                    }
                }
            }
            let mut end = events
                .get(next)
                .map_or(BATCH, |e| (e.offset as usize).clamp(start + 1, BATCH));
            for (p, g) in gate_in.iter().enumerate() {
                let Some(g) = g else { continue };
                let high = self.gate_high[p];
                if let Some(i) = (start + 1..end).find(|&i| (clean(g[i]) > GATE_THRESHOLD) != high)
                {
                    end = i;
                }
            }
            for (p, on) in gate_out_on.iter().enumerate() {
                if *on {
                    gate_out[p][start..end].fill(self.gate_out[p]);
                }
            }
            let sh = self.shared;
            let Self {
                voices,
                current,
                draining,
                pads,
                ..
            } = self;
            for v in voices.iter_mut().filter(|v| v.active) {
                let own = match v.slot {
                    Slot::Current => current.as_deref(),
                    Slot::Draining => draining.as_deref(),
                };
                let Some(own) = own else {
                    v.kill();
                    continue;
                };
                let [l, r] = if routed[usize::from(v.pad)] || fx_on[usize::from(v.pad)] {
                    &mut pad_mix[usize::from(v.pad)]
                } else {
                    &mut mix
                };
                v.render(
                    own,
                    current.as_deref(),
                    [&mut l[start..end], &mut r[start..end]],
                    pads,
                    &sh,
                    &cv.at(start),
                );
            }
            start = end;
        }
        // Pads with an effect set: run the chain on the pad's pair, feed the sends and (unless the
        // pad has its own outputs) add the pair to the main mix.
        let mut sends = [[[0.0f32; BATCH]; 2]; 2];
        for p in 0..PAD_COUNT {
            if !fx_on[p] {
                continue;
            }
            self.fx[p].process(&mut pad_mix[p]);
            let amounts = [self.fx[p].params().send1, self.fx[p].params().send2];
            for (send, amount) in sends.iter_mut().zip(amounts) {
                if amount > 0.0 {
                    for (dst, src) in send.iter_mut().zip(&pad_mix[p]) {
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d += s * amount;
                        }
                    }
                }
            }
            if !routed[p] {
                for (dst, src) in mix.iter_mut().zip(&pad_mix[p]) {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d += s;
                    }
                }
            }
        }
        let [mix_l, mix_r] = &mut mix;
        for (l, r) in mix_l.iter_mut().zip(mix_r.iter_mut()) {
            let m = self.master.next_value();
            *l = soft_limit(*l * m);
            *r = soft_limit(*r * m);
        }
        *ctx.output(OUT_L) = mix[0];
        *ctx.output(OUT_R) = mix[1];
        for (ids, send) in [(SEND1_L, SEND1_R), (SEND2_L, SEND2_R)]
            .into_iter()
            .zip(&sends)
        {
            for (id, src) in [(ids.0, &send[0]), (ids.1, &send[1])] {
                if ctx.is_connected(id) {
                    let dst = ctx.output(id);
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d = if s.is_finite() { *s } else { 0.0 };
                    }
                }
            }
        }
        for (p, on) in gate_out_on.iter().enumerate() {
            if *on {
                *ctx.output(pad_gate(p).1) = gate_out[p];
            }
        }
        for (p, on) in routed.iter().enumerate() {
            if !on {
                continue;
            }
            let (l, r) = pad_out(p);
            for (id, src) in [(l, &pad_mix[p][0]), (r, &pad_mix[p][1])] {
                let dst = ctx.output(id);
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = if s.is_finite() { *s } else { 0.0 };
                }
            }
        }
        #[cfg(test)]
        self.update_probe();
    }
}

#[cfg(test)]
mod tests;
