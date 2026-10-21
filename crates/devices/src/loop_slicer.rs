//! `atomik.loop-slicer`: plays the slices of one loop from keys.
//!
//! The device plays a [`SampleMap`] installed with [`Device::swap_samples`]. Slice *n* (1-based)
//! sits on key `23 + n` (C1 = 24 up to key 87, 64 slices). A slice is a zone with
//! `key_lo == key_hi == its key`; its `start..end` is the slice, its gain, pan, transpose, tune and
//! `reverse` are the per-slice settings. Zones on other keys are ignored, notes on other keys are
//! ignored. When a key has several zones the first one in map order is the slice. A note on a key
//! without a slice (or without a map) does nothing, and does not retrigger either.
//!
//! * **Voices.** 16 voices, all preallocated; `voices` (1..=16) limits how many may sound. One hit
//!   starts one voice. With no free voice the *oldest started* voice is stolen (one that is
//!   already fading out is taken first): it fades out over 2 ms, then the new hit starts from
//!   silence. Lowering `voices` fades the surplus voices over 2 ms. Voices are numbered by a start
//!   counter, so stealing is deterministic.
//! * **Retrigger.** A hit of a slice fades every earlier voice of the same slice (and cancels its
//!   pending hits) over 2 ms; the new voice starts at once. Different slices overlap.
//! * **Envelope.** A linear attack of `env.attack` ms (click protection). Then, in *gate* mode, the
//!   amp holds at 1 while the key is down and the exponential decay `10^(-3 t / decay)` (-60 dB
//!   after `env.decay` ms) starts at note-off (during the attack too). In *one-shot* mode (the
//!   default) the decay starts right after the attack and note-off is ignored. The voice goes
//!   idle below -80 dB, at the end of the slice (end fade of 64 frames or 1.5 ms, as the sampler,
//!   at `end`; for a reversed slice at `start`) or when its fade ends. `env.mode` is read at the
//!   hit; `env.decay` is read live (at batch and event segment boundaries). The zone's own
//!   `trigger` and loop fields are ignored: slices play once (reversed ones too).
//! * **Pitch.** Ratio `2^((key - root + transpose + tune.semi)/12 + tune_cents/1200 +
//!   cv * cv.pitch_amount/12)` (CV clamped to -1..1, updated every 16 samples), cubic Hermite from
//!   `atomik_dsp::Playhead`. Slices normally have root = key, so the ratio is 1 and the slice is
//!   reproduced bit-exactly once the attack is over.
//! * **Reverse.** A zone with `reverse` plays from `end - 1` down to `start` through the shared
//!   playhead rule of `atomik_dsp::PlayRegion::reversed`.
//! * **Level.** Voice gain = envelope x zone gain x `(1 - velocity.amp x (1 - velocity))`. A mono
//!   loop goes through a constant-power pan (-3 dB at the centre) by the zone pan, a stereo loop is
//!   balanced (unity at the centre). The sum is scaled by `master.level` (5 ms smoothing) and soft
//!   limited like the poly-synth.
//! * **`loop.beats`** (1..=64) is stored but unused by the audio: the app and the UI use it for
//!   grid slicing and for writing the loop as a note clip.
//!
//! **Swapping maps** follow `atomik.sampler` and `atomik.drum-machine` exactly: voices on the draining map are cut; a voice on the old map whose zone index exists in the
//! new map with a pointer-equal buffer moves to that zone (it keeps its position and adopts the
//! zone's start/end/gain/pan/root/reverse); all others drain with a 5 ms fade. A pending hit
//! survives only if the new map has a slice zone of its key at the same index.

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SampleMap, SampleMapRef, SocketDescriptor, SocketId, SocketKind, Zone, velocity_to_7bit,
};
use atomik_dsp::{FilterMode, Smoother, Svf};
use std::sync::Arc;

use crate::cv::{clean, db_gain};
use crate::poly_synth::soft_limit;

mod voice;
use voice::{Pending, Slot, Voice};

/// Number of slice keys (and the most slices).
pub const SLICE_COUNT: usize = 64;
/// Key of slice 1 (slice `n` is on key `SLICE_KEY_BASE + n - 1`).
pub const SLICE_KEY_BASE: u8 = 24;
/// Voices (fixed; `voices` limits them).
pub const MAX_VOICES: usize = 16;

/// Whether `zone` is the zone of the slice on `key` (`key_lo == key_hi == key`).
pub(crate) fn slice_key_ok(zone: &Zone, key: u8) -> bool {
    zone.key_lo == key && zone.key_hi == key
}

/// The zero-based slice index of `key`, if it is a slice key.
fn slice_of(key: u8) -> Option<usize> {
    (SLICE_KEY_BASE..SLICE_KEY_BASE + SLICE_COUNT as u8)
        .contains(&key)
        .then(|| usize::from(key - SLICE_KEY_BASE))
}

/// `filter.cutoff` at its maximum: the filter is open (bypassed unless the cutoff CV is cabled).
const FILTER_OPEN_HZ: f32 = 20_000.0;
/// Samples between control-rate updates of the filter coefficient.
const FILTER_CONTROL_INTERVAL: usize = 16;
/// Highest filter cutoff as a fraction of the sample rate.
const MAX_CUTOFF_RATIO: f32 = 0.45;
/// Damping `1/Q` of the 12 dB low-pass (Butterworth, Q = 1/sqrt 2).
const FILTER_K: f32 = std::f32::consts::SQRT_2;

const P_MASTER: u32 = 0;
const P_VOICES: u32 = 1;
const P_TUNE_SEMI: u32 = 2;
const P_ATTACK: u32 = 3;
const P_DECAY: u32 = 4;
const P_MODE: u32 = 5;
const P_VEL_AMP: u32 = 6;
const P_BEATS: u32 = 7;
const P_CV_PITCH: u32 = 8;
const P_FILTER_CUTOFF: u32 = 9;
const P_CV_CUTOFF: u32 = 10;
const P_CV_LEVEL: u32 = 11;
const PARAM_COUNT: usize = 12;

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

static PARAMS: [ParamDescriptor; PARAM_COUNT] = [
    param(
        P_MASTER,
        "master.level",
        "Master level",
        (-60.0, 6.0, -6.0),
        Taper::Decibel,
        Unit::Db,
        false,
    ),
    param(
        P_VOICES,
        "voices",
        "Voices",
        (1.0, MAX_VOICES as f32, MAX_VOICES as f32),
        Taper::Linear,
        Unit::None,
        true,
    ),
    param(
        P_TUNE_SEMI,
        "tune.semi",
        "Tune semi",
        (-24.0, 24.0, 0.0),
        Taper::Linear,
        Unit::Semitones,
        true,
    ),
    param(
        P_ATTACK,
        "env.attack",
        "Attack",
        (0.5, 50.0, 0.5),
        Taper::Exponential,
        Unit::Ms,
        false,
    ),
    param(
        P_DECAY,
        "env.decay",
        "Decay",
        (5.0, 10_000.0, 2_000.0),
        Taper::Exponential,
        Unit::Ms,
        false,
    ),
    param(
        P_MODE,
        "env.mode",
        "Envelope mode",
        (0.0, 1.0, 1.0),
        Taper::Linear,
        MODES,
        true,
    ),
    param(
        P_VEL_AMP,
        "velocity.amp",
        "Velocity to amp",
        (0.0, 1.0, 0.5),
        Taper::Linear,
        Unit::Percent,
        false,
    ),
    param(
        P_BEATS,
        "loop.beats",
        "Loop beats",
        (1.0, 64.0, 4.0),
        Taper::Linear,
        Unit::None,
        true,
    ),
    param(
        P_CV_PITCH,
        "cv.pitch_amount",
        "CV to pitch",
        (0.0, 24.0, 12.0),
        Taper::Linear,
        Unit::Semitones,
        false,
    ),
    param(
        P_FILTER_CUTOFF,
        "filter.cutoff",
        "Filter cutoff",
        (20.0, FILTER_OPEN_HZ, FILTER_OPEN_HZ),
        Taper::Exponential,
        Unit::Hz,
        false,
    ),
    param(
        P_CV_CUTOFF,
        "cv.cutoff_amount",
        "CV to cutoff",
        (-4.0, 4.0, 2.0),
        Taper::Linear,
        Unit::Octaves,
        false,
    ),
    param(
        P_CV_LEVEL,
        "cv.level_amount",
        "CV to level",
        (-24.0, 24.0, 12.0),
        Taper::Linear,
        Unit::Db,
        false,
    ),
];

const NOTES_IN: SocketId = SocketId(0);
const PITCH_CV_IN: SocketId = SocketId(1);
const CUTOFF_CV_IN: SocketId = SocketId(2);
const LEVEL_CV_IN: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);

static SOCKETS: [SocketDescriptor; 6] = [
    SocketDescriptor {
        id: NOTES_IN,
        key: "notes_in",
        name: "Notes in",
        kind: SocketKind::Notes,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: PITCH_CV_IN,
        key: "pitch_cv_in",
        name: "Pitch CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: CUTOFF_CV_IN,
        key: "cutoff_cv_in",
        name: "Cutoff CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: LEVEL_CV_IN,
        key: "level_cv_in",
        name: "Level CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: OUT_L,
        key: "out_l",
        name: "Out L",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: OUT_R,
        key: "out_r",
        name: "Out R",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
];

/// Static description of the loop slicer.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.loop-slicer",
    name: "Loop slicer",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`LoopSlicer`] (register it with `register_with_samples`).
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(LoopSlicer::new()),
    }
}

/// Master gain smoothing time.
const MASTER_SMOOTH_MS: f32 = 5.0;
/// Fade-out of a stolen, retriggered or switched-off voice.
const FADE_MS: f32 = 2.0;
/// Fade-out of a voice whose map is draining.
const DRAIN_FADE_MS: f32 = 5.0;

/// Values shared by all voices.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shared {
    pub sample_rate: f32,
    /// Attack length in samples (at least 1).
    pub attack: u32,
    /// Per-sample decay multiplier for -60 dB over `env.decay`.
    pub decay_coef: f32,
    /// `env.mode` is gate (read at the hit).
    pub gate: bool,
    pub velocity_amount: f32,
    pub tune_semi: f32,
    pub cv_pitch: f32,
    pub filter_cutoff: f32,
    pub cv_cutoff: f32,
    pub cv_level_db: f32,
}

/// Index of the first slice zone of every key in the current map (`u32::MAX`: none).
type SliceTable = [u32; SLICE_COUNT];
const NO_SLICE: u32 = u32::MAX;

/// Scans `map` for the first zone of every slice key (no allocation).
fn scan_slices(map: Option<&SampleMap>) -> SliceTable {
    let mut out = [NO_SLICE; SLICE_COUNT];
    let Some(map) = map else {
        return out;
    };
    for (zi, z) in map.zones().iter().enumerate() {
        if z.key_lo != z.key_hi {
            continue;
        }
        if let Some(s) = slice_of(z.key_lo)
            && out[s] == NO_SLICE
        {
            out[s] = zi as u32;
        }
    }
    out
}

/// The loop slicer device.
#[derive(Debug)]
pub struct LoopSlicer {
    values: [f32; PARAM_COUNT],
    shared: Shared,
    master: Smoother,
    fresh: bool,
    /// The global low-pass over the summed voices (stereo pair) and its coefficient ramp.
    filters: [Svf; 2],
    fg: f32,
    fg_target: f32,
    fg_step: f32,
    filter_ready: bool,
    voices: Vec<Voice>,
    voice_limit: usize,
    next_order: u64,
    slices: SliceTable,
    current: Option<SampleMapRef>,
    draining: Option<SampleMapRef>,
    /// Voices cut because a second swap arrived while they were still draining (read by the
    /// tests' probe).
    #[cfg_attr(not(test), allow(dead_code))]
    cut_voices: u64,
    #[cfg(test)]
    probe: Option<Arc<tests::Probe>>,
}

impl Default for LoopSlicer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopSlicer {
    /// A loop slicer with default params and no map (call `prepare` before processing).
    pub fn new() -> Self {
        let mut d = Self {
            values: PARAMS.map(|p| p.default),
            shared: Shared {
                sample_rate: 48_000.0,
                attack: 24,
                decay_coef: 1.0,
                gate: false,
                velocity_amount: 0.5,
                tune_semi: 0.0,
                cv_pitch: 12.0,
                filter_cutoff: FILTER_OPEN_HZ,
                cv_cutoff: 2.0,
                cv_level_db: 12.0,
            },
            master: Smoother::new(0.0, MASTER_SMOOTH_MS, 48_000.0),
            fresh: true,
            filters: [Svf::new(); 2],
            fg: 0.0,
            fg_target: 0.0,
            fg_step: 0.0,
            filter_ready: false,
            voices: vec![Voice::new(); MAX_VOICES],
            voice_limit: MAX_VOICES,
            next_order: 0,
            slices: [NO_SLICE; SLICE_COUNT],
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
        let sr = self.shared.sample_rate;
        match id {
            P_MASTER => self
                .master
                .set_target(PARAMS[P_MASTER as usize].db_param_gain(v)),
            P_VOICES => {
                self.voice_limit = (v.round().max(1.0) as usize).min(MAX_VOICES);
                let fade = self.samples(FADE_MS);
                for vc in &mut self.voices[self.voice_limit..] {
                    vc.pending = None;
                    if vc.active {
                        vc.start_fade(fade);
                    }
                }
            }
            P_TUNE_SEMI => self.shared.tune_semi = v,
            P_ATTACK => self.shared.attack = (v * 0.001 * sr).round().max(1.0) as u32,
            P_DECAY => {
                let n = (v * 0.001 * sr).max(1.0);
                self.shared.decay_coef = (-3.0 * std::f32::consts::LN_10 / n).exp();
            }
            P_MODE => self.shared.gate = v < 0.5,
            P_VEL_AMP => self.shared.velocity_amount = v,
            P_CV_PITCH => self.shared.cv_pitch = v,
            P_FILTER_CUTOFF => self.shared.filter_cutoff = v,
            P_CV_CUTOFF => self.shared.cv_cutoff = v,
            P_CV_LEVEL => self.shared.cv_level_db = v,
            _ => {}
        }
    }

    /// A hit of `key` (a slice key) at `velocity`.
    fn note_on(&mut self, key: u8, velocity: f32) {
        let Some(map) = self.current.as_deref() else {
            return;
        };
        let Some(slice) = slice_of(key) else {
            return;
        };
        let zi = self.slices[slice];
        let sounds = map
            .zones()
            .get(zi as usize)
            .is_some_and(|z| slice_key_ok(z, key) && z.contains(key, velocity_to_7bit(velocity)));
        if !sounds {
            return;
        }
        let fade = self.samples(FADE_MS);
        let limit = self.voice_limit;
        // Retrigger: earlier hits of this slice fade out, pending ones are cancelled.
        for v in self.voices.iter_mut().filter(|v| v.active) {
            if let Some(pe) = v.pending {
                if pe.key == key {
                    v.pending = None;
                }
            } else if v.key == key {
                v.start_fade(fade);
            }
        }
        let order = self.next_order;
        self.next_order += 1;
        let sh = self.shared;
        if let Some(v) = self.voices[..limit].iter_mut().find(|v| !v.active) {
            v.order = order;
            v.start(map, zi as usize, key, velocity, &sh);
            return;
        }
        // Steal: a voice that is already fading out first, else the oldest.
        let voices = &self.voices[..limit];
        let victim = voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_fading() && v.pending.is_none())
            .min_by_key(|(_, v)| v.order)
            .or_else(|| voices.iter().enumerate().min_by_key(|(_, v)| v.order))
            .map(|(i, _)| i);
        if let Some(v) = victim.map(|i| &mut self.voices[i]) {
            v.order = order;
            v.pending = Some(Pending {
                key,
                velocity,
                zone: zi,
            });
            v.start_fade(fade);
        }
    }

    fn note_off(&mut self, key: u8) {
        let gate = self.shared.gate;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            if let Some(pe) = v.pending {
                if pe.key == key && gate {
                    v.pending = None;
                }
            } else if v.key == key {
                v.release();
            }
        }
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
            NoteKind::Off { key } => {
                if slice_of(key).is_some() {
                    self.note_off(key);
                }
            }
            NoteKind::AllOff => {
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

impl LoopSlicer {
    /// Control-rate update of the global low-pass: the cutoff is `filter.cutoff` times
    /// `2^(cv.cutoff_amount * cv)` (octaves); the coefficient ramps over the next 16 samples.
    fn filter_control(&mut self, cv: f32, connected: bool) {
        let sh = &self.shared;
        let octaves = if connected {
            sh.cv_cutoff * clean(cv)
        } else {
            0.0
        };
        let cutoff =
            (sh.filter_cutoff * octaves.exp2()).clamp(20.0, MAX_CUTOFF_RATIO * sh.sample_rate);
        let g = Svf::cutoff_to_g(cutoff, sh.sample_rate);
        let snap = !self.filter_ready;
        self.filter_ready = true;
        if snap {
            self.filters.iter_mut().for_each(Svf::reset);
        }
        // Land exactly on the previous target (no drift), then ramp towards the new one.
        self.fg = if snap { g } else { self.fg_target };
        self.fg_target = g;
        self.fg_step = (g - self.fg) / FILTER_CONTROL_INTERVAL as f32;
        if snap || self.fg_step == 0.0 {
            for f in &mut self.filters {
                f.set_coefficients(self.fg, FILTER_K);
            }
        }
    }

    /// Filters the summed voices in place (only called while the filter is engaged).
    fn filter_mix(&mut self, mix: &mut [[f32; BATCH]; 2], cv: &[f32; BATCH], connected: bool) {
        for n in 0..BATCH {
            if n % FILTER_CONTROL_INTERVAL == 0 {
                self.filter_control(cv[n], connected);
            }
            if self.fg_step != 0.0 {
                self.fg += self.fg_step;
                for f in &mut self.filters {
                    f.set_coefficients(self.fg, FILTER_K);
                }
            }
            let yl = self.filters[0].process_mode(mix[0][n], FilterMode::Lowpass);
            let yr = self.filters[1].process_mode(mix[1][n], FilterMode::Lowpass);
            if yl.is_finite() && yr.is_finite() {
                (mix[0][n], mix[1][n]) = (yl, yr);
            } else {
                self.filters.iter_mut().for_each(Svf::reset);
                (mix[0][n], mix[1][n]) = (0.0, 0.0);
            }
        }
    }
}

impl Device for LoopSlicer {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.shared.sample_rate = info.sample_rate.hz() as f32;
        if self.voices.len() < MAX_VOICES {
            self.voices.resize(MAX_VOICES, Voice::new());
        }
        self.master
            .set_time(MASTER_SMOOTH_MS, self.shared.sample_rate);
        self.apply_all();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.kill();
        }
        self.next_order = 0;
        let m = self.master.target();
        self.master.set_immediate(m);
        self.fresh = true;
        self.filters.iter_mut().for_each(Svf::reset);
        self.filter_ready = false;
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
        // Pending hits survive only if the new map has a slice zone of their key at that index.
        for v in self.voices.iter_mut().filter(|v| v.active) {
            if let Some(pe) = v.pending {
                let ok = map
                    .as_deref()
                    .and_then(|m| m.zones().get(pe.zone as usize))
                    .is_some_and(|z| {
                        slice_key_ok(z, pe.key) && z.contains(pe.key, velocity_to_7bit(pe.velocity))
                    });
                if !ok {
                    v.pending = None;
                }
            }
        }
        self.slices = scan_slices(map.as_deref());
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
        let cv = *ctx.input(PITCH_CV_IN);
        let cv_cutoff = *ctx.input(CUTOFF_CV_IN);
        let cutoff_on = ctx.is_connected(CUTOFF_CV_IN);
        let level_on = ctx.is_connected(LEVEL_CV_IN);
        let cv_level = *ctx.input(LEVEL_CV_IN);
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
            let sh = self.shared;
            let Self {
                voices,
                current,
                draining,
                ..
            } = self;
            let [mix_l, mix_r] = &mut mix;
            for v in voices.iter_mut().filter(|v| v.active) {
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
                    &cv[start..end],
                    &sh,
                );
            }
            start = end;
        }
        if cutoff_on || self.shared.filter_cutoff < FILTER_OPEN_HZ {
            self.filter_mix(&mut mix, &cv_cutoff, cutoff_on);
        } else {
            self.filter_ready = false;
        }
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
        #[cfg(test)]
        self.update_probe();
    }
}

#[cfg(test)]
mod tests;
