//! `atomik.chord-wheel`: a chord progression player. Eight slots hold a chord (root, quality, slash
//! bass) and a length; the progression plays in time with the transport through a rhythm style
//! and goes out as notes. The progression is made by hand or by the generator of
//! [`atomik_core::progression`] (which the app runs, never the audio thread).
//!
//! * **Time.** In `transport` mode the progression runs from song tick 0 and loops: the cycle is the
//!   sum of the active slot lengths (`slot.N.beats`) and slot 1 starts every cycle. Everything is
//!   a pure function of the transport tick and the params, so a bounce, a live run, a locate and a
//!   loop pass agree at any block size. Starting mid-cycle (a locate, the start of the song) plays
//!   the chord of that slot from the next hit; the styles that hold a chord (`sustain`, `push`,
//!   `strum`) start it at once, and a `root` bass starts at once.
//! * **Styles.** `sustain` one chord per slot, `pulse 1/4` and `pulse 1/8` repeat it, `offbeat` stabs
//!   the off eighths, `push` anticipates every change by an eighth, `strum` rolls the chord (at
//!   least 25 ms), `broken` plays low, high, middle, high in eighths and `3-3-2` the syncopated
//!   figure of three, three and two eighths. `gate` is the fraction of a hit's length that sounds.
//! * **Voicing.** The chords are voiced by `atomik_core::progression::voice` from the params
//!   (`voice.*`): with `voice.lead` each chord takes the inversion nearest to the one before it
//!   (slot 1 is voiced alone). The voicings are built when a param changes, never
//!   while playing. `bass` adds the root (or the slash bass) two registers below: `root` for the
//!   length of the chord, `pulse` on every hit. With `bass` off a slash bass sits below the chord.
//! * **Dynamics.** `velocity` sets the level, `accent` lowers every hit but the first of a chord,
//!   `humanize` moves every note up to 10 ms either way and its velocity by up to 12 percent, from
//!   a hash of the device seed, the note's tick and its key (deterministic).
//! * **Play modes.** `transport` plays by itself (and with `thru` passes `notes_in` on). `held` is
//!   the same but only sounds while a key is held on `notes_in` (the keys are not passed on).
//!   `step` plays the next slot's chord at every key press for as long as the key is held, at the
//!   velocity of the key, without the transport.
//! * **Key.** `key.source` `song` moves the whole progression by the distance from `key.root` to the
//!   song key at the start of the batch (a song key of the other mode maps to its relative key).
//!   Without a song key the progression plays as written.
//! * **Note safety.** Every note it starts it closes: on stop, locate, loop wrap, bypass, a change
//!   of the playing slot or of what the cycle looks like, a mode switch and `AllOff`. At most 16
//!   notes sound at once (further starts are dropped), and a key that is already sounding is
//!   closed before it starts again.
//! * **Bypass** (`bypass`, last): see `crate::note_fx::PlayerBypass`.
//! * **Meters.** The panel reads the playing slot (1..=8, 0 = none) and the sounding keys.
//! * **Generator params** (`gen.mood`, `gen.spice`, `gen.seed`) and `slot.N.lock` do nothing on
//!   the audio thread; the app reads them.

use atomik_core::progression::{
    BEATS, Chord, Extend, Key, KeyMode, MAX_SLOTS, MOOD_LABELS, Spread, VoiceOpts, Voicing,
    song_key_shift, voice,
};
use atomik_core::theory::{ChordKind, PITCH_NAMES};
use atomik_core::{
    BATCH, NoteBuffer, NoteEvent, NoteKind, PPQ, ParamDescriptor, ParamId, Taper, Unit,
};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketKind,
};
use atomik_dsp::rng::{hash64, unit_f64};

use crate::bypass::bypass_param;
use crate::note_clock::{EPS_FRAMES, JumpTracker, NOTES_IN, NOTES_OUT};
use crate::note_fx::{PlayerBypass, push_sorted};

static SOCKETS: [SocketDescriptor; 2] = [
    SocketDescriptor {
        id: NOTES_IN,
        key: "notes_in",
        name: "Notes in",
        kind: SocketKind::Notes,
        direction: atomik_device_sdk::Direction::Input,
    },
    SocketDescriptor {
        id: NOTES_OUT,
        key: "notes_out",
        name: "Notes out",
        kind: SocketKind::Notes,
        direction: atomik_device_sdk::Direction::Output,
    },
];

// ---------------------------------------------------------------- params

const P_KEY_ROOT: u32 = 0;
const P_KEY_MODE: u32 = 1;
const P_KEY_SOURCE: u32 = 2;
const P_SLOTS: u32 = 3;
/// First slot param; slot `n` (0-based) param `f` is `P_SLOT0 + 5 * n + f`.
const P_SLOT0: u32 = 4;
const F_ROOT: u32 = 0;
const F_QUALITY: u32 = 1;
const F_BASS: u32 = 2;
const F_BEATS: u32 = 3;
const F_LOCK: u32 = 4;
const P_MOOD: u32 = 44;
const P_SPICE: u32 = 45;
const P_SEED: u32 = 46;
const P_LEAD: u32 = 47;
const P_REGISTER: u32 = 48;
const P_SPREAD: u32 = 49;
const P_EXTEND: u32 = 50;
const P_STYLE: u32 = 51;
const P_GATE: u32 = 52;
const P_STRUM: u32 = 53;
const P_VELOCITY: u32 = 54;
const P_ACCENT: u32 = 55;
const P_HUMANIZE: u32 = 56;
const P_BASS: u32 = 57;
const P_PLAY: u32 = 58;
const P_THRU: u32 = 59;
const P_BYPASS: u32 = 60;
const PARAM_COUNT: usize = 61;

/// The labels of `slot.N.quality`, in [`ChordKind::ALL`] order (a test pins them to the core).
pub(crate) static QUALITY_LABELS: [&str; 21] = [
    "Major",
    "Minor",
    "Diminished",
    "Augmented",
    "Sus2",
    "Sus4",
    "6",
    "m6",
    "7",
    "maj7",
    "m7",
    "m7b5",
    "dim7",
    "9",
    "maj9",
    "m9",
    "add9",
    "11",
    "13",
    "Power (5)",
    "Octave",
];

static BASS_LABELS: [&str; 13] = [
    "Root", "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
static BEATS_LABELS: [&str; 8] = ["1", "2", "3", "4", "6", "8", "12", "16"];
static STYLE_LABELS: [&str; 8] = [
    "Sustain",
    "Pulse 1/4",
    "Pulse 1/8",
    "Offbeat",
    "Push",
    "Strum",
    "Broken",
    "3-3-2",
];
static REGISTER_LABELS: [&str; 4] = ["Low", "Mid-low", "Mid", "High"];
static SPREAD_LABELS: [&str; 3] = ["Close", "Open", "Wide"];
static EXTEND_LABELS: [&str; 3] = ["As written", "7ths", "9ths"];
static BASS_MODE_LABELS: [&str; 3] = ["Off", "Root", "Pulse"];
static PLAY_LABELS: [&str; 3] = ["Transport", "Held", "Step"];
static MODE_LABELS: [&str; 2] = ["Major", "Minor"];
static SOURCE_LABELS: [&str; 2] = ["Device", "Song"];

const fn param(
    id: usize,
    key: &'static str,
    name: &'static str,
    range: (f32, f32, f32),
    unit: Unit,
    discrete: bool,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id as u32),
        key,
        name,
        min: range.0,
        max: range.1,
        default: range.2,
        taper: Taper::Linear,
        unit,
        discrete,
    }
}

/// The keys and names of the five params of every slot.
const SLOT_TEXT: [[(&str, &str); 5]; MAX_SLOTS] = [
    [
        ("slot.1.root", "Slot 1 root"),
        ("slot.1.quality", "Slot 1 quality"),
        ("slot.1.bass", "Slot 1 bass"),
        ("slot.1.beats", "Slot 1 beats"),
        ("slot.1.lock", "Slot 1 lock"),
    ],
    [
        ("slot.2.root", "Slot 2 root"),
        ("slot.2.quality", "Slot 2 quality"),
        ("slot.2.bass", "Slot 2 bass"),
        ("slot.2.beats", "Slot 2 beats"),
        ("slot.2.lock", "Slot 2 lock"),
    ],
    [
        ("slot.3.root", "Slot 3 root"),
        ("slot.3.quality", "Slot 3 quality"),
        ("slot.3.bass", "Slot 3 bass"),
        ("slot.3.beats", "Slot 3 beats"),
        ("slot.3.lock", "Slot 3 lock"),
    ],
    [
        ("slot.4.root", "Slot 4 root"),
        ("slot.4.quality", "Slot 4 quality"),
        ("slot.4.bass", "Slot 4 bass"),
        ("slot.4.beats", "Slot 4 beats"),
        ("slot.4.lock", "Slot 4 lock"),
    ],
    [
        ("slot.5.root", "Slot 5 root"),
        ("slot.5.quality", "Slot 5 quality"),
        ("slot.5.bass", "Slot 5 bass"),
        ("slot.5.beats", "Slot 5 beats"),
        ("slot.5.lock", "Slot 5 lock"),
    ],
    [
        ("slot.6.root", "Slot 6 root"),
        ("slot.6.quality", "Slot 6 quality"),
        ("slot.6.bass", "Slot 6 bass"),
        ("slot.6.beats", "Slot 6 beats"),
        ("slot.6.lock", "Slot 6 lock"),
    ],
    [
        ("slot.7.root", "Slot 7 root"),
        ("slot.7.quality", "Slot 7 quality"),
        ("slot.7.bass", "Slot 7 bass"),
        ("slot.7.beats", "Slot 7 beats"),
        ("slot.7.lock", "Slot 7 lock"),
    ],
    [
        ("slot.8.root", "Slot 8 root"),
        ("slot.8.quality", "Slot 8 quality"),
        ("slot.8.bass", "Slot 8 bass"),
        ("slot.8.beats", "Slot 8 beats"),
        ("slot.8.lock", "Slot 8 lock"),
    ],
];

/// The default progression: C, G, Am, F (the rest C major, unused).
const DEFAULT_ROOTS: [f32; MAX_SLOTS] = [0.0, 7.0, 9.0, 5.0, 0.0, 0.0, 0.0, 0.0];
const DEFAULT_QUALITIES: [f32; MAX_SLOTS] = [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

const fn build_params() -> [ParamDescriptor; PARAM_COUNT] {
    let blank = param(0, "", "", (0.0, 1.0, 0.0), Unit::None, true);
    let mut p = [blank; PARAM_COUNT];
    p[0] = param(
        0,
        "key.root",
        "Key root",
        (0.0, 11.0, 0.0),
        Unit::Choice(&PITCH_NAMES),
        true,
    );
    p[1] = param(
        1,
        "key.mode",
        "Key mode",
        (0.0, 1.0, 0.0),
        Unit::Choice(&MODE_LABELS),
        true,
    );
    p[2] = param(
        2,
        "key.source",
        "Key source",
        (0.0, 1.0, 0.0),
        Unit::Choice(&SOURCE_LABELS),
        true,
    );
    p[3] = param(3, "slots", "Slots", (1.0, 8.0, 4.0), Unit::None, true);
    let mut n = 0;
    while n < MAX_SLOTS {
        let at = P_SLOT0 as usize + 5 * n;
        let t = SLOT_TEXT[n];
        p[at] = param(
            at,
            t[0].0,
            t[0].1,
            (0.0, 11.0, DEFAULT_ROOTS[n]),
            Unit::Choice(&PITCH_NAMES),
            true,
        );
        p[at + 1] = param(
            at + 1,
            t[1].0,
            t[1].1,
            (0.0, 20.0, DEFAULT_QUALITIES[n]),
            Unit::Choice(&QUALITY_LABELS),
            true,
        );
        p[at + 2] = param(
            at + 2,
            t[2].0,
            t[2].1,
            (0.0, 12.0, 0.0),
            Unit::Choice(&BASS_LABELS),
            true,
        );
        p[at + 3] = param(
            at + 3,
            t[3].0,
            t[3].1,
            (0.0, 7.0, 3.0),
            Unit::Choice(&BEATS_LABELS),
            true,
        );
        p[at + 4] = param(at + 4, t[4].0, t[4].1, (0.0, 1.0, 0.0), Unit::Toggle, true);
        n += 1;
    }
    p[44] = param(
        44,
        "gen.mood",
        "Mood",
        (0.0, 11.0, 0.0),
        Unit::Choice(&MOOD_LABELS),
        true,
    );
    p[45] = param(
        45,
        "gen.spice",
        "Spice",
        (0.0, 1.0, 0.3),
        Unit::Percent,
        false,
    );
    p[46] = param(46, "gen.seed", "Seed", (0.0, 9999.0, 0.0), Unit::None, true);
    p[47] = param(
        47,
        "voice.lead",
        "Voice leading",
        (0.0, 1.0, 1.0),
        Unit::Toggle,
        true,
    );
    p[48] = param(
        48,
        "voice.register",
        "Register",
        (0.0, 3.0, 2.0),
        Unit::Choice(&REGISTER_LABELS),
        true,
    );
    p[49] = param(
        49,
        "voice.spread",
        "Spread",
        (0.0, 2.0, 0.0),
        Unit::Choice(&SPREAD_LABELS),
        true,
    );
    p[50] = param(
        50,
        "voice.extend",
        "Extend",
        (0.0, 2.0, 0.0),
        Unit::Choice(&EXTEND_LABELS),
        true,
    );
    p[51] = param(
        51,
        "style",
        "Style",
        (0.0, 7.0, 0.0),
        Unit::Choice(&STYLE_LABELS),
        true,
    );
    p[52] = param(52, "gate", "Gate", (0.05, 1.0, 0.9), Unit::Percent, false);
    p[53] = param(53, "strum", "Strum", (0.0, 1.0, 0.0), Unit::Percent, false);
    p[54] = param(
        54,
        "velocity",
        "Velocity",
        (0.0, 1.0, 0.8),
        Unit::Percent,
        false,
    );
    p[55] = param(
        55,
        "accent",
        "Accent",
        (0.0, 1.0, 0.3),
        Unit::Percent,
        false,
    );
    p[56] = param(
        56,
        "humanize",
        "Humanize",
        (0.0, 1.0, 0.0),
        Unit::Percent,
        false,
    );
    p[57] = param(
        57,
        "bass",
        "Bass",
        (0.0, 2.0, 1.0),
        Unit::Choice(&BASS_MODE_LABELS),
        true,
    );
    p[58] = param(
        58,
        "play.mode",
        "Play mode",
        (0.0, 2.0, 0.0),
        Unit::Choice(&PLAY_LABELS),
        true,
    );
    p[59] = param(59, "thru", "Thru", (0.0, 1.0, 0.0), Unit::Toggle, true);
    p[60] = bypass_param(P_BYPASS);
    p
}

static PARAMS: [ParamDescriptor; PARAM_COUNT] = build_params();

/// Static description of the Chord Wheel device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.chord-wheel",
    name: "Chord Wheel",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`ChordWheel`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(ChordWheel::new()),
    }
}

// ---------------------------------------------------------------- styles

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Sustain,
    Pulse4,
    Pulse8,
    Offbeat,
    Push,
    Strum,
    Broken,
    ThreeThreeTwo,
}

impl Style {
    fn from_index(i: i32) -> Self {
        match i {
            0 => Style::Sustain,
            1 => Style::Pulse4,
            2 => Style::Pulse8,
            3 => Style::Offbeat,
            4 => Style::Push,
            5 => Style::Strum,
            6 => Style::Broken,
            _ => Style::ThreeThreeTwo,
        }
    }

    /// The styles that hold a chord for the whole slot.
    fn holds(self) -> bool {
        matches!(self, Style::Sustain | Style::Push | Style::Strum)
    }
}

/// Which notes of the chord a hit plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    All,
    Low,
    High,
    Mid,
}

/// One hit of a repeating pattern: offset and length in ticks from the start of the period.
#[derive(Debug, Clone, Copy)]
struct PatHit {
    off: i64,
    len: i64,
    pick: Pick,
}

const BEAT: i64 = PPQ;
const EIGHTH: i64 = PPQ / 2;

const PULSE4: [PatHit; 1] = [PatHit {
    off: 0,
    len: BEAT,
    pick: Pick::All,
}];
const PULSE8: [PatHit; 1] = [PatHit {
    off: 0,
    len: EIGHTH,
    pick: Pick::All,
}];
const OFFBEAT: [PatHit; 1] = [PatHit {
    off: EIGHTH,
    len: EIGHTH,
    pick: Pick::All,
}];
const BROKEN: [PatHit; 4] = [
    PatHit {
        off: 0,
        len: EIGHTH,
        pick: Pick::Low,
    },
    PatHit {
        off: EIGHTH,
        len: EIGHTH,
        pick: Pick::High,
    },
    PatHit {
        off: 2 * EIGHTH,
        len: EIGHTH,
        pick: Pick::Mid,
    },
    PatHit {
        off: 3 * EIGHTH,
        len: EIGHTH,
        pick: Pick::High,
    },
];
const SYNCOPATED: [PatHit; 3] = [
    PatHit {
        off: 0,
        len: 3 * EIGHTH,
        pick: Pick::All,
    },
    PatHit {
        off: 3 * EIGHTH,
        len: 3 * EIGHTH,
        pick: Pick::All,
    },
    PatHit {
        off: 6 * EIGHTH,
        len: 2 * EIGHTH,
        pick: Pick::All,
    },
];

/// The repeating pattern of a style: its period and hits, or `None` for the styles that hold a
/// chord (one hit per slot).
fn pattern(style: Style) -> Option<(i64, &'static [PatHit])> {
    match style {
        Style::Sustain | Style::Push | Style::Strum => None,
        Style::Pulse4 => Some((BEAT, &PULSE4)),
        Style::Pulse8 => Some((EIGHTH, &PULSE8)),
        Style::Offbeat => Some((BEAT, &OFFBEAT)),
        Style::Broken => Some((4 * EIGHTH, &BROKEN)),
        Style::ThreeThreeTwo => Some((8 * EIGHTH, &SYNCOPATED)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BassMode {
    Off,
    Root,
    Pulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayMode {
    Transport,
    Held,
    Step,
}

// ---------------------------------------------------------------- state

/// What is set on the slot params.
#[derive(Debug, Clone, Copy)]
struct SlotParams {
    root: u8,
    quality: u8,
    /// 0 = root, else the bass pitch class + 1.
    bass: u8,
    beats: u8,
}

/// One slot as the cycle plays it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlotLayout {
    voicing: Voicing,
    /// The bass key, when `bass` is on.
    bass: Option<u8>,
    /// The slash bass key below the voicing, when `bass` is off.
    below: Option<u8>,
    start: i64,
    len: i64,
}

const EMPTY_SLOT: SlotLayout = SlotLayout {
    voicing: Voicing::EMPTY,
    bass: None,
    below: None,
    start: 0,
    len: 0,
};

/// The cycle: what the audio thread plays, rebuilt when a param changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    n: usize,
    total: i64,
    style: Style,
    bass: BassMode,
    mode: PlayMode,
    slots: [SlotLayout; MAX_SLOTS],
}

const EMPTY_LAYOUT: Layout = Layout {
    n: 0,
    total: 0,
    style: Style::Sustain,
    bass: BassMode::Root,
    mode: PlayMode::Transport,
    slots: [EMPTY_SLOT; MAX_SLOTS],
};

const PENDING_CAP: usize = 128;
/// Notes sounding at most.
const MAX_SOUNDING: usize = 16;
/// Notes of one hit: eight voiced keys, a slash bass and the bass.
const HIT_NOTES: usize = 10;

#[derive(Debug, Clone, Copy)]
struct Ev {
    t: f64,
    key: u8,
    vel: f32,
    on: bool,
    id: u32,
    seq: u32,
    used: bool,
}

const EMPTY_EV: Ev = Ev {
    t: 0.0,
    key: 0,
    vel: 0.0,
    on: false,
    id: 0,
    seq: 0,
    used: false,
};

/// Notes scheduled but not yet emitted, in the time base of the mode (ticks, or frames in `step`).
#[derive(Debug, Clone)]
struct Pending {
    ev: [Ev; PENDING_CAP],
    seq: u32,
}

impl Pending {
    const fn new() -> Self {
        Self {
            ev: [EMPTY_EV; PENDING_CAP],
            seq: 0,
        }
    }

    fn clear(&mut self) {
        for e in &mut self.ev {
            e.used = false;
        }
    }

    fn free(&self) -> usize {
        self.ev.iter().filter(|e| !e.used).count()
    }

    fn push(&mut self, t: f64, key: u8, vel: f32, on: bool, id: u32) {
        if let Some(slot) = self.ev.iter_mut().find(|e| !e.used) {
            self.seq = self.seq.wrapping_add(1);
            *slot = Ev {
                t,
                key,
                vel,
                on,
                id,
                seq: self.seq,
                used: true,
            };
        }
    }

    /// The earliest event: by time, an `Off` before an `On` at one time, then insertion order.
    fn earliest(&self) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, e) in self.ev.iter().enumerate() {
            if !e.used {
                continue;
            }
            let better = match best {
                None => true,
                Some(b) => {
                    let o = &self.ev[b];
                    e.t.total_cmp(&o.t)
                        .then(e.on.cmp(&o.on))
                        .then(e.seq.cmp(&o.seq))
                        .is_lt()
                }
            };
            if better {
                best = Some(i);
            }
        }
        best
    }

    /// Drops the not yet started notes of `id`; returns whether any was there.
    fn cancel_ons(&mut self, id: u32) -> bool {
        let mut any = false;
        for e in &mut self.ev {
            if e.used && e.on && e.id == id {
                e.used = false;
                any = true;
            }
        }
        any
    }
}

/// The time base of one batch.
#[derive(Debug, Clone, Copy)]
struct Clock {
    /// Position at frame 0.
    t0: f64,
    /// Units per frame.
    tpf: f64,
    /// Units per millisecond.
    tpm: f64,
    /// The wrap inside the batch: frame and position after it.
    wrap: Option<(usize, f64)>,
}

impl Clock {
    fn at(&self, frame: usize) -> f64 {
        match self.wrap {
            Some((w, to)) if frame >= w => to + (frame - w) as f64 * self.tpf,
            _ => self.t0 + frame as f64 * self.tpf,
        }
    }
}

/// The first frame in `a..=b` at or after `t` (`b` = not in this segment).
fn frame_of(t: f64, t_a: f64, tpf: f64, a: usize, b: usize) -> usize {
    let x = (t - t_a) / tpf - EPS_FRAMES;
    if x <= 0.0 {
        a
    } else if x >= (b - a) as f64 {
        b
    } else {
        a + x.ceil() as usize
    }
}

/// The Chord Wheel device.
#[derive(Debug, Clone)]
pub struct ChordWheel {
    bypass: PlayerBypass,
    // Params.
    key: Key,
    song: bool,
    slots: usize,
    slot: [SlotParams; MAX_SLOTS],
    lead: bool,
    register: u8,
    spread: Spread,
    extend: Extend,
    style: Style,
    gate: f32,
    strum: f32,
    velocity: f32,
    accent: f32,
    humanize: f32,
    bass: BassMode,
    mode: PlayMode,
    thru: bool,
    // Build state.
    /// The params changed: rebuild the layout at the next batch.
    stale: bool,
    /// The song key shift the layout was built with.
    shift: u8,
    layout: Layout,
    // Runtime.
    seed: u64,
    sample_rate: f64,
    pending: Pending,
    sounding: [bool; 128],
    owner: [u32; 128],
    n_sounding: usize,
    next_id: u32,
    clock: JumpTracker,
    /// Start from the current position (chase) in the next segment.
    fresh: bool,
    /// The transport plays and the device runs on it.
    active: bool,
    /// Emit an `AllOff` at the start of the next batch (a `reset` while something sounded).
    flush_alloff: bool,
    /// The slot of the cycle at the start of the last batch.
    cur: Option<usize>,
    /// Input keys held (`held` mode).
    held: [bool; 128],
    held_n: usize,
    /// The id of the chord an input key started (`step` mode), 0 = none.
    step_id: [u32; 128],
    step_next: usize,
    step_slot: u8,
    /// Frames since the start (the time base of `step` mode).
    local: f64,
    /// Input keys passed through (`thru`).
    passed: [bool; 128],
    wrap_done: bool,
    meter: [u8; 9],
}

impl ChordWheel {
    fn new() -> Self {
        let mut slot = [SlotParams {
            root: 0,
            quality: 0,
            bass: 0,
            beats: 3,
        }; MAX_SLOTS];
        for (i, s) in slot.iter_mut().enumerate() {
            s.root = DEFAULT_ROOTS[i] as u8;
            s.quality = DEFAULT_QUALITIES[i] as u8;
        }
        Self {
            bypass: PlayerBypass::new(),
            key: Key::new(0, KeyMode::Major),
            song: false,
            slots: 4,
            slot,
            lead: true,
            register: 2,
            spread: Spread::Close,
            extend: Extend::AsWritten,
            style: Style::Sustain,
            gate: 0.9,
            strum: 0.0,
            velocity: 0.8,
            accent: 0.3,
            humanize: 0.0,
            bass: BassMode::Root,
            mode: PlayMode::Transport,
            thru: false,
            stale: true,
            shift: 0,
            layout: EMPTY_LAYOUT,
            seed: 0,
            sample_rate: 48_000.0,
            pending: Pending::new(),
            sounding: [false; 128],
            owner: [0; 128],
            n_sounding: 0,
            next_id: 0,
            clock: JumpTracker::new(),
            fresh: true,
            active: false,
            flush_alloff: false,
            cur: None,
            held: [false; 128],
            held_n: 0,
            step_id: [0; 128],
            step_next: 0,
            step_slot: 0,
            local: 0.0,
            passed: [false; 128],
            wrap_done: false,
            meter: [0; 9],
        }
    }

    // ------------------------------------------------------------ layout

    /// The chord of slot `k` as written, moved by the song key shift.
    fn chord_of(&self, k: usize) -> Chord {
        let s = &self.slot[k];
        let kind = ChordKind::ALL[usize::from(s.quality).min(ChordKind::ALL.len() - 1)];
        let mut c = Chord::new(s.root, kind);
        if s.bass > 0 {
            c = c.over(s.bass - 1);
        }
        c.transposed(self.shift)
    }

    /// Builds the cycle from the params (voicings, bass keys, start ticks).
    fn build(&self) -> Layout {
        let n = self.slots.clamp(1, MAX_SLOTS);
        let key = Key::new(self.key.root + self.shift, self.key.mode);
        let opts = VoiceOpts {
            key,
            lead: self.lead,
            register: self.register,
            spread: self.spread,
            extend: self.extend,
        };
        let mut chords = [Chord::new(0, ChordKind::Major); MAX_SLOTS];
        for (k, c) in chords.iter_mut().enumerate().take(n) {
            *c = self.chord_of(k);
        }
        let mut v = [Voicing::EMPTY; MAX_SLOTS];
        // Slot 1 is voiced alone and every later slot leads from the one before it, so a change
        // of a slot never moves the slots before it (what plays is not re-voiced behind it).
        for k in 0..n {
            let prev = if k > 0 { Some(v[k - 1]) } else { None };
            v[k] = voice(chords[k], prev.as_ref(), opts);
        }
        let centre = [36i32, 48, 60, 72][usize::from(self.register.min(3))];
        let mut out = EMPTY_LAYOUT;
        out.n = n;
        out.style = self.style;
        out.bass = self.bass;
        out.mode = self.mode;
        let mut start = 0i64;
        for k in 0..n {
            let len = i64::from(BEATS[usize::from(self.slot[k].beats).min(BEATS.len() - 1)]) * BEAT;
            let c = chords[k];
            let pc = c.bass.unwrap_or(c.root);
            let (bass, below) = match self.bass {
                BassMode::Off => {
                    let below = c.bass.and_then(|b| {
                        let low = i32::from(v[k].lowest()?);
                        let down = (low - i32::from(b)).rem_euclid(12);
                        let key = low - if down == 0 { 12 } else { down };
                        u8::try_from(key).ok()
                    });
                    (None, below)
                }
                _ => {
                    let lo = centre - 30;
                    let mut key = lo + (i32::from(pc) - lo).rem_euclid(12);
                    while key < 24 {
                        key += 12;
                    }
                    while key > 100 {
                        key -= 12;
                    }
                    (Some(key as u8), None)
                }
            };
            out.slots[k] = SlotLayout {
                voicing: v[k],
                bass,
                below,
                start,
                len,
            };
            start += len;
        }
        out.total = start;
        out
    }

    /// The anticipation of the style in ticks.
    fn push_shift(&self) -> i64 {
        if self.layout.style == Style::Push {
            EIGHTH
        } else {
            0
        }
    }

    /// The slot that holds the cycle position `pos`, and the cycle position in the slot.
    fn slot_at(&self, pos: i64) -> Option<(usize, i64)> {
        let l = &self.layout;
        (0..l.n)
            .find(|k| pos >= l.slots[*k].start && pos < l.slots[*k].start + l.slots[*k].len)
            .map(|k| (k, pos - l.slots[k].start))
    }

    // ------------------------------------------------------------ output

    fn next_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    /// Closes everything that sounds at `frame` and forgets what is scheduled.
    fn close_all(&mut self, out: &mut NoteBuffer, frame: usize) {
        if self.n_sounding > 0 {
            for k in 0..128usize {
                if self.sounding[k] {
                    push_sorted(
                        out,
                        NoteEvent {
                            offset: frame.min(BATCH - 1) as u16,
                            kind: NoteKind::Off { key: k as u8 },
                        },
                    );
                }
            }
        }
        self.sounding = [false; 128];
        self.owner = [0; 128];
        self.n_sounding = 0;
        self.pending.clear();
        self.step_id = [0; 128];
    }

    /// Emits what is due in frames `a..b` of a segment that starts at `t_a`.
    fn emit_due(&mut self, out: &mut NoteBuffer, a: usize, b: usize, t_a: f64, tpf: f64) {
        while let Some(i) = self.pending.earliest() {
            let ev = self.pending.ev[i];
            let f = frame_of(ev.t, t_a, tpf, a, b);
            if f >= b {
                break;
            }
            self.pending.ev[i].used = false;
            let key = usize::from(ev.key & 0x7F);
            if ev.on {
                if self.n_sounding >= MAX_SOUNDING && !self.sounding[key] {
                    continue;
                }
                if self.sounding[key] {
                    push_sorted(
                        out,
                        NoteEvent {
                            offset: f as u16,
                            kind: NoteKind::Off { key: ev.key },
                        },
                    );
                    self.sounding[key] = false;
                    self.n_sounding -= 1;
                }
                let sent = push_sorted(
                    out,
                    NoteEvent {
                        offset: f as u16,
                        kind: NoteKind::On {
                            key: ev.key,
                            velocity: ev.vel,
                        },
                    },
                );
                if sent {
                    self.sounding[key] = true;
                    self.owner[key] = ev.id;
                    self.n_sounding += 1;
                }
            } else if self.sounding[key] && self.owner[key] == ev.id {
                push_sorted(
                    out,
                    NoteEvent {
                        offset: f as u16,
                        kind: NoteKind::Off { key: ev.key },
                    },
                );
                self.sounding[key] = false;
                self.owner[key] = 0;
                self.n_sounding -= 1;
            }
        }
    }

    // ------------------------------------------------------------ hits

    /// The jitter of a note starting at `at` on `key`: timing in `-1..1` and velocity in `-1..1`.
    fn jitter(&self, at: i64, key: u8) -> (f64, f64) {
        let h = hash64(self.seed, at as u64, u64::from(key));
        let g = hash64(self.seed ^ 0xA5A5_5A5A_C3C3_3C3C, at as u64, u64::from(key));
        (unit_f64(h) * 2.0 - 1.0, unit_f64(g) * 2.0 - 1.0)
    }

    /// Schedules one note: on at `at` (shifted by `delay` and the humanize jitter, never before
    /// `not_before`) and off at `end`.
    #[allow(clippy::too_many_arguments)]
    fn schedule_note(
        &mut self,
        key: u8,
        vel: f32,
        nominal: i64,
        delay: f64,
        end: f64,
        not_before: f64,
        clock: &Clock,
    ) {
        if self.pending.free() < 2 {
            return;
        }
        let h = f64::from(self.humanize);
        let (jt, jv) = self.jitter(nominal, key);
        let on = (nominal as f64 + delay + jt * 10.0 * clock.tpm * h).max(not_before);
        let v = (f64::from(vel) * (1.0 + jv * 0.12 * h)).clamp(1.0 / 127.0, 1.0) as f32;
        let off = if end > on { end } else { on + 1.0 };
        let id = self.next_id();
        self.pending.push(on, key, v, true, id);
        self.pending.push(off, key, v, false, id);
    }

    /// The keys a pick of slot `k` plays.
    fn pick_keys(&self, k: usize, pick: Pick, out: &mut [u8; HIT_NOTES]) -> usize {
        let s = &self.layout.slots[k];
        let v = s.voicing.as_slice();
        if v.is_empty() {
            return 0;
        }
        match pick {
            Pick::All => {
                let mut n = 0;
                if let Some(b) = s.below {
                    out[n] = b;
                    n += 1;
                }
                for key in v {
                    out[n] = *key;
                    n += 1;
                }
                n
            }
            Pick::Low => {
                out[0] = v[0];
                1
            }
            Pick::High => {
                out[0] = v[v.len() - 1];
                1
            }
            Pick::Mid => {
                out[0] = v[v.len() / 2];
                1
            }
        }
    }

    /// One hit of slot `k` starting at the tick `nominal`: the chord (or a part of it) and the
    /// bass as the settings say. `len` is the length of the hit and `slot_end` where the slot
    /// ends, both in ticks from `nominal` and absolute.
    #[allow(clippy::too_many_arguments)]
    fn hit(
        &mut self,
        k: usize,
        pick: Pick,
        nominal: i64,
        len: i64,
        slot_end: i64,
        first: bool,
        not_before: f64,
        clock: &Clock,
    ) {
        let base = if first {
            self.velocity
        } else {
            self.velocity * (1.0 - 0.45 * self.accent)
        };
        let mut keys = [0u8; HIT_NOTES];
        let n = self.pick_keys(k, pick, &mut keys);
        let end = nominal as f64 + (len as f64 * f64::from(self.gate)).max(1.0);
        let spread = if self.layout.style == Style::Strum {
            (self.strum * 60.0).max(25.0)
        } else {
            self.strum * 60.0
        };
        for (j, key) in keys[..n].iter().enumerate() {
            let delay = if pick == Pick::All && n > 1 {
                f64::from(spread) * clock.tpm * j as f64 / (n - 1) as f64
            } else {
                0.0
            };
            self.schedule_note(*key, base, nominal, delay, end, not_before, clock);
        }
        // The bass: once per chord (`root`) or on the hits that play the low note (`pulse`).
        let bass = self.layout.slots[k].bass;
        if let Some(b) = bass {
            let bass_vel = base * 0.9;
            match self.layout.bass {
                BassMode::Root if first => {
                    self.schedule_note(
                        b,
                        bass_vel,
                        nominal,
                        0.0,
                        slot_end as f64,
                        not_before,
                        clock,
                    );
                }
                BassMode::Pulse if matches!(pick, Pick::All | Pick::Low) => {
                    self.schedule_note(b, bass_vel, nominal, 0.0, end, not_before, clock);
                }
                _ => {}
            }
        }
    }

    /// Finds and schedules the hits whose nominal tick falls in `lo..hi`.
    fn discover(&mut self, lo: f64, hi: f64, not_before: f64, clock: &Clock) {
        let total = self.layout.total;
        if total <= 0 {
            return;
        }
        let shift = self.push_shift();
        let tol = clock.tpf * EPS_FRAMES;
        let c0 = ((lo + shift as f64) / total as f64).floor() as i64;
        for c in c0..=c0 + 1 {
            for k in 0..self.layout.n {
                let s = self.layout.slots[k];
                let a = c * total - shift + s.start;
                let end = a + s.len;
                if (end as f64) <= lo - tol || (a as f64) >= hi - tol {
                    continue;
                }
                match pattern(self.layout.style) {
                    None => {
                        let at = a as f64;
                        if at >= lo - tol && at < hi - tol {
                            self.hit(k, Pick::All, a, s.len, end, true, not_before, clock);
                        }
                    }
                    Some((period, hits)) => {
                        let j0 = (((lo - a as f64) / period as f64).floor() as i64).max(0);
                        let mut jp = j0;
                        'periods: while a + jp * period < end
                            && ((a + jp * period) as f64) < hi - tol
                        {
                            for (h, ph) in hits.iter().enumerate() {
                                let off = jp * period + ph.off;
                                if off >= s.len {
                                    break 'periods;
                                }
                                let start = a + off;
                                let at = start as f64;
                                if at >= hi - tol {
                                    break 'periods;
                                }
                                if at >= lo - tol {
                                    let len = ph.len.min(s.len - off);
                                    self.hit(
                                        k,
                                        ph.pick,
                                        start,
                                        len,
                                        end,
                                        jp == 0 && h == 0,
                                        not_before,
                                        clock,
                                    );
                                }
                            }
                            jp += 1;
                        }
                    }
                }
            }
        }
    }

    /// Starting in the middle of a slot: the styles that hold a chord play it at once, a `root`
    /// bass starts at once, the others wait for the next hit.
    fn chase(&mut self, t: f64, clock: &Clock) {
        let total = self.layout.total;
        if total <= 0 {
            return;
        }
        let shift = self.push_shift();
        let q = t + shift as f64;
        let c = (q / total as f64).floor() as i64;
        let pos = q - (c * total) as f64;
        let Some((k, _)) = self.slot_at(pos.floor() as i64) else {
            return;
        };
        let s = self.layout.slots[k];
        let a = c * total - shift + s.start;
        if t - a as f64 <= 1e-3 {
            // On the slot start: the hits of the window start it.
            return;
        }
        let end = a + s.len;
        let now = t.floor() as i64;
        let left = (end - now).max(1);
        if self.layout.style.holds() {
            self.hit(k, Pick::All, now, left, end, true, t, clock);
        } else if self.layout.bass == BassMode::Root
            && let Some(b) = s.bass
        {
            let v = self.velocity * 0.9;
            self.schedule_note(b, v, now, 0.0, end as f64, t, clock);
        }
    }

    // ------------------------------------------------------------ batches

    /// Runs frames `a..b` of the batch: discovers the hits due and emits what is scheduled.
    fn segment(&mut self, out: &mut NoteBuffer, a: usize, b: usize, clock: &Clock) {
        if a >= b {
            return;
        }
        let t_a = clock.at(a);
        let t_b = t_a + (b - a) as f64 * clock.tpf;
        if self.mode != PlayMode::Step
            && self.active
            && (self.mode == PlayMode::Transport || self.held_n > 0)
        {
            let look = f64::from(self.humanize) * 10.0 * clock.tpm;
            let lo = if self.fresh {
                self.fresh = false;
                self.chase(t_a, clock);
                t_a
            } else {
                t_a + look
            };
            self.discover(lo, t_b + look, t_a, clock);
        }
        self.emit_due(out, a, b, t_a, clock.tpf);
    }

    /// `segment` over `a..b` with the loop wrap of the batch handled.
    fn run(&mut self, out: &mut NoteBuffer, a: usize, b: usize, clock: &Clock) {
        if let Some((w, _)) = clock.wrap
            && !self.wrap_done
            && (a..b).contains(&w)
        {
            self.segment(out, a, w, clock);
            self.wrap_done = true;
            self.close_all(out, w);
            self.fresh = true;
            self.segment(out, w, b, clock);
        } else {
            self.segment(out, a, b, clock);
        }
    }

    fn pass(&mut self, out: &mut NoteBuffer, e: NoteEvent, frame: usize) {
        push_sorted(
            out,
            NoteEvent {
                offset: frame as u16,
                kind: e.kind,
            },
        );
    }

    /// One input event at `frame`.
    fn input(&mut self, out: &mut NoteBuffer, e: NoteEvent, frame: usize, clock: &Clock) {
        match e.kind {
            NoteKind::PitchBend { .. } | NoteKind::Cc { .. } | NoteKind::ChannelPressure { .. } => {
                self.pass(out, e, frame);
                return;
            }
            _ => {}
        }
        match self.mode {
            PlayMode::Transport => {
                if !self.thru {
                    return;
                }
                match e.kind {
                    NoteKind::On { key, .. } => {
                        if push_sorted(
                            out,
                            NoteEvent {
                                offset: frame as u16,
                                kind: e.kind,
                            },
                        ) {
                            self.passed[usize::from(key & 0x7F)] = true;
                        }
                    }
                    NoteKind::Off { key } => {
                        if std::mem::take(&mut self.passed[usize::from(key & 0x7F)]) {
                            self.pass(out, e, frame);
                        }
                    }
                    NoteKind::AllOff => {
                        self.passed = [false; 128];
                        self.pass(out, e, frame);
                    }
                    NoteKind::PolyPressure { key, .. } | NoteKind::NoteExpr { key, .. }
                        if self.passed[usize::from(key & 0x7F)] =>
                    {
                        self.pass(out, e, frame);
                    }
                    _ => {}
                }
            }
            PlayMode::Held => match e.kind {
                NoteKind::On { key, .. } => {
                    let k = usize::from(key & 0x7F);
                    if !self.held[k] {
                        self.held[k] = true;
                        self.held_n += 1;
                        if self.held_n == 1 {
                            self.fresh = true;
                        }
                    }
                }
                NoteKind::Off { key } => {
                    let k = usize::from(key & 0x7F);
                    if std::mem::take(&mut self.held[k]) {
                        self.held_n -= 1;
                        if self.held_n == 0 {
                            self.close_all(out, frame);
                        }
                    }
                }
                NoteKind::AllOff => {
                    self.held = [false; 128];
                    self.held_n = 0;
                    self.close_all(out, frame);
                }
                _ => {}
            },
            PlayMode::Step => match e.kind {
                NoteKind::On { key, velocity } => self.step_press(key, velocity, frame, clock),
                NoteKind::Off { key } => self.step_release(key, frame, clock),
                NoteKind::AllOff => {
                    self.close_all(out, frame);
                    self.step_slot = 0;
                }
                _ => {}
            },
        }
    }

    /// A key press in `step` mode: the next chord starts and sounds until the key is released.
    fn step_press(&mut self, key: u8, velocity: f32, frame: usize, clock: &Clock) {
        let n = self.layout.n;
        if n == 0 {
            return;
        }
        let k = usize::from(key & 0x7F);
        if self.step_id[k] != 0 {
            self.step_release(key, frame, clock);
        }
        let idx = self.step_next % n;
        self.step_next = (idx + 1) % n;
        self.step_slot = (idx + 1) as u8;
        let id = self.next_id();
        self.step_id[k] = id;
        let mut keys = [0u8; HIT_NOTES];
        let mut count = 0;
        if let Some(b) = self.layout.slots[idx].bass {
            keys[0] = b;
            count = 1;
        }
        let mut chord = [0u8; HIT_NOTES];
        let m = self.pick_keys(idx, Pick::All, &mut chord);
        for c in &chord[..m.min(HIT_NOTES - count)] {
            keys[count] = *c;
            count += 1;
        }
        let spread = if self.layout.style == Style::Strum {
            (self.strum * 60.0).max(25.0)
        } else {
            self.strum * 60.0
        };
        let now = self.local + frame as f64;
        let h = f64::from(self.humanize);
        for (j, kk) in keys[..count].iter().enumerate() {
            if self.pending.free() < 1 {
                break;
            }
            let delay = if count > 1 {
                f64::from(spread) * clock.tpm * j as f64 / (count - 1) as f64
            } else {
                0.0
            };
            let (_, jv) = self.jitter(now as i64, *kk);
            let v = (f64::from(velocity) * (1.0 + jv * 0.12 * h)).clamp(1.0 / 127.0, 1.0) as f32;
            self.pending.push(now + delay, *kk, v, true, id);
        }
    }

    /// The release of a key in `step` mode: its chord ends.
    fn step_release(&mut self, key: u8, frame: usize, _clock: &Clock) {
        let k = usize::from(key & 0x7F);
        let id = std::mem::take(&mut self.step_id[k]);
        if id == 0 {
            return;
        }
        self.pending.cancel_ons(id);
        let now = self.local + frame as f64;
        for note in 0..128usize {
            if self.sounding[note] && self.owner[note] == id && self.pending.free() > 0 {
                self.pending.push(now, note as u8, 0.0, false, id);
            }
        }
    }

    /// The slot that plays at the batch start, for the meter.
    fn current_slot(&self, t0: f64) -> Option<usize> {
        if self.layout.total <= 0 {
            return None;
        }
        let q = t0 + self.push_shift() as f64;
        let pos = q - (q / self.layout.total as f64).floor() * self.layout.total as f64;
        self.slot_at(pos.floor() as i64).map(|(k, _)| k)
    }

    /// Rebuilds the layout when a param changed or the song key moved; closes the playing chord
    /// when what it plays is no longer what the cycle says.
    fn refresh(&mut self, out: &mut NoteBuffer, shift: u8) {
        if !self.stale && shift == self.shift {
            return;
        }
        self.stale = false;
        self.shift = shift;
        let next = self.build();
        let old = self.layout;
        self.layout = next;
        let changed = match self.cur {
            Some(k) if old.mode == PlayMode::Transport || old.mode == PlayMode::Held => {
                old.n != next.n
                    || old.total != next.total
                    || old.style != next.style
                    || old.bass != next.bass
                    || old.mode != next.mode
                    || old.slots[k] != next.slots[k.min(MAX_SLOTS - 1)]
            }
            _ => old.mode != next.mode || old.style != next.style || old.bass != next.bass,
        };
        if changed && (self.n_sounding > 0 || self.pending.free() < PENDING_CAP) {
            self.close_all(out, 0);
            self.fresh = true;
        } else if changed {
            self.fresh = true;
        }
        if old.mode != next.mode {
            self.held = [false; 128];
            self.held_n = 0;
            self.step_id = [0; 128];
            self.clock.invalidate();
        }
    }

    fn publish_meter(&mut self, slot: Option<usize>) {
        self.meter = [0; 9];
        self.meter[0] = match self.mode {
            PlayMode::Step if self.step_id.iter().any(|i| *i != 0) => self.step_slot,
            PlayMode::Step => 0,
            _ => slot.map_or(0, |k| k as u8 + 1),
        };
        let mut n = 1;
        for k in 0..128usize {
            if self.sounding[k] && n < 9 {
                self.meter[n] = k as u8;
                n += 1;
            }
        }
    }
}

impl Device for ChordWheel {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz();
        self.seed = info.seed;
        self.reset();
        self.flush_alloff = false;
        self.stale = true;
    }

    fn reset(&mut self) {
        if self.n_sounding > 0 || self.passed.iter().any(|p| *p) {
            self.flush_alloff = true;
        }
        self.pending.clear();
        self.sounding = [false; 128];
        self.owner = [0; 128];
        self.n_sounding = 0;
        self.clock.invalidate();
        self.fresh = true;
        self.active = false;
        self.cur = None;
        self.held = [false; 128];
        self.held_n = 0;
        self.step_id = [0; 128];
        self.step_next = 0;
        self.step_slot = 0;
        self.local = 0.0;
        self.passed = [false; 128];
        self.meter = [0; 9];
        if self.bypass.reset() {
            self.flush_alloff = true;
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let int = |lo: f32, hi: f32| plain.round().clamp(lo, hi) as i32;
        let i = id.0;
        match i {
            P_KEY_ROOT => self.key = Key::new(int(0.0, 11.0) as u8, self.key.mode),
            P_KEY_MODE => {
                self.key.mode = if int(0.0, 1.0) == 0 {
                    KeyMode::Major
                } else {
                    KeyMode::Minor
                }
            }
            P_KEY_SOURCE => self.song = int(0.0, 1.0) == 1,
            P_SLOTS => self.slots = int(1.0, 8.0) as usize,
            P_SLOT0..=43 => {
                let n = ((i - P_SLOT0) / 5) as usize;
                match (i - P_SLOT0) % 5 {
                    F_ROOT => self.slot[n].root = int(0.0, 11.0) as u8,
                    F_QUALITY => self.slot[n].quality = int(0.0, 20.0) as u8,
                    F_BASS => self.slot[n].bass = int(0.0, 12.0) as u8,
                    F_BEATS => self.slot[n].beats = int(0.0, 7.0) as u8,
                    F_LOCK => return,
                    _ => {}
                }
            }
            P_MOOD | P_SPICE | P_SEED => return,
            P_LEAD => self.lead = plain >= 0.5,
            P_REGISTER => self.register = int(0.0, 3.0) as u8,
            P_SPREAD => self.spread = Spread::from_index(int(0.0, 2.0) as usize),
            P_EXTEND => self.extend = Extend::from_index(int(0.0, 2.0) as usize),
            P_STYLE => self.style = Style::from_index(int(0.0, 7.0)),
            P_GATE => {
                self.gate = plain.clamp(0.05, 1.0);
                return;
            }
            P_STRUM => {
                self.strum = plain.clamp(0.0, 1.0);
                return;
            }
            P_VELOCITY => {
                self.velocity = plain.clamp(0.0, 1.0);
                return;
            }
            P_ACCENT => {
                self.accent = plain.clamp(0.0, 1.0);
                return;
            }
            P_HUMANIZE => {
                self.humanize = plain.clamp(0.0, 1.0);
                return;
            }
            P_BASS => {
                self.bass = match int(0.0, 2.0) {
                    0 => BassMode::Off,
                    1 => BassMode::Root,
                    _ => BassMode::Pulse,
                }
            }
            P_PLAY => {
                self.mode = match int(0.0, 2.0) {
                    0 => PlayMode::Transport,
                    1 => PlayMode::Held,
                    _ => PlayMode::Step,
                }
            }
            P_THRU => {
                self.thru = plain >= 0.5;
                return;
            }
            P_BYPASS => {
                self.bypass.set_param(plain);
                return;
            }
            _ => return,
        }
        self.stale = true;
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.flush_alloff {
            self.flush_alloff = false;
            push_sorted(
                ctx.notes_out(NOTES_OUT),
                NoteEvent {
                    offset: 0,
                    kind: NoteKind::AllOff,
                },
            );
        }
        let time = ctx.time;
        if self.bypass.begin_batch(ctx.notes_out(NOTES_OUT)) {
            // The switch closes what the device sent and starts it from nothing.
            self.close_all(ctx.notes_out(NOTES_OUT), 0);
            self.passed = [false; 128];
            self.held = [false; 128];
            self.held_n = 0;
            self.clock.invalidate();
            self.fresh = true;
            self.active = false;
        }
        if self.bypass.is_active() {
            for i in 0..ctx.notes_in(NOTES_IN).len() {
                let e = ctx.notes_in(NOTES_IN)[i];
                let _ = self.bypass.route(ctx.notes_out(NOTES_OUT), e);
            }
            self.publish_meter(None);
            self.local += BATCH as f64;
            return;
        }
        // The song key at the start of the batch moves the progression.
        let shift = if self.song {
            time.harmony
                .at(0)
                .and_then(|m| m.key)
                .and_then(|scale| song_key_shift(self.key, scale))
                .unwrap_or(0)
        } else {
            0
        };
        self.refresh(ctx.notes_out(NOTES_OUT), shift);
        // Thru ends when the mode or the switch says so.
        if (!self.thru || self.mode != PlayMode::Transport) && self.passed.iter().any(|p| *p) {
            for k in 0..128usize {
                if std::mem::take(&mut self.passed[k]) {
                    push_sorted(
                        ctx.notes_out(NOTES_OUT),
                        NoteEvent {
                            offset: 0,
                            kind: NoteKind::Off { key: k as u8 },
                        },
                    );
                }
            }
        }
        let tpf_ok = time.ticks_per_frame > 0.0 && time.ticks_per_frame.is_finite();
        let sr_ms = self.sample_rate / 1000.0;
        let clock = if self.mode == PlayMode::Step {
            Clock {
                t0: self.local,
                tpf: 1.0,
                tpm: sr_ms,
                wrap: None,
            }
        } else {
            let wrap = time
                .loop_wrap
                .filter(|w| (1..BATCH).contains(&usize::from(w.offset)))
                .map(|w| (usize::from(w.offset), w.to_tick));
            Clock {
                t0: time.tick,
                tpf: if tpf_ok { time.ticks_per_frame } else { 1.0 },
                tpm: sr_ms * if tpf_ok { time.ticks_per_frame } else { 1.0 },
                wrap,
            }
        };
        self.wrap_done = false;
        let mut slot_now = None;
        if self.mode == PlayMode::Step {
            self.active = false;
        } else if !(time.playing && tpf_ok) {
            if self.active || self.n_sounding > 0 {
                self.close_all(ctx.notes_out(NOTES_OUT), 0);
            }
            self.active = false;
            self.fresh = true;
            self.clock.invalidate();
        } else {
            let jump = self.clock.observe(time.tick, clock.tpf, 0);
            if jump {
                if self.n_sounding > 0 || self.pending.free() < PENDING_CAP {
                    self.close_all(ctx.notes_out(NOTES_OUT), 0);
                }
                self.fresh = true;
            }
            self.active = true;
            let sounds = self.mode == PlayMode::Transport || self.held_n > 0;
            slot_now = if sounds {
                self.current_slot(time.tick)
            } else {
                None
            };
        }
        self.cur = slot_now;
        // Inputs and the generated notes in frame order.
        let mut frame = 0usize;
        for i in 0..ctx.notes_in(NOTES_IN).len() {
            let e = ctx.notes_in(NOTES_IN)[i];
            let Some(e) = self.bypass.route(ctx.notes_out(NOTES_OUT), e) else {
                continue;
            };
            let f = usize::from(e.offset).min(BATCH - 1).max(frame);
            self.run(ctx.notes_out(NOTES_OUT), frame, f, &clock);
            frame = f;
            self.input(ctx.notes_out(NOTES_OUT), e, f, &clock);
        }
        self.run(ctx.notes_out(NOTES_OUT), frame, BATCH, &clock);
        if self.mode != PlayMode::Step && self.active {
            let end = clock.at(BATCH);
            self.clock.advance(end, clock.tpf, 0);
        }
        self.local += BATCH as f64;
        self.publish_meter(slot_now);
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meter.len().min(out.len());
        for (dst, v) in out[..n].iter_mut().zip(self.meter) {
            *dst = f32::from(v);
        }
        n
    }
}

#[cfg(test)]
mod tests;
