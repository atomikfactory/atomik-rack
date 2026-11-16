//! The mood tables of the progression generator: for every mood and key mode a vocabulary of
//! chords with affinities, the idiomatic moves of the mood, how it starts and cadences, a few
//! signature progressions and how long its chords last.
//!
//! Degrees are semitones above the key's tonic (`0` = I, `7` = V, `10` = bVII ...), whatever the
//! mode: a minor key names its chords against the major scale of the tonic (so the natural minor
//! chords are i, ii, bIII, iv, v, bVI and bVII, as in pop harmony).
//!
//! The weight of a move is `function x root motion x idiom x affinity x spice` (see
//! `progression::trans`); the tables here only carry what is particular to a mood.
//!
//! Moods (character, in a line each):
//! * **Pop**: radio loops of the four chords I V vi IV in every order, triads, the odd borrowed bVII.
//! * **Uplifting**: bright and forward, IV and V everywhere, add9 shimmer, bVII lifts to IV.
//! * **Melancholy**: starts on vi, descends (IV, iii, ii), borrows the minor iv, ends open.
//! * **Dark**: minor-side harmony, Andalusian and phrygian falls, V pulling into i, bII and vii.
//! * **Dreamy**: major sevenths and sus colours, plagal float, lydian II, nothing resolves hard.
//! * **Jazz**: ii-V-I cells and turnarounds on sevenths, secondary dominants, tritone subs.
//! * **Neo Soul**: lush ninths and thirteenths, IV maj9 - iii - vi falls, bVII9 and chromatic slides.
//! * **Lo-fi**: mellow diatonic sevenths (I vi ii V), the iv7 borrow, loops that never quite land.
//! * **Cinematic**: slow, wide, borrowed bVI and bVII and chromatic mediants, long chords.
//! * **Anthem**: big open triads, I IV V and the stadium bVII, V as the turnaround.
//! * **Blues**: dominant sevenths on I IV V, twelve-bar and eight-bar shapes, quick IV, turnarounds.
//! * **Modal**: no leading tone: mixolydian I bVII IV, lydian II, dorian i - IV7, phrygian bII.

use crate::theory::ChordKind as K;

/// How a chord counts against the spice setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Col {
    /// Always available.
    Core,
    /// Borrowed from the parallel mode.
    Borrowed,
    /// A secondary dominant that wants to resolve to the degree.
    Secondary(u8),
    /// A tritone substitute that wants to resolve to the degree.
    Tritone(u8),
    /// A chromatic mediant.
    Mediant,
    /// A sus, add or modal colour.
    Colour,
}

/// A chord of a mood's vocabulary.
#[derive(Debug, Clone, Copy)]
pub(super) struct Node {
    /// Semitones above the tonic.
    pub d: u8,
    pub k: K,
    /// Affinity, 1..=10.
    pub w: u8,
    pub c: Col,
}

/// A signature progression: steps are `(degree, kind, beats)`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Template {
    pub w: u8,
    /// The least spice (permille) it appears at; 0 = always.
    pub min_spice: u16,
    pub steps: &'static [(u8, K, u8)],
}

/// How a mood lays out the chord lengths.
#[derive(Debug, Clone, Copy)]
pub(super) struct Len {
    /// A predominant followed by a dominant share one bar (2 + 2 beats).
    pub split_pair: bool,
    /// Chance (percent) of 8 beats for the last chord.
    pub long_last: u8,
    /// Chance (percent) of 8 beats for any chord of a progression of four or fewer.
    pub long_any: u8,
}

/// Everything the generator knows about one mood in one mode.
#[derive(Debug)]
pub(super) struct Profile {
    pub nodes: &'static [Node],
    /// `(from, to, percent)` multipliers of idiomatic moves, by degree.
    pub idioms: &'static [(u8, u8, u16)],
    /// Percent multipliers of the first chord by degree (the tonic defaults to 500, the rest 30).
    pub start: &'static [(u8, u16)],
    /// The same for the last chord (unlisted degrees 60).
    pub end: &'static [(u8, u16)],
    /// The same for the fourth chord of eight (unlisted degrees 70).
    pub half: &'static [(u8, u16)],
    pub templates: &'static [Template],
    /// Percent kept for every earlier use of the same chord.
    pub diversity: u16,
    /// Percent chance that eight chords are a phrase of four played twice with a new ending.
    pub repeat: u8,
    /// Percent chance of a signature progression when one fits the length.
    pub tmpl: u8,
    pub len: Len,
}

const MAJ: K = K::Major;
const MIN: K = K::Minor;
const DIM: K = K::Dim;
const SUS2: K = K::Sus2;
const SUS4: K = K::Sus4;
const D7: K = K::Seventh;
const M7: K = K::MajorSeventh;
const N7: K = K::MinorSeventh;
const HD: K = K::HalfDim;
const O7: K = K::DimSeventh;
const D9: K = K::Ninth;
const M9: K = K::MajorNinth;
const N9: K = K::MinorNinth;
const A9: K = K::Add9;
const D13: K = K::Thirteenth;

const CORE: Col = Col::Core;
const BOR: Col = Col::Borrowed;
const MED: Col = Col::Mediant;
const COL: Col = Col::Colour;
const fn sec(t: u8) -> Col {
    Col::Secondary(t)
}
const fn tri(t: u8) -> Col {
    Col::Tritone(t)
}
const fn nd(d: u8, k: K, w: u8, c: Col) -> Node {
    Node { d, k, w, c }
}
const fn tp(w: u8, min_spice: u16, steps: &'static [(u8, K, u8)]) -> Template {
    Template {
        w,
        min_spice,
        steps,
    }
}
const PLAIN: Len = Len {
    split_pair: false,
    long_last: 0,
    long_any: 0,
};

// ---------------------------------------------------------------- Pop

static POP_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, MAJ, 10, CORE),
        nd(2, MIN, 4, CORE),
        nd(4, MIN, 3, CORE),
        nd(5, MAJ, 10, CORE),
        nd(7, MAJ, 10, CORE),
        nd(9, MIN, 10, CORE),
        nd(10, MAJ, 5, BOR),
        nd(5, MIN, 4, BOR),
        nd(8, MAJ, 3, BOR),
        nd(4, MAJ, 5, sec(9)),
        nd(2, MAJ, 4, sec(7)),
        nd(9, MAJ, 3, sec(2)),
        nd(7, SUS4, 3, COL),
        nd(5, A9, 3, COL),
    ],
    idioms: &[
        (9, 5, 250),
        (0, 7, 160),
        (7, 9, 110),
        (0, 9, 150),
        (5, 7, 110),
        (7, 7, 300),
    ],
    start: &[(9, 250)],
    end: &[(7, 300), (5, 150), (9, 100), (0, 100)],
    half: &[(7, 300), (5, 120), (9, 100)],
    templates: &[
        tp(10, 0, &[(0, MAJ, 4), (7, MAJ, 4), (9, MIN, 4), (5, MAJ, 4)]),
        tp(8, 0, &[(9, MIN, 4), (5, MAJ, 4), (0, MAJ, 4), (7, MAJ, 4)]),
        tp(6, 0, &[(0, MAJ, 4), (9, MIN, 4), (5, MAJ, 4), (7, MAJ, 4)]),
        tp(5, 0, &[(0, MAJ, 4), (5, MAJ, 4), (9, MIN, 4), (7, MAJ, 4)]),
        tp(3, 0, &[(0, MAJ, 4), (4, MIN, 4), (5, MAJ, 4), (7, MAJ, 4)]),
    ],
    diversity: 55,
    repeat: 55,
    tmpl: 50,
    len: PLAIN,
};

static POP_MINOR: Profile = Profile {
    nodes: &[
        nd(0, MIN, 10, CORE),
        nd(3, MAJ, 8, CORE),
        nd(5, MIN, 7, CORE),
        nd(7, MAJ, 5, CORE),
        nd(8, MAJ, 10, CORE),
        nd(10, MAJ, 9, CORE),
        nd(7, MIN, 2, CORE),
        nd(5, MAJ, 4, BOR),
        nd(1, MAJ, 2, BOR),
        nd(10, D7, 2, sec(3)),
        nd(3, SUS2, 2, COL),
    ],
    idioms: &[
        (8, 10, 160),
        (10, 0, 130),
        (0, 8, 150),
        (3, 10, 150),
        (8, 3, 150),
        (5, 7, 140),
        (0, 3, 120),
    ],
    start: &[],
    end: &[(10, 200), (7, 250), (8, 100), (3, 100)],
    half: &[(7, 250), (10, 150), (3, 100)],
    templates: &[
        tp(
            10,
            0,
            &[(0, MIN, 4), (8, MAJ, 4), (3, MAJ, 4), (10, MAJ, 4)],
        ),
        tp(6, 0, &[(0, MIN, 4), (3, MAJ, 4), (10, MAJ, 4), (8, MAJ, 4)]),
        tp(6, 0, &[(0, MIN, 4), (5, MIN, 4), (8, MAJ, 4), (7, MAJ, 4)]),
        tp(5, 0, &[(0, MIN, 4), (10, MAJ, 4), (8, MAJ, 4), (7, MAJ, 4)]),
    ],
    diversity: 55,
    repeat: 55,
    tmpl: 50,
    len: PLAIN,
};

// ---------------------------------------------------------------- Uplifting

static UPLIFTING_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, MAJ, 8, CORE),
        nd(0, A9, 3, CORE),
        nd(2, MIN, 5, CORE),
        nd(4, MIN, 3, CORE),
        nd(5, MAJ, 10, CORE),
        nd(5, A9, 3, CORE),
        nd(7, MAJ, 10, CORE),
        nd(9, MIN, 8, CORE),
        nd(10, MAJ, 6, BOR),
        nd(5, MIN, 3, BOR),
        nd(2, MAJ, 4, sec(7)),
        nd(4, MAJ, 3, sec(9)),
        nd(7, SUS4, 3, COL),
    ],
    idioms: &[
        (5, 7, 150),
        (9, 5, 160),
        (0, 9, 110),
        (5, 0, 110),
        (10, 5, 200),
        (10, 0, 200),
        (7, 7, 300),
        (0, 0, 300),
    ],
    start: &[(5, 150)],
    end: &[(7, 300), (5, 150), (9, 100), (0, 100)],
    half: &[(7, 300), (5, 150), (9, 100)],
    templates: &[
        tp(6, 0, &[(0, MAJ, 4), (7, MAJ, 4), (9, MIN, 4), (5, MAJ, 4)]),
        tp(6, 0, &[(0, MAJ, 4), (5, MAJ, 4), (9, MIN, 4), (7, MAJ, 4)]),
        tp(4, 0, &[(9, MIN, 4), (5, MAJ, 4), (0, MAJ, 4), (7, MAJ, 4)]),
        tp(5, 0, &[(5, MAJ, 4), (0, MAJ, 4), (7, MAJ, 4), (9, MIN, 4)]),
        tp(3, 0, &[(0, MAJ, 4), (4, MIN, 4), (5, MAJ, 4), (7, MAJ, 4)]),
        tp(
            4,
            400,
            &[(0, MAJ, 4), (10, MAJ, 4), (5, MAJ, 4), (7, MAJ, 4)],
        ),
    ],
    diversity: 55,
    repeat: 55,
    tmpl: 50,
    len: PLAIN,
};

static UPLIFTING_MINOR: Profile = Profile {
    nodes: &[
        nd(0, MIN, 8, CORE),
        nd(3, MAJ, 8, CORE),
        nd(5, MAJ, 8, CORE),
        nd(8, MAJ, 6, CORE),
        nd(10, MAJ, 9, CORE),
        nd(7, MAJ, 4, CORE),
        nd(7, MIN, 3, CORE),
        nd(5, MIN, 3, BOR),
        nd(7, SUS4, 3, COL),
    ],
    idioms: &[
        (3, 10, 150),
        (10, 5, 200),
        (5, 0, 150),
        (0, 3, 140),
        (8, 10, 140),
        (10, 3, 120),
    ],
    start: &[],
    end: &[(7, 250), (5, 200), (10, 150), (8, 100)],
    half: &[(7, 250), (5, 150), (10, 150)],
    templates: &[
        tp(8, 0, &[(0, MIN, 4), (3, MAJ, 4), (10, MAJ, 4), (5, MAJ, 4)]),
        tp(6, 0, &[(0, MIN, 4), (8, MAJ, 4), (3, MAJ, 4), (10, MAJ, 4)]),
        tp(
            4,
            0,
            &[(0, MIN, 4), (10, MAJ, 4), (8, MAJ, 4), (10, MAJ, 4)],
        ),
    ],
    diversity: 55,
    repeat: 55,
    tmpl: 50,
    len: PLAIN,
};

// ---------------------------------------------------------------- Melancholy

static MELANCHOLY_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, MAJ, 8, CORE),
        nd(2, MIN, 6, CORE),
        nd(4, MIN, 7, CORE),
        nd(5, MAJ, 9, CORE),
        nd(7, MAJ, 7, CORE),
        nd(9, MIN, 10, CORE),
        nd(5, MIN, 6, BOR),
        nd(8, MAJ, 3, BOR),
        nd(4, MAJ, 5, sec(9)),
        nd(9, MAJ, 3, sec(2)),
        nd(0, A9, 3, COL),
    ],
    idioms: &[
        (9, 5, 200),
        (4, 9, 180),
        (5, 4, 150),
        (4, 2, 130),
        (5, 5, 500),
        (2, 0, 120),
        (0, 4, 130),
    ],
    start: &[(9, 250)],
    end: &[(7, 200), (5, 150), (0, 150), (9, 100)],
    half: &[(7, 250), (5, 150), (4, 100)],
    templates: &[
        tp(10, 0, &[(9, MIN, 4), (5, MAJ, 4), (0, MAJ, 4), (7, MAJ, 4)]),
        tp(8, 0, &[(0, MAJ, 4), (4, MIN, 4), (9, MIN, 4), (5, MAJ, 4)]),
        tp(5, 0, &[(9, MIN, 4), (7, MAJ, 4), (5, MAJ, 4), (7, MAJ, 4)]),
        tp(5, 0, &[(0, MAJ, 4), (9, MIN, 4), (4, MIN, 4), (5, MAJ, 4)]),
        tp(
            4,
            300,
            &[(5, MAJ, 4), (5, MIN, 4), (0, MAJ, 4), (7, MAJ, 4)],
        ),
        // Canon: the eight-chord ground bass.
        tp(
            8,
            0,
            &[
                (0, MAJ, 4),
                (7, MAJ, 4),
                (9, MIN, 4),
                (4, MIN, 4),
                (5, MAJ, 4),
                (0, MAJ, 4),
                (5, MAJ, 4),
                (7, MAJ, 4),
            ],
        ),
    ],
    diversity: 55,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 25,
        long_any: 0,
    },
};

static MELANCHOLY_MINOR: Profile = Profile {
    nodes: &[
        nd(0, MIN, 10, CORE),
        nd(2, DIM, 3, CORE),
        nd(3, MAJ, 8, CORE),
        nd(5, MIN, 8, CORE),
        nd(7, MAJ, 5, CORE),
        nd(7, MIN, 4, CORE),
        nd(8, MAJ, 9, CORE),
        nd(10, MAJ, 7, CORE),
        nd(5, MAJ, 3, BOR),
        nd(1, MAJ, 2, MED),
        nd(11, O7, 3, sec(0)),
    ],
    idioms: &[
        (0, 5, 140),
        (5, 8, 140),
        (8, 7, 200),
        (10, 8, 130),
        (8, 3, 130),
        (3, 10, 130),
        (7, 0, 130),
    ],
    start: &[],
    end: &[(7, 200), (10, 130), (3, 100), (8, 100)],
    half: &[(7, 250), (10, 150), (3, 100)],
    templates: &[
        tp(
            10,
            0,
            &[(0, MIN, 4), (8, MAJ, 4), (3, MAJ, 4), (10, MAJ, 4)],
        ),
        tp(8, 0, &[(0, MIN, 4), (5, MIN, 4), (8, MAJ, 4), (7, MAJ, 4)]),
        tp(7, 0, &[(0, MIN, 4), (10, MAJ, 4), (8, MAJ, 4), (7, MAJ, 4)]),
        tp(5, 0, &[(0, MIN, 4), (5, MIN, 4), (10, MAJ, 4), (3, MAJ, 4)]),
        tp(4, 0, &[(0, MIN, 4), (3, MAJ, 4), (5, MIN, 4), (8, MAJ, 4)]),
    ],
    diversity: 55,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 25,
        long_any: 0,
    },
};

// ---------------------------------------------------------------- Dark

static DARK_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, MAJ, 8, CORE),
        nd(5, MIN, 9, CORE),
        nd(8, MAJ, 8, CORE),
        nd(10, MAJ, 8, CORE),
        nd(3, MAJ, 5, CORE),
        nd(9, MIN, 5, CORE),
        nd(7, MAJ, 5, CORE),
        nd(1, MAJ, 3, BOR),
        nd(11, O7, 3, sec(0)),
        nd(4, MAJ, 3, sec(9)),
        nd(6, DIM, 2, COL),
    ],
    idioms: &[
        (0, 8, 170),
        (8, 10, 170),
        (10, 0, 120),
        (0, 5, 150),
        (5, 10, 180),
        (10, 7, 120),
        (5, 0, 120),
        (8, 7, 160),
    ],
    start: &[],
    end: &[(7, 300), (10, 150), (5, 120), (8, 100)],
    half: &[(7, 250), (10, 150), (5, 120)],
    templates: &[
        tp(6, 0, &[(0, MAJ, 4), (8, MAJ, 4), (10, MAJ, 4), (7, MAJ, 4)]),
        tp(6, 0, &[(0, MAJ, 4), (5, MIN, 4), (8, MAJ, 4), (10, MAJ, 4)]),
        tp(5, 0, &[(0, MAJ, 4), (10, MAJ, 4), (8, MAJ, 4), (7, MAJ, 4)]),
        tp(4, 0, &[(0, MAJ, 4), (5, MIN, 4), (10, MAJ, 4), (7, MAJ, 4)]),
    ],
    diversity: 55,
    repeat: 55,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 30,
        long_any: 0,
    },
};

static DARK_MINOR: Profile = Profile {
    nodes: &[
        nd(0, MIN, 10, CORE),
        nd(1, MAJ, 5, CORE),
        nd(3, MAJ, 5, CORE),
        nd(5, MIN, 8, CORE),
        nd(7, MAJ, 8, CORE),
        nd(7, MIN, 3, CORE),
        nd(8, MAJ, 9, CORE),
        nd(10, MAJ, 4, CORE),
        nd(2, DIM, 3, CORE),
        nd(11, DIM, 4, CORE),
        nd(1, D7, 3, tri(0)),
        nd(11, O7, 4, sec(0)),
        nd(4, MAJ, 2, MED),
    ],
    idioms: &[
        (0, 1, 220),
        (1, 0, 200),
        (0, 8, 160),
        (8, 7, 200),
        (5, 7, 200),
        (0, 5, 150),
        (10, 8, 150),
        (7, 0, 150),
        (11, 0, 150),
    ],
    start: &[],
    end: &[(7, 400), (1, 200), (10, 100), (11, 150)],
    half: &[(7, 300), (10, 120), (1, 120)],
    templates: &[
        // Andalusian fall.
        tp(8, 0, &[(0, MIN, 4), (10, MAJ, 4), (8, MAJ, 4), (7, MAJ, 4)]),
        tp(7, 0, &[(0, MIN, 4), (8, MAJ, 4), (5, MIN, 4), (7, MAJ, 4)]),
        tp(5, 0, &[(0, MIN, 4), (1, MAJ, 4), (8, MAJ, 4), (7, MAJ, 4)]),
        tp(5, 0, &[(0, MIN, 4), (8, MAJ, 4), (10, MAJ, 4), (7, MAJ, 4)]),
        tp(4, 0, &[(0, MIN, 4), (5, MIN, 4), (10, MAJ, 4), (8, MAJ, 4)]),
    ],
    diversity: 55,
    repeat: 55,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 30,
        long_any: 0,
    },
};

// ---------------------------------------------------------------- Dreamy

static DREAMY_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, M7, 10, CORE),
        nd(0, A9, 4, CORE),
        nd(0, SUS2, 3, CORE),
        nd(5, M7, 9, CORE),
        nd(5, A9, 4, CORE),
        nd(4, N7, 6, CORE),
        nd(9, N7, 8, CORE),
        nd(2, N7, 5, CORE),
        nd(7, SUS4, 6, CORE),
        nd(2, MAJ, 4, COL),
        nd(10, MAJ, 4, BOR),
        nd(8, M7, 4, BOR),
        nd(5, MIN, 3, BOR),
    ],
    idioms: &[
        (0, 5, 150),
        (5, 0, 130),
        (4, 5, 170),
        (0, 4, 130),
        (9, 5, 160),
        (0, 0, 500),
        (5, 5, 400),
        (7, 0, 120),
    ],
    start: &[(5, 150)],
    end: &[(5, 200), (7, 200), (0, 150), (9, 150)],
    half: &[(5, 150), (7, 200), (4, 150)],
    templates: &[
        tp(8, 0, &[(0, M7, 4), (5, M7, 4), (4, N7, 4), (5, M7, 4)]),
        tp(7, 0, &[(0, M7, 4), (4, N7, 4), (5, M7, 4), (7, SUS4, 4)]),
        tp(7, 0, &[(0, M7, 4), (9, N7, 4), (5, M7, 4), (4, N7, 4)]),
        tp(5, 0, &[(0, A9, 4), (5, M7, 4), (9, N7, 4), (7, SUS4, 4)]),
        tp(4, 300, &[(0, M7, 4), (2, MAJ, 4), (5, M7, 4), (7, SUS4, 4)]),
    ],
    diversity: 60,
    repeat: 55,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 25,
        long_any: 30,
    },
};

static DREAMY_MINOR: Profile = Profile {
    nodes: &[
        nd(0, N7, 10, CORE),
        nd(0, N9, 5, CORE),
        nd(3, M7, 7, CORE),
        nd(5, N7, 7, CORE),
        nd(7, N7, 3, CORE),
        nd(8, M7, 9, CORE),
        nd(10, M7, 6, CORE),
        nd(10, MAJ, 6, CORE),
        nd(7, SUS4, 3, CORE),
        nd(5, MAJ, 4, BOR),
        nd(1, M7, 3, MED),
    ],
    idioms: &[
        (0, 8, 170),
        (8, 3, 160),
        (8, 10, 140),
        (5, 8, 150),
        (10, 0, 130),
        (3, 10, 130),
        (0, 0, 400),
        (3, 5, 130),
    ],
    start: &[],
    end: &[(10, 200), (7, 150), (3, 100), (8, 100)],
    half: &[(10, 150), (7, 150), (3, 100)],
    templates: &[
        tp(8, 0, &[(0, N7, 4), (8, M7, 4), (3, M7, 4), (10, M7, 4)]),
        tp(7, 0, &[(0, N9, 4), (5, N7, 4), (8, M7, 4), (10, MAJ, 4)]),
        tp(5, 0, &[(8, M7, 4), (10, M7, 4), (0, N7, 4), (3, M7, 4)]),
    ],
    diversity: 60,
    repeat: 55,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 25,
        long_any: 30,
    },
};

// ---------------------------------------------------------------- Jazz

static JAZZ_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, M7, 9, CORE),
        nd(0, K::Sixth, 3, CORE),
        nd(2, N7, 10, CORE),
        nd(4, N7, 6, CORE),
        nd(5, M7, 6, CORE),
        nd(7, D7, 10, CORE),
        nd(7, D13, 3, CORE),
        nd(9, N7, 8, CORE),
        nd(11, HD, 3, CORE),
        nd(9, D7, 7, sec(2)),
        nd(4, D7, 6, sec(9)),
        nd(2, D7, 6, sec(7)),
        nd(0, D7, 4, sec(5)),
        nd(1, D7, 5, tri(0)),
        nd(5, N7, 4, BOR),
        nd(10, D7, 4, BOR),
    ],
    idioms: &[
        (2, 7, 300),
        (7, 0, 200),
        (4, 9, 250),
        (9, 2, 200),
        (0, 9, 150),
        (0, 4, 130),
        (5, 2, 150),
        (11, 4, 200),
    ],
    start: &[(2, 300), (4, 150), (9, 150)],
    end: &[(7, 300), (0, 150), (2, 80)],
    half: &[(7, 250), (2, 120), (9, 100)],
    templates: &[
        tp(10, 0, &[(2, N7, 4), (7, D7, 4), (0, M7, 4), (9, N7, 4)]),
        tp(10, 0, &[(0, M7, 4), (9, N7, 4), (2, N7, 4), (7, D7, 4)]),
        tp(7, 200, &[(4, N7, 4), (9, D7, 4), (2, N7, 4), (7, D7, 4)]),
        tp(5, 0, &[(0, M7, 4), (5, M7, 4), (2, N7, 4), (7, D7, 4)]),
        tp(4, 300, &[(0, M7, 4), (4, N7, 4), (9, N7, 4), (2, N7, 4)]),
    ],
    diversity: 50,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: true,
        long_last: 0,
        long_any: 0,
    },
};

static JAZZ_MINOR: Profile = Profile {
    nodes: &[
        nd(0, N7, 10, CORE),
        nd(0, K::MinorSixth, 3, CORE),
        nd(2, HD, 10, CORE),
        nd(3, M7, 6, CORE),
        nd(5, N7, 8, CORE),
        nd(7, D7, 10, CORE),
        nd(7, N7, 2, CORE),
        nd(8, M7, 8, CORE),
        nd(10, D7, 4, CORE),
        nd(11, O7, 4, CORE),
        nd(1, D7, 5, tri(0)),
        nd(5, D7, 3, BOR),
        nd(3, D7, 3, sec(8)),
    ],
    idioms: &[
        (2, 7, 300),
        (7, 0, 200),
        (0, 5, 150),
        (5, 2, 130),
        (8, 2, 150),
        (3, 8, 140),
        (11, 0, 150),
    ],
    start: &[(2, 200)],
    end: &[(7, 300), (0, 130)],
    half: &[(7, 250), (2, 120)],
    templates: &[
        tp(10, 0, &[(2, HD, 4), (7, D7, 4), (0, N7, 4), (8, M7, 4)]),
        tp(7, 0, &[(0, N7, 4), (5, N7, 4), (2, HD, 4), (7, D7, 4)]),
        tp(7, 0, &[(0, N7, 4), (8, M7, 4), (2, HD, 4), (7, D7, 4)]),
        tp(4, 0, &[(0, N7, 4), (3, M7, 4), (8, M7, 4), (7, D7, 4)]),
    ],
    diversity: 50,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: true,
        long_last: 0,
        long_any: 0,
    },
};

// ---------------------------------------------------------------- Neo Soul

static NEO_SOUL_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, M9, 9, CORE),
        nd(5, M9, 9, CORE),
        nd(4, N7, 6, CORE),
        nd(9, N9, 9, CORE),
        nd(2, N9, 9, CORE),
        nd(7, D13, 5, CORE),
        nd(7, D9, 6, CORE),
        nd(11, HD, 3, CORE),
        nd(10, D9, 4, CORE),
        nd(8, M7, 5, BOR),
        nd(5, N9, 5, BOR),
        nd(1, D9, 4, tri(0)),
        nd(9, D9, 4, sec(2)),
        nd(4, D7, 4, sec(9)),
        nd(3, M9, 3, MED),
    ],
    idioms: &[
        (5, 4, 220),
        (4, 9, 220),
        (9, 2, 200),
        (2, 7, 200),
        (10, 0, 200),
        (0, 5, 140),
        (5, 5, 400),
        (7, 4, 150),
    ],
    start: &[(5, 250), (2, 150)],
    end: &[(2, 200), (7, 150), (0, 200), (10, 150), (9, 100)],
    half: &[(7, 200), (2, 150), (4, 100)],
    templates: &[
        tp(8, 0, &[(5, M9, 4), (4, N7, 4), (9, N9, 4), (2, N9, 4)]),
        tp(8, 0, &[(0, M9, 4), (9, N9, 4), (2, N9, 4), (7, D13, 4)]),
        tp(6, 0, &[(5, M9, 4), (7, D9, 4), (4, N7, 4), (9, N9, 4)]),
        tp(6, 0, &[(2, N9, 4), (7, D13, 4), (0, M9, 4), (5, M9, 4)]),
        tp(4, 350, &[(0, M9, 4), (10, D9, 4), (5, M9, 4), (5, N9, 4)]),
    ],
    diversity: 55,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: true,
        long_last: 25,
        long_any: 0,
    },
};

static NEO_SOUL_MINOR: Profile = Profile {
    nodes: &[
        nd(0, N9, 9, CORE),
        nd(2, HD, 5, CORE),
        nd(3, M7, 6, CORE),
        nd(5, N9, 7, CORE),
        nd(7, D7, 5, CORE),
        nd(7, N7, 3, CORE),
        nd(8, M7, 8, CORE),
        nd(10, D9, 7, CORE),
        nd(1, M7, 3, MED),
        nd(5, D9, 4, BOR),
        nd(1, D9, 4, tri(0)),
    ],
    idioms: &[
        (0, 5, 160),
        (5, 10, 200),
        (10, 3, 180),
        (3, 8, 150),
        (8, 7, 180),
        (0, 8, 150),
        (7, 0, 130),
    ],
    start: &[],
    end: &[(7, 200), (10, 200), (0, 150), (3, 100)],
    half: &[(7, 200), (10, 150)],
    templates: &[
        tp(8, 0, &[(0, N9, 4), (5, N9, 4), (10, D9, 4), (3, M7, 4)]),
        tp(8, 0, &[(0, N9, 4), (8, M7, 4), (3, M7, 4), (10, D9, 4)]),
        tp(5, 0, &[(2, HD, 4), (7, D7, 4), (0, N9, 4), (8, M7, 4)]),
    ],
    diversity: 55,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: true,
        long_last: 25,
        long_any: 0,
    },
};

// ---------------------------------------------------------------- Lo-fi

static LOFI_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, M7, 10, CORE),
        nd(9, N7, 10, CORE),
        nd(2, N7, 8, CORE),
        nd(7, D7, 7, CORE),
        nd(7, D9, 3, CORE),
        nd(5, M7, 9, CORE),
        nd(4, N7, 7, CORE),
        nd(5, N7, 5, BOR),
        nd(8, M7, 4, BOR),
        nd(10, D7, 3, BOR),
        nd(9, D7, 4, sec(2)),
        nd(4, D7, 3, sec(9)),
        nd(1, D7, 3, tri(0)),
        nd(0, M9, 3, COL),
    ],
    idioms: &[
        (0, 9, 180),
        (9, 2, 200),
        (2, 7, 220),
        (5, 4, 170),
        (4, 9, 200),
        (5, 5, 500),
        (0, 5, 130),
        (7, 9, 120),
    ],
    start: &[(5, 200), (9, 150)],
    end: &[(7, 250), (2, 150), (9, 150), (4, 100), (0, 100)],
    half: &[(7, 200), (2, 150), (4, 100)],
    templates: &[
        tp(10, 0, &[(0, M7, 4), (9, N7, 4), (2, N7, 4), (7, D7, 4)]),
        tp(6, 0, &[(2, N7, 4), (7, D7, 4), (0, M7, 4), (9, N7, 4)]),
        tp(6, 0, &[(0, M7, 4), (5, M7, 4), (4, N7, 4), (9, N7, 4)]),
        tp(4, 0, &[(5, M7, 4), (4, N7, 4), (2, N7, 4), (0, M7, 4)]),
        tp(4, 300, &[(5, M7, 4), (5, N7, 4), (0, M7, 4), (7, D7, 4)]),
    ],
    diversity: 55,
    repeat: 50,
    tmpl: 50,
    len: Len {
        split_pair: true,
        long_last: 15,
        long_any: 0,
    },
};

static LOFI_MINOR: Profile = Profile {
    nodes: &[
        nd(0, N7, 10, CORE),
        nd(2, HD, 4, CORE),
        nd(3, M7, 7, CORE),
        nd(5, N7, 8, CORE),
        nd(7, N7, 5, CORE),
        nd(7, D7, 6, CORE),
        nd(8, M7, 9, CORE),
        nd(10, D7, 6, CORE),
        nd(10, M7, 4, CORE),
        nd(5, D7, 3, BOR),
        nd(1, M7, 3, MED),
    ],
    idioms: &[
        (0, 8, 170),
        (8, 3, 150),
        (5, 10, 160),
        (3, 10, 130),
        (10, 0, 130),
        (0, 5, 150),
        (8, 7, 110),
    ],
    start: &[],
    end: &[(7, 200), (10, 180), (3, 100), (8, 100)],
    half: &[(7, 200), (10, 150), (3, 100)],
    templates: &[
        tp(8, 0, &[(0, N7, 4), (8, M7, 4), (3, M7, 4), (10, D7, 4)]),
        tp(7, 0, &[(0, N7, 4), (5, N7, 4), (8, M7, 4), (7, D7, 4)]),
        tp(5, 0, &[(0, N7, 4), (3, M7, 4), (8, M7, 4), (10, D7, 4)]),
    ],
    diversity: 55,
    repeat: 50,
    tmpl: 50,
    len: Len {
        split_pair: true,
        long_last: 15,
        long_any: 0,
    },
};

// ---------------------------------------------------------------- Cinematic

static CINEMATIC_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, MAJ, 9, CORE),
        nd(2, MIN, 3, CORE),
        nd(4, MIN, 4, CORE),
        nd(5, MAJ, 8, CORE),
        nd(7, MAJ, 8, CORE),
        nd(9, MIN, 8, CORE),
        nd(8, MAJ, 6, BOR),
        nd(10, MAJ, 6, BOR),
        nd(5, MIN, 4, BOR),
        nd(3, MAJ, 5, MED),
        nd(4, MAJ, 4, MED),
        nd(9, MAJ, 3, MED),
        nd(1, MAJ, 2, MED),
        nd(0, SUS2, 3, COL),
        nd(7, SUS4, 3, COL),
    ],
    idioms: &[
        (0, 9, 150),
        (9, 5, 200),
        (5, 0, 150),
        (5, 7, 130),
        (8, 10, 200),
        (10, 0, 150),
        (8, 5, 130),
        (0, 8, 140),
        (0, 0, 300),
        (7, 7, 300),
    ],
    start: &[(9, 150)],
    end: &[(7, 250), (5, 150), (10, 150), (0, 120)],
    half: &[(7, 200), (5, 150), (9, 120)],
    templates: &[
        tp(6, 0, &[(9, MIN, 8), (5, MAJ, 8), (0, MAJ, 8), (7, MAJ, 8)]),
        tp(5, 0, &[(0, MAJ, 8), (7, MAJ, 4), (9, MIN, 8), (5, MAJ, 4)]),
        tp(
            5,
            500,
            &[(0, MAJ, 4), (8, MAJ, 4), (5, MAJ, 4), (7, MAJ, 4)],
        ),
        tp(
            5,
            600,
            &[(0, MAJ, 4), (3, MAJ, 4), (8, MAJ, 4), (10, MAJ, 4)],
        ),
        tp(
            4,
            600,
            &[(0, MAJ, 4), (8, MAJ, 4), (10, MAJ, 4), (7, MAJ, 4)],
        ),
    ],
    diversity: 55,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 50,
        long_any: 45,
    },
};

static CINEMATIC_MINOR: Profile = Profile {
    nodes: &[
        nd(0, MIN, 10, CORE),
        nd(3, MAJ, 8, CORE),
        nd(5, MIN, 5, CORE),
        nd(7, MAJ, 5, CORE),
        nd(7, MIN, 2, CORE),
        nd(8, MAJ, 9, CORE),
        nd(10, MAJ, 8, CORE),
        nd(5, MAJ, 4, BOR),
        nd(4, MAJ, 3, MED),
        nd(9, MAJ, 3, MED),
        nd(1, MAJ, 3, MED),
        nd(11, O7, 2, sec(0)),
    ],
    idioms: &[
        (0, 8, 170),
        (8, 3, 170),
        (3, 10, 170),
        (10, 0, 150),
        (0, 10, 160),
        (8, 10, 130),
        (10, 8, 110),
        (0, 4, 150),
    ],
    start: &[],
    end: &[(7, 250), (10, 180), (8, 120), (3, 100)],
    half: &[(7, 200), (10, 150), (3, 100)],
    templates: &[
        tp(8, 0, &[(0, MIN, 8), (8, MAJ, 8), (3, MAJ, 8), (10, MAJ, 8)]),
        tp(6, 0, &[(0, MIN, 4), (5, MIN, 4), (8, MAJ, 4), (10, MAJ, 4)]),
        tp(5, 0, &[(0, MIN, 4), (3, MAJ, 4), (10, MAJ, 4), (8, MAJ, 4)]),
        tp(
            4,
            600,
            &[(0, MIN, 4), (8, MAJ, 4), (4, MAJ, 4), (10, MAJ, 4)],
        ),
    ],
    diversity: 55,
    repeat: 45,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 50,
        long_any: 45,
    },
};

// ---------------------------------------------------------------- Anthem

static ANTHEM_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, MAJ, 10, CORE),
        nd(0, SUS2, 2, CORE),
        nd(2, MIN, 3, CORE),
        nd(5, MAJ, 10, CORE),
        nd(5, A9, 3, CORE),
        nd(7, MAJ, 10, CORE),
        nd(9, MIN, 8, CORE),
        nd(10, MAJ, 7, BOR),
        nd(3, MAJ, 4, BOR),
        nd(8, MAJ, 3, BOR),
        nd(2, MAJ, 4, sec(7)),
        nd(7, SUS4, 4, COL),
    ],
    idioms: &[
        (0, 5, 150),
        (5, 7, 170),
        (10, 5, 240),
        (0, 10, 150),
        (9, 5, 170),
        (5, 0, 130),
        (0, 7, 140),
        (7, 7, 300),
    ],
    start: &[],
    end: &[(7, 300), (0, 150), (5, 150), (9, 100)],
    half: &[(7, 300), (5, 150), (9, 100)],
    templates: &[
        tp(8, 0, &[(0, MAJ, 4), (7, MAJ, 4), (9, MIN, 4), (5, MAJ, 4)]),
        tp(6, 0, &[(0, MAJ, 4), (5, MAJ, 4), (7, MAJ, 4), (5, MAJ, 4)]),
        tp(6, 0, &[(0, MAJ, 4), (5, MAJ, 4), (9, MIN, 4), (7, MAJ, 4)]),
        tp(
            5,
            300,
            &[(0, MAJ, 4), (10, MAJ, 4), (5, MAJ, 4), (7, MAJ, 4)],
        ),
        tp(5, 0, &[(9, MIN, 4), (5, MAJ, 4), (0, MAJ, 4), (7, MAJ, 4)]),
    ],
    diversity: 55,
    repeat: 60,
    tmpl: 50,
    len: PLAIN,
};

static ANTHEM_MINOR: Profile = Profile {
    nodes: &[
        nd(0, MIN, 10, CORE),
        nd(3, MAJ, 9, CORE),
        nd(5, MIN, 4, CORE),
        nd(7, MAJ, 5, CORE),
        nd(8, MAJ, 10, CORE),
        nd(10, MAJ, 10, CORE),
        nd(5, MAJ, 4, BOR),
        nd(7, SUS4, 3, COL),
    ],
    idioms: &[
        (8, 10, 160),
        (3, 10, 150),
        (10, 0, 130),
        (0, 8, 150),
        (0, 3, 140),
        (8, 3, 140),
        (10, 8, 130),
    ],
    start: &[],
    end: &[(10, 200), (7, 250), (8, 100), (3, 100)],
    half: &[(7, 250), (10, 150), (3, 100)],
    templates: &[
        tp(8, 0, &[(0, MIN, 4), (8, MAJ, 4), (3, MAJ, 4), (10, MAJ, 4)]),
        tp(6, 0, &[(0, MIN, 4), (3, MAJ, 4), (8, MAJ, 4), (10, MAJ, 4)]),
        tp(
            5,
            0,
            &[(0, MIN, 4), (10, MAJ, 4), (8, MAJ, 4), (10, MAJ, 4)],
        ),
        tp(4, 0, &[(8, MAJ, 4), (10, MAJ, 4), (0, MIN, 4), (3, MAJ, 4)]),
    ],
    diversity: 55,
    repeat: 60,
    tmpl: 50,
    len: PLAIN,
};

// ---------------------------------------------------------------- Blues

static BLUES_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, D7, 10, CORE),
        nd(5, D7, 8, CORE),
        nd(7, D7, 7, CORE),
        nd(10, D7, 4, BOR),
        nd(5, N7, 3, BOR),
        nd(0, D9, 3, COL),
        nd(5, D9, 3, COL),
        nd(7, D9, 3, COL),
        nd(7, D13, 3, COL),
        nd(9, D7, 3, sec(2)),
    ],
    idioms: &[
        (0, 5, 160),
        (5, 0, 160),
        (7, 5, 130),
        (0, 7, 120),
        (7, 0, 150),
        (5, 7, 100),
    ],
    start: &[],
    end: &[(7, 300), (0, 100), (5, 80)],
    half: &[(5, 150), (0, 100), (7, 100)],
    templates: &[
        // Twelve bars with a turnaround.
        tp(
            10,
            0,
            &[
                (0, D7, 16),
                (5, D7, 8),
                (0, D7, 8),
                (7, D7, 4),
                (5, D7, 4),
                (0, D7, 4),
                (7, D7, 4),
            ],
        ),
        tp(8, 0, &[(0, D7, 4), (5, D7, 4), (0, D7, 4), (7, D7, 4)]),
        tp(6, 0, &[(0, D7, 8), (5, D7, 4), (7, D7, 4)]),
        tp(
            4,
            0,
            &[(0, D7, 4), (5, D7, 4), (0, D7, 4), (5, D7, 4), (7, D7, 4)],
        ),
        // Eight bars.
        tp(
            8,
            0,
            &[
                (0, D7, 4),
                (5, D7, 4),
                (0, D7, 4),
                (7, D7, 4),
                (5, D7, 4),
                (0, D7, 4),
                (7, D7, 4),
                (5, D7, 4),
            ],
        ),
        tp(
            6,
            0,
            &[
                (0, D7, 4),
                (5, D7, 4),
                (0, D7, 4),
                (7, D7, 4),
                (5, D7, 4),
                (7, D7, 4),
            ],
        ),
    ],
    diversity: 100,
    repeat: 0,
    tmpl: 85,
    len: PLAIN,
};

static BLUES_MINOR: Profile = Profile {
    nodes: &[
        nd(0, N7, 10, CORE),
        nd(5, N7, 8, CORE),
        nd(7, D7, 7, CORE),
        nd(8, D7, 4, CORE),
        nd(7, N7, 3, CORE),
        nd(2, HD, 3, CORE),
        nd(10, D7, 3, BOR),
        nd(1, D7, 3, tri(0)),
        nd(0, N9, 3, COL),
    ],
    idioms: &[
        (0, 5, 160),
        (5, 0, 160),
        (8, 7, 200),
        (7, 0, 150),
        (7, 5, 120),
        (0, 7, 110),
    ],
    start: &[],
    end: &[(7, 300), (0, 100)],
    half: &[(5, 150), (0, 100), (7, 100)],
    templates: &[
        tp(
            10,
            0,
            &[
                (0, N7, 16),
                (5, N7, 8),
                (0, N7, 8),
                (8, D7, 4),
                (7, D7, 4),
                (0, N7, 4),
                (7, D7, 4),
            ],
        ),
        tp(8, 0, &[(0, N7, 4), (5, N7, 4), (0, N7, 4), (7, D7, 4)]),
        tp(6, 0, &[(0, N7, 8), (5, N7, 4), (7, D7, 4)]),
        tp(
            4,
            0,
            &[(0, N7, 4), (5, N7, 4), (0, N7, 4), (8, D7, 4), (7, D7, 4)],
        ),
        // Eight bars.
        tp(
            8,
            0,
            &[
                (0, N7, 4),
                (5, N7, 4),
                (0, N7, 4),
                (7, D7, 4),
                (5, N7, 4),
                (0, N7, 4),
                (7, D7, 4),
                (5, N7, 4),
            ],
        ),
        tp(
            6,
            0,
            &[
                (0, N7, 4),
                (5, N7, 4),
                (0, N7, 4),
                (7, D7, 4),
                (5, N7, 4),
                (7, D7, 4),
            ],
        ),
    ],
    diversity: 100,
    repeat: 0,
    tmpl: 85,
    len: PLAIN,
};

// ---------------------------------------------------------------- Modal

static MODAL_MAJOR: Profile = Profile {
    nodes: &[
        nd(0, MAJ, 10, CORE),
        nd(2, MAJ, 6, CORE),
        nd(2, MIN, 3, CORE),
        nd(5, MAJ, 8, CORE),
        nd(7, MIN, 4, CORE),
        nd(9, MIN, 3, CORE),
        nd(10, MAJ, 9, CORE),
        nd(3, MAJ, 4, BOR),
        nd(8, MAJ, 4, BOR),
        nd(6, DIM, 2, COL),
        nd(0, SUS2, 3, COL),
    ],
    idioms: &[
        (0, 10, 200),
        (10, 5, 250),
        (5, 0, 150),
        (0, 2, 180),
        (2, 0, 150),
        (0, 7, 130),
        (0, 0, 300),
        (7, 10, 150),
        (10, 0, 130),
    ],
    start: &[],
    end: &[(10, 200), (5, 200), (0, 130), (2, 100)],
    half: &[(10, 150), (5, 150), (2, 100)],
    templates: &[
        tp(
            10,
            0,
            &[(0, MAJ, 4), (10, MAJ, 4), (5, MAJ, 4), (10, MAJ, 4)],
        ),
        tp(6, 0, &[(0, MAJ, 4), (2, MAJ, 4), (0, MAJ, 4), (10, MAJ, 4)]),
        tp(6, 0, &[(0, MAJ, 4), (5, MAJ, 4), (10, MAJ, 4), (5, MAJ, 4)]),
        tp(4, 0, &[(0, MAJ, 4), (10, MAJ, 4), (7, MIN, 4), (5, MAJ, 4)]),
    ],
    diversity: 100,
    repeat: 50,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 0,
        long_any: 25,
    },
};

static MODAL_MINOR: Profile = Profile {
    nodes: &[
        nd(0, N7, 10, CORE),
        nd(1, MAJ, 5, CORE),
        nd(2, N7, 4, CORE),
        nd(3, MAJ, 6, CORE),
        nd(5, D7, 8, CORE),
        nd(5, MAJ, 5, CORE),
        nd(7, N7, 5, CORE),
        nd(10, MAJ, 8, CORE),
        nd(8, MAJ, 4, BOR),
        nd(0, SUS2, 2, COL),
    ],
    idioms: &[
        (0, 5, 260),
        (5, 0, 200),
        (0, 10, 160),
        (10, 5, 180),
        (0, 1, 160),
        (1, 0, 220),
        (10, 0, 150),
        (3, 10, 140),
        (0, 0, 300),
    ],
    start: &[],
    end: &[(10, 200), (5, 200), (0, 120), (1, 120)],
    half: &[(10, 150), (5, 150), (1, 100)],
    templates: &[
        tp(10, 0, &[(0, N7, 4), (5, D7, 4), (0, N7, 4), (10, MAJ, 4)]),
        tp(6, 0, &[(0, N7, 4), (10, MAJ, 4), (5, D7, 4), (10, MAJ, 4)]),
        tp(6, 0, &[(0, N7, 4), (1, MAJ, 4), (0, N7, 4), (10, MAJ, 4)]),
        tp(6, 0, &[(0, N7, 4), (5, D7, 4), (10, MAJ, 4), (5, D7, 4)]),
    ],
    diversity: 100,
    repeat: 50,
    tmpl: 50,
    len: Len {
        split_pair: false,
        long_last: 0,
        long_any: 25,
    },
};

/// The profile of a mood (by [`super::Mood`] index) in a mode (`false` = major, `true` = minor).
pub(super) fn profile(mood: usize, minor: bool) -> &'static Profile {
    let table: [(&Profile, &Profile); 12] = [
        (&POP_MAJOR, &POP_MINOR),
        (&UPLIFTING_MAJOR, &UPLIFTING_MINOR),
        (&MELANCHOLY_MAJOR, &MELANCHOLY_MINOR),
        (&DARK_MAJOR, &DARK_MINOR),
        (&DREAMY_MAJOR, &DREAMY_MINOR),
        (&JAZZ_MAJOR, &JAZZ_MINOR),
        (&NEO_SOUL_MAJOR, &NEO_SOUL_MINOR),
        (&LOFI_MAJOR, &LOFI_MINOR),
        (&CINEMATIC_MAJOR, &CINEMATIC_MINOR),
        (&ANTHEM_MAJOR, &ANTHEM_MINOR),
        (&BLUES_MAJOR, &BLUES_MINOR),
        (&MODAL_MAJOR, &MODAL_MINOR),
    ];
    let (major, minor_profile) = table[mood.min(11)];
    if minor { minor_profile } else { major }
}
