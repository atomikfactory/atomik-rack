//! The note clips of the factory library (test code): twelve patterns as tables, and the writer
//! that turns each into an `.atomikclip` file exactly as Save to Library does. See
//! `app_core_factory_build.rs` for how the tables become files.
//!
//! Drum clips use the keys of the factory drum kits (kick 36, rim 37, snare 38, clap 39, low tom
//! 41, closed hat 42, perc 43, mid tom 45, open hat 46), so they play any of them.

use atomik_core::PPQ;
use atomik_model::{Clip, ClipContent, ClipFile, ClipId, Note, save_clip_file};

/// A step pattern letter: accent, normal, ghost or rest.
fn velocity_of(letter: char) -> Option<f32> {
    match letter {
        'X' => Some(1.0),
        'x' => Some(0.82),
        'o' => Some(0.5),
        _ => None,
    }
}

/// What a clip plays. Positions and lengths are in beats.
pub(in crate::app_core) enum Pattern {
    /// One lane per key, a letter per step (`X` accent, `x` normal, `o` ghost, `.` rest). Every
    /// string has one letter per step of the whole clip.
    Drums {
        step: f64,
        lanes: &'static [(u8, &'static str)],
    },
    /// A line of single notes, one per step (key 0 is a rest). The accent letters repeat over
    /// the steps. `gate` is the length of a note as a share of the step.
    Line {
        step: f64,
        gate: f64,
        keys: &'static [u8],
        accents: &'static str,
    },
    /// Chords held for a time: `(start, length, keys)`.
    Chords {
        velocity: f32,
        changes: &'static [(f64, f64, &'static [u8])],
    },
    /// Arpeggios: for each span `(start, length, chord)` one note per step, taking the chord
    /// key `order[n % order.len()]`. The accent letters repeat over the steps of a span.
    Arp {
        step: f64,
        gate: f64,
        order: &'static [usize],
        accents: &'static str,
        spans: &'static [(f64, f64, &'static [u8])],
    },
}

/// One factory clip.
pub(in crate::app_core) struct ClipDef {
    /// The clip name (and the file name).
    pub name: &'static str,
    /// What kind of pattern it is (the browser's category column).
    pub category: &'static str,
    /// Words to search and filter by.
    pub tags: &'static [&'static str],
    /// One line on what it plays (for the notes of a release; not saved in the item).
    pub sound: &'static str,
    /// The tempo it was made at (informational).
    pub bpm: f64,
    /// Its length in bars of 4/4.
    pub bars: i64,
    /// The notes.
    pub pattern: Pattern,
}

/// The factory clips, in the order the manifest lists them.
pub(in crate::app_core) const CLIPS: &[ClipDef] = &[
    ClipDef {
        name: "Four On The Floor",
        category: "Drums",
        tags: &["drums", "house", "straight", "driving"],
        sound: "A kick on every beat, a clap on two and four, closed hats on the beat and open \
                hats between: a steady dance-floor pulse, two bars.",
        bpm: 124.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X...X...X...X...X...X...X...X..."),
                (39, "....X.......X.......X.......X..."),
                (42, "x...x...x...x...x...x...x...x..."),
                (46, "..x...x...x...x...x...x...x...x."),
            ],
        },
    },
    ClipDef {
        name: "Half-Time Groove",
        category: "Drums",
        tags: &["drums", "half-time", "heavy", "slow"],
        sound: "A kick that leans on the off-beats, one snare in the middle of each bar, \
                steady eighth hats and a late open hat: a heavy half-time feel, two bars.",
        bpm: 140.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X.....x...x.....X.....x.....x..."),
                (38, "........X.......o...o...X......."),
                (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x."),
                (46, "..............................x."),
            ],
        },
    },
    ClipDef {
        name: "Broken Beat",
        category: "Drums",
        tags: &["drums", "syncopated", "breakbeat", "shuffled"],
        sound: "A syncopated kick, a snare with ghost notes around it, a rim click and busy \
                hats: a loose, broken pattern over two bars.",
        bpm: 96.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X..x..x...x..x..X.....x..x.x...."),
                (38, "....X..o....X..x....X..o....X.o."),
                (37, "..x.......x.......x.......x...x."),
                (42, "x.xxx.xxx.xxx.xxx.xxx.xxx.xxx.xx"),
            ],
        },
    },
    ClipDef {
        name: "Triplet Shuffle",
        category: "Drums",
        tags: &["drums", "shuffle", "triplet", "swing"],
        sound: "A swung groove in triplets: hats on the first and third of each group, a kick \
                and a rim shot with a snare on the backbeat, two bars.",
        bpm: 108.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 1.0 / 3.0,
            lanes: &[
                (36, "X.....X..X..X.....X....."),
                (38, "......X.........X..o..X."),
                (42, "x.xx.xx.xx.xx.xx.xx.xx.x"),
                (37, ".......x.........x......"),
            ],
        },
    },
    ClipDef {
        name: "Octave Pump",
        category: "Bass",
        tags: &["bass", "octave", "eighths", "driving"],
        sound: "Eighth notes that jump between a low A and the A above it, then follow the \
                chords down to F and G: a pumping bass line, two bars.",
        bpm: 122.0,
        bars: 2,
        pattern: Pattern::Line {
            step: 0.5,
            gate: 0.8,
            keys: &[
                33, 45, 33, 45, 33, 45, 33, 45, 29, 41, 29, 41, 31, 43, 31, 43,
            ],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Walking Minor",
        category: "Bass",
        tags: &["bass", "walking", "minor", "quarter-notes"],
        sound: "A walking line of quarter notes through A minor that climbs, turns at F and \
                comes home by way of E, four bars.",
        bpm: 100.0,
        bars: 4,
        pattern: Pattern::Line {
            step: 1.0,
            gate: 0.9,
            keys: &[
                33, 36, 38, 40, 41, 40, 38, 36, 38, 41, 43, 41, 40, 38, 36, 35,
            ],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Syncopated Sub",
        category: "Bass",
        tags: &["bass", "sub", "syncopated", "sixteenths"],
        sound: "Short low notes on F that land between the beats and answer with a step up to \
                B flat and C: a sparse, bouncing sub line, two bars.",
        bpm: 90.0,
        bars: 2,
        pattern: Pattern::Line {
            step: 0.25,
            gate: 0.7,
            keys: &[
                29, 0, 0, 29, 0, 0, 29, 0, 0, 0, 29, 0, 32, 0, 0, 0, 29, 0, 0, 29, 0, 0, 29, 0, 0,
                34, 0, 0, 36, 0, 32, 0,
            ],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Minor Four Chords",
        category: "Chords",
        tags: &["chords", "minor", "pad", "progression"],
        sound: "A minor, F, C and G, one bar each in open voicings with the bass note \
                doubled low: the classic four-chord loop for pads and keys.",
        bpm: 90.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.75,
            changes: &[
                (0.0, 4.0, &[45, 57, 60, 64]),
                (4.0, 4.0, &[41, 57, 60, 65]),
                (8.0, 4.0, &[48, 55, 60, 64]),
                (12.0, 4.0, &[43, 55, 59, 62]),
            ],
        },
    },
    ClipDef {
        name: "Open Fifths Rise",
        category: "Chords",
        tags: &["chords", "fifths", "open", "ambient"],
        sound: "Stacked fifths that step up from C through D and E flat to F, one bar each: \
                bare, open chords that sound neither major nor minor.",
        bpm: 70.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.7,
            changes: &[
                (0.0, 4.0, &[48, 55, 62, 69]),
                (4.0, 4.0, &[50, 57, 64, 71]),
                (8.0, 4.0, &[51, 58, 65, 72]),
                (12.0, 4.0, &[53, 60, 67, 74]),
            ],
        },
    },
    ClipDef {
        name: "Seventh Changes",
        category: "Chords",
        tags: &["chords", "jazz", "sevenths", "stabs"],
        sound: "D minor seventh, G seventh, C major seventh and A minor seventh as half-note \
                stabs with the second chord of each bar pushed a beat early: a jazzy turnaround.",
        bpm: 100.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.7,
            changes: &[
                (0.0, 1.5, &[50, 53, 57, 60]),
                (1.5, 2.5, &[50, 53, 57, 60]),
                (4.0, 1.5, &[43, 47, 50, 53]),
                (5.5, 2.5, &[43, 47, 50, 53]),
                (8.0, 1.5, &[48, 52, 55, 59]),
                (9.5, 2.5, &[48, 52, 55, 59]),
                (12.0, 1.5, &[45, 52, 55, 60]),
                (13.5, 2.5, &[45, 52, 55, 60]),
            ],
        },
    },
    ClipDef {
        name: "Rising Triad Arp",
        category: "Arpeggio",
        tags: &["arpeggio", "sixteenths", "major", "bright"],
        sound: "Sixteenth notes that climb a C major triad to the octave and fall back, then \
                the same shape on G over a B bass: a bright, running figure, two bars.",
        bpm: 120.0,
        bars: 2,
        pattern: Pattern::Arp {
            step: 0.25,
            gate: 0.6,
            order: &[0, 1, 2, 3, 2, 1],
            accents: "Xxxx",
            spans: &[(0.0, 4.0, &[60, 64, 67, 72]), (4.0, 4.0, &[59, 62, 67, 71])],
        },
    },
    ClipDef {
        name: "Wide Minor Arp",
        category: "Arpeggio",
        tags: &["arpeggio", "eighths", "minor", "wide"],
        sound: "Eighth notes that sweep up six notes of A minor, F, C and G over two octaves \
                and come back down, one bar per chord: a wide, flowing arpeggio.",
        bpm: 110.0,
        bars: 4,
        pattern: Pattern::Arp {
            step: 0.5,
            gate: 0.9,
            order: &[0, 1, 2, 3, 4, 5, 4, 3],
            accents: "Xxxx",
            spans: &[
                (0.0, 4.0, &[45, 52, 57, 60, 64, 69]),
                (4.0, 4.0, &[41, 48, 53, 57, 60, 65]),
                (8.0, 4.0, &[48, 55, 60, 64, 67, 72]),
                (12.0, 4.0, &[43, 50, 55, 59, 62, 67]),
            ],
        },
    },
    ClipDef {
        name: "Boom Bap Swing",
        category: "Drums",
        tags: &["drums", "hip-hop", "laid-back", "boom-bap"],
        sound: "A kick that skips around the downbeat, a snare on two and four with a rim ghost and steady eighth hats: a laid-back boom-bap groove over two bars.",
        bpm: 88.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X.....x...x.....X.....x..x....x."),
                (38, "........X...............X......."),
                (37, "..............o...........o....."),
                (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.xx"),
            ],
        },
    },
    ClipDef {
        name: "Trap Rolls",
        category: "Drums",
        tags: &["drums", "trap", "rolling", "half-time"],
        sound: "A sparse kick, a snare on the third beat of each bar, sixteenth hats that stutter and an open hat at the end of each bar: a rolling half-time trap pattern.",
        bpm: 140.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X..........x....X.......x......."),
                (38, "........X...............X......."),
                (42, "xxxxxxxxxxxx.x.xxxxxxxxxxxxxxxxx"),
                (46, "..............o...............o."),
            ],
        },
    },
    ClipDef {
        name: "Breakbeat Chop",
        category: "Drums",
        tags: &["drums", "breakbeat", "fast", "chopped"],
        sound: "A kick and snare that trade places with ghost snares between them over steady hats: a fast, chopped breakbeat in two bars.",
        bpm: 160.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X.....x...x.....X.x.....x.....x."),
                (38, "....X..o.o..X.......X.......X..o"),
                (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x."),
                (46, "...............x................"),
            ],
        },
    },
    ClipDef {
        name: "Dub Steppers",
        category: "Drums",
        tags: &["drums", "dub", "one-drop", "slow"],
        sound: "A one-drop feel: the kick and snare fall together on the third beat of each bar, with a soft percussion tick and open eighth hats.",
        bpm: 70.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X...............X..............."),
                (37, "................................"),
                (38, "........X...............X......."),
                (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x."),
                (43, "....o.......o.......o.......o..."),
            ],
        },
    },
    ClipDef {
        name: "Techno Drive",
        category: "Drums",
        tags: &["drums", "techno", "four-on-the-floor", "driving"],
        sound: "A straight kick on every beat, a clap on two and four, busy ghosted hats and open hats on the off-beats: a driving techno pattern.",
        bpm: 132.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X...X...X...X...X...X...X...X..."),
                (39, "....X.......X.......X.......X..."),
                (42, "o.xxo.xxo.xxo.xxo.xxo.xxo.xxo.xx"),
                (46, "..x...x...x...x...x...x...x...x."),
            ],
        },
    },
    ClipDef {
        name: "Bossa Pulse",
        category: "Drums",
        tags: &["drums", "bossa", "latin", "gentle"],
        sound: "A gentle bossa pulse: a soft two-note kick, a rim clave over it, steady hats and a light percussion tick.",
        bpm: 120.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X..x..X...x..X..X..x..X...x..X.."),
                (37, "X..X..X...X..X..X..X..X...X..X.."),
                (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x."),
                (43, "..o...o...o...o...o...o...o...o."),
            ],
        },
    },
    ClipDef {
        name: "Drum And Bass Roll",
        category: "Drums",
        tags: &["drums", "drum-and-bass", "fast", "rolling"],
        sound: "A two-step kick, a snare on two and four, fast hats and an open hat that closes each bar: a rolling drum and bass pattern.",
        bpm: 174.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 0.25,
            lanes: &[
                (36, "X.........x.....X.....x...x....."),
                (38, "....X.......X.......X.......X..."),
                (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x."),
                (46, "..............x...............x."),
            ],
        },
    },
    ClipDef {
        name: "Shuffle Rock",
        category: "Drums",
        tags: &["drums", "shuffle", "triplet", "rock"],
        sound: "A rocking shuffle in triplets: a kick that doubles up, a snare on two and four and hats on the first and third of each triplet.",
        bpm: 100.0,
        bars: 2,
        pattern: Pattern::Drums {
            step: 1.0 / 3.0,
            lanes: &[
                (36, "X.....X.X.....X.....X.X."),
                (38, "......X.........X.....X."),
                (42, "x.xx.xx.xx.xx.xx.xx.xx.x"),
                (46, ".......x................"),
            ],
        },
    },
    ClipDef {
        name: "Dub Sub Roots",
        category: "Bass",
        tags: &["bass", "sub", "dub", "sparse"],
        sound: "Long low notes with gaps between them that move from A to C and D and back down to G: a sparse dub bass line, four bars.",
        bpm: 70.0,
        bars: 4,
        pattern: Pattern::Line {
            step: 1.0,
            gate: 1.0,
            keys: &[33, 0, 33, 0, 33, 0, 36, 0, 38, 0, 38, 0, 31, 0, 33, 0],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Acid Climb",
        category: "Bass",
        tags: &["bass", "acid", "sixteenths", "minor"],
        sound: "Sixteenth notes that hop between the low A and notes above it, with a climb at the end of the second bar: an acid-style line.",
        bpm: 132.0,
        bars: 2,
        pattern: Pattern::Line {
            step: 0.25,
            gate: 0.6,
            keys: &[
                33, 33, 45, 33, 0, 33, 48, 33, 45, 0, 33, 33, 52, 33, 45, 0, 33, 33, 45, 33, 0, 33,
                48, 33, 50, 0, 53, 33, 52, 50, 48, 45,
            ],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Disco Octaves",
        category: "Bass",
        tags: &["bass", "octave", "disco", "eighths"],
        sound: "Eighth notes that bounce between a low note and the octave above it, moving from A to D and E: a classic disco bass.",
        bpm: 118.0,
        bars: 2,
        pattern: Pattern::Line {
            step: 0.5,
            gate: 0.75,
            keys: &[
                33, 45, 33, 45, 33, 45, 33, 45, 38, 50, 38, 50, 40, 52, 40, 52,
            ],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Slow Fifths",
        category: "Bass",
        tags: &["bass", "fifths", "slow", "open"],
        sound: "Half-bar notes that alternate a root and the fifth above it while the root walks A, C, D and G: a slow, open bass line.",
        bpm: 80.0,
        bars: 4,
        pattern: Pattern::Line {
            step: 2.0,
            gate: 1.0,
            keys: &[33, 40, 36, 43, 38, 45, 31, 38],
            accents: "Xx",
        },
    },
    ClipDef {
        name: "Rolling Sixteenths",
        category: "Bass",
        tags: &["bass", "sixteenths", "driving", "steady"],
        sound: "Steady sixteenth notes on one pitch that move to the next root every half bar and run up at the end: a driving, rolling bass.",
        bpm: 140.0,
        bars: 2,
        pattern: Pattern::Line {
            step: 0.25,
            gate: 0.5,
            keys: &[
                36, 36, 36, 36, 36, 36, 36, 36, 34, 34, 34, 34, 34, 34, 34, 34, 31, 31, 31, 31, 31,
                31, 31, 31, 33, 33, 33, 33, 33, 33, 36, 38,
            ],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Two Note Rocker",
        category: "Bass",
        tags: &["bass", "rock", "syncopated", "eighths"],
        sound: "Two notes in a syncopated eighth pattern, a root and its octave, answered by a rising turn at the end of each bar.",
        bpm: 110.0,
        bars: 2,
        pattern: Pattern::Line {
            step: 0.5,
            gate: 0.8,
            keys: &[33, 0, 33, 45, 0, 33, 0, 36, 38, 0, 38, 50, 0, 38, 0, 40],
            accents: "Xxxx",
        },
    },
    ClipDef {
        name: "Warm Minor Sevenths",
        category: "Chords",
        tags: &["chords", "minor", "sevenths", "warm"],
        sound: "Four held minor-seventh and major-seventh chords, one per bar, voiced close together: a warm progression for pads and keys.",
        bpm: 84.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.72,
            changes: &[
                (0.0, 4.0, &[57, 60, 64, 67]),
                (4.0, 4.0, &[53, 57, 60, 64]),
                (8.0, 4.0, &[55, 59, 62, 65]),
                (12.0, 4.0, &[52, 55, 59, 62]),
            ],
        },
    },
    ClipDef {
        name: "Bright Major Lift",
        category: "Chords",
        tags: &["chords", "major", "rising", "bright"],
        sound: "Four major and minor triads that climb by step, one per bar: a bright, lifting progression.",
        bpm: 116.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.78,
            changes: &[
                (0.0, 4.0, &[60, 64, 67]),
                (4.0, 4.0, &[62, 65, 69]),
                (8.0, 4.0, &[64, 67, 71]),
                (12.0, 4.0, &[65, 69, 72]),
            ],
        },
    },
    ClipDef {
        name: "Suspended Drift",
        category: "Chords",
        tags: &["chords", "suspended", "ambient", "slow"],
        sound: "Four long suspended chords that never settle, one per bar: a slow, open bed for ambient sounds.",
        bpm: 66.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.66,
            changes: &[
                (0.0, 4.0, &[57, 62, 64]),
                (4.0, 4.0, &[55, 60, 62]),
                (8.0, 4.0, &[50, 55, 57]),
                (12.0, 4.0, &[52, 57, 59]),
            ],
        },
    },
    ClipDef {
        name: "Dorian Vamp",
        category: "Chords",
        tags: &["chords", "dorian", "vamp", "groove"],
        sound: "A two-chord vamp that rocks between a minor-seventh and a dominant chord every half bar: a groove for organs and keys.",
        bpm: 104.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.74,
            changes: &[
                (0.0, 2.0, &[50, 57, 60, 65]),
                (2.0, 2.0, &[50, 57, 60, 65]),
                (4.0, 2.0, &[55, 59, 62, 65]),
                (6.0, 2.0, &[55, 59, 62, 65]),
                (8.0, 2.0, &[50, 57, 60, 65]),
                (10.0, 2.0, &[50, 57, 60, 65]),
                (12.0, 4.0, &[55, 59, 62, 65]),
            ],
        },
    },
    ClipDef {
        name: "Half Step Stabs",
        category: "Chords",
        tags: &["chords", "stabs", "chromatic", "dance"],
        sound: "Short seventh chords that step up by a half tone and fall back, then rest on a long chord: a chromatic dance stab pattern.",
        bpm: 124.0,
        bars: 4,
        pattern: Pattern::Chords {
            velocity: 0.8,
            changes: &[
                (0.0, 1.0, &[60, 64, 67, 71]),
                (1.0, 1.0, &[61, 65, 68, 72]),
                (2.0, 2.0, &[60, 64, 67, 71]),
                (4.0, 4.0, &[57, 60, 64, 67]),
                (8.0, 1.0, &[60, 64, 67, 71]),
                (9.0, 1.0, &[61, 65, 68, 72]),
                (10.0, 2.0, &[60, 64, 67, 71]),
                (12.0, 4.0, &[55, 59, 62, 65]),
            ],
        },
    },
    ClipDef {
        name: "Pentatonic Run",
        category: "Arpeggio",
        tags: &["arpeggio", "pentatonic", "sixteenths", "running"],
        sound: "A five-note minor pentatonic shape that runs up and down in sixteenths, then shifts down a step for the second bar.",
        bpm: 118.0,
        bars: 2,
        pattern: Pattern::Arp {
            step: 0.25,
            gate: 0.6,
            order: &[0, 1, 2, 3, 4, 3, 2, 1],
            accents: "Xxxx",
            spans: &[
                (0.0, 4.0, &[57, 60, 62, 64, 67]),
                (4.0, 4.0, &[55, 57, 60, 62, 64]),
            ],
        },
    },
    ClipDef {
        name: "Descending Sevenths",
        category: "Arpeggio",
        tags: &["arpeggio", "descending", "sevenths", "eighths"],
        sound: "Eighth notes that fall through each seventh chord from the top, one chord per bar: a falling pattern over four bars.",
        bpm: 96.0,
        bars: 4,
        pattern: Pattern::Arp {
            step: 0.5,
            gate: 0.85,
            order: &[3, 2, 1, 0],
            accents: "Xxxx",
            spans: &[
                (0.0, 4.0, &[57, 60, 64, 67]),
                (4.0, 4.0, &[53, 57, 60, 64]),
                (8.0, 4.0, &[55, 59, 62, 65]),
                (12.0, 4.0, &[52, 55, 59, 62]),
            ],
        },
    },
    ClipDef {
        name: "Fifths Bounce",
        category: "Arpeggio",
        tags: &["arpeggio", "fifths", "bouncy", "sixteenths"],
        sound: "Sixteenth notes that bounce between a root, its fifth and the octave: a bright, bouncing figure.",
        bpm: 128.0,
        bars: 2,
        pattern: Pattern::Arp {
            step: 0.25,
            gate: 0.5,
            order: &[0, 2, 0, 2, 1, 2, 1, 2],
            accents: "Xxxx",
            spans: &[(0.0, 4.0, &[48, 55, 60]), (4.0, 4.0, &[50, 57, 62])],
        },
    },
    ClipDef {
        name: "Slow Wide Spread",
        category: "Arpeggio",
        tags: &["arpeggio", "wide", "slow", "ambient"],
        sound: "One note per beat across a six-note chord spread over two octaves, two chords over four bars: a slow, wide arpeggio.",
        bpm: 72.0,
        bars: 4,
        pattern: Pattern::Arp {
            step: 1.0,
            gate: 1.0,
            order: &[0, 1, 2, 3, 4, 5, 4, 3],
            accents: "Xx",
            spans: &[
                (0.0, 8.0, &[45, 52, 57, 60, 64, 69]),
                (8.0, 8.0, &[43, 50, 55, 59, 62, 67]),
            ],
        },
    },
    ClipDef {
        name: "Triplet Climb",
        category: "Arpeggio",
        tags: &["arpeggio", "triplet", "minor", "flowing"],
        sound: "Triplet notes that climb through a minor seventh chord and back, with the second bar a tone lower: a flowing figure.",
        bpm: 110.0,
        bars: 2,
        pattern: Pattern::Arp {
            step: 1.0 / 3.0,
            gate: 0.7,
            order: &[0, 1, 2, 3, 2, 1],
            accents: "Xxx",
            spans: &[(0.0, 4.0, &[60, 63, 67, 70]), (4.0, 4.0, &[58, 62, 65, 69])],
        },
    },
];

/// Beats to ticks.
#[allow(clippy::cast_possible_truncation)] // a few bars of beats
fn ticks(beats: f64) -> i64 {
    (beats * PPQ as f64).round() as i64
}

/// The notes of `def`, in start order.
///
/// # Panics
/// A pattern that does not fill the clip.
pub(in crate::app_core) fn clip_notes(def: &ClipDef) -> Vec<Note> {
    let name = def.name;
    let beats = def.bars as f64 * 4.0;
    let mut out: Vec<Note> = Vec::new();
    match &def.pattern {
        Pattern::Drums { step, lanes } => {
            let steps = (beats / step).round() as usize;
            for &(key, lane) in *lanes {
                assert_eq!(lane.chars().count(), steps, "{name}: lane {key}");
                for (i, letter) in lane.chars().enumerate() {
                    if let Some(v) = velocity_of(letter) {
                        out.push(Note::new(
                            ticks(i as f64 * step),
                            ticks(step * 0.45),
                            key,
                            v,
                        ));
                    }
                }
            }
        }
        Pattern::Line {
            step,
            gate,
            keys,
            accents,
        } => {
            assert_eq!(keys.len(), (beats / step).round() as usize, "{name}: steps");
            let accents: Vec<char> = accents.chars().collect();
            for (i, &key) in keys.iter().enumerate() {
                let letter = accents[i % accents.len()];
                if key != 0
                    && let Some(v) = velocity_of(letter)
                {
                    out.push(Note::new(
                        ticks(i as f64 * step),
                        ticks(step * gate),
                        key,
                        v,
                    ));
                }
            }
        }
        Pattern::Chords { velocity, changes } => {
            for &(start, length, keys) in *changes {
                assert!(start + length <= beats, "{name}: a chord runs past the end");
                for &key in keys {
                    out.push(Note::new(
                        ticks(start),
                        ticks(length * 0.95),
                        key,
                        *velocity,
                    ));
                }
            }
        }
        Pattern::Arp {
            step,
            gate,
            order,
            accents,
            spans,
        } => {
            let accents: Vec<char> = accents.chars().collect();
            for &(start, length, chord) in *spans {
                assert!(start + length <= beats, "{name}: a span runs past the end");
                let count = (length / step).round() as usize;
                for n in 0..count {
                    let key = chord[order[n % order.len()]];
                    let v = velocity_of(accents[n % accents.len()]).unwrap_or(0.8);
                    out.push(Note::new(
                        ticks(start + n as f64 * step),
                        ticks(step * gate),
                        key,
                        v,
                    ));
                }
            }
        }
    }
    out.sort_by_key(|n| (n.tick, n.key));
    out
}

/// The clip of `def` placed at song tick `start`: named for the pattern, with its notes.
///
/// # Panics
/// A pattern that does not fill the clip.
pub(in crate::app_core) fn clip_of(def: &ClipDef, start: i64) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: ticks(def.bars as f64 * 4.0),
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: Some(def.name.to_owned()),
        color: None,
        grid: None,
        content: ClipContent::Notes(clip_notes(def).into()),
    }
}

/// The clip file text of `def`.
///
/// # Panics
/// A pattern the model refuses.
pub(in crate::app_core) fn clip_text(def: &ClipDef) -> String {
    let file = ClipFile {
        tempo_bpm: def.bpm,
        meter: (4, 4),
        clip: clip_of(def, 0),
    };
    save_clip_file(&file).unwrap_or_else(|e| panic!("{}: {e}", def.name))
}
