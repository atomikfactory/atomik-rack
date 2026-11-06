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

/// The clip file text of `def`.
///
/// # Panics
/// A pattern the model refuses.
pub(in crate::app_core) fn clip_text(def: &ClipDef) -> String {
    let clip = Clip {
        id: ClipId(0),
        start: 0,
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
    };
    let file = ClipFile {
        tempo_bpm: def.bpm,
        meter: (4, 4),
        clip,
    };
    save_clip_file(&file).unwrap_or_else(|e| panic!("{}: {e}", def.name))
}
