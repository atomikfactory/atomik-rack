//! The Chord Wheel's rack panel (five rack units): the wheel at the left, the eight progression
//! slots and the controls at the right.
//!
//! ```text
//! WHEEL                          PROGRESSION                        [-] 4 SLOTS [+]
//!      ___                       [1 Am   lock][2 F    lock][3 C   lock][4 G   lock]
//!   .-/ C \-.   three rings:     [5 ...      ][6 ...     ][7 ...    ][8 ...      ]
//!  | G  Am  F |  majors, relative SETTINGS
//!   '-\___/-'    minors, dim     mood spice key mode follow
//! [GENERATE] [SUGGEST]           style gate strum velocity accent  ... TO LANE, TO CLIP
//! ```
//!
//! # The wheel
//!
//! Three rings of twelve wedges in fifths order with C at the top: the major chords outside, their
//! relative minors in the middle and the diminished chord on each major's leading tone inside. The
//! seven triads of the home key (`key.root`, `key.mode`) are tinted and carry their roman numeral;
//! the key's name is in the hub. A wedge stands for the chord's triad: the chord of a slot is
//! marked on the wedge of its root and family (a `Am7` on the A minor wedge), the selected slot's
//! wedge is outlined, the playing slot's wedge is lit and small numbered tags show which slots
//! sit on a wedge.
//!
//! * A **click** sets the selected slot to the wedge's chord, plays it (while the transport is
//!   stopped) and selects the next slot.
//! * A **right click** opens a menu of qualities for the wedge's root and a Slash Bass submenu.
//! * A click with the **range-select role** held (Shift by default) makes the wedge's chord the
//!   home key (`key.root` and `key.mode`, one undo step); the diminished ring has no key.
//! * With **SUGGEST** on the wedges are tinted by the ranking of [`progression::suggest_next`] after
//!   the chord of the slot before the selected one, and the three best carry the badges 1 to 3.
//!
//! # The progression
//!
//! Eight cards: the slot number, a padlock (a locked slot keeps its chord when GENERATE runs), the
//! chord symbol, its roman numeral and its length. A click selects the slot; a vertical drag or the
//! wheel on the length steps `slot.N.beats`; the `-` and `+` change `slots`. The slots past
//! `slots` are dimmed and cannot be selected. The selected slot and the sounding slot (the first
//! meter value) are marked.
//!
//! The view state (the selected slot, the SUGGEST switch) belongs to the panel, not to the
//! project: the rack view keeps it per device.

use crate::autolayout::{CONTENT_X, MARGIN_BOTTOM, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::context_menu::{Entry, MenuItemDef, SubmenuDef};
use crate::paint::{button_sized, fit_label_raw};
use crate::seq_panel::{PlayerInfo, back_layout, place_controls};
use crate::theme::{Theme, contrast_ratio, mix};
use crate::widget::current_value;
use atomik_core::progression::{
    Chord, Key, KeyMode, Mood, Strength, roman, suggest_next, symbol_in,
};
use atomik_core::theory::ChordKind;
use atomik_device_sdk::{DeviceDescriptor, RACK_UNIT};
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, CubicBez, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the Chord Wheel.
pub const CHORD_WHEEL_TYPE: &str = "atomik.chord-wheel";
/// Height of the panel in rack units.
pub const CHORD_WHEEL_UNITS: u8 = 5;
/// The slots of a progression.
pub const SLOTS: usize = atomik_core::progression::MAX_SLOTS;

/// Outer radius of the wheel.
const R_OUT: f64 = 138.0;
/// Drawn radii (inner, outer) of the rings: majors, minors, diminished.
const RINGS: [(f64, f64); 3] = [(104.0, 138.0), (70.0, 102.0), (38.0, 68.0)];
/// The hub (inside it the wheel has no wedge).
const HUB_R: f64 = 36.0;
/// Where a press changes ring: inner to middle, middle to outer.
const SPLIT: [f64; 2] = [69.0, 103.0];
/// Width of the wheel's column.
const LEFT_W: f64 = 280.0;
/// Left edge of the right column.
const RX0: f64 = CONTENT_X.0 + LEFT_W + 10.0;
/// Width of a column of controls.
const COL_W: f64 = 67.0;
/// Top of the controls.
const CONTROLS_TOP: f64 = 138.0;
/// Size of a progression card.
const CARD: (f64, f64) = (81.0, 48.0);
const CARD_GAP: f64 = 4.0;

/// The panel's controls in cell order (five per row); the two empty keys are the cells of TO LANE
/// and TO CLIP.
const CONTROLS: [&str; 20] = [
    "gen.mood",
    "gen.spice",
    "key.root",
    "key.mode",
    "key.source",
    "style",
    "gate",
    "strum",
    "velocity",
    "accent",
    "humanize",
    "bass",
    "play.mode",
    "thru",
    "voice.lead",
    "voice.register",
    "voice.spread",
    "voice.extend",
    "",
    "",
];

/// The label of a control of the panel (the generated one for the rest).
pub fn wheel_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "gen.mood" => "Mood",
        "gen.spice" => "Spice",
        "key.root" => "Key",
        "key.mode" => "Mode",
        "key.source" => "Follow song",
        "style" => "Style",
        "gate" => "Gate",
        "strum" => "Strum",
        "velocity" => "Velocity",
        "accent" => "Accent",
        "humanize" => "Humanize",
        "bass" => "Bass",
        "play.mode" => "Play",
        "thru" => "Thru",
        "voice.lead" => "Voice lead",
        "voice.register" => "Register",
        "voice.spread" => "Spread",
        "voice.extend" => "Extend",
        _ => return None,
    })
}

// ------------------------------------------------------------------ the wedges

/// A ring of the wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ring {
    /// The major chords (outside).
    Major,
    /// The relative minors (middle).
    Minor,
    /// The diminished chords (inside).
    Dim,
}

impl Ring {
    /// The rings from the outside in.
    pub const ALL: [Ring; 3] = [Ring::Major, Ring::Minor, Ring::Dim];

    /// The position in [`Ring::ALL`].
    pub fn index(self) -> usize {
        self.pick(0, 1, 2)
    }

    fn pick<T>(self, major: T, minor: T, dim: T) -> T {
        match self {
            Ring::Major => major,
            Ring::Minor => minor,
            Ring::Dim => dim,
        }
    }

    /// The triad of the ring.
    pub fn kind(self) -> ChordKind {
        self.pick(ChordKind::Major, ChordKind::Minor, ChordKind::Dim)
    }

    /// The ring that holds chords of `kind`: the minor sixths, sevenths and ninths on the minor
    /// ring, the diminished and half-diminished chords inside, everything else with the majors.
    pub fn of(kind: ChordKind) -> Ring {
        match kind {
            ChordKind::Minor
            | ChordKind::MinorSixth
            | ChordKind::MinorSeventh
            | ChordKind::MinorNinth => Ring::Minor,
            ChordKind::Dim | ChordKind::HalfDim | ChordKind::DimSeventh => Ring::Dim,
            _ => Ring::Major,
        }
    }

    /// The drawn radii (inner, outer).
    fn radii(self) -> (f64, f64) {
        RINGS[self.index()]
    }
}

/// One wedge: a ring and the position round it (0 = top, clockwise in fifths: C, G, D ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Wedge {
    /// The ring.
    pub ring: Ring,
    /// The position, `0..12`.
    pub pos: u8,
}

impl Wedge {
    /// The wedge at `pos` (taken modulo 12) of `ring`.
    pub fn new(ring: Ring, pos: u8) -> Self {
        Self {
            ring,
            pos: pos % 12,
        }
    }

    /// The root pitch class of the wedge's chord.
    pub fn root(self) -> u8 {
        let p = (u16::from(self.pos) * 7) % 12;
        let offset = self.ring.pick(0, 9, 11);
        ((p + offset) % 12) as u8
    }

    /// The chord of the wedge: its triad.
    pub fn chord(self) -> Chord {
        Chord::new(self.root(), self.ring.kind())
    }

    /// The wedge a chord belongs to (by its root and family, see [`Ring::of`]).
    pub fn of(chord: Chord) -> Wedge {
        let ring = Ring::of(chord.kind);
        let shifted = (u16::from(chord.root % 12) + 12 - u16::from(ring.pick(0u8, 9, 11))) % 12;
        Wedge::new(ring, ((shifted * 7) % 12) as u8)
    }

    /// The angle of the wedge's middle, radians from the right, clockwise (y down).
    fn angle(self) -> f64 {
        -std::f64::consts::FRAC_PI_2 + f64::from(self.pos) * std::f64::consts::PI / 6.0
    }
}

/// The key a wedge makes when it is made the home key: the major wedges the major key, the minor
/// wedges the minor key; the diminished ring makes none.
pub fn key_of(w: Wedge) -> Option<Key> {
    match w.ring {
        Ring::Major => Some(Key::new(w.root(), KeyMode::Major)),
        Ring::Minor => Some(Key::new(w.root(), KeyMode::Minor)),
        Ring::Dim => None,
    }
}

/// Whether the wedge's triad is one of the seven triads of `key`.
pub fn is_diatonic(key: Key, w: Wedge) -> bool {
    let rel = (w.root() + 12 - key.root) % 12;
    let triads: [(u8, Ring); 7] = match key.mode {
        KeyMode::Major => [
            (0, Ring::Major),
            (2, Ring::Minor),
            (4, Ring::Minor),
            (5, Ring::Major),
            (7, Ring::Major),
            (9, Ring::Minor),
            (11, Ring::Dim),
        ],
        KeyMode::Minor => [
            (0, Ring::Minor),
            (2, Ring::Dim),
            (3, Ring::Major),
            (5, Ring::Minor),
            (7, Ring::Minor),
            (8, Ring::Major),
            (10, Ring::Major),
        ],
    };
    triads.contains(&(rel, w.ring))
}

/// How a suggested wedge is tinted and its rank (1 to 3) when it is one of the best three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggested {
    /// The wedge.
    pub wedge: Wedge,
    /// The best strength of the chords that fold onto it.
    pub strength: Strength,
    /// 1 to 3 for the three best wedges, 0 for the others.
    pub rank: u8,
}

fn strength_order(s: Strength) -> u8 {
    match s {
        Strength::Strong => 0,
        Strength::Colour => 1,
        Strength::Surprise => 2,
    }
}

/// The suggestions of the model after `prev`, folded onto the wedges: a chord with a seventh or
/// an extension counts on its triad's wedge, a wedge takes the best strength of the chords on it
/// and the first three wedges in the model's ranking are numbered.
pub fn suggestions(key: Key, mood: Mood, prev: Chord, spice: f32) -> Vec<Suggested> {
    let mut out: Vec<Suggested> = Vec::new();
    for (chord, strength) in suggest_next(key, mood, prev, spice) {
        let wedge = Wedge::of(chord);
        match out.iter_mut().find(|s| s.wedge == wedge) {
            Some(s) => {
                if strength_order(strength) < strength_order(s.strength) {
                    s.strength = strength;
                }
            }
            None => {
                let rank = if out.len() < 3 {
                    out.len() as u8 + 1
                } else {
                    0
                };
                out.push(Suggested {
                    wedge,
                    strength,
                    rank,
                });
            }
        }
    }
    out
}

// ------------------------------------------------------------------ geometry

/// A button or area of the Chord Wheel panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelButton {
    /// The wheel: the wedge under the pointer decides.
    Wheel,
    /// Card `0..8`: selects the slot.
    Card(u8),
    /// The padlock of card `0..8`.
    Lock(u8),
    /// The length of card `0..8` (drag or scroll).
    Beats(u8),
    /// One slot fewer.
    SlotsDec,
    /// One slot more.
    SlotsInc,
    /// GENERATE.
    Generate,
    /// The SUGGEST switch.
    Suggest,
    /// TO LANE: the progression to the song's chord lane.
    ToLane,
    /// TO CLIP: the progression to a note clip.
    ToClip,
}

/// Geometry of the Chord Wheel panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordWheelPanel {
    /// The centre of the wheel.
    pub centre: Point,
    /// The square the wheel sits in.
    pub wheel: Rect,
    /// The SONG tag in the title of the wheel.
    pub song_tag: Rect,
    /// The eight progression cards.
    pub cards: [Rect; SLOTS],
    /// The padlock of each card.
    pub locks: [Rect; SLOTS],
    /// The length field of each card.
    pub beats: [Rect; SLOTS],
    /// `-` of the slot count.
    pub slots_dec: Rect,
    /// The slot count read-out.
    pub slots_text: Rect,
    /// `+` of the slot count.
    pub slots_inc: Rect,
    /// GENERATE.
    pub generate: Rect,
    /// SUGGEST.
    pub suggest: Rect,
    /// TO LANE.
    pub to_lane: Rect,
    /// TO CLIP.
    pub to_clip: Rect,
    /// The line that names the selected slot under the wheel.
    pub readout: Rect,
    /// The two lines of hints under it.
    pub hint: Rect,
}

impl ChordWheelPanel {
    /// Every button, in drawing order (a later one is on top where they overlap; the wheel is
    /// not one of them).
    pub fn buttons(&self) -> Vec<(WheelButton, Rect)> {
        let mut v = Vec::new();
        for (i, r) in self.cards.iter().enumerate() {
            v.push((WheelButton::Card(i as u8), *r));
        }
        for (i, r) in self.locks.iter().enumerate() {
            v.push((WheelButton::Lock(i as u8), *r));
        }
        for (i, r) in self.beats.iter().enumerate() {
            v.push((WheelButton::Beats(i as u8), *r));
        }
        v.extend([
            (WheelButton::SlotsDec, self.slots_dec),
            (WheelButton::SlotsInc, self.slots_inc),
            (WheelButton::Generate, self.generate),
            (WheelButton::Suggest, self.suggest),
            (WheelButton::ToLane, self.to_lane),
            (WheelButton::ToClip, self.to_clip),
        ]);
        v
    }

    /// The rectangle of a button (the square of the wheel for the wheel itself).
    pub fn rect(&self, b: WheelButton) -> Rect {
        match b {
            WheelButton::Wheel => self.wheel,
            WheelButton::Card(i) => self.cards[usize::from(i).min(SLOTS - 1)],
            WheelButton::Lock(i) => self.locks[usize::from(i).min(SLOTS - 1)],
            WheelButton::Beats(i) => self.beats[usize::from(i).min(SLOTS - 1)],
            WheelButton::SlotsDec => self.slots_dec,
            WheelButton::SlotsInc => self.slots_inc,
            WheelButton::Generate => self.generate,
            WheelButton::Suggest => self.suggest,
            WheelButton::ToLane => self.to_lane,
            WheelButton::ToClip => self.to_clip,
        }
    }

    /// The wedge under a panel point: `None` in the hub and outside the wheel. A press changes
    /// ring half way through the gap between two rings, and wedge at the line half way between
    /// two chord centres.
    pub fn wedge_at(&self, p: Point) -> Option<Wedge> {
        let (dx, dy) = (p.x - self.centre.x, p.y - self.centre.y);
        let r = dx.hypot(dy);
        if !(HUB_R..=R_OUT).contains(&r) {
            return None;
        }
        let ring = if r < SPLIT[0] {
            Ring::Dim
        } else if r < SPLIT[1] {
            Ring::Minor
        } else {
            Ring::Major
        };
        // Degrees clockwise from the top.
        let deg = (dy.atan2(dx).to_degrees() + 90.0).rem_euclid(360.0);
        let pos = ((deg + 15.0) / 30.0).floor() as u8 % 12;
        Some(Wedge { ring, pos })
    }

    /// The middle of a wedge (panel coordinates).
    pub fn wedge_centre(&self, w: Wedge) -> Point {
        let (r0, r1) = w.ring.radii();
        self.polar((r0 + r1) / 2.0, w.angle())
    }

    fn polar(&self, r: f64, angle: f64) -> Point {
        pt(
            self.centre.x + r * angle.cos(),
            self.centre.y + r * angle.sin(),
        )
    }
}

/// The panel of `atomik.chord-wheel` (five rack units, see the module documentation); the back is
/// the generic socket layout.
pub fn chord_wheel_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let top = MARGIN_TOP + TITLE_H + 6.0;
    let centre = pt(x0 + LEFT_W / 2.0, top + R_OUT);
    let wheel = Rect::new(
        centre.x - R_OUT,
        centre.y - R_OUT,
        centre.x + R_OUT,
        centre.y + R_OUT,
    );
    let rx1 = CONTENT_X.1;
    // The cards: two rows of four.
    let cards = std::array::from_fn(|i| {
        let (col, row) = ((i % 4) as f64, (i / 4) as f64);
        let x = RX0 + col * (CARD.0 + CARD_GAP);
        let y = top + row * (CARD.1 + CARD_GAP);
        Rect::new(x, y, x + CARD.0, y + CARD.1)
    });
    let locks = std::array::from_fn(|i| {
        let c = cards[i];
        Rect::new(c.x1 - 18.0, c.y0 + 2.0, c.x1 - 2.0, c.y0 + 17.0)
    });
    let beats = std::array::from_fn(|i| {
        let c = cards[i];
        Rect::new(c.x1 - 44.0, c.y1 - 15.0, c.x1 - 3.0, c.y1 - 3.0)
    });
    let stepper_y = (MARGIN_TOP - 2.0, MARGIN_TOP + 13.0);
    let slots_dec = Rect::new(rx1 - 92.0, stepper_y.0, rx1 - 76.0, stepper_y.1);
    let slots_text = Rect::new(rx1 - 74.0, stepper_y.0, rx1 - 18.0, stepper_y.1);
    let slots_inc = Rect::new(rx1 - 16.0, stepper_y.0, rx1, stepper_y.1);
    // Under the wheel.
    let by = (top + 2.0 * R_OUT + 8.0, top + 2.0 * R_OUT + 29.0);
    let half = (LEFT_W - 4.0) / 2.0;
    let generate = Rect::new(x0, by.0, x0 + half, by.1);
    let suggest = Rect::new(x0 + half + 4.0, by.0, x0 + LEFT_W, by.1);
    let readout = Rect::new(x0, by.1 + 6.0, x0 + LEFT_W, by.1 + 24.0);
    let hint = Rect::new(x0, by.1 + 27.0, x0 + LEFT_W, by.1 + 50.0);
    // TO LANE and TO CLIP take the last two cells of the controls.
    let cell = |n: usize| {
        let (col, row) = ((n % 5) as f64, (n / 5) as f64);
        let cx = RX0 + col * COL_W + COL_W / 2.0;
        let y =
            CONTROLS_TOP + row * crate::autolayout::ROW_H + (crate::autolayout::ROW_H - 24.0) / 2.0;
        Rect::new(cx - 30.0, y, cx + 30.0, y + 24.0)
    };
    let panel = ChordWheelPanel {
        centre,
        wheel,
        song_tag: Rect::new(
            x0 + LEFT_W - 84.0,
            MARGIN_TOP - 1.0,
            x0 + LEFT_W,
            MARGIN_TOP + TITLE_H + 1.0,
        ),
        cards,
        locks,
        beats,
        slots_dec,
        slots_text,
        slots_inc,
        generate,
        suggest,
        to_lane: cell(18),
        to_clip: cell(19),
        readout,
        hint,
    };
    let front = place_controls(desc, &CONTROLS, RX0, COL_W, 5, CONTROLS_TOP);
    let front_sections = vec![
        Section {
            label: "WHEEL".to_string(),
            rect: Rect::new(x0, MARGIN_TOP, x0 + LEFT_W - 90.0, MARGIN_TOP + TITLE_H),
        },
        Section {
            label: "PROGRESSION".to_string(),
            rect: Rect::new(RX0, MARGIN_TOP, rx1 - 100.0, MARGIN_TOP + TITLE_H),
        },
        Section {
            label: "SETTINGS".to_string(),
            rect: Rect::new(RX0, CONTROLS_TOP - 14.0, rx1, CONTROLS_TOP - 14.0 + TITLE_H),
        },
    ];
    let (units, back, back_sections) = back_layout(desc, CHORD_WHEEL_UNITS);
    debug_assert!(
        f64::from(units) * f64::from(RACK_UNIT)
            > front.iter().map(|c| c.rect.y1).fold(0.0, f64::max) + MARGIN_BOTTOM
    );
    debug_assert!(f64::from(units) * f64::from(RACK_UNIT) > readout.y1 + MARGIN_BOTTOM);
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        scale_strip: None,
        chord_wheel: Some(panel),
        modsrc: None,
        wavetable: None,
        fm: None,
        granular: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back,
        back_sections,
    }
}

// ------------------------------------------------------------------ the device's state

/// What the panel reads of a Chord Wheel device.
#[derive(Debug, Clone, PartialEq)]
pub struct WheelState {
    /// The home key.
    pub key: Key,
    /// The active slots, 1 to 8.
    pub slots: usize,
    /// The chord of each slot.
    pub chords: [Chord; SLOTS],
    /// The length of each slot in beats.
    pub beats: [u8; SLOTS],
    /// Whether each slot is locked.
    pub locks: [bool; SLOTS],
    /// The mood of the generator.
    pub mood: Mood,
    /// The spice of the generator.
    pub spice: f32,
    /// Whether the device follows the song key.
    pub follows_song: bool,
}

fn param(device: &RackDevice, desc: &DeviceDescriptor, key: &str) -> f32 {
    desc.param_by_key(key)
        .map_or(0.0, |p| current_value(device, p))
}

/// The state of a Chord Wheel device (defaults for a param it does not hold).
pub fn wheel_state(device: &RackDevice, desc: &DeviceDescriptor) -> WheelState {
    let get = |k: &str| param(device, desc, k);
    let mode = if get("key.mode") >= 0.5 {
        KeyMode::Minor
    } else {
        KeyMode::Major
    };
    let key = Key::new(get("key.root").round().clamp(0.0, 11.0) as u8, mode);
    let slot = |n: usize, f: &str| get(&format!("slot.{}.{f}", n + 1));
    let chords = std::array::from_fn(|n| {
        let kind = ChordKind::ALL
            [(slot(n, "quality").round().max(0.0) as usize).min(ChordKind::ALL.len() - 1)];
        let chord = Chord::new(slot(n, "root").round().clamp(0.0, 11.0) as u8, kind);
        match slot(n, "bass").round() as i32 {
            b if b >= 1 => chord.over((b - 1) as u8),
            _ => chord,
        }
    });
    WheelState {
        key,
        slots: (get("slots").round() as usize).clamp(1, SLOTS),
        chords,
        beats: std::array::from_fn(|n| {
            let i = (slot(n, "beats").round().max(0.0) as usize)
                .min(atomik_core::progression::BEATS.len() - 1);
            atomik_core::progression::BEATS[i]
        }),
        locks: std::array::from_fn(|n| slot(n, "lock") >= 0.5),
        mood: Mood::from_index(get("gen.mood").round().max(0.0) as usize).unwrap_or(Mood::Pop),
        spice: get("gen.spice").clamp(0.0, 1.0),
        follows_song: get("key.source") >= 0.5,
    }
}

/// Whether the device follows the song key (`key.source`).
pub fn follows_song(device: &RackDevice, desc: &DeviceDescriptor) -> bool {
    param(device, desc, "key.source") >= 0.5
}

/// The slot the panel selects: `edit` kept inside the active slots.
pub fn selected_slot(edit: usize, slots: usize) -> usize {
    edit.min(slots.clamp(1, SLOTS) - 1)
}

/// The slot after `slot` among the `slots` active ones (the first after the last).
pub fn next_slot(slot: usize, slots: usize) -> usize {
    (slot + 1) % slots.clamp(1, SLOTS)
}

/// The slot the suggestions follow: the one before `slot` (the last active slot before the
/// first).
pub fn previous_slot(slot: usize, slots: usize) -> usize {
    let slots = slots.clamp(1, SLOTS);
    (slot + slots - 1) % slots
}

/// The slot a meter value names (`1..=8`; 0 and anything else: none), 0-based.
pub fn playing_slot(meters: &[f32]) -> Option<usize> {
    let v = meters.first().copied()?;
    (1.0..=8.0).contains(&v).then(|| v as usize - 1)
}

/// The length a drag of `dy` logical pixels upwards leaves, as an index in the lengths (8 px
/// per step).
pub fn dragged_beats(start: usize, dy: f64) -> usize {
    (start as f64 + (dy / 8.0).round()).clamp(0.0, 7.0) as usize
}

// ------------------------------------------------------------------ the menu

/// The qualities the right-click menu of a wedge offers for its root.
pub fn menu_kinds(ring: Ring) -> &'static [ChordKind] {
    match ring {
        Ring::Major => &[
            ChordKind::Major,
            ChordKind::Seventh,
            ChordKind::MajorSeventh,
            ChordKind::Sixth,
            ChordKind::Add9,
            ChordKind::Ninth,
            ChordKind::MajorNinth,
            ChordKind::Sus2,
            ChordKind::Sus4,
            ChordKind::Aug,
        ],
        Ring::Minor => &[
            ChordKind::Minor,
            ChordKind::MinorSeventh,
            ChordKind::MinorSixth,
            ChordKind::MinorNinth,
            ChordKind::Sus2,
            ChordKind::Sus4,
        ],
        Ring::Dim => &[ChordKind::Dim, ChordKind::HalfDim, ChordKind::DimSeventh],
    }
}

/// The command of every row of the wheel's menu (the argument is [`pick_arg`]).
pub const PICK_COMMAND: &str = "rack.wheel_pick";

/// The argument of a menu row: the slot (0-based), the chord's root, its quality (index in
/// [`ChordKind::ALL`]) and its bass pitch class (`-` for none), joined by colons.
pub fn pick_arg(slot: usize, chord: Chord) -> String {
    let kind = ChordKind::ALL
        .iter()
        .position(|k| *k == chord.kind)
        .unwrap_or(0);
    match chord.bass {
        Some(b) => format!("{slot}:{}:{kind}:{b}", chord.root),
        None => format!("{slot}:{}:{kind}:-", chord.root),
    }
}

/// The slot and chord of a menu row's argument ([`pick_arg`]); `None` for anything else.
pub fn parse_pick(arg: &str) -> Option<(usize, Chord)> {
    let mut parts = arg.split(':');
    let slot: usize = parts.next()?.parse().ok()?;
    let root: u8 = parts.next()?.parse().ok()?;
    let kind: usize = parts.next()?.parse().ok()?;
    let bass = parts.next()?;
    if parts.next().is_some() || slot >= SLOTS || root >= 12 {
        return None;
    }
    let chord = Chord::new(root, *ChordKind::ALL.get(kind)?);
    match bass {
        "-" => Some((slot, chord)),
        b => Some((slot, chord.over(b.parse::<u8>().ok().filter(|b| *b < 12)?))),
    }
}

/// The right-click menu of `wedge` for slot `slot` of a progression in `key` whose chord is now
/// `current`: the qualities of the wedge's root, then a Slash Bass submenu (no slash, the chord
/// tones, then the other notes of the key). The row of the current chord is checked.
pub fn menu_entries(slot: usize, wedge: Wedge, key: Key, current: Chord) -> Vec<Entry> {
    let item = |label: String, chord: Chord, checked: bool| {
        Entry::Item(MenuItemDef {
            command: PICK_COMMAND.to_string(),
            arg: Some(pick_arg(slot, chord)),
            label,
            shortcut: None,
            enabled: true,
            checked: Some(checked),
            swatch: None,
        })
    };
    let root = wedge.root();
    let mut entries: Vec<Entry> = menu_kinds(wedge.ring)
        .iter()
        .map(|kind| {
            let chord = Chord::new(root, *kind);
            item(symbol_in(key, chord), chord, current == chord)
        })
        .collect();
    // The bass notes of the chord the quality rows would give: the current one when it is on
    // this root, else the wedge's triad.
    let base = if current.root == root {
        Chord {
            bass: None,
            ..current
        }
    } else {
        wedge.chord()
    };
    let mut basses: Vec<u8> = Vec::new();
    for iv in base.kind.intervals() {
        let pc = (base.root + iv % 12) % 12;
        if pc != base.root && !basses.contains(&pc) {
            basses.push(pc);
        }
    }
    let tones = basses.len();
    let scale: &[u8] = match key.mode {
        KeyMode::Major => &[0, 2, 4, 5, 7, 9, 11],
        KeyMode::Minor => &[0, 2, 3, 5, 7, 8, 10],
    };
    for iv in scale {
        let pc = (key.root + iv) % 12;
        if pc != base.root && !basses.contains(&pc) {
            basses.push(pc);
        }
    }
    let on_this = current.root == root;
    let mut rows = vec![item(
        "No slash".to_string(),
        base,
        on_this && current.bass.is_none(),
    )];
    for (n, pc) in basses.iter().enumerate() {
        if n == tones {
            rows.push(Entry::Separator);
        }
        let chord = base.over(*pc);
        rows.push(item(
            symbol_in(key, chord),
            chord,
            on_this && current.bass == Some(*pc),
        ));
    }
    entries.push(Entry::Separator);
    entries.push(Entry::Submenu(SubmenuDef {
        label: "Slash Bass".to_string(),
        enabled: true,
        entries: rows,
    }));
    entries
}

// ------------------------------------------------------------------ painting

/// The text colour that reads on `fill`: the theme's, or black or white when that is better.
fn on(theme: &Theme, fill: Color) -> Color {
    [theme.text, Color::BLACK, Color::WHITE]
        .into_iter()
        .max_by(|a, b| {
            contrast_ratio(*a, fill)
                .partial_cmp(&contrast_ratio(*b, fill))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(theme.text)
}

/// A circular arc as one cubic (`a1 - a0` at most a quarter turn).
fn arc(c: Point, r: f64, a0: f64, a1: f64) -> CubicBez {
    let k = 4.0 / 3.0 * ((a1 - a0) / 4.0).tan();
    let p0 = pt(c.x + r * a0.cos(), c.y + r * a0.sin());
    let p3 = pt(c.x + r * a1.cos(), c.y + r * a1.sin());
    CubicBez::new(
        p0,
        pt(p0.x - k * r * a0.sin(), p0.y + k * r * a0.cos()),
        pt(p3.x + k * r * a1.sin(), p3.y - k * r * a1.cos()),
        p3,
    )
}

/// The ring segment between the radii `r0` and `r1` and the angles `a0` and `a1`.
fn segment(c: Point, r0: f64, r1: f64, a0: f64, a1: f64) -> BezPath {
    let mut path = BezPath::new();
    let outer = arc(c, r1, a0, a1);
    path.move_to(outer.p0);
    path.curve_to(outer.p1, outer.p2, outer.p3);
    let inner = arc(c, r0, a1, a0);
    path.line_to(inner.p0);
    path.curve_to(inner.p1, inner.p2, inner.p3);
    path.close_path();
    path
}

/// What the wheel draws besides the device's params.
struct Marks {
    selected: Wedge,
    playing: Option<Wedge>,
    suggest: Vec<Suggested>,
}

/// The root names of the wedges of each ring at positions 0 to 11, spelled as a circle of fifths
/// is written: sharps from B round to F#, flats from Db on. The minor ring is the relative minor
/// of the major above it (D#m under F#, Bbm under Db) and the diminished ring the leading tone
/// of that major (E#, written F, under F#).
const WHEEL_ROOTS: [[&str; 12]; 3] = [
    [
        "C", "G", "D", "A", "E", "B", "F#", "Db", "Ab", "Eb", "Bb", "F",
    ],
    [
        "A", "E", "B", "F#", "C#", "G#", "D#", "Bb", "F", "C", "G", "D",
    ],
    [
        "B", "F#", "C#", "G#", "D#", "A#", "F", "C", "G", "D", "A", "E",
    ],
];

/// The label of a wedge: a fixed spelling for each position, whatever the home key is (the cards,
/// the readout and the wedge menu spell the chord for the home key instead).
pub fn wedge_label(w: Wedge) -> String {
    let root = WHEEL_ROOTS[w.ring.index()][usize::from(w.pos % 12)];
    match w.ring {
        Ring::Major => root.to_string(),
        Ring::Minor => format!("{root}m"),
        Ring::Dim => format!("{root}\u{b0}"),
    }
}

fn paint_wheel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &ChordWheelPanel,
    state: &WheelState,
    marks: &Marks,
    accent: Color,
) {
    let key = state.key;
    let tags = |w: Wedge| -> Vec<usize> {
        (0..state.slots)
            .filter(|n| Wedge::of(state.chords[*n]) == w)
            .collect()
    };
    for ring in Ring::ALL {
        let (r0, r1) = ring.radii();
        let base = match ring {
            Ring::Major => theme.button,
            Ring::Minor => mix(theme.button, theme.lcd, 0.4),
            Ring::Dim => theme.lcd,
        };
        let (size, small) = match ring {
            Ring::Major => (11.5_f32, 7.0_f32),
            Ring::Minor => (10.0, 7.0),
            Ring::Dim => (8.5, 6.5),
        };
        for pos in 0..12u8 {
            let w = Wedge::new(ring, pos);
            let mid = w.angle();
            let half = 14.4_f64.to_radians();
            let diatonic = is_diatonic(key, w);
            let sug = marks.suggest.iter().find(|s| s.wedge == w);
            let mut face = if diatonic {
                mix(base, accent, 0.28)
            } else {
                base
            };
            if let Some(s) = sug {
                face = match s.strength {
                    Strength::Strong => mix(base, accent, 0.62),
                    Strength::Colour => mix(base, theme.highlight, 0.3),
                    Strength::Surprise => mix(base, theme.text_muted, 0.3),
                };
            }
            let playing = marks.playing == Some(w);
            if playing {
                face = mix(base, accent, 0.9);
            }
            let shape = segment(panel.centre, r0, r1, mid - half, mid + half);
            c.fill_with(shape.clone(), face);
            let selected = marks.selected == w;
            c.stroke_with(
                shape,
                if selected { 2.2 } else { 0.8 },
                if selected {
                    accent
                } else {
                    theme.ink.with_alpha(0.16)
                },
            );
            // The names.
            let centre = panel.wedge_centre(w);
            let ink = if playing || sug.is_some() {
                on(theme, face)
            } else if diatonic {
                theme.text
            } else {
                theme.text.with_alpha(0.7)
            };
            let name = wedge_label(w);
            let numeral = diatonic.then(|| roman(key, w.chord()));
            let dy = if numeral.is_some() {
                -1.5
            } else {
                0.35 * f64::from(size)
            };
            c.text(
                &TextRun::verbatim(name, pt(centre.x, centre.y + dy), size, ink)
                    .align(Align::Center)
                    .weight(if diatonic { 700 } else { 500 }),
            );
            if let Some(n) = numeral {
                c.text(
                    &TextRun::verbatim(
                        n,
                        pt(centre.x, centre.y + 7.5),
                        small,
                        ink.with_alpha(0.85),
                    )
                    .align(Align::Center)
                    .weight(600),
                );
            }
            // Tags of the slots on the wedge, and the suggestion badge.
            let radius = r1 - 6.5;
            let step = (12.0 / radius).max(0.14);
            let list = tags(w);
            for (k, n) in list.iter().enumerate() {
                let a = mid - 0.19 - step * k as f64;
                let p = panel.polar(radius, a);
                c.fill_with(vello::kurbo::Circle::new(p, 5.0), accent);
                c.text(
                    &TextRun::verbatim(
                        (n + 1).to_string(),
                        pt(p.x, p.y + 2.6),
                        7.0,
                        on(theme, accent),
                    )
                    .align(Align::Center)
                    .weight(700),
                );
            }
            if let Some(s) = sug.filter(|s| s.rank > 0) {
                let p = panel.polar(radius, mid + 0.19);
                c.fill_with(vello::kurbo::Circle::new(p, 5.2), theme.text);
                c.text(
                    &TextRun::verbatim(
                        s.rank.to_string(),
                        pt(p.x, p.y + 2.7),
                        7.5,
                        theme.panel_bottom,
                    )
                    .align(Align::Center)
                    .weight(700),
                );
            }
        }
    }
    // The outline of the selected slot's wedge goes last, so its neighbours do not cut it.
    let sel = marks.selected;
    let (r0, r1) = sel.ring.radii();
    let half = 14.4_f64.to_radians();
    c.stroke_with(
        segment(panel.centre, r0, r1, sel.angle() - half, sel.angle() + half),
        2.2,
        accent,
    );
    // The hub: the home key.
    let hub = vello::kurbo::Circle::new(panel.centre, HUB_R);
    c.fill_with(hub, theme.lcd);
    c.stroke_with(
        vello::kurbo::Circle::new(panel.centre, HUB_R),
        1.0,
        theme.ink.with_alpha(0.16),
    );
    let name = symbol_in(key, Chord::new(key.root, ChordKind::Major));
    c.text(
        &TextRun::verbatim(name, pt(panel.centre.x, panel.centre.y + 3.0), 18.0, accent)
            .align(Align::Center)
            .weight(700),
    );
    let mode = match key.mode {
        KeyMode::Major => "MAJOR",
        KeyMode::Minor => "MINOR",
    };
    c.text(
        &TextRun::verbatim(
            mode,
            pt(panel.centre.x, panel.centre.y + 14.5),
            7.5,
            theme.text_muted,
        )
        .align(Align::Center)
        .weight(600)
        .spacing(0.8),
    );
    c.text(
        &TextRun::verbatim(
            "KEY",
            pt(panel.centre.x, panel.centre.y - 12.0),
            7.0,
            theme.text_muted,
        )
        .align(Align::Center)
        .weight(600)
        .spacing(1.0),
    );
}

/// A padlock in `r`: shut and lit when `locked`, open and dim otherwise.
fn paint_lock<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    locked: bool,
    accent: Color,
    alpha: f32,
) {
    let (cx, cy) = (r.center().x, r.center().y + 1.5);
    let col = if locked {
        accent
    } else {
        theme.text_muted.with_alpha(0.75 * alpha)
    };
    let body = Rect::new(cx - 4.5, cy - 0.5, cx + 4.5, cy + 5.0);
    let mut shackle = BezPath::new();
    shackle.move_to(pt(cx - 3.0, cy - 0.5));
    shackle.line_to(pt(cx - 3.0, cy - 2.8));
    shackle.curve_to(
        pt(cx - 3.0, cy - 7.0),
        pt(cx + 3.0, cy - 7.0),
        pt(cx + 3.0, cy - 2.8),
    );
    shackle.line_to(pt(cx + 3.0, if locked { cy - 0.5 } else { cy - 3.2 }));
    c.stroke_with(shackle, 1.3, col);
    if locked {
        c.fill_with(RoundedRect::from_rect(body, 1.2), col);
    } else {
        c.stroke_with(RoundedRect::from_rect(body.inset(-0.5), 1.2), 1.2, col);
    }
}

fn paint_cards<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &ChordWheelPanel,
    state: &WheelState,
    selected: usize,
    playing: Option<usize>,
    accent: Color,
) {
    for n in 0..SLOTS {
        let r = panel.cards[n];
        let active = n < state.slots;
        let alpha = if active { 1.0 } else { 0.38 };
        let chord = state.chords[n];
        let lit = playing == Some(n) && active;
        let sel = selected == n && active;
        let face = if lit {
            mix(theme.button, accent, 0.5)
        } else if sel {
            mix(theme.button, accent, 0.22)
        } else {
            theme.button
        };
        c.fill_with(RoundedRect::from_rect(r, 4.0), face.with_alpha(alpha));
        c.stroke_with(
            RoundedRect::from_rect(r.inset(-0.5), 4.5),
            if sel { 2.0 } else { 1.0 },
            if sel {
                accent
            } else {
                theme.bevel_dark.with_alpha(alpha)
            },
        );
        let ink = if lit { on(theme, face) } else { theme.text };
        let muted = if lit {
            ink.with_alpha(0.8)
        } else {
            theme.text_muted.with_alpha(alpha)
        };
        c.text(
            &TextRun::verbatim((n + 1).to_string(), pt(r.x0 + 6.0, r.y0 + 11.0), 8.0, muted)
                .weight(700),
        );
        paint_lock(
            c,
            theme,
            panel.locks[n],
            state.locks[n] && active,
            accent,
            alpha,
        );
        let symbol = symbol_in(state.key, chord);
        let size = match symbol.chars().count() {
            0..=5 => 14.0,
            6..=7 => 12.0,
            _ => 10.5,
        };
        c.text(
            &TextRun::verbatim(
                fit_label_raw(&symbol, r.width() - 8.0, size),
                pt(r.center().x, r.y0 + 29.0),
                size,
                ink.with_alpha(alpha.max(0.55)),
            )
            .align(Align::Center)
            .weight(700),
        );
        let numeral = roman(state.key, chord);
        c.text(
            &TextRun::verbatim(
                fit_label_raw(&numeral, 36.0, 8.5),
                pt(r.x0 + 6.0, r.y1 - 5.0),
                8.5,
                if lit { ink } else { accent.with_alpha(alpha) },
            )
            .weight(600),
        );
        let b = panel.beats[n];
        c.fill_with(
            RoundedRect::from_rect(b, 3.0),
            theme.lcd.with_alpha(0.85 * alpha),
        );
        c.text(
            &TextRun::verbatim(
                format!("{} BEATS", state.beats[n]),
                pt(b.center().x, b.center().y + 2.8),
                7.5,
                theme.text.with_alpha(alpha.max(0.55)),
            )
            .align(Align::Center)
            .weight(600),
        );
    }
}

/// Draws the panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &ChordWheelPanel,
    device: &RackDevice,
    desc: &DeviceDescriptor,
    info: &PlayerInfo<'_>,
    accent: Color,
) {
    let state = wheel_state(device, desc);
    let selected = selected_slot(info.edit, state.slots);
    let playing = playing_slot(info.meters).filter(|n| *n < state.slots);
    let suggest_on = info.fx_mode != 0;
    let suggest = if suggest_on {
        let prev = state.chords[previous_slot(selected, state.slots)];
        suggestions(state.key, state.mood, prev, state.spice)
    } else {
        Vec::new()
    };
    let marks = Marks {
        selected: Wedge::of(state.chords[selected]),
        playing: playing.map(|n| Wedge::of(state.chords[n])),
        suggest,
    };
    paint_wheel(c, theme, panel, &state, &marks, accent);
    paint_cards(c, theme, panel, &state, selected, playing, accent);
    // The slot count.
    crate::paint::edit_button(c, theme, panel.slots_dec, "-", state.slots > 1);
    crate::paint::edit_button(c, theme, panel.slots_inc, "+", state.slots < SLOTS);
    c.fill_with(RoundedRect::from_rect(panel.slots_text, 3.0), theme.lcd);
    c.text(
        &TextRun::verbatim(
            format!("{} SLOTS", state.slots),
            pt(
                panel.slots_text.center().x,
                panel.slots_text.center().y + 3.0,
            ),
            8.0,
            theme.text,
        )
        .align(Align::Center)
        .weight(600),
    );
    // The buttons.
    button_sized(c, theme, panel.generate, "GENERATE", true, 9.0, 1.0);
    button_sized(c, theme, panel.suggest, "SUGGEST", true, 9.0, 1.0);
    if suggest_on {
        c.stroke_with(
            RoundedRect::from_rect(panel.suggest.inset(-0.5), 3.5),
            1.5,
            accent,
        );
        let led = pt(panel.suggest.x0 + 9.0, panel.suggest.center().y);
        c.fill_with(vello::kurbo::Circle::new(led, 2.6), accent);
    }
    button_sized(c, theme, panel.to_lane, "TO LANE", true, 8.5, 0.8);
    button_sized(c, theme, panel.to_clip, "TO CLIP", true, 8.5, 0.8);
    paint_readout(c, theme, panel, &state, selected, accent);
}

fn paint_readout<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &ChordWheelPanel,
    state: &WheelState,
    selected: usize,
    accent: Color,
) {
    let r = panel.readout;
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        theme.ink.with_alpha(0.07),
    );
    let chord = state.chords[selected];
    let mid = r.center().y + 3.5;
    c.text(
        &TextRun::verbatim(
            format!("SLOT {}", selected + 1),
            pt(r.x0 + 7.0, mid),
            8.0,
            accent,
        )
        .weight(700)
        .spacing(0.6),
    );
    c.text(
        &TextRun::verbatim(
            symbol_in(state.key, chord),
            pt(r.x0 + 62.0, mid + 0.5),
            11.0,
            theme.text,
        )
        .weight(700),
    );
    c.text(
        &TextRun::verbatim(
            format!(
                "{}   {} BEATS",
                roman(state.key, chord),
                state.beats[selected]
            ),
            pt(r.x1 - 7.0, mid),
            8.5,
            theme.text_muted,
        )
        .align(Align::End),
    );
    let h = panel.hint;
    for (i, line) in [
        "Click a wedge to set the slot, right-click for quality and bass",
        "Shift-click a wedge to make it the key",
    ]
    .into_iter()
    .enumerate()
    {
        c.text(&TextRun::new(
            line,
            pt(h.x0 + 2.0, h.y0 + 8.0 + i as f64 * 11.0),
            7.5,
            theme.text_muted,
        ));
    }
}
