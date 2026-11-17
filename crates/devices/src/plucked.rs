//! `atomik.plucked`: Plucked Strings, a physical-model instrument for guitars, basses, banjos and
//! the like. Each key plucks a waveguide string ([`atomik_dsp::pluck`]) that rings, bends and is
//! damped like one, and the sum goes through the body of the chosen instrument.
//!
//! # Signal flow
//!
//! ```text
//! pick (noise burst -> hardness low-pass -> position comb) -> string loop (1 or 2 per key)
//!   -> pan -> volume and expression -> DC block -> body -> output gain -> soft limit
//! ```
//!
//! * **Voices.** Sixteen. A key takes a free voice; with none free the quietest released voice
//!   (else the oldest) is stolen and fades out over 2 ms before the new note starts. A key struck
//!   again while its string still rings plucks the same string again (no click, as a tremolo
//!   picker would), so repeated notes do not eat voices. The sustain pedal (CC 64), volume (CC 7),
//!   expression (CC 11), pitch bend and its range (`bend.range`, RPN 0) work as on the other
//!   instruments, and the project's master tune shifts every string.
//! * **String.** The waveguide has the loop gain of a decay (`decay`, milliseconds for the
//!   fundamental to fall by 60 dB, at A3 and shortened for higher keys by `(220 Hz / f)^0.35`),
//!   a loop low-pass that follows the pitch (`brightness`) and a dispersion allpass chain
//!   (`stiffness`: the higher partials run sharp, from none to bell-like). Tuning compensates the
//!   exact delay of both filters, so a note is within a fraction of a cent of its key at every
//!   sample rate.
//! * **Pick.** `pick.hardness` is the cutoff of the excitation (1.2 kHz soft to 15 kHz hard, lower
//!   for a gentler strike), `pick.position` how far along the string it is struck (3 % to 50 %: the
//!   partials with a node there are cancelled). `vel.sens` is how much velocity matters: the
//!   level goes from `1 - vel.sens + vel.sens * velocity` and a soft strike is also duller.
//! * **Release.** A key-off damps the string as a finger would: the fall of 60 dB then takes
//!   `50 ms * (decay / 50 ms)^sustain`, so `sustain` 0 is a quick mute and 1 is let-ring (the
//!   release is the decay). A released voice is freed when it is inaudible.
//! * **Palm mute.** `palm_mute` (and the note's slide expression, whichever is larger) divides the
//!   decay by up to 10 and darkens the loop by up to 75 %.
//! * **Course.** `course.mode` Single, Unison (a second string, `course.detune` cents apart,
//!   spread evenly either side) or Octave (a second string an octave up, level 0.5, detuned by
//!   `course.detune` cents). The mode is read when a note starts.
//! * **Body.** `body` chooses an instrument body ([`atomik_dsp::pluck::body::BODY_NAMES`]) and
//!   `body.mix` how much of its filtering is heard.
//! * **Output.** `stereo.width` spreads the keys (low left, high right) and the two strings of a
//!   course; `out.gain` is in dB.
//!
//! # Per-note expression
//!
//! Each key's [`atomik_core::NoteKind::NoteExpr`] values are read like the other instruments, and
//! this is how playing techniques (bends, slides, vibrato, palm mutes, harmonics) reach the
//! strings:
//!
//! * **Tuning** (semitones) adds to the pitch, with the pitch bend. A change glides over 6 ms (a
//!   linear ramp of the delay length, so a bend curve never clicks); within the first 64 samples
//!   of a note it applies at once, so a pre-bend starts on pitch.
//! * **Slide** (`0..=1`) is the palm-mute amount of that note.
//! * **Pressure** is the technique code: below [`EXPR_NORMAL_BELOW`] (0.25) a normal note, up to
//!   [`EXPR_HARMONIC_BELOW`] (0.75) a harmonic, above that a dead note. [`EXPR_HARMONIC`] (0.5)
//!   and [`EXPR_DEAD`] (1.0) are the values to send. A harmonic is struck with a band-limited
//!   pick at the note's own pitch (its tuning expression carries the octave or fifth) and rings
//!   as a nearly pure tone; a dead note is a noise burst into a string damped to 45 ms, so it
//!   is a short, unpitched click. Both ignore the pick position. The technique is read when the
//!   note starts (the expression of a note follows its note-on at the same frame).
//! * The pressure of the key (`PolyPressure`, `ChannelPressure`) is not used.
//!
//! Everything is deterministic (the pick noise is a function of the device seed, the key and a note
//! counter), allocation-free in `process`, and finite for any parameters and notes.

use crate::controller_state::{
    ControllerState, DEFAULT_BEND_RANGE, Effect, KeyExpression, MAX_BEND_RANGE,
};
use crate::poly_synth::soft_limit;
use crate::tool_support::{choice, expo, finite, linear};
use atomik_core::{BATCH, NoteEvent, NoteKind, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::pluck::body::{BODY_NAMES, Body};
use atomik_dsp::pluck::{Pick, PickParams, PluckString, StringTone};
use atomik_dsp::rng::hash64;
use atomik_dsp::{STEAL_FADE_MS, Smoother, exp2_det, log2_det, sin_cycles};

/// Voices (keys that can sound at once).
pub const MAX_VOICES: usize = 16;
/// The pressure expression of a harmonic.
pub const EXPR_HARMONIC: f32 = 0.5;
/// The pressure expression of a dead note.
pub const EXPR_DEAD: f32 = 1.0;
/// Pressure expressions below this are normal notes.
pub const EXPR_NORMAL_BELOW: f32 = 0.25;
/// Pressure expressions below this (and not below [`EXPR_NORMAL_BELOW`]) are harmonics.
pub const EXPR_HARMONIC_BELOW: f32 = 0.75;

/// Samples between control updates (pitch, loop and pan of a voice); aligned to the sample count of
/// the device, so the output does not depend on how the host cuts the render.
const STEP: u32 = 16;
/// Time a change of tuning takes, milliseconds.
const GLIDE_MS: f32 = 6.0;
/// A note this young takes its tuning expression at once.
const IMMEDIATE: u32 = 64;
/// A released voice below this level is free.
const IDLE: f32 = 1e-5;
/// The level of the pick's burst at full velocity (its RMS).
const PICK_LEVEL: f32 = 0.2;
/// The shortest finger-mute fall of 60 dB, seconds.
const MUTE_S: f32 = 0.05;
/// Seconds the output gain takes to follow a change.
const GAIN_SMOOTH_MS: f32 = 10.0;
/// Pole of the DC blocker, as a corner in Hz.
const DC_HZ: f32 = 8.0;

const P_BODY: u32 = 0;
const P_BRIGHT: u32 = 1;
const P_PICK_POS: u32 = 2;
const P_PICK_HARD: u32 = 3;
const P_DECAY: u32 = 4;
const P_SUSTAIN: u32 = 5;
const P_PALM: u32 = 6;
const P_STIFF: u32 = 7;
const P_COURSE: u32 = 8;
const P_DETUNE: u32 = 9;
const P_BODY_MIX: u32 = 10;
const P_OUT: u32 = 11;
const P_WIDTH: u32 = 12;
const P_VEL: u32 = 13;
const P_BEND: u32 = 14;

/// The `course.mode` choices.
const COURSES: &[&str] = &["Single", "Unison", "Octave"];
const BODIES: Unit = Unit::Choice(&BODY_NAMES);

static PARAMS: [ParamDescriptor; 15] = [
    ParamDescriptor {
        id: ParamId(P_BODY),
        key: "body",
        name: "Body",
        min: 0.0,
        max: (BODY_NAMES.len() - 1) as f32,
        default: 1.0,
        taper: atomik_core::Taper::Linear,
        unit: BODIES,
        discrete: true,
    },
    linear(
        P_BRIGHT,
        "brightness",
        "Brightness",
        0.0,
        1.0,
        0.55,
        Unit::Percent,
    ),
    linear(
        P_PICK_POS,
        "pick.position",
        "Pick position",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    linear(
        P_PICK_HARD,
        "pick.hardness",
        "Pick hardness",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    expo(
        P_DECAY,
        "decay",
        "Decay",
        100.0,
        20_000.0,
        3_000.0,
        Unit::Ms,
    ),
    linear(
        P_SUSTAIN,
        "sustain",
        "Sustain",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    linear(
        P_PALM,
        "palm_mute",
        "Palm mute",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    linear(
        P_STIFF,
        "stiffness",
        "Stiffness",
        0.0,
        1.0,
        0.2,
        Unit::Percent,
    ),
    choice(P_COURSE, "course.mode", "Course", COURSES, 0.0),
    linear(
        P_DETUNE,
        "course.detune",
        "Course detune",
        0.0,
        50.0,
        6.0,
        Unit::Cents,
    ),
    linear(
        P_BODY_MIX,
        "body.mix",
        "Body mix",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
    linear(P_OUT, "out.gain", "Output gain", -24.0, 12.0, 0.0, Unit::Db),
    linear(
        P_WIDTH,
        "stereo.width",
        "Stereo width",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    linear(P_VEL, "vel.sens", "Velocity", 0.0, 1.0, 0.7, Unit::Percent),
    linear(
        P_BEND,
        "bend.range",
        "Bend range",
        0.0,
        MAX_BEND_RANGE,
        DEFAULT_BEND_RANGE,
        Unit::Semitones,
    ),
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

/// Static description of Plucked Strings.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.plucked",
    name: "Plucked Strings",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`PluckedStrings`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(PluckedStrings::new()),
    }
}

/// The course of a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Course {
    Single,
    Unison,
    Octave,
}

/// How a note is played (read from the pressure expression when it starts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Technique {
    Normal,
    Harmonic,
    Dead,
}

impl Technique {
    fn of(pressure: f32) -> Self {
        if pressure < EXPR_NORMAL_BELOW {
            Self::Normal
        } else if pressure < EXPR_HARMONIC_BELOW {
            Self::Harmonic
        } else {
            Self::Dead
        }
    }
}

/// The params in the form the voices use.
#[derive(Debug, Clone, Copy)]
struct Shared {
    brightness: f32,
    pick_position: f32,
    pick_hardness: f32,
    decay_s: f32,
    sustain: f32,
    palm: f32,
    stiffness: f32,
    course: Course,
    detune: f32,
    width: f32,
    velocity: f32,
    /// Master tune in cents.
    tune: f32,
    sample_rate: f32,
}

/// One key: one or two strings and the pick that plucks them.
struct Voice {
    strings: [PluckString; 2],
    pick: Pick,
    key: u8,
    velocity: f32,
    technique: Technique,
    course: Course,
    gate: bool,
    /// Rendering (the strings are ticked).
    active: bool,
    /// A pluck is due before the next sample.
    starting: bool,
    /// The note that starts next, with its velocity.
    queued: Option<(u8, f32)>,
    /// Fading out before the queued note starts (a stolen voice).
    fading: bool,
    fade: f32,
    /// The strings are cleared when the queued note starts (after a steal).
    clear: bool,
    order: u32,
    age: u32,
    env: f32,
    glide: Smoother,
    /// Gains of the two strings to left and right.
    gain: [[f32; 2]; 2],
}

impl Voice {
    fn new(sample_rate: f32) -> Self {
        Self {
            strings: [PluckString::new(sample_rate), PluckString::new(sample_rate)],
            pick: Pick::new(sample_rate),
            key: 0,
            velocity: 0.0,
            technique: Technique::Normal,
            course: Course::Single,
            gate: false,
            active: false,
            starting: false,
            queued: None,
            fading: false,
            fade: 1.0,
            clear: false,
            order: 0,
            age: 0,
            env: 0.0,
            glide: Smoother::new(0.0, GLIDE_MS, sample_rate / STEP as f32),
            gain: [[0.0; 2]; 2],
        }
    }

    fn reset(&mut self, sample_rate: f32) {
        for s in &mut self.strings {
            s.reset();
        }
        self.pick.reset();
        self.gate = false;
        self.active = false;
        self.starting = false;
        self.queued = None;
        self.fading = false;
        self.fade = 1.0;
        self.clear = false;
        self.order = 0;
        self.age = 0;
        self.env = 0.0;
        self.glide = Smoother::new(0.0, GLIDE_MS, sample_rate / STEP as f32);
    }

    /// The key this voice answers to (the queued note's when one waits).
    fn current_key(&self) -> u8 {
        self.queued.map_or(self.key, |(k, _)| k)
    }

    fn taken(&self) -> bool {
        self.active || self.starting
    }

    fn strings_in_use(&self) -> usize {
        if self.course == Course::Single { 1 } else { 2 }
    }

    /// Updates pitch, loop and pan from the note's expression. `steps` is the glide of the delay
    /// in samples (0 jumps).
    fn control(&mut self, c: &Shared, expr: KeyExpression, bend: f32, steps: u32) {
        let target = expr.pitch + bend;
        if self.age < IMMEDIATE {
            self.glide.set_immediate(target);
        } else {
            self.glide.set_target(target);
        }
        let semitones = f32::from(self.key) + self.glide.next_value() + c.tune * 0.01;
        let hz = 440.0 * exp2_det((semitones - 69.0) * (1.0 / 12.0));

        // The loop.
        let mut decay = c.decay_s * exp2_det(-0.35 * log2_det(hz * (1.0 / 220.0)));
        let mut brightness = c.brightness;
        let mut stiffness = c.stiffness;
        let palm = c.palm.max(expr.slide).clamp(0.0, 1.0);
        match self.technique {
            Technique::Normal => {
                decay *= exp2_det(-3.3 * palm);
                brightness *= 1.0 - 0.75 * palm;
            }
            Technique::Harmonic => {
                brightness *= 0.35;
                decay *= exp2_det(-3.3 * palm);
            }
            Technique::Dead => {
                decay = 0.045;
                brightness = 0.1;
                stiffness = 0.0;
            }
        }
        if !self.gate {
            // A finger on the string: the fall takes MUTE_S up to the whole decay.
            let ratio = (decay / MUTE_S).max(1.0);
            decay = (MUTE_S * exp2_det(c.sustain * log2_det(ratio))).min(decay);
        }
        let tone = StringTone {
            decay_s: decay,
            brightness,
            stiffness,
        };

        // The strings of the course.
        let d = c.detune;
        let (cents, levels): ([f32; 2], [f32; 2]) = match self.course {
            Course::Single => ([0.0, 0.0], [1.0, 0.0]),
            Course::Unison => ([-0.5 * d, 0.5 * d], [std::f32::consts::FRAC_1_SQRT_2; 2]),
            Course::Octave => ([0.0, 1_200.0 + d], [1.0, 0.5]),
        };
        let n = self.strings_in_use();
        for (s, cents) in self.strings[..n].iter_mut().zip(cents) {
            s.set_pitch(hz * exp2_det(cents * (1.0 / 1_200.0)), &tone, steps);
        }

        // The pan: keys spread low to high, the two strings of a course either side.
        let centre = ((f32::from(self.key) - 60.0) * (1.0 / 30.0)).clamp(-1.0, 1.0) * c.width * 0.6;
        let spread = if n == 2 { c.width * 0.15 } else { 0.0 };
        for (i, (&level, g)) in levels.iter().zip(&mut self.gain).enumerate() {
            let pan = (centre + if i == 0 { -spread } else { spread }).clamp(-1.0, 1.0);
            let theta = f64::from(pan + 1.0) * 0.125;
            g[0] = level * sin_cycles(theta + 0.25) as f32;
            g[1] = level * sin_cycles(theta) as f32;
        }
    }

    /// One sample: the stereo output of the voice.
    #[inline]
    fn tick(&mut self, fade_step: f32) -> (f32, f32) {
        let e = self.pick.sample();
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for i in 0..self.strings_in_use() {
            let y = self.strings[i].tick(e);
            l += y * self.gain[i][0];
            r += y * self.gain[i][1];
        }
        self.env = (l.abs() + r.abs()).max(self.env * 0.999_5);
        self.age = self.age.saturating_add(1);
        if self.fading {
            self.fade -= fade_step;
            if self.fade <= 0.0 {
                self.fade = 0.0;
                self.fading = false;
                self.clear = true;
            }
            let g = self.fade;
            return (l * g, r * g);
        }
        if !self.gate && self.env < IDLE {
            self.active = false;
        }
        (l, r)
    }
}

/// Plucked Strings.
pub struct PluckedStrings {
    sample_rate: f32,
    seed: u64,
    values: [f32; 15],
    shared: Shared,
    voices: Vec<Voice>,
    ctl: ControllerState,
    body: Body,
    body_mix: f32,
    gain: Smoother,
    dc: [(f32, f32); 2],
    dc_pole: f32,
    fade_step: f32,
    clock: u32,
    plucks: u32,
    counter: u32,
    fresh: bool,
    #[cfg(test)]
    probe: Option<std::sync::Arc<tests::Probe>>,
}

impl Default for PluckedStrings {
    fn default() -> Self {
        Self::new()
    }
}

impl PluckedStrings {
    /// Plucked Strings with the default params at 48 kHz (`prepare` sets the real rate and seed).
    pub fn new() -> Self {
        let sr = 48_000.0;
        let mut s = Self {
            sample_rate: sr,
            seed: 0,
            values: std::array::from_fn(|i| PARAMS[i].default),
            shared: Shared {
                brightness: 0.0,
                pick_position: 0.0,
                pick_hardness: 0.0,
                decay_s: 1.0,
                sustain: 0.0,
                palm: 0.0,
                stiffness: 0.0,
                course: Course::Single,
                detune: 0.0,
                width: 0.0,
                velocity: 0.0,
                tune: 0.0,
                sample_rate: sr,
            },
            voices: (0..MAX_VOICES).map(|_| Voice::new(sr)).collect(),
            ctl: ControllerState::new(),
            body: Body::new(sr),
            body_mix: 0.0,
            gain: Smoother::new(1.0, GAIN_SMOOTH_MS, sr),
            dc: [(0.0, 0.0); 2],
            dc_pole: 0.0,
            fade_step: 0.0,
            clock: 0,
            plucks: 0,
            counter: 0,
            fresh: true,
            #[cfg(test)]
            probe: None,
        };
        s.derive_rate();
        s.apply_all();
        s.reset();
        s
    }

    fn derive_rate(&mut self) {
        let sr = self.sample_rate;
        self.shared.sample_rate = sr;
        self.dc_pole = 1.0 - std::f32::consts::TAU * DC_HZ / sr;
        self.fade_step = 1.0 / (STEAL_FADE_MS * 0.001 * sr).max(1.0);
        self.ctl.set_sample_rate(sr);
        self.gain.set_time(GAIN_SMOOTH_MS, sr);
    }

    fn apply_all(&mut self) {
        for i in 0..PARAMS.len() {
            self.apply(i as u32);
        }
    }

    fn apply(&mut self, id: u32) {
        let v = finite(self.values[id as usize]);
        let p = &PARAMS[id as usize];
        let v = v.clamp(p.min, p.max);
        let s = &mut self.shared;
        match id {
            P_BODY => {
                self.body.set_kind(v.round() as usize);
            }
            P_BRIGHT => s.brightness = v,
            P_PICK_POS => s.pick_position = 0.03 + 0.47 * v,
            P_PICK_HARD => s.pick_hardness = v,
            P_DECAY => s.decay_s = v * 0.001,
            P_SUSTAIN => s.sustain = v,
            P_PALM => s.palm = v,
            P_STIFF => s.stiffness = v,
            P_COURSE => {
                s.course = match v.round() as u32 {
                    0 => Course::Single,
                    1 => Course::Unison,
                    _ => Course::Octave,
                }
            }
            P_DETUNE => s.detune = v,
            P_BODY_MIX => self.body_mix = v,
            P_OUT => self.gain.set_target(exp2_det(v * 0.166_096_4)),
            P_WIDTH => s.width = v,
            P_VEL => s.velocity = v,
            P_BEND => self.ctl.set_bend_range(v),
            _ => {}
        }
    }

    // ---- notes ----

    fn note_on(&mut self, key: u8, velocity: f32) {
        let key = key.min(127);
        self.ctl.note_on(key);
        let velocity = finite(velocity).clamp(0.0, 1.0);
        self.clock = self.clock.wrapping_add(1);
        let order = self.clock;
        // The same key still ringing (or waiting): pluck that string again.
        let slot = self
            .voices
            .iter()
            .position(|v| v.taken() && v.current_key() == key)
            .or_else(|| self.voices.iter().position(|v| !v.taken()));
        let slot = slot.unwrap_or_else(|| self.victim());
        let steal = self.voices[slot].taken() && self.voices[slot].current_key() != key;
        let v = &mut self.voices[slot];
        v.queued = Some((key, velocity));
        v.order = order;
        v.starting = true;
        v.gate = true;
        // A voice that is sounding fades out first; one that has not begun can be taken at once.
        if steal && v.active {
            v.fading = true;
        }
    }

    /// The voice to steal: a released one, the quietest, else the oldest.
    fn victim(&self) -> usize {
        let mut best = 0;
        for (i, v) in self.voices.iter().enumerate() {
            let b = &self.voices[best];
            let older = (v.order.wrapping_sub(b.order) as i32).cmp(&0);
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
    }

    fn release_key(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.taken()) {
            if v.current_key() == key {
                v.gate = false;
            }
        }
    }

    fn handle(&mut self, kind: NoteKind) {
        match kind {
            NoteKind::On { key, velocity } => self.note_on(key, velocity),
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

    /// The queued note of voice `i` starts: pitch, loop and the pluck.
    fn start_voice(&mut self, i: usize) {
        let shared = self.shared;
        let bend = self.ctl.bend_semitones();
        self.plucks = self.plucks.wrapping_add(1);
        let v = &mut self.voices[i];
        let Some((key, velocity)) = v.queued.take() else {
            v.starting = false;
            return;
        };
        let expr = self.ctl.note_expr(key);
        let fresh = !v.active || v.clear;
        if fresh {
            for s in &mut v.strings {
                s.reset();
            }
            v.env = 0.0;
        }
        v.clear = false;
        v.fading = false;
        v.fade = 1.0;
        v.starting = false;
        v.active = true;
        v.key = key;
        v.velocity = velocity;
        v.age = 0;
        v.technique = Technique::of(expr.pressure);
        if fresh {
            v.course = shared.course;
        }
        v.glide.set_immediate(expr.pitch + bend);
        v.control(&shared, expr, bend, if fresh { 0 } else { STEP });
        // The pick.
        let hz = shared.sample_rate / v.strings[0].period().max(1.0);
        let strike = 1.0 - shared.velocity * (1.0 - velocity);
        let mut cutoff =
            1_200.0 * exp2_det(3.6 * shared.pick_hardness) * (0.45 + 0.55 * strike * strike);
        let mut position = shared.pick_position;
        let mut level = PICK_LEVEL * (1.0 - shared.velocity + shared.velocity * velocity);
        match v.technique {
            Technique::Normal => {}
            Technique::Harmonic => {
                cutoff = cutoff.min(2.5 * hz);
                position = 0.0;
                level *= 0.9;
            }
            Technique::Dead => {
                cutoff = cutoff.min(2_500.0);
                position = 0.0;
                level *= 1.3;
            }
        }
        let seed = hash64(self.seed, u64::from(self.plucks), u64::from(key));
        v.pick.start(
            &PickParams {
                period: v.strings[0].period(),
                cutoff_hz: cutoff,
                position,
                level,
            },
            seed,
        );
    }

    /// Renders the samples `range` of the batch into `left` and `right`.
    fn render(&mut self, left: &mut [f32; BATCH], right: &mut [f32; BATCH], range: (usize, usize)) {
        let shared = self.shared;
        let bend = self.ctl.bend_semitones();
        for n in range.0..range.1 {
            let tick = self.counter & (STEP - 1) == 0;
            self.counter = self.counter.wrapping_add(1);
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for i in 0..MAX_VOICES {
                if self.voices[i].starting && !self.voices[i].fading {
                    self.start_voice(i);
                }
                if !self.voices[i].active {
                    continue;
                }
                if tick {
                    let expr = self.ctl.note_expr(self.voices[i].key);
                    self.voices[i].control(&shared, expr, bend, STEP);
                }
                let (a, b) = self.voices[i].tick(self.fade_step);
                l += a;
                r += b;
            }
            left[n] = l;
            right[n] = r;
        }
    }
}

impl Device for PluckedStrings {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.seed = info.seed;
        self.voices = (0..MAX_VOICES)
            .map(|_| Voice::new(self.sample_rate))
            .collect();
        self.body = Body::new(self.sample_rate);
        self.derive_rate();
        self.apply_all();
        self.reset();
    }

    fn reset(&mut self) {
        let sr = self.sample_rate;
        for v in &mut self.voices {
            v.reset(sr);
        }
        self.body.reset();
        self.dc = [(0.0, 0.0); 2];
        self.ctl.reset();
        self.clock = 0;
        self.plucks = 0;
        self.counter = 0;
        self.fresh = true;
        let g = self.gain.target();
        self.gain.set_immediate(g);
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let i = id.0 as usize;
        if i < self.values.len() {
            self.values[i] = plain;
            self.apply(id.0);
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            let g = self.gain.target();
            self.gain.set_immediate(g);
            self.fresh = false;
        }
        self.shared.tune = finite(ctx.time.tune_cents).clamp(-1_200.0, 1_200.0);
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
        let pole = self.dc_pole;
        for n in 0..BATCH {
            // DC blocker: y = x - x1 + pole y1, per channel.
            let mut out = [left[n], right[n]];
            for (o, (x1, y1)) in out.iter_mut().zip(&mut self.dc) {
                let y = *o - *x1 + pole * *y1;
                *x1 = *o;
                *y1 = if y.abs() < 1e-20 { 0.0 } else { y };
                *o = y;
            }
            let (l, r) = self.body.process(out[0], out[1], self.body_mix);
            let g = self.gain.next_value() * gain[n];
            left[n] = soft_limit(finite(l) * g);
            right[n] = soft_limit(finite(r) * g);
        }
        *ctx.output(OUT_L) = left;
        *ctx.output(OUT_R) = right;
        #[cfg(test)]
        self.update_probe();
    }
}

#[cfg(test)]
impl PluckedStrings {
    fn update_probe(&self) {
        use std::sync::atomic::Ordering;
        if let Some(probe) = &self.probe {
            for (slot, v) in probe.keys.iter().zip(&self.voices) {
                let k = if v.taken() {
                    u32::from(v.current_key()) + 1
                } else {
                    0
                };
                slot.store(k, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests;
