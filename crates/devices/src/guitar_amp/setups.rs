//! The factory setups of the guitar amp: a value for every param but `setup` and `bypass`.
//!
//! A setup is only a starting point: choosing one in the app sets all of these params in one
//! undo step (`atomik-app` expands a `setup` change into a batch of param changes) and the
//! player tweaks from there. The device itself ignores the `setup` param, it only remembers
//! which one was chosen last. The values are this design's own; the names describe the sound.
//!
//! | Setup | Idea |
//! |---|---|
//! | Clean | open, full and sparkling at the edge of breakup; a 2x12, nothing else |
//! | Crunch | a driven rhythm sound: mid-forward, a touch of sag, a small room |
//! | Industrial Rhythm | very high gain, tight and percussive for drop-tuned guitars: the high-pass and mid boost keep the low end from smearing, a scooped but present mid, strong presence, a closed 4x12 and a noise gate |
//! | 80s Lead | hot, singing sustain: mid-forward, sagging, compressed, with a 380 ms ping-pong delay and a plate-like reverb |
//! | Synthwave | clean to light crunch, bright and wide: chorus, a dotted-eighth delay at the song's tempo and a big reverb |

/// The number of params a setup sets (every param of the device but `setup` and `bypass`).
pub const SETUP_PARAMS: usize = 29;

/// A factory setup.
#[derive(Debug, Clone, Copy)]
pub struct Setup {
    /// Name shown in the setup selector.
    pub name: &'static str,
    /// `(param key, plain value)` for every param, in the device's param order.
    pub values: [(&'static str, f32); SETUP_PARAMS],
}

/// The names of the setups, in selector order.
pub const SETUP_NAMES: [&str; 5] = [
    "Clean",
    "Crunch",
    "Industrial Rhythm",
    "80s Lead",
    "Synthwave",
];

/// The cabinet choices (`cab` plain values).
const OPEN_1X12: f32 = 0.0;
const COMBO_2X12: f32 = 1.0;
const CLOSED_4X12: f32 = 2.0;

/// `delay.sync` choice index of the dotted eighth note.
const DOTTED_EIGHTH: f32 = 7.0;

/// Builds a setup from its values, in the device's param order.
const fn setup(name: &'static str, v: [f32; SETUP_PARAMS]) -> Setup {
    const KEYS: [&str; SETUP_PARAMS] = [
        "input",
        "gate",
        "tight",
        "boost",
        "gain",
        "bass",
        "mid",
        "treble",
        "presence",
        "depth",
        "sag",
        "master",
        "level",
        "cab",
        "mic",
        "chorus.on",
        "chorus.mix",
        "chorus.rate",
        "chorus.depth",
        "delay.on",
        "delay.time",
        "delay.sync",
        "delay.feedback",
        "delay.tone",
        "delay.mix",
        "reverb.on",
        "reverb.size",
        "reverb.decay",
        "reverb.mix",
    ];
    let mut values = [("", 0.0); SETUP_PARAMS];
    let mut i = 0;
    while i < SETUP_PARAMS {
        values[i] = (KEYS[i], v[i]);
        i += 1;
    }
    Setup { name, values }
}

/// The setups in selector order; index 0 equals the descriptor's defaults.
///
/// Each row: input, gate, tight, boost | gain, bass, mid, treble, presence, depth | sag, master,
/// level | cab, mic | chorus on, mix, rate, depth | delay on, time, sync, feedback, tone, mix |
/// reverb on, size, decay, mix. The levels make the setups about as loud as each other for the same
/// guitar signal.
#[rustfmt::skip]
pub static SETUPS: [Setup; 5] = [
    setup("Clean", [
        0.0, -80.0, 20.0, 0.0,   2.5, 5.0, 5.5, 5.5, 4.0, 3.0,   3.0, 6.0, -1.5,   COMBO_2X12, 0.35,
        0.0, 0.4, 0.8, 0.4,   0.0, 380.0, 0.0, 0.3, 5_000.0, 0.25,   0.0, 0.4, 1.2, 0.2,
    ]),
    setup("Crunch", [
        0.0, -80.0, 80.0, 3.0,   6.5, 4.5, 6.5, 6.0, 5.5, 4.0,   5.0, 7.0, -8.0,   COMBO_2X12, 0.3,
        0.0, 0.4, 0.8, 0.4,   0.0, 380.0, 0.0, 0.3, 5_000.0, 0.25,   1.0, 0.3, 1.0, 0.12,
    ]),
    setup("Industrial Rhythm", [
        0.0, -52.0, 150.0, 9.0,   9.0, 6.5, 3.5, 6.5, 7.0, 6.0,   1.5, 7.0, -10.0,   CLOSED_4X12, 0.25,
        0.0, 0.4, 0.8, 0.4,   0.0, 380.0, 0.0, 0.3, 5_000.0, 0.25,   0.0, 0.3, 1.0, 0.1,
    ]),
    setup("80s Lead", [
        0.0, -56.0, 90.0, 6.0,   8.0, 5.5, 7.5, 6.0, 5.5, 4.0,   6.0, 6.0, -4.5,   CLOSED_4X12, 0.35,
        0.0, 0.4, 0.8, 0.4,   1.0, 380.0, 0.0, 0.38, 4_200.0, 0.28,   1.0, 0.45, 1.8, 0.22,
    ]),
    setup("Synthwave", [
        0.0, -80.0, 40.0, 0.0,   3.5, 4.5, 5.0, 7.0, 6.0, 2.5,   3.5, 6.0, -2.5,   OPEN_1X12, 0.1,
        1.0, 0.5, 0.7, 0.7,   1.0, 380.0, DOTTED_EIGHTH, 0.45, 6_000.0, 0.3,   1.0, 0.8, 4.5, 0.35,
    ]),
];

/// The setup with selector index `i` (clamped).
pub fn setup_at(i: usize) -> &'static Setup {
    &SETUPS[i.min(SETUPS.len() - 1)]
}
