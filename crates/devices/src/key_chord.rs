//! `atomik.key-chord`: lock notes to a scale and play chords from single notes (a per-event
//! note map).
//!
//! Stages for every `On`, in order: scale, chord, voicing, octave shift.
//!
//! * **Scale** (`scale.root`, `scale.kind`): `scale.mode` is `off` (notes pass), `snap` (the key
//!   moves into the scale: `nearest` ties down like the piano roll, `up` and `down` take the
//!   first in-scale key at or above or below and fall back to the other side at the end of the
//!   MIDI range) or `filter` (an out-of-scale note and its Off vanish).
//! * **Chord** (`chord`): `off`, a diatonic triad, seventh or ninth built on the scale (a
//!   chromatic scale has no degrees, so the diatonic choices stack the major, major seventh and
//!   major ninth shapes on the key), or one of the fixed chord kinds. `chord.inversion` moves the
//!   lowest note up an octave per step, at most `notes - 1` steps (chords of two notes keep
//!   their position).
//! * **Voicing** (`chord.voicing`), on the sorted keys: `open` raises every second note an
//!   octave, `drop-2` lowers the second highest note an octave (chords of three or more
//!   notes), `bass` adds the chord root an octave below.
//! * **Octave** (`chord.octave`) shifts every note by octaves; a note outside 0..=127 is dropped,
//!   not clamped. At most eight notes sound per key.
//! * **Velocity.** Notes on the pitch class of the chord root keep the played velocity exactly;
//!   the added notes get `chord.velocity` times that (at 0 only the root notes sound).
//! * **Events.** Bend, CC and pressure pass. Polyphonic pressure and per-note expressions follow
//!   the lowest note an input key emitted and drop when it sounds nothing.
//! * **Note safety.** A `NoteRouter` remembers what every input key emitted, so a param change
//!   while keys are held releases the old chord exactly, with no stuck or doubled notes;
//!   chords that share a key keep it sounding until the last of them is released. `AllOff`
//!   closes everything and passes on; a `reset` leaves an `AllOff` for the next batch when a
//!   note sounded.
//! * **Bypass** (`bypass`, last param): see `crate::note_fx::PlayerBypass`.

use atomik_core::theory::{
    ChordKind, DiatonicSize, MAX_CHORD_KEYS, MAX_KEY, PITCH_NAMES, Scale, ScaleKind, chord_into,
    diatonic_chord_into,
};
use atomik_core::{NoteBuffer, NoteEvent, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};

use crate::bypass::bypass_param;
use crate::note_fx::{NoteRouter, PlayerBypass, push_sorted};

const NOTES_IN: SocketId = SocketId(0);
const NOTES_OUT: SocketId = SocketId(1);

static SOCKETS: [SocketDescriptor; 2] = [
    SocketDescriptor {
        id: NOTES_IN,
        key: "notes_in",
        name: "Notes in",
        kind: SocketKind::Notes,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: NOTES_OUT,
        key: "notes_out",
        name: "Notes out",
        kind: SocketKind::Notes,
        direction: Direction::Output,
    },
];

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    range: (f32, f32, f32),
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
        taper: Taper::Linear,
        unit,
        discrete,
    }
}

/// The labels of `scale.kind`, in [`ScaleKind::ALL`] order (a test pins them to the core).
pub(crate) static SCALE_KIND_LABELS: [&str; 16] = [
    "Chromatic",
    "Major",
    "Natural Minor",
    "Harmonic Minor",
    "Melodic Minor",
    "Dorian",
    "Phrygian",
    "Lydian",
    "Mixolydian",
    "Locrian",
    "Major Pentatonic",
    "Minor Pentatonic",
    "Blues",
    "Whole Tone",
    "Phrygian Dominant",
    "Hungarian Minor",
];

/// The labels of `chord`: off, the diatonic sizes, then [`ChordKind::ALL`] (a test pins them to
/// the core).
static CHORD_LABELS: [&str; 25] = [
    "Off",
    "Triad",
    "7th",
    "9th",
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

const SCALE_MODES: Unit = Unit::Choice(&["off", "snap", "filter"]);
const SNAPS: Unit = Unit::Choice(&["nearest", "up", "down"]);
const VOICINGS: Unit = Unit::Choice(&["close", "open", "drop-2", "bass"]);

const P_ROOT: u32 = 0;
const P_KIND: u32 = 1;
const P_MODE: u32 = 2;
const P_SNAP: u32 = 3;
const P_CHORD: u32 = 4;
const P_INVERSION: u32 = 5;
const P_VOICING: u32 = 6;
const P_OCTAVE: u32 = 7;
const P_VELOCITY: u32 = 8;
const P_BYPASS: u32 = 9;

static PARAMS: [ParamDescriptor; 10] = [
    param(
        P_ROOT,
        "scale.root",
        "Root",
        (0.0, 11.0, 0.0),
        Unit::Choice(&PITCH_NAMES),
        true,
    ),
    param(
        P_KIND,
        "scale.kind",
        "Scale",
        (0.0, 15.0, 1.0),
        Unit::Choice(&SCALE_KIND_LABELS),
        true,
    ),
    param(
        P_MODE,
        "scale.mode",
        "Scale mode",
        (0.0, 2.0, 1.0),
        SCALE_MODES,
        true,
    ),
    param(P_SNAP, "scale.snap", "Snap", (0.0, 2.0, 0.0), SNAPS, true),
    param(
        P_CHORD,
        "chord",
        "Chord",
        (0.0, 24.0, 0.0),
        Unit::Choice(&CHORD_LABELS),
        true,
    ),
    param(
        P_INVERSION,
        "chord.inversion",
        "Inversion",
        (0.0, 3.0, 0.0),
        Unit::None,
        true,
    ),
    param(
        P_VOICING,
        "chord.voicing",
        "Voicing",
        (0.0, 3.0, 0.0),
        VOICINGS,
        true,
    ),
    param(
        P_OCTAVE,
        "chord.octave",
        "Octave",
        (-2.0, 2.0, 0.0),
        Unit::Octaves,
        true,
    ),
    param(
        P_VELOCITY,
        "chord.velocity",
        "Added velocity",
        (0.0, 1.0, 1.0),
        Unit::Percent,
        false,
    ),
    bypass_param(P_BYPASS),
];

/// Static description of the Key & Chord device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.key-chord",
    name: "Key & Chord",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`KeyChord`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(KeyChord::new()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaleMode {
    Off,
    Snap,
    Filter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Snap {
    Nearest,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Voicing {
    Close,
    Open,
    Drop2,
    Bass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChordSel {
    Off,
    Diatonic(DiatonicSize),
    Fixed(ChordKind),
}

/// The Key & Chord device.
#[derive(Debug, Clone)]
pub struct KeyChord {
    router: NoteRouter,
    bypass: PlayerBypass,
    /// Emit an `AllOff` at the start of the next batch (a `reset` while something sounded).
    flush_alloff: bool,
    scale: Scale,
    mode: ScaleMode,
    snap: Snap,
    chord: ChordSel,
    inversion: u8,
    voicing: Voicing,
    octave: i32,
    added_velocity: f32,
}

impl KeyChord {
    fn new() -> Self {
        Self {
            router: NoteRouter::new(),
            bypass: PlayerBypass::new(),
            flush_alloff: false,
            scale: Scale::new(0, ScaleKind::Major),
            mode: ScaleMode::Snap,
            snap: Snap::Nearest,
            chord: ChordSel::Off,
            inversion: 0,
            voicing: Voicing::Close,
            octave: 0,
            added_velocity: 1.0,
        }
    }

    /// The in-scale key at or above `key`, if the range has one.
    fn up_from(&self, key: u8) -> Option<u8> {
        (key..=MAX_KEY).find(|k| self.scale.contains(*k))
    }

    /// The in-scale key at or below `key`, if the range has one.
    fn down_from(&self, key: u8) -> Option<u8> {
        (0..=key).rev().find(|k| self.scale.contains(*k))
    }

    /// The key the scale stage gives `key`, or `None` when the filter drops it.
    fn scale_stage(&self, key: u8) -> Option<u8> {
        match self.mode {
            ScaleMode::Off => Some(key),
            ScaleMode::Filter => self.scale.contains(key).then_some(key),
            ScaleMode::Snap => match self.snap {
                Snap::Nearest => Some(self.scale.nearest(key)),
                Snap::Up => self.up_from(key).or_else(|| self.down_from(key)),
                Snap::Down => self.down_from(key).or_else(|| self.up_from(key)),
            },
        }
    }

    /// The chord keys on `base` (ascending, at most eight) before voicing.
    fn chord_keys(&self, base: u8, out: &mut [u8; MAX_CHORD_KEYS]) -> usize {
        match self.chord {
            ChordSel::Off => {
                out[0] = base;
                1
            }
            ChordSel::Fixed(kind) => chord_into(base, kind, self.inversion, out),
            ChordSel::Diatonic(size) if self.scale.is_chromatic() => {
                let kind = match size {
                    DiatonicSize::Triad => ChordKind::Major,
                    DiatonicSize::Seventh => ChordKind::MajorSeventh,
                    DiatonicSize::Ninth => ChordKind::MajorNinth,
                };
                chord_into(base, kind, self.inversion, out)
            }
            ChordSel::Diatonic(size) => {
                diatonic_chord_into(self.scale, base, size, self.inversion, out)
            }
        }
    }

    /// The notes an input `On` plays, written to `out`; returns how many.
    fn notes(&self, key: u8, velocity: f32, out: &mut [(u8, f32); MAX_CHORD_KEYS]) -> usize {
        let Some(base) = self.scale_stage(key) else {
            return 0;
        };
        let mut raw = [0u8; MAX_CHORD_KEYS];
        let n = self.chord_keys(base, &mut raw);
        let mut keys = [0i32; MAX_CHORD_KEYS];
        for (slot, k) in keys.iter_mut().zip(&raw[..n]) {
            *slot = i32::from(*k);
        }
        let mut n_keys = n;
        match self.voicing {
            Voicing::Close => {}
            Voicing::Open => {
                for k in keys[..n_keys].iter_mut().skip(1).step_by(2) {
                    *k += 12;
                }
            }
            Voicing::Drop2 => {
                if n_keys >= 3 {
                    keys[n_keys - 2] -= 12;
                }
            }
            Voicing::Bass => {
                if n_keys < MAX_CHORD_KEYS {
                    keys[n_keys] = i32::from(base) - 12;
                    n_keys += 1;
                }
            }
        }
        let shift = 12 * self.octave;
        let mut kept = 0;
        for i in 0..n_keys {
            let k = keys[i] + shift;
            if (0..=i32::from(MAX_KEY)).contains(&k) {
                keys[kept] = k;
                kept += 1;
            }
        }
        keys[..kept].sort_unstable();
        let mut count = 0;
        for i in 0..kept {
            if i > 0 && keys[i - 1] == keys[i] {
                continue;
            }
            let k = keys[i] as u8;
            let v = if k % 12 == base % 12 {
                velocity
            } else if self.added_velocity > 0.0 {
                (velocity * self.added_velocity).max(1.0 / 127.0)
            } else {
                continue;
            };
            out[count] = (k, v);
            count += 1;
        }
        count
    }

    fn apply(&mut self, out: &mut NoteBuffer, e: NoteEvent) {
        let frame = usize::from(e.offset);
        let pass = |out: &mut NoteBuffer, kind: NoteKind| {
            push_sorted(
                out,
                NoteEvent {
                    offset: e.offset,
                    kind,
                },
            );
        };
        match e.kind {
            NoteKind::On { key, velocity } => {
                let mut notes = [(0u8, 0.0f32); MAX_CHORD_KEYS];
                let n = self.notes(key, velocity, &mut notes);
                self.router.on(out, frame, key, &notes[..n]);
            }
            NoteKind::Off { key } => self.router.off(out, frame, key),
            NoteKind::AllOff => {
                self.router.all_off(out, frame);
                pass(out, NoteKind::AllOff);
            }
            NoteKind::PitchBend { .. } | NoteKind::Cc { .. } | NoteKind::ChannelPressure { .. } => {
                pass(out, e.kind)
            }
            NoteKind::PolyPressure { key, value } => {
                if let Some(k) = self.router.follow(key) {
                    pass(out, NoteKind::PolyPressure { key: k, value });
                }
            }
            NoteKind::NoteExpr { key, kind, value } => {
                if let Some(k) = self.router.follow(key) {
                    pass(
                        out,
                        NoteKind::NoteExpr {
                            key: k,
                            kind,
                            value,
                        },
                    );
                }
            }
        }
    }
}

impl Device for KeyChord {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, _info: &PrepareInfo) {
        self.reset();
        self.flush_alloff = false;
    }

    fn reset(&mut self) {
        if self.router.any_sounding() {
            self.flush_alloff = true;
        }
        self.router.clear();
        if self.bypass.reset() {
            self.flush_alloff = true;
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let int = |lo: f32, hi: f32| plain.round().clamp(lo, hi) as i32;
        match id.0 {
            P_ROOT => self.scale = Scale::new(int(0.0, 11.0) as u8, self.scale.kind),
            P_KIND => {
                self.scale = Scale::new(self.scale.root, ScaleKind::ALL[int(0.0, 15.0) as usize]);
            }
            P_MODE => {
                self.mode = match int(0.0, 2.0) {
                    0 => ScaleMode::Off,
                    1 => ScaleMode::Snap,
                    _ => ScaleMode::Filter,
                }
            }
            P_SNAP => {
                self.snap = match int(0.0, 2.0) {
                    0 => Snap::Nearest,
                    1 => Snap::Up,
                    _ => Snap::Down,
                }
            }
            P_CHORD => {
                self.chord = match int(0.0, 24.0) {
                    0 => ChordSel::Off,
                    i @ 1..=3 => ChordSel::Diatonic(DiatonicSize::ALL[(i - 1) as usize]),
                    i => ChordSel::Fixed(ChordKind::ALL[(i - 4) as usize]),
                }
            }
            P_INVERSION => self.inversion = int(0.0, 3.0) as u8,
            P_VOICING => {
                self.voicing = match int(0.0, 3.0) {
                    0 => Voicing::Close,
                    1 => Voicing::Open,
                    2 => Voicing::Drop2,
                    _ => Voicing::Bass,
                }
            }
            P_OCTAVE => self.octave = int(-2.0, 2.0),
            P_VELOCITY => self.added_velocity = plain.clamp(0.0, 1.0),
            P_BYPASS => self.bypass.set_param(plain),
            _ => {}
        }
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
        if self.bypass.begin_batch(ctx.notes_out(NOTES_OUT)) {
            // The switch closes what the device sent and starts it from nothing.
            self.router.close_all(ctx.notes_out(NOTES_OUT), 0);
            self.router.clear();
        }
        for i in 0..ctx.notes_in(NOTES_IN).len() {
            let e = ctx.notes_in(NOTES_IN)[i];
            if let Some(e) = self.bypass.route(ctx.notes_out(NOTES_OUT), e) {
                self.apply(ctx.notes_out(NOTES_OUT), e);
            }
        }
    }
}

#[cfg(test)]
mod tests;
