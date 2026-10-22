//! `atomik.mod-source`: two LFOs and an ADSR envelope as control-voltage sources.
//!
//! The device makes no sound. It has a notes input (key sync and envelope trigger), a gate CV
//! input (envelope trigger), rate and amount CV inputs per LFO and three CV outputs:
//! `lfo1_cv_out` and `lfo2_cv_out` (`-1..=1`) and `env_cv_out` (`0..=1`). It is a
//! [`DeviceKind::Utility`] with only CV sockets besides the notes input, so adding it to a rack
//! creates no cables.
//!
//! # LFO
//!
//! Each LFO has a `shape` (sine, triangle, saw up, saw down, pulse, sample & hold, smooth
//! random), a `morph` (`-1..=1`, `0` neutral: skew, curve, pulse width or glide depending on the
//! shape; the exact definitions are in [`atomik_dsp::mod_source`]), a `phase` offset (cycles), an
//! `amount` (`0..=1`) and a `polarity` (bipolar `v`, or unipolar `(v + 1) / 2`). The output is
//! `shape * amount * amount_cv`, so it stays within `-1..=1` (`0..=1` unipolar).
//!
//! * **Rate.** Free: `rate` Hz (0.01 to 40, exponential) times `2^(4 * rate_cv * rate_cv_trim)`,
//!   where the connected CV is sanitised and clamped to `-1..=1` and `rate_cv_trim` is `-1..=1` (default +1,
//!   negative inverts), so the CV moves the rate by up to 4 octaves.
//!   Synced (`sync` on): one cycle lasts the note `div` (1/64 to 8 bars, each straight, triplet or
//!   dotted; bars follow the time signature) and the rate CV bends that in octaves the same way.
//! * **Sync semantics.** While the transport **plays**, a synced LFO's phase is a pure function
//!   of the song tick, `phase = tick / ticks_per_cycle`, so it follows locates, loops (including
//!   the wrap inside a batch) and tempo changes. With a rate CV connected (and a non-zero rate CV
//!   amount) the phase is instead integrated from the CV-bent rate and re-aligned to the song
//!   position at every jump (first batch, locate, loop wrap, start). While the transport is
//!   **stopped** a synced LFO free-runs at the synced rate from its last phase. A free LFO never
//!   looks at the transport.
//! * **Key sync.** With `key_sync` on, every note-on restarts the LFO at phase 0 (plus `phase`
//!   offset) at the note's sample offset. In locked sync it re-bases the song-locked phase
//!   (until the next jump) so the cycle starts at the note.
//! * **Randomness.** Sample & hold and smooth random take their values from
//!   `hash64(seed, lfo, cycle)`, a pure function of the device seed (`PrepareInfo::seed`), the
//!   LFO number and the cycle index (plus a counter of key-sync restarts), so a rerun, a locate
//!   and a bounce see the same values.
//!
//! # Envelope
//!
//! `env_attack`, `env_decay`, `env_sustain`, `env_release` (see [`atomik_dsp::mod_source::ModEnv`]
//! for the curves) and `env_amount` scale the output. The gate is high while any note is held or
//! `gate_cv_in` is above 0.5 (the threshold is exclusive: exactly 0.5 is low). `env_mode`:
//!
//! * **retrigger** (default): every note-on and every rising edge of the gate CV restarts the
//!   attack from the **current level** (never from zero, so there is no click);
//! * **legato**: a trigger while the gate is already high (another note held, or the gate CV
//!   high) does nothing; only the first of an overlapping group starts the attack.
//!
//! The release starts when the last note is released and the gate CV is low. `AllOff` releases
//! every note. Notes and gate CV are handled at their sample offsets.
//!
//! # Meter values (for the scope)
//!
//! 32 floats, see [`METER_COUNT`]: `0..3` the latest outputs (lfo1, lfo2, env), `3..5` the LFO
//! phases in `0..1`, `5` the envelope stage (0 idle, 1 attack, 2 decay, 3 sustain, 4 release),
//! `6` a push counter modulo 4096, `7..15`, `15..23` and `23..31` the last 8 history points of
//! lfo1, lfo2 and env (oldest first) and `31` the spacing of the history points in
//! milliseconds. A history point is the output at every `sample_rate / 400` frames, so the 8
//! points cover about 20 ms and a reader that polls at 60 Hz or faster sees every point.
//!
//! Everything in the output path is deterministic (`+ - * /`, [`atomik_dsp::sin_cycles`],
//! [`atomik_dsp::exp2_det`], integer hashes): the streams are bit-identical on every platform.

use crate::note_clock::JumpTracker;
use atomik_core::{BATCH, Batch, NoteKind, PPQ, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::mod_source::{ModEnv, ModShape, mod_shape};
use atomik_dsp::rng::{hash64, unit_f64};
use atomik_dsp::{AdsrStage, exp2_det};

#[cfg(test)]
mod tests;

/// Number of meter values the device publishes.
pub const METER_COUNT: usize = 32;
/// History points per signal in the meter values.
pub const SCOPE_POINTS: usize = 8;
/// History points per second of audio.
pub const SCOPE_RATE_HZ: f64 = 400.0;

/// Params per LFO.
const LFO_PARAMS: u32 = 10;
const O_SHAPE: u32 = 0;
const O_MORPH: u32 = 1;
const O_RATE: u32 = 2;
const O_SYNC: u32 = 3;
const O_DIV: u32 = 4;
const O_PHASE: u32 = 5;
const O_AMOUNT: u32 = 6;
const O_POLARITY: u32 = 7;
const O_KEY_SYNC: u32 = 8;
const O_RATE_CV: u32 = 9;
/// Octaves of rate change at full CV and full trim.
const RATE_CV_OCTAVES: f32 = 4.0;

const P_ENV_ATTACK: u32 = 20;
const P_ENV_DECAY: u32 = 21;
const P_ENV_SUSTAIN: u32 = 22;
const P_ENV_RELEASE: u32 = 23;
const P_ENV_AMOUNT: u32 = 24;
const P_ENV_MODE: u32 = 25;

/// Choice labels of `div`: for each note length a triplet, a straight and a dotted entry.
const DIV_LABELS: [&str; 30] = [
    "1/64 trip",
    "1/64",
    "1/64 dot",
    "1/32 trip",
    "1/32",
    "1/32 dot",
    "1/16 trip",
    "1/16",
    "1/16 dot",
    "1/8 trip",
    "1/8",
    "1/8 dot",
    "1/4 trip",
    "1/4",
    "1/4 dot",
    "1/2 trip",
    "1/2",
    "1/2 dot",
    "1 bar trip",
    "1 bar",
    "1 bar dot",
    "2 bars trip",
    "2 bars",
    "2 bars dot",
    "4 bars trip",
    "4 bars",
    "4 bars dot",
    "8 bars trip",
    "8 bars",
    "8 bars dot",
];
/// Straight length of each note in quarter-note beats (bars at 4/4).
const DIV_BEATS: [f64; 10] = [0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0];
/// Index of the first bar-based note length in [`DIV_BEATS`].
const FIRST_BAR_NOTE: usize = 6;
/// The default `div`: a straight quarter note.
const DEFAULT_DIV: f32 = 13.0;

/// Quarter-note beats per cycle of `div` index `i` at a time signature `(num, den)`.
pub fn div_beats(i: usize, time_signature: (u8, u8)) -> f64 {
    let i = i.min(DIV_LABELS.len() - 1);
    let base = i / 3;
    let mut beats = DIV_BEATS[base];
    if base >= FIRST_BAR_NOTE {
        let (num, den) = time_signature;
        let bar = f64::from(num.max(1)) * 4.0 / f64::from(den.max(1));
        beats *= bar / 4.0;
    }
    match i % 3 {
        0 => beats * (2.0 / 3.0),
        1 => beats,
        _ => beats * 1.5,
    }
}

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    (min, max, default): (f32, f32, f32),
    taper: Taper,
    unit: Unit,
    discrete: bool,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper,
        unit,
        discrete,
    }
}

const SHAPES: &[&str] = &[
    "sine",
    "triangle",
    "saw up",
    "saw down",
    "pulse",
    "sample & hold",
    "smooth random",
];

macro_rules! lfo_params {
    ($base:expr, $key:literal, $name:literal, $shape:expr, $rate:expr) => {
        [
            param(
                $base + O_SHAPE,
                concat!($key, "_shape"),
                concat!($name, " shape"),
                (0.0, 6.0, $shape),
                Taper::Linear,
                Unit::Choice(SHAPES),
                true,
            ),
            param(
                $base + O_MORPH,
                concat!($key, "_morph"),
                concat!($name, " morph"),
                (-1.0, 1.0, 0.0),
                Taper::Linear,
                Unit::None,
                false,
            ),
            param(
                $base + O_RATE,
                concat!($key, "_rate"),
                concat!($name, " rate"),
                (0.01, 40.0, $rate),
                Taper::Exponential,
                Unit::Hz,
                false,
            ),
            param(
                $base + O_SYNC,
                concat!($key, "_sync"),
                concat!($name, " tempo sync"),
                (0.0, 1.0, 0.0),
                Taper::Linear,
                Unit::Toggle,
                true,
            ),
            param(
                $base + O_DIV,
                concat!($key, "_div"),
                concat!($name, " division"),
                (0.0, 29.0, DEFAULT_DIV),
                Taper::Linear,
                Unit::Choice(&DIV_LABELS),
                true,
            ),
            param(
                $base + O_PHASE,
                concat!($key, "_phase"),
                concat!($name, " phase"),
                (0.0, 1.0, 0.0),
                Taper::Linear,
                Unit::None,
                false,
            ),
            param(
                $base + O_AMOUNT,
                concat!($key, "_amount"),
                concat!($name, " amount"),
                (0.0, 1.0, 1.0),
                Taper::Linear,
                Unit::Percent,
                false,
            ),
            param(
                $base + O_POLARITY,
                concat!($key, "_polarity"),
                concat!($name, " polarity"),
                (0.0, 1.0, 0.0),
                Taper::Linear,
                Unit::Choice(&["bipolar", "unipolar"]),
                true,
            ),
            param(
                $base + O_KEY_SYNC,
                concat!($key, "_key_sync"),
                concat!($name, " key sync"),
                (0.0, 1.0, 0.0),
                Taper::Linear,
                Unit::Toggle,
                true,
            ),
            param(
                $base + O_RATE_CV,
                concat!($key, "_rate_cv_trim"),
                concat!($name, " rate CV trim"),
                (-1.0, 1.0, 1.0),
                Taper::Linear,
                Unit::None,
                false,
            ),
        ]
    };
}

const LFO1: [ParamDescriptor; 10] = lfo_params!(0, "lfo1", "LFO 1", 0.0, 1.0);
const LFO2: [ParamDescriptor; 10] = lfo_params!(LFO_PARAMS, "lfo2", "LFO 2", 1.0, 0.5);

const fn all_params() -> [ParamDescriptor; 26] {
    let mut out = [LFO1[0]; 26];
    let mut i = 0;
    while i < 10 {
        out[i] = LFO1[i];
        out[10 + i] = LFO2[i];
        i += 1;
    }
    out[20] = param(
        P_ENV_ATTACK,
        "env_attack",
        "Env attack",
        (1.0, 10_000.0, 10.0),
        Taper::Exponential,
        Unit::Ms,
        false,
    );
    out[21] = param(
        P_ENV_DECAY,
        "env_decay",
        "Env decay",
        (1.0, 10_000.0, 200.0),
        Taper::Exponential,
        Unit::Ms,
        false,
    );
    out[22] = param(
        P_ENV_SUSTAIN,
        "env_sustain",
        "Env sustain",
        (0.0, 1.0, 0.7),
        Taper::Linear,
        Unit::Percent,
        false,
    );
    out[23] = param(
        P_ENV_RELEASE,
        "env_release",
        "Env release",
        (1.0, 10_000.0, 300.0),
        Taper::Exponential,
        Unit::Ms,
        false,
    );
    out[24] = param(
        P_ENV_AMOUNT,
        "env_amount",
        "Env amount",
        (0.0, 1.0, 1.0),
        Taper::Linear,
        Unit::Percent,
        false,
    );
    out[25] = param(
        P_ENV_MODE,
        "env_mode",
        "Env mode",
        (0.0, 1.0, 0.0),
        Taper::Linear,
        Unit::Choice(&["retrigger", "legato"]),
        true,
    );
    out
}

static PARAMS: [ParamDescriptor; 26] = all_params();

const NOTES_IN: SocketId = SocketId(0);
const GATE_CV_IN: SocketId = SocketId(1);
const LFO1_RATE_CV_IN: SocketId = SocketId(2);
const LFO1_AMOUNT_CV_IN: SocketId = SocketId(3);
const LFO2_RATE_CV_IN: SocketId = SocketId(4);
const LFO2_AMOUNT_CV_IN: SocketId = SocketId(5);
const LFO1_CV_OUT: SocketId = SocketId(6);
const LFO2_CV_OUT: SocketId = SocketId(7);
const ENV_CV_OUT: SocketId = SocketId(8);

const fn sock(
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
    sock(
        NOTES_IN,
        "notes_in",
        "Notes in",
        SocketKind::Notes,
        Direction::Input,
    ),
    sock(
        GATE_CV_IN,
        "gate_cv_in",
        "Gate CV in",
        SocketKind::Cv,
        Direction::Input,
    ),
    sock(
        LFO1_RATE_CV_IN,
        "lfo1_rate_cv_in",
        "LFO 1 rate CV in",
        SocketKind::Cv,
        Direction::Input,
    ),
    sock(
        LFO1_AMOUNT_CV_IN,
        "lfo1_amount_cv_in",
        "LFO 1 amount CV in",
        SocketKind::Cv,
        Direction::Input,
    ),
    sock(
        LFO2_RATE_CV_IN,
        "lfo2_rate_cv_in",
        "LFO 2 rate CV in",
        SocketKind::Cv,
        Direction::Input,
    ),
    sock(
        LFO2_AMOUNT_CV_IN,
        "lfo2_amount_cv_in",
        "LFO 2 amount CV in",
        SocketKind::Cv,
        Direction::Input,
    ),
    sock(
        LFO1_CV_OUT,
        "lfo1_cv_out",
        "LFO 1 CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
    sock(
        LFO2_CV_OUT,
        "lfo2_cv_out",
        "LFO 2 CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
    sock(
        ENV_CV_OUT,
        "env_cv_out",
        "Envelope CV out",
        SocketKind::Cv,
        Direction::Output,
    ),
];

/// Static description of the Mod Source.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.mod-source",
    name: "Mod Source",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`ModSource`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(ModSource::new()),
    }
}

/// Golden-ratio constant mixing the key-sync restart counter into the seed.
const SEED_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// A sanitised CV sample clamped to `lo..=hi` (non-finite gives 0).
#[inline]
fn cv(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { 0.0 }
}

/// Transport data of one batch shared by both LFOs.
#[derive(Clone, Copy)]
struct BatchTime {
    tick: f64,
    tpf: f64,
    wrap: Option<(usize, f64)>,
    playing: bool,
    sample_rate: f64,
    /// Ticks per frame at the batch tempo (used while stopped).
    tempo_tpf: f64,
    time_signature: (u8, u8),
    jump: bool,
}

/// One LFO: parameters and phase state.
#[derive(Debug, Clone)]
struct Lfo {
    index: u64,
    shape: ModShape,
    morph: f32,
    rate: f32,
    sync: bool,
    div: usize,
    phase: f32,
    amount: f32,
    unipolar: bool,
    key_sync: bool,
    rate_cv: f32,
    /// Phase in cycles since the last (re)start, before the `phase` offset.
    acc: f64,
    /// Locked sync: cycles of the song-locked phase at the last key-sync restart.
    origin: f64,
    retrig: u64,
    was_locked: bool,
    cache_cycle: i64,
    cache: [f32; 3],
    cache_ok: bool,
    last_phase: f32,
}

impl Lfo {
    fn new(index: u64, shape: ModShape, rate: f32) -> Self {
        Self {
            index,
            shape,
            morph: 0.0,
            rate,
            sync: false,
            div: DEFAULT_DIV as usize,
            phase: 0.0,
            amount: 1.0,
            unipolar: false,
            key_sync: false,
            rate_cv: 1.0,
            acc: 0.0,
            origin: 0.0,
            retrig: 0,
            was_locked: false,
            cache_cycle: 0,
            cache: [0.0; 3],
            cache_ok: false,
            last_phase: 0.0,
        }
    }

    fn reset(&mut self) {
        self.acc = 0.0;
        self.origin = 0.0;
        self.retrig = 0;
        self.was_locked = false;
        self.cache_ok = false;
        self.last_phase = 0.0;
    }

    fn set_param(&mut self, offset: u32, v: f32) {
        match offset {
            O_SHAPE => self.shape = ModShape::from_index(v.round().max(0.0) as usize),
            O_MORPH => self.morph = v.clamp(-1.0, 1.0),
            O_RATE => self.rate = v.clamp(0.01, 40.0),
            O_SYNC => self.sync = v >= 0.5,
            O_DIV => self.div = (v.round().max(0.0) as usize).min(DIV_LABELS.len() - 1),
            O_PHASE => self.phase = v.clamp(0.0, 1.0),
            O_AMOUNT => self.amount = v.clamp(0.0, 1.0),
            O_POLARITY => self.unipolar = v >= 0.5,
            O_KEY_SYNC => self.key_sync = v >= 0.5,
            O_RATE_CV => self.rate_cv = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn rand_value(&self, seed: u64, cycle: i64) -> f32 {
        let s = seed ^ self.retrig.wrapping_mul(SEED_MIX);
        (unit_f64(hash64(s, self.index, cycle as u64)) * 2.0 - 1.0) as f32
    }

    /// Renders one batch into `out`.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        seed: u64,
        t: &BatchTime,
        rate_cv: Option<&Batch>,
        amount_cv: Option<&Batch>,
        note_on: &[bool; BATCH],
        out: &mut Batch,
    ) {
        let locked = self.sync && t.playing;
        let tpc = if self.sync {
            div_beats(self.div, t.time_signature) * PPQ as f64
        } else {
            1.0
        };
        let cv_active = rate_cv.is_some() && self.rate_cv != 0.0;
        let base_inc = if self.sync {
            t.tempo_tpf / tpc
        } else {
            f64::from(self.rate) / t.sample_rate
        };
        let offset = f64::from(self.phase);
        if locked && !self.was_locked {
            self.origin = 0.0;
        }
        let mut realign = t.jump || !self.was_locked;
        for n in 0..BATCH {
            let (tick, wrapped_here) = match t.wrap {
                Some((o, to)) if n >= o => (to + (n - o) as f64 * t.tpf, n == o),
                _ => (t.tick + n as f64 * t.tpf, false),
            };
            let mul = match rate_cv {
                Some(c) => f64::from(exp2_det(
                    cv(c[n], -1.0, 1.0) * self.rate_cv * RATE_CV_OCTAVES,
                )),
                None => 1.0,
            };
            let reset = self.key_sync && note_on[n];
            if reset {
                self.retrig = self.retrig.wrapping_add(1);
                self.cache_ok = false;
            }
            if locked {
                let lt = tick / tpc;
                if reset {
                    self.origin = lt;
                    self.acc = 0.0;
                } else if !cv_active || realign || wrapped_here {
                    self.acc = lt - self.origin;
                }
                realign = false;
            } else if reset {
                self.acc = 0.0;
            }

            let total = self.acc + offset;
            let cycle = total.floor();
            let frac = total - cycle;
            let rand = if self.shape.is_random() {
                let c = cycle as i64;
                if !self.cache_ok || c != self.cache_cycle {
                    self.cache = [
                        self.rand_value(seed, c.wrapping_sub(1)),
                        self.rand_value(seed, c),
                        self.rand_value(seed, c.wrapping_add(1)),
                    ];
                    self.cache_cycle = c;
                    self.cache_ok = true;
                }
                self.cache
            } else {
                [0.0; 3]
            };
            let mut v = mod_shape(self.shape, frac, self.morph, rand);
            if self.unipolar {
                v = 0.5 * (v + 1.0);
            }
            let amt = self.amount
                * match amount_cv {
                    Some(c) => cv(c[n], 0.0, 1.0),
                    None => 1.0,
                };
            out[n] = (v * amt).clamp(-1.0, 1.0);
            self.last_phase = frac as f32;

            if locked {
                if cv_active {
                    self.acc += t.tpf / tpc * mul;
                }
            } else {
                self.acc += (base_inc * mul).min(0.5);
            }
        }
        self.was_locked = locked;
    }
}

/// The envelope with its gate logic.
#[derive(Debug, Clone)]
struct Envelope {
    env: ModEnv,
    amount: f32,
    legato: bool,
    held: u128,
    cv_gate: bool,
}

impl Envelope {
    fn new() -> Self {
        Self {
            env: ModEnv::new(48_000.0),
            amount: 1.0,
            legato: false,
            held: 0,
            cv_gate: false,
        }
    }

    fn reset(&mut self) {
        self.env.reset();
        self.held = 0;
        self.cv_gate = false;
    }

    fn press(&mut self, was_active: bool) {
        if !self.legato || !was_active {
            self.env.gate_on();
        }
    }

    fn release_check(&mut self) {
        if self.held == 0 && !self.cv_gate {
            self.env.gate_off();
        }
    }

    fn event(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, .. } => {
                let was = self.held != 0 || self.cv_gate;
                self.held |= 1u128 << (key & 127);
                self.press(was);
            }
            NoteKind::Off { key } => {
                self.held &= !(1u128 << (key & 127));
                self.release_check();
            }
            NoteKind::AllOff => {
                self.held = 0;
                self.release_check();
            }
            _ => {}
        }
    }
}

/// The two-LFO plus envelope CV source.
#[derive(Debug, Clone)]
pub struct ModSource {
    sample_rate: f64,
    seed: u64,
    lfos: [Lfo; 2],
    env: Envelope,
    attack_ms: f32,
    decay_ms: f32,
    sustain: f32,
    release_ms: f32,
    clock: JumpTracker,
    // Meter state.
    ring: [[f32; SCOPE_POINTS]; 3],
    pushes: u32,
    dec_acc: f64,
    dec_interval: f64,
    last: [f32; 3],
    stage: f32,
}

impl Default for ModSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ModSource {
    /// A Mod Source with default params.
    pub fn new() -> Self {
        let mut m = Self {
            sample_rate: 48_000.0,
            seed: 0,
            lfos: [
                Lfo::new(0, ModShape::Sine, 1.0),
                Lfo::new(1, ModShape::Triangle, 0.5),
            ],
            env: Envelope::new(),
            attack_ms: 10.0,
            decay_ms: 200.0,
            sustain: 0.7,
            release_ms: 300.0,
            clock: JumpTracker::new(),
            ring: [[0.0; SCOPE_POINTS]; 3],
            pushes: 0,
            dec_acc: 0.0,
            dec_interval: 120.0,
            last: [0.0; 3],
            stage: 0.0,
        };
        m.update_env();
        m
    }

    fn update_env(&mut self) {
        self.env
            .env
            .set_adsr(self.attack_ms, self.decay_ms, self.sustain, self.release_ms);
    }

    fn clear_state(&mut self) {
        for l in &mut self.lfos {
            l.reset();
        }
        self.env.reset();
        self.clock.invalidate();
        self.ring = [[0.0; SCOPE_POINTS]; 3];
        self.pushes = 0;
        self.dec_acc = 0.0;
        self.last = [0.0; 3];
        self.stage = 0.0;
    }
}

impl Device for ModSource {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz();
        self.seed = info.seed;
        self.env.env.set_sample_rate(self.sample_rate as f32);
        self.dec_interval = self.sample_rate / SCOPE_RATE_HZ;
        self.update_env();
        self.clear_state();
    }

    fn reset(&mut self) {
        self.clear_state();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            0..=9 => self.lfos[0].set_param(id.0, v),
            10..=19 => self.lfos[1].set_param(id.0 - LFO_PARAMS, v),
            P_ENV_ATTACK => {
                self.attack_ms = v.clamp(1.0, 10_000.0);
                self.update_env();
            }
            P_ENV_DECAY => {
                self.decay_ms = v.clamp(1.0, 10_000.0);
                self.update_env();
            }
            P_ENV_SUSTAIN => {
                self.sustain = v.clamp(0.0, 1.0);
                self.update_env();
            }
            P_ENV_RELEASE => {
                self.release_ms = v.clamp(1.0, 10_000.0);
                self.update_env();
            }
            P_ENV_AMOUNT => self.env.amount = v.clamp(0.0, 1.0),
            P_ENV_MODE => self.env.legato = v >= 0.5,
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let time = ctx.time;
        let tpf = time.ticks_per_frame;
        let end_tick = match time.loop_wrap {
            Some(w) => w.to_tick + (BATCH - usize::from(w.offset).min(BATCH)) as f64 * tpf,
            None => time.tick + BATCH as f64 * tpf,
        };
        let jump = self.clock.observe(time.tick, tpf, 0);
        if time.playing {
            self.clock.advance(end_tick, tpf, 0);
        } else {
            self.clock.invalidate();
        }
        let bt = BatchTime {
            tick: time.tick,
            tpf,
            wrap: time
                .loop_wrap
                .map(|w| (usize::from(w.offset).min(BATCH), w.to_tick)),
            playing: time.playing,
            sample_rate: self.sample_rate,
            tempo_tpf: time.tempo.bpm() * PPQ as f64 / (60.0 * self.sample_rate),
            time_signature: time.time_signature,
            jump,
        };

        let mut note_on = [false; BATCH];
        for e in ctx.notes_in(NOTES_IN) {
            if matches!(e.kind, NoteKind::On { .. }) {
                note_on[usize::from(e.offset).min(BATCH - 1)] = true;
            }
        }
        let grab = |ctx: &ProcessContext<'_>, id: SocketId| -> Option<Batch> {
            ctx.is_connected(id).then(|| *ctx.input(id))
        };
        let rate1 = grab(ctx, LFO1_RATE_CV_IN);
        let amt1 = grab(ctx, LFO1_AMOUNT_CV_IN);
        let rate2 = grab(ctx, LFO2_RATE_CV_IN);
        let amt2 = grab(ctx, LFO2_AMOUNT_CV_IN);
        let gate = grab(ctx, GATE_CV_IN);

        let mut out1 = [0.0f32; BATCH];
        let mut out2 = [0.0f32; BATCH];
        let mut out_env = [0.0f32; BATCH];
        self.lfos[0].run(
            self.seed,
            &bt,
            rate1.as_ref(),
            amt1.as_ref(),
            &note_on,
            &mut out1,
        );
        self.lfos[1].run(
            self.seed,
            &bt,
            rate2.as_ref(),
            amt2.as_ref(),
            &note_on,
            &mut out2,
        );

        // Envelope: notes at their offsets, then the gate CV of the same sample.
        let events = ctx.notes_in(NOTES_IN);
        let mut ei = 0;
        if gate.is_none() && self.env.cv_gate {
            self.env.cv_gate = false;
            self.env.release_check();
        }
        for n in 0..BATCH {
            while ei < events.len() && usize::from(events[ei].offset) <= n {
                self.env.event(events[ei].kind);
                ei += 1;
            }
            if let Some(g) = &gate {
                let high = cv(g[n], -1e30, 1e30) > 0.5;
                if high && !self.env.cv_gate {
                    let was = self.env.held != 0;
                    self.env.cv_gate = true;
                    self.env.press(was);
                } else if !high && self.env.cv_gate {
                    self.env.cv_gate = false;
                    self.env.release_check();
                }
            }
            out_env[n] = (self.env.env.next_sample() * self.env.amount).clamp(0.0, 1.0);
        }

        // Scope history: one point every `sample_rate / SCOPE_RATE_HZ` frames.
        for n in 0..BATCH {
            self.dec_acc += 1.0;
            if self.dec_acc >= self.dec_interval {
                self.dec_acc -= self.dec_interval;
                for (ring, v) in self.ring.iter_mut().zip([out1[n], out2[n], out_env[n]]) {
                    ring.rotate_left(1);
                    ring[SCOPE_POINTS - 1] = v;
                }
                self.pushes = (self.pushes + 1) % 4096;
            }
        }
        self.last = [out1[BATCH - 1], out2[BATCH - 1], out_env[BATCH - 1]];
        self.stage = match self.env.env.stage() {
            AdsrStage::Idle | AdsrStage::Fade => 0.0,
            AdsrStage::Attack => 1.0,
            AdsrStage::Decay => 2.0,
            AdsrStage::Sustain => 3.0,
            AdsrStage::Release => 4.0,
        };

        *ctx.output(LFO1_CV_OUT) = out1;
        *ctx.output(LFO2_CV_OUT) = out2;
        *ctx.output(ENV_CV_OUT) = out_env;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let mut v = [0.0f32; METER_COUNT];
        v[0] = self.last[0];
        v[1] = self.last[1];
        v[2] = self.last[2];
        v[3] = self.lfos[0].last_phase;
        v[4] = self.lfos[1].last_phase;
        v[5] = self.stage;
        v[6] = self.pushes as f32;
        for (s, ring) in self.ring.iter().enumerate() {
            let base = 7 + s * SCOPE_POINTS;
            v[base..base + SCOPE_POINTS].copy_from_slice(ring);
        }
        v[31] = (1000.0 / SCOPE_RATE_HZ) as f32;
        let n = out.len().min(METER_COUNT);
        out[..n].copy_from_slice(&v[..n]);
        n
    }

    fn latency_frames(&self) -> u32 {
        0
    }
}
