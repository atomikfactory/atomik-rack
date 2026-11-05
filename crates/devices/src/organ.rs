//! `atomik.organ`: a drawbar organ. Nine drawbars mix the harmonics of a shared tone generator,
//! a percussion on the second or third harmonic, a key click, a vibrato and chorus scanner and an
//! overdrive.
//!
//! # Signal flow
//!
//! ```text
//! tone generator (one sine per semitone) -> key contacts -> nine drawbars -> + percussion + click
//!   -> scanner (vibrato / chorus) -> overdrive -> level -> soft limit -> volume and expression
//! ```
//!
//! * **Tone generator.** One sine wheel per semitone, from 12 semitones below the lowest key to
//!   well above the highest, all of them running at once and phase-locked to each other (each is a
//!   rotating phasor, advanced with additions and multiplications only, and normalised once per
//!   batch). A chord therefore shares its wheels: two drawbars of two keys that land on the same
//!   pitch read the same wheel, as on the instrument this design follows. The wheels start at
//!   scattered phases that are a pure function of the device seed, so the first chord is not a spike.
//!   A wheel whose frequency is above 45 % of the sample rate is left out, so nothing aliases and a
//!   high key simply loses its highest drawbars. The generator runs only while a key sounds.
//! * **Drawbars.** Nine footages, in equal temperament: 16', 5 1/3', 8', 4', 2 2/3', 2', 1 3/5',
//!   1 1/3' and 1', that is `-12, +7, 0, +12, +19, +24, +28, +31, +36` semitones from the key. Each
//!   drawbar has nine positions: 0 is off and every step up is 3 dB louder, 8 is full level. A
//!   drawbar move is smoothed over 10 ms.
//! * **Keys.** Every key has a gate and an envelope that opens in about 1 ms and closes in about
//!   4 ms (no velocity: an organ key is on or off). Sixteen keys can sound. A key struck again
//!   while it still sounds uses the same voice; when all sixteen are taken, the quietest released
//!   voice (else the oldest key) is reused. The sustain pedal (CC 64) holds released keys, volume
//!   (CC 7) and expression (CC 11) multiply the output as a square law, the other controllers are
//!   ignored.
//! * **Percussion.** `perc.mode` adds the 2nd or 3rd harmonic (the 4' or the 2 2/3' wheel of each
//!   sounding key) with a decay: `perc.decay` Fast or Slow is the time the tone takes to fall by
//!   60 dB (0.2 s or 0.9 s) and `perc.level` Normal or Soft (6 dB lower). The decay is one envelope
//!   for the whole instrument that restarts only when a key is struck while no other key is held,
//!   so a run of legato notes is not re-struck. While percussion is on, the 1' drawbar is silent.
//! * **Key click.** `key.click` adds a burst of high-passed noise (a decay of 4 ms to -60 dB) at a
//!   key press, and a smaller one at a release. The noise is a function of the device seed.
//! * **Scanner.** `scanner.type` is Off, the vibrato V1 to V3 or the chorus C1 to C3. The signal goes
//!   through a delay of 0.7 ms swung by a sine of `scanner.rate` Hz; V1, V2 and V3 swing it by
//!   0.12, 0.24 and 0.42 ms and use only the delayed signal (a pitch vibrato), C1 to C3 swing by the
//!   same amounts but add the delayed signal to the direct one half and half, and the right output
//!   reads the swing a quarter turn later for width. Changes between types glide over 30 ms; Off is
//!   bit-exact bypass.
//! * **Overdrive.** `out.drive` runs a rational soft clipper with up to 18 dB of gain in front,
//!   a square-root makeup behind; 0 is bit-exact off. `out.level` is a gain in dB after it, and a
//!   soft limit keeps the output below 2 for any setting.
//! * **Sounds.** `sound` selects a factory sound ([`sounds`]). The device ignores it; the app turns
//!   a change of `sound` into a batch of param changes (one undo step).
//!
//! Everything is deterministic (no libm call in the audio path), allocation-free and finite for any
//! parameters and notes. The stereo output feeds the rotary speaker ([`crate::rotary_speaker`])
//! directly.

use crate::controller_state::{ControllerState, Effect};
use crate::tool_support::{choice, count, finite, linear};
use atomik_core::{BATCH, NoteEvent, NoteKind, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::rng::{hash64, unit_f64};
use atomik_dsp::{DelayLine, Rng, Smoother, exp2_det, exp2_det_f64, sin_cycles};

pub mod sounds;

/// Number of drawbars.
pub const BARS: usize = 9;
/// Voices (keys that can sound at once).
pub const MAX_VOICES: usize = 16;
/// Wheels: pitches from 12 semitones below key 0 to 36 above key 127.
const WHEELS: usize = 176;
/// The wheel index of pitch 0 (the offset of the lowest footage).
const PITCH_BIAS: i32 = 12;
/// Marks a drawbar whose wheel is above the limit; reads the always-zero last slot.
const DEAD: u8 = WHEELS as u8;
/// Semitones above the key of each drawbar footage.
pub const BAR_OFFSETS: [i32; BARS] = [-12, 7, 0, 12, 19, 24, 28, 31, 36];
/// Wheels above this share of the sample rate are not generated.
const WHEEL_LIMIT: f64 = 0.45;

const P_SOUND: u32 = 0;
const P_BAR_FIRST: u32 = 1;
const P_PERC_MODE: u32 = 10;
const P_PERC_DECAY: u32 = 11;
const P_PERC_LEVEL: u32 = 12;
const P_CLICK: u32 = 13;
const P_SCANNER: u32 = 14;
const P_SCANNER_RATE: u32 = 15;
const P_DRIVE: u32 = 16;
const P_LEVEL: u32 = 17;

/// The `perc.mode` choices.
const PERC_MODES: &[&str] = &["Off", "2nd", "3rd"];
/// The `perc.decay` choices.
const PERC_DECAYS: &[&str] = &["Fast", "Slow"];
/// The `perc.level` choices.
const PERC_LEVELS: &[&str] = &["Soft", "Normal"];
/// The `scanner.type` choices.
const SCANNERS: &[&str] = &["Off", "V1", "V2", "V3", "C1", "C2", "C3"];
const SOUNDS: Unit = Unit::Choice(&sounds::SOUND_NAMES);

/// Decay times (to -60 dB) of the percussion, seconds: fast and slow.
const PERC_DECAY_S: [f64; 2] = [0.2, 0.9];
/// Percussion level of Soft and Normal.
const PERC_LEVEL: [f32; 2] = [0.5, 1.0];
/// Scanner delay swing in milliseconds for V1..V3 and C1..C3.
const SCAN_DEPTH_MS: [f32; 3] = [0.12, 0.24, 0.42];
/// The scanner's mean delay, milliseconds.
const SCAN_CENTRE_MS: f32 = 0.7;
/// The longest scanner delay, milliseconds (centre plus the deepest swing).
const SCAN_MAX_MS: f64 = 1.2;
/// Key envelope time constants, milliseconds.
const ATTACK_MS: f64 = 1.0;
const RELEASE_MS: f64 = 4.0;
/// A released voice below this level is idle.
const IDLE: f32 = 1e-4;
/// Scale from the drawbar sum to the output (one key at full registration stays well below 1).
const OUT_SCALE: f32 = 0.15;
/// Key click: the noise level at a click of 1 (before the scale), the decay in milliseconds to
/// -60 dB, and the share a key release gives.
const CLICK_LEVEL: f32 = 0.5;
const CLICK_DECAY_MS: f64 = 4.0;
const CLICK_RELEASE: f32 = 0.4;
/// The drive's most gain, as a linear factor (18 dB).
const MAX_DRIVE_GAIN: f32 = 7.943_282;
/// Seconds the smoothed drawbars and scanner take to follow a change.
const BAR_SMOOTH_MS: f32 = 10.0;
const SCAN_SMOOTH_MS: f32 = 30.0;

/// Names of the drawbar params, in footage order.
const BAR_KEYS: [&str; BARS] = [
    "drawbar.16",
    "drawbar.5_1_3",
    "drawbar.8",
    "drawbar.4",
    "drawbar.2_2_3",
    "drawbar.2",
    "drawbar.1_3_5",
    "drawbar.1_1_3",
    "drawbar.1",
];
/// Display names of the drawbars.
const BAR_NAMES: [&str; BARS] = [
    "16'", "5 1/3'", "8'", "4'", "2 2/3'", "2'", "1 3/5'", "1 1/3'", "1'",
];

const fn bar(i: usize, default: f32) -> ParamDescriptor {
    count(
        P_BAR_FIRST + i as u32,
        BAR_KEYS[i],
        BAR_NAMES[i],
        0.0,
        8.0,
        default,
    )
}

static PARAMS: [ParamDescriptor; 18] = [
    ParamDescriptor {
        id: ParamId(P_SOUND),
        key: "sound",
        name: "Sound",
        min: 0.0,
        max: (sounds::SOUND_NAMES.len() - 1) as f32,
        default: 0.0,
        taper: atomik_core::Taper::Linear,
        unit: SOUNDS,
        discrete: true,
    },
    bar(0, 8.0),
    bar(1, 8.0),
    bar(2, 8.0),
    bar(3, 0.0),
    bar(4, 0.0),
    bar(5, 0.0),
    bar(6, 0.0),
    bar(7, 0.0),
    bar(8, 0.0),
    choice(P_PERC_MODE, "perc.mode", "Percussion", PERC_MODES, 0.0),
    choice(P_PERC_DECAY, "perc.decay", "Decay", PERC_DECAYS, 0.0),
    choice(P_PERC_LEVEL, "perc.level", "Perc level", PERC_LEVELS, 1.0),
    linear(
        P_CLICK,
        "key.click",
        "Key click",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    choice(P_SCANNER, "scanner.type", "Scanner", SCANNERS, 0.0),
    linear(
        P_SCANNER_RATE,
        "scanner.rate",
        "Scan rate",
        4.0,
        9.0,
        6.8,
        Unit::Hz,
    ),
    linear(P_DRIVE, "out.drive", "Drive", 0.0, 1.0, 0.0, Unit::Percent),
    linear(P_LEVEL, "out.level", "Level", -24.0, 6.0, 0.0, Unit::Db),
];

const NOTES_IN: SocketId = SocketId(0);
const OUT_L: SocketId = SocketId(1);
const OUT_R: SocketId = SocketId(2);

static SOCKETS: [SocketDescriptor; 3] = [
    SocketDescriptor {
        id: NOTES_IN,
        key: "notes_in",
        name: "Notes in",
        kind: SocketKind::Notes,
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

/// Static description of the organ.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.organ",
    name: "Organ",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Organ`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Organ::new()),
    }
}

/// The linear gain of a drawbar position: 0 is silent, 8 is 1 and each step is 3 dB.
pub fn bar_gain(position: u8) -> f32 {
    const STEP: f32 = 0.707_945_8;
    if position == 0 {
        return 0.0;
    }
    let mut g = 1.0f32;
    for _ in position.min(8)..8 {
        g *= STEP;
    }
    g
}

/// The wheel each drawbar of `key` reads (`DEAD` where the wheel is not generated).
fn wheels_for(key: u8, live: usize) -> [u8; BARS] {
    std::array::from_fn(|k| {
        let w = (i32::from(key) + BAR_OFFSETS[k] + PITCH_BIAS) as usize;
        if w < live { w as u8 } else { DEAD }
    })
}

/// One key.
#[derive(Debug, Clone, Copy)]
struct Voice {
    key: u8,
    gate: bool,
    env: f32,
    age: u32,
    wheels: [u8; BARS],
}

impl Voice {
    const EMPTY: Self = Self {
        key: 0,
        gate: false,
        env: 0.0,
        age: 0,
        wheels: [DEAD; BARS],
    };

    fn sounding(&self) -> bool {
        self.gate || self.env > 0.0
    }
}

/// The scanner type as (delay swing in ms, share of the delayed signal, right-channel phase).
fn scanner_target(kind: u8) -> (f32, f32, f32) {
    match kind {
        0 => (SCAN_DEPTH_MS[0], 0.0, 0.0),
        1..=3 => (SCAN_DEPTH_MS[usize::from(kind) - 1], 1.0, 0.0),
        _ => (SCAN_DEPTH_MS[usize::from(kind.min(6)) - 4], 0.5, 0.25),
    }
}

/// `x` unchanged up to 1, then a smooth approach to 2.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        let t = (a - 1.0).min(1.0e6);
        (1.0 + t / (1.0 + t)).copysign(x)
    }
}

/// The organ device.
#[derive(Debug, Clone)]
pub struct Organ {
    sample_rate: f32,
    seed: u64,
    // Params.
    bar_pos: [u8; BARS],
    perc_mode: u8,
    perc_slow: bool,
    perc_normal: bool,
    click: f32,
    scanner: u8,
    scanner_rate: f32,
    drive: f32,
    level_db: f32,
    // Derived.
    drive_gain: f32,
    drive_makeup: f32,
    level: f32,
    perc_gain: f32,
    perc_coef: f32,
    attack: f32,
    release: f32,
    click_coef: f32,
    // State.
    fresh: bool,
    tune: f32,
    live: usize,
    wheel_x: [f64; WHEELS],
    wheel_y: [f64; WHEELS],
    wheel_c: [f64; WHEELS],
    wheel_s: [f64; WHEELS],
    sines: [f32; WHEELS + 1],
    voices: [Voice; MAX_VOICES],
    clock: u32,
    bars: [Smoother; BARS],
    perc_env: f32,
    click_env: f32,
    noise: Rng,
    noise_prev: f32,
    scan_depth: Smoother,
    scan_mix: Smoother,
    scan_spread: Smoother,
    scan_phase: f64,
    line_l: DelayLine,
    line_r: DelayLine,
    ctl: ControllerState,
}

impl Default for Organ {
    fn default() -> Self {
        Self::new()
    }
}

fn line_len(sample_rate: f32) -> usize {
    (SCAN_MAX_MS * 0.001 * f64::from(sample_rate)).ceil() as usize + 4
}

impl Organ {
    /// An organ with default params at 48 kHz (`prepare` sets the real rate and seed).
    pub fn new() -> Self {
        let mut o = Self {
            sample_rate: 48_000.0,
            seed: 0,
            bar_pos: [8, 8, 8, 0, 0, 0, 0, 0, 0],
            perc_mode: 0,
            perc_slow: false,
            perc_normal: true,
            click: 0.3,
            scanner: 0,
            scanner_rate: 6.8,
            drive: 0.0,
            level_db: 0.0,
            drive_gain: 1.0,
            drive_makeup: 1.0,
            level: 1.0,
            perc_gain: 0.0,
            perc_coef: 0.0,
            attack: 0.0,
            release: 0.0,
            click_coef: 0.0,
            fresh: true,
            tune: 0.0,
            live: 0,
            wheel_x: [1.0; WHEELS],
            wheel_y: [0.0; WHEELS],
            wheel_c: [1.0; WHEELS],
            wheel_s: [0.0; WHEELS],
            sines: [0.0; WHEELS + 1],
            voices: [Voice::EMPTY; MAX_VOICES],
            clock: 0,
            bars: [Smoother::new(0.0, BAR_SMOOTH_MS, 48_000.0); BARS],
            perc_env: 0.0,
            click_env: 0.0,
            noise: Rng::new(0),
            noise_prev: 0.0,
            scan_depth: Smoother::new(0.0, SCAN_SMOOTH_MS, 48_000.0),
            scan_mix: Smoother::new(0.0, SCAN_SMOOTH_MS, 48_000.0),
            scan_spread: Smoother::new(0.0, SCAN_SMOOTH_MS, 48_000.0),
            scan_phase: 0.0,
            line_l: DelayLine::new(line_len(48_000.0)),
            line_r: DelayLine::new(line_len(48_000.0)),
            ctl: ControllerState::new(),
        };
        o.derive();
        o.retune(0.0);
        o.reset();
        o
    }

    /// The per-sample coefficient of a one-pole approach with time constant `ms`.
    fn coefficient(ms: f64, sample_rate: f32) -> f32 {
        let n = ms * 0.001 * f64::from(sample_rate);
        (1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / n)) as f32
    }

    /// The per-sample multiplier of a fall by 60 dB in `ms` milliseconds.
    fn fall(ms: f64, sample_rate: f32) -> f32 {
        // 60 dB is a factor of 1000: 2^-9.965784...
        exp2_det_f64(-9.965_784_284_662_087 / (ms * 0.001 * f64::from(sample_rate))) as f32
    }

    /// Everything that follows from the params and the sample rate.
    fn derive(&mut self) {
        let sr = self.sample_rate;
        self.drive_gain = 1.0 + self.drive * (MAX_DRIVE_GAIN - 1.0);
        self.drive_makeup = 1.0 / self.drive_gain.sqrt();
        self.level = exp2_det(self.level_db * 0.166_096_4);
        let on = self.perc_mode != 0;
        let level = PERC_LEVEL[usize::from(self.perc_normal)];
        self.perc_gain = if on { level } else { 0.0 };
        let slow = usize::from(self.perc_slow);
        self.perc_coef = Self::fall(PERC_DECAY_S[slow] * 1000.0, sr);
        self.attack = Self::coefficient(ATTACK_MS, sr);
        self.release = Self::coefficient(RELEASE_MS, sr);
        self.click_coef = Self::fall(CLICK_DECAY_MS, sr);
        let targets: [f32; BARS] = std::array::from_fn(|b| self.bar_target(b));
        for (s, t) in self.bars.iter_mut().zip(targets) {
            s.set_time(BAR_SMOOTH_MS, sr);
            s.set_target(t);
        }
        let (depth, mix, spread) = scanner_target(self.scanner);
        for s in [
            &mut self.scan_depth,
            &mut self.scan_mix,
            &mut self.scan_spread,
        ] {
            s.set_time(SCAN_SMOOTH_MS, sr);
        }
        self.scan_depth.set_target(depth);
        self.scan_mix.set_target(mix);
        self.scan_spread.set_target(spread);
        self.ctl.set_sample_rate(sr);
    }

    /// The gain drawbar `b` aims for: its position, with the 1' drawbar silent under percussion.
    fn bar_target(&self, b: usize) -> f32 {
        if b == BARS - 1 && self.perc_mode != 0 {
            0.0
        } else {
            bar_gain(self.bar_pos[b])
        }
    }

    /// Computes the wheel steps for the master tune and the sample rate.
    fn retune(&mut self, tune_cents: f32) {
        self.tune = tune_cents;
        let sr = f64::from(self.sample_rate);
        let limit = WHEEL_LIMIT * sr;
        let mut live = 0;
        for w in 0..WHEELS {
            let pitch = w as f64 - f64::from(PITCH_BIAS);
            let hz = 440.0 * exp2_det_f64((pitch - 69.0 + f64::from(tune_cents) * 0.01) / 12.0);
            let step = hz / sr;
            self.wheel_c[w] = sin_cycles(step + 0.25);
            self.wheel_s[w] = sin_cycles(step);
            if hz < limit {
                live = w + 1;
            }
        }
        for s in &mut self.sines[live..] {
            *s = 0.0;
        }
        self.live = live;
    }

    /// The rational soft clipper (exact enough, and deterministic); `drive` 0 is bit-exact off.
    #[inline]
    fn saturate(&self, x: f32) -> f32 {
        if self.drive <= 0.0 {
            return x;
        }
        let u = (x * self.drive_gain).clamp(-3.0, 3.0);
        let y = u * (27.0 + u * u) / (27.0 + 9.0 * u * u);
        y * self.drive_makeup
    }

    fn any_sounding(&self) -> bool {
        self.voices.iter().any(Voice::sounding)
    }

    fn note_on(&mut self, key: u8) {
        let key = key.min(127);
        self.ctl.note_on(key);
        let held = self.voices.iter().any(|v| v.gate);
        let slot = self
            .voices
            .iter()
            .position(|v| v.sounding() && v.key == key)
            .or_else(|| self.voices.iter().position(|v| !v.sounding()))
            .unwrap_or_else(|| {
                // Every voice is taken: a released one, the quietest; else the oldest key.
                let mut best = 0;
                for (i, v) in self.voices.iter().enumerate() {
                    let b = &self.voices[best];
                    // Older is the smaller age (a wrapped counter compares by the difference).
                    let older = (v.age.wrapping_sub(b.age) as i32).cmp(&0);
                    let order = v.gate.cmp(&b.gate).then_with(|| {
                        if v.gate {
                            older
                        } else {
                            v.env.total_cmp(&b.env).then(older)
                        }
                    });
                    if order.is_lt() {
                        best = i;
                    }
                }
                best
            });
        let was_gated = self.voices[slot].gate && self.voices[slot].key == key;
        self.clock = self.clock.wrapping_add(1);
        let v = &mut self.voices[slot];
        v.key = key;
        v.gate = true;
        v.age = self.clock;
        v.wheels = wheels_for(key, self.live);
        if !held {
            self.perc_env = 1.0;
        }
        if !was_gated {
            self.click_env = 1.0;
        }
    }

    fn release_key(&mut self, key: u8) {
        let mut any = false;
        for v in self.voices.iter_mut().filter(|v| v.gate && v.key == key) {
            v.gate = false;
            any = true;
        }
        if any {
            self.click_env = self.click_env.max(CLICK_RELEASE);
        }
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, .. } => self.note_on(key),
            NoteKind::Off { key } => {
                let key = key.min(127);
                if self.ctl.note_off(key) {
                    self.release_key(key);
                }
            }
            NoteKind::AllOff => {
                self.ctl.all_off();
                for v in &mut self.voices {
                    v.gate = false;
                }
            }
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
            }
        }
    }

    /// Advances the wheels one sample.
    #[inline]
    fn spin(&mut self) {
        let n = self.live;
        let wheels = self.wheel_x[..n]
            .iter_mut()
            .zip(&mut self.wheel_y[..n])
            .zip(self.wheel_c[..n].iter().zip(&self.wheel_s[..n]));
        for (((x, y), (c, s)), out) in wheels.zip(&mut self.sines[..n]) {
            let nx = *x * c - *y * s;
            let ny = *x * s + *y * c;
            *x = nx;
            *y = ny;
            *out = ny as f32;
        }
    }

    /// Pulls every phasor back to the unit circle (one Newton step of `1 / sqrt`).
    fn normalise(&mut self) {
        let n = self.live;
        for (x, y) in self.wheel_x[..n].iter_mut().zip(&mut self.wheel_y[..n]) {
            let k = 1.5 - 0.5 * (*x * *x + *y * *y);
            *x *= k;
            *y *= k;
        }
    }

    /// Renders the samples `range` of the batch into `left` and `right`.
    fn render(&mut self, left: &mut [f32; BATCH], right: &mut [f32; BATCH], range: (usize, usize)) {
        let sr = f64::from(self.sample_rate);
        let perc_bar = if self.perc_mode == 2 { 4 } else { 3 };
        for n in range.0..range.1 {
            let mut sum = 0.0f32;
            let mut perc = 0.0f32;
            if self.any_sounding() {
                self.spin();
                let mut now = [0.0f32; BARS];
                for (g, s) in now.iter_mut().zip(&mut self.bars) {
                    *g = s.next_value();
                }
                let (attack, release) = (self.attack, self.release);
                for v in self.voices.iter_mut().filter(|v| v.sounding()) {
                    let (target, coef) = if v.gate {
                        (1.0, attack)
                    } else {
                        (0.0, release)
                    };
                    v.env += (target - v.env) * coef;
                    if !v.gate && v.env < IDLE {
                        v.env = 0.0;
                    }
                    let sines = &self.sines;
                    let s: f32 = now
                        .iter()
                        .zip(v.wheels)
                        .map(|(g, w)| g * sines[usize::from(w)])
                        .sum();
                    sum += s * v.env;
                    perc += self.sines[usize::from(v.wheels[perc_bar])] * v.env;
                }
            } else {
                for s in &mut self.bars {
                    s.next_value();
                }
            }
            if self.perc_env > 0.0 {
                sum += perc * self.perc_env * self.perc_gain;
                self.perc_env *= self.perc_coef;
                if self.perc_env < 1e-5 {
                    self.perc_env = 0.0;
                }
            }
            if self.click_env > 0.0 {
                let noise = self.noise.next_bipolar();
                let burst = noise - self.noise_prev;
                self.noise_prev = noise;
                sum += burst * self.click_env * self.click * CLICK_LEVEL;
                self.click_env *= self.click_coef;
                if self.click_env < 1e-5 {
                    self.click_env = 0.0;
                }
            }
            let x = finite(sum) * OUT_SCALE;

            // Scanner: a delay swung by a sine; the share of it that is heard follows the type.
            let depth = self.scan_depth.next_value();
            let mix = self.scan_mix.next_value();
            let spread = f64::from(self.scan_spread.next_value());
            let ms_to_samples = (sr * 0.001) as f32;
            let swing_l = sin_cycles(self.scan_phase) as f32;
            let swing_r = sin_cycles(self.scan_phase + spread) as f32;
            let delay_l = (SCAN_CENTRE_MS + depth * swing_l) * ms_to_samples;
            let delay_r = (SCAN_CENTRE_MS + depth * swing_r) * ms_to_samples;
            let (wet_l, wet_r) = (self.line_l.read(delay_l), self.line_r.read(delay_r));
            self.line_l.write(x);
            self.line_r.write(x);
            let p = self.scan_phase + f64::from(self.scanner_rate) / sr;
            self.scan_phase = p - p.floor();
            let (l, r) = if mix <= 0.0 {
                (x, x)
            } else {
                (x * (1.0 - mix) + wet_l * mix, x * (1.0 - mix) + wet_r * mix)
            };

            left[n] = soft_limit(self.saturate(l) * self.level);
            right[n] = soft_limit(self.saturate(r) * self.level);
        }
    }
}

impl Device for Organ {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.line_l = DelayLine::new(line_len(self.sample_rate));
        self.line_r = DelayLine::new(line_len(self.sample_rate));
        self.retune(0.0);
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.fresh = true;
        self.voices = [Voice::EMPTY; MAX_VOICES];
        self.clock = 0;
        self.perc_env = 0.0;
        self.click_env = 0.0;
        self.noise = Rng::new(hash64(self.seed, 0x6f72_6761, 1));
        self.noise_prev = 0.0;
        self.scan_phase = 0.0;
        self.line_l.clear();
        self.line_r.clear();
        self.ctl.reset();
        for w in 0..WHEELS {
            let turns = unit_f64(hash64(self.seed, w as u64, 2));
            self.wheel_x[w] = sin_cycles(turns + 0.25);
            self.wheel_y[w] = sin_cycles(turns);
        }
        self.sines = [0.0; WHEELS + 1];
        self.settle();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_SOUND => {}
            i @ P_BAR_FIRST..=9 => {
                let b = (i - P_BAR_FIRST) as usize;
                self.bar_pos[b] = v.round().clamp(0.0, 8.0) as u8;
                let t = self.bar_target(b);
                self.bars[b].set_target(t);
            }
            P_PERC_MODE => {
                self.perc_mode = v.round().clamp(0.0, 2.0) as u8;
                self.derive();
            }
            P_PERC_DECAY => {
                self.perc_slow = v.round() >= 1.0;
                self.derive();
            }
            P_PERC_LEVEL => {
                self.perc_normal = v.round() >= 1.0;
                self.derive();
            }
            P_CLICK => self.click = v.clamp(0.0, 1.0),
            P_SCANNER => {
                self.scanner = v.round().clamp(0.0, 6.0) as u8;
                self.derive();
            }
            P_SCANNER_RATE => self.scanner_rate = v.clamp(4.0, 9.0),
            P_DRIVE => {
                self.drive = v.clamp(0.0, 1.0);
                self.derive();
            }
            P_LEVEL => {
                self.level_db = v.clamp(-24.0, 6.0);
                self.derive();
            }
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            self.settle();
            self.fresh = false;
        }
        let tune = finite(ctx.time.tune_cents).clamp(-1_200.0, 1_200.0);
        if tune.to_bits() != self.tune.to_bits() {
            self.retune(tune);
        }
        let events = ctx.notes_in(NOTES_IN);
        let (mut left, mut right) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        let mut gain = [1.0f32; BATCH];
        let mut next = 0;
        let mut start = 0;
        // Render in segments between note events so events are sample-accurate.
        while start < BATCH {
            while next < events.len() && (events[next].offset as usize).min(BATCH - 1) <= start {
                let NoteEvent { kind, .. } = events[next];
                self.handle(kind);
                next += 1;
            }
            let end = events
                .get(next)
                .map_or(BATCH, |e| (e.offset as usize).clamp(start + 1, BATCH));
            self.render(&mut left, &mut right, (start, end));
            self.ctl.fill_gain(&mut gain[start..end]);
            start = end;
        }
        self.normalise();
        for n in 0..BATCH {
            left[n] *= gain[n];
            right[n] *= gain[n];
        }
        *ctx.output(OUT_L) = left;
        *ctx.output(OUT_R) = right;
    }
}

impl Organ {
    /// Jumps the smoothed values to their targets (the start of a run, after a reset).
    fn settle(&mut self) {
        for b in 0..BARS {
            let t = self.bar_target(b);
            self.bars[b].set_immediate(t);
        }
        let (depth, mix, spread) = scanner_target(self.scanner);
        self.scan_depth.set_immediate(depth);
        self.scan_mix.set_immediate(mix);
        self.scan_spread.set_immediate(spread);
    }
}

#[cfg(test)]
mod tests;
