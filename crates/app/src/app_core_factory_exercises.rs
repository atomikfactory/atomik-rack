//! The tab exercises of the factory library (test code): small practice songs for the Tab Editor,
//! one project per exercise, and the writer that turns the tables into the files
//! `Projects/Exercises/<name>/<name>.atomik`. See `app_core_factory_build.rs` for how the tables
//! become files.
//!
//! Every exercise is made up for this library: scales, arpeggios, chord-change drills, banjo
//! rolls, ukulele strums, bass root-and-fifth lines and a mandolin tremolo study. No song, tune or
//! lyric of anybody else is in them (the lyric of a chord sheet counts the beats).
//!
//! An exercise is a [`Part`]: notes as `(string, fret)` on the tuning of its instrument (strings
//! in tab order, string 0 on the top line), the chord symbols and diagrams over the bars, and
//! section markers. [`build_exercise`] makes a tab score of it, imports it the way a tab file is
//! imported (so the track plays Plucked Strings with the patch of its instrument, the clip holds
//! the tab and the techniques' expression), adds the chord sheet and the song's title and notes,
//! and saves the project. The files are a pure function of this table and of the Plucked
//! patches, so a test compares what is committed with what the table builds.

use super::AUTHOR;
use crate::app_core::AppCore;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{Command, SongCommand, SongInfo, new_project_doc, save_project_checked};
use atomik_render::default_registry;
use atomik_tab::instrument::find_instrument;
use atomik_tab::score::{
    Bend, DiagramRef, Harmonic, HarmonicKind, SlideKind, StrumDirection, TabNote, TabScore,
    TabTrack, TechFlags, VibratoKind,
};
use atomik_tab::sheet::parse_sheet;
use atomik_tab::voicing::Voicing;
use atomik_tab::{ChordSymbol, Tuning};
use std::path::Path;

/// The category of the exercises in the factory manifest.
pub(in crate::app_core) const CATEGORY: &str = "Exercise";

const BAR: i64 = 4 * PPQ;
const EIGHTH: i64 = PPQ / 2;
const SIXTEENTH: i64 = PPQ / 4;

/// One exercise.
pub(in crate::app_core) struct ExerciseDef {
    /// The project name (and the folder and file name).
    pub name: &'static str,
    /// The tab instrument id of its track.
    pub instrument: &'static str,
    /// Words to search and filter by.
    pub tags: &'static [&'static str],
    /// What it trains, in a line or two (the notes of the song).
    pub trains: &'static str,
    /// The tempo.
    pub bpm: f64,
    /// The music.
    pub part: fn() -> Part,
    /// A chord sheet in the bracket form (made up: counting words), when the exercise has chords.
    pub sheet: &'static str,
}

/// The music of one exercise.
pub(in crate::app_core) struct Part {
    pub notes: Vec<TabNote>,
    /// `(tick, chord name)` for the chord lane.
    pub chords: Vec<(i64, &'static str)>,
    /// `(tick, chord name, frets per string, -1 for a muted string)`.
    pub diagrams: Vec<(i64, &'static str, Vec<i8>)>,
    /// `(tick, name)`.
    pub markers: Vec<(i64, &'static str)>,
}

impl Part {
    fn new() -> Self {
        Self {
            notes: Vec::new(),
            chords: Vec::new(),
            diagrams: Vec::new(),
            markers: Vec::new(),
        }
    }

    fn note(&mut self, tick: i64, len: i64, string: u8, fret: u8) -> &mut TabNote {
        self.notes.push(TabNote::new(tick, len, string, fret));
        self.notes.last_mut().expect("a note was just pushed")
    }

    /// A chord at `tick`: its name for the lane and its diagram.
    fn chord(&mut self, tick: i64, name: &'static str, shape: &[i8]) {
        self.chords.push((tick, name));
        self.diagrams.push((tick, name, shape.to_vec()));
    }

    /// Every sounding string of `shape` at `tick` for `len`, as one stroke.
    fn strum(&mut self, tick: i64, len: i64, shape: &[i8], dir: StrumDirection) {
        for (string, fret) in shape.iter().enumerate() {
            if let Ok(f) = u8::try_from(*fret) {
                self.note(tick, len, string as u8, f).tech.strum = Some(dir);
            }
        }
    }
}

// ------------------------------------------------------------------------------ the shapes

const G_GUITAR: [i8; 6] = [3, 0, 0, 0, 2, 3];
const C_GUITAR: [i8; 6] = [0, 1, 0, 2, 3, -1];
const D_GUITAR: [i8; 6] = [2, 3, 2, 0, -1, -1];
const EM_GUITAR: [i8; 6] = [0, 0, 0, 2, 2, 0];
const AM_GUITAR: [i8; 6] = [0, 1, 2, 2, 0, -1];

/// Open G tuning of the five-string banjo, strings 1 to 4 then the short fifth string.
const G_BANJO: [i8; 5] = [0, 0, 0, 0, 0];
const C_BANJO: [i8; 5] = [2, 1, 0, 2, 0];
const D7_BANJO: [i8; 5] = [0, 1, 2, 0, 0];

/// Soprano ukulele, tab order A E C G (the G string on the bottom line).
const C_UKE: [i8; 4] = [3, 0, 0, 0];
const AM_UKE: [i8; 4] = [0, 0, 0, 2];
const F_UKE: [i8; 4] = [0, 1, 0, 2];
const G_UKE: [i8; 4] = [2, 3, 2, 0];

/// The fingering of a shape as a diagram (no fingers: the editor shows plain dots).
fn voicing(shape: &[i8]) -> Voicing {
    let frets: Vec<Option<u8>> = shape.iter().map(|f| u8::try_from(*f).ok()).collect();
    let low = frets.iter().flatten().filter(|f| **f > 0).min().copied();
    Voicing {
        fingers: vec![None; frets.len()],
        barres: Vec::new(),
        base_fret: low.filter(|f| *f > 4).unwrap_or(1),
        frets,
    }
}

// ----------------------------------------------------------------------------- the exercises

/// `up` then back down: the scale as a line of positions.
fn up_and_down(up: &[(u8, u8)]) -> Vec<(u8, u8)> {
    let mut line = up.to_vec();
    line.extend(up.iter().rev().skip(1));
    line
}

/// A line of `(string, fret)` as eighth notes from tick 0, the last one held to the bar line.
fn eighths(line: &[(u8, u8)], part: &mut Part) {
    for (i, (string, fret)) in line.iter().enumerate() {
        part.note(i as i64 * EIGHTH, EIGHTH, *string, *fret);
    }
    let end = line.len() as i64 * EIGHTH;
    if let Some(last) = part.notes.last_mut() {
        last.len = (end + BAR - 1) / BAR * BAR - last.tick;
    }
}

fn major_scale_guitar() -> Part {
    // C major in open position from C3 to G4, up and down.
    let mut p = Part::new();
    let up = [
        (4, 3),
        (3, 0),
        (3, 2),
        (3, 3),
        (2, 0),
        (2, 2),
        (1, 0),
        (1, 1),
        (1, 3),
        (0, 0),
        (0, 1),
        (0, 3),
    ];
    eighths(&up_and_down(&up), &mut p);
    p.markers.push((0, "C major, open position"));
    p
}

fn minor_pentatonic_guitar() -> Part {
    // A minor pentatonic, the first box (frets 5 to 8), up and down.
    let mut p = Part::new();
    let up = [
        (5, 5),
        (5, 8),
        (4, 5),
        (4, 7),
        (3, 5),
        (3, 7),
        (2, 5),
        (2, 7),
        (1, 5),
        (1, 8),
        (0, 5),
        (0, 8),
    ];
    eighths(&up_and_down(&up), &mut p);
    p.markers.push((0, "A minor pentatonic, box 1"));
    p
}

fn fingerpicking_guitar() -> Part {
    // Two passes of four chords: a bass note, then the strings above it, eight eighths a bar.
    let mut p = Part::new();
    let chords: [(&'static str, [i8; 6], u8); 4] = [
        ("Am", AM_GUITAR, 4),
        ("C", C_GUITAR, 4),
        ("G", G_GUITAR, 5),
        ("Em", EM_GUITAR, 5),
    ];
    let order = [
        None,
        Some(2),
        Some(1),
        Some(0),
        Some(1),
        Some(2),
        Some(1),
        Some(0),
    ];
    for bar in 0..8_i64 {
        let (name, shape, bass) = chords[bar as usize % 4];
        p.chord(bar * BAR, name, &shape);
        if bar % 4 == 0 {
            p.markers.push((
                bar * BAR,
                if bar == 0 {
                    "First pass"
                } else {
                    "Second pass"
                },
            ));
        }
        for (i, string) in order.iter().enumerate() {
            let string = string.unwrap_or(bass);
            let fret = u8::try_from(shape[usize::from(string)]).unwrap_or(0);
            p.note(bar * BAR + i as i64 * EIGHTH, EIGHTH, string, fret);
        }
    }
    p
}

fn changes_guitar() -> Part {
    // G C D Em: one slow strum a bar, then four strums a bar.
    let mut p = Part::new();
    let chords: [(&'static str, [i8; 6]); 4] = [
        ("G", G_GUITAR),
        ("C", C_GUITAR),
        ("D", D_GUITAR),
        ("Em", EM_GUITAR),
    ];
    for bar in 0..8_i64 {
        let (name, shape) = chords[bar as usize % 4];
        p.chord(bar * BAR, name, &shape);
        if bar == 0 {
            p.markers.push((0, "One strum a bar"));
        }
        if bar == 4 {
            p.markers.push((bar * BAR, "Four strums a bar"));
        }
        if bar < 4 {
            p.strum(bar * BAR, BAR, &shape, StrumDirection::Down);
        } else {
            for beat in 0..4 {
                p.strum(bar * BAR + beat * PPQ, PPQ, &shape, StrumDirection::Down);
            }
        }
    }
    p
}

fn technique_sampler_guitar() -> Part {
    // Eight bars, one technique each, on a few strings.
    let mut p = Part::new();
    let mark = |p: &mut Part, bar: i64, name: &'static str| p.markers.push((bar * BAR, name));
    mark(&mut p, 0, "Palm mute");
    for i in 0..8 {
        let t = p.note(i * EIGHTH, EIGHTH, 5, 0);
        t.tech.flags.insert(TechFlags::PALM_MUTE);
    }
    mark(&mut p, 1, "Dead notes");
    for i in 0..8 {
        let t = p.note(BAR + i * EIGHTH, EIGHTH, 5, 0);
        if i % 2 == 1 {
            t.tech.flags.insert(TechFlags::DEAD);
        }
    }
    mark(&mut p, 2, "Hammer-ons and pull-offs");
    for (i, (fret, flag)) in [
        (0, None),
        (2, Some(TechFlags::HAMMER_ON)),
        (0, Some(TechFlags::PULL_OFF)),
        (2, Some(TechFlags::HAMMER_ON)),
        (0, Some(TechFlags::PULL_OFF)),
        (2, Some(TechFlags::HAMMER_ON)),
        (0, Some(TechFlags::PULL_OFF)),
        (0, None),
    ]
    .into_iter()
    .enumerate()
    {
        let t = p.note(2 * BAR + i as i64 * EIGHTH, EIGHTH, 4, fret);
        if let Some(flag) = flag {
            t.tech.flags.insert(flag);
        }
    }
    mark(&mut p, 3, "Slides");
    for (i, (fret, slide)) in [
        (5, Some(SlideKind::Legato)),
        (7, None),
        (9, Some(SlideKind::Shift)),
        (12, Some(SlideKind::OutDown)),
    ]
    .into_iter()
    .enumerate()
    {
        p.note(3 * BAR + i as i64 * PPQ, PPQ, 3, fret).tech.slide = slide;
    }
    mark(&mut p, 4, "Bend and vibrato");
    p.note(4 * BAR, 2 * PPQ, 1, 7).tech.bend = Some(Bend::up(200));
    let t = p.note(4 * BAR + 2 * PPQ, 2 * PPQ, 1, 8);
    t.tech.vibrato = Some(VibratoKind::Wide);
    mark(&mut p, 5, "Natural harmonics");
    for (i, fret) in [12, 7, 5, 12].into_iter().enumerate() {
        p.note(5 * BAR + i as i64 * PPQ, PPQ, 5, fret).tech.harmonic = Some(Harmonic {
            kind: HarmonicKind::Natural,
            fret: None,
        });
    }
    mark(&mut p, 6, "Let ring");
    for (i, (string, fret)) in [(5, 0), (4, 2), (3, 2), (2, 0), (1, 0), (0, 0)]
        .into_iter()
        .enumerate()
    {
        let len = if i == 5 { 3 * EIGHTH } else { EIGHTH };
        let t = p.note(6 * BAR + i as i64 * EIGHTH, len, string, fret);
        t.tech.flags.insert(TechFlags::LET_RING);
    }
    mark(&mut p, 7, "Accents and ghost notes");
    for i in 0..8 {
        let t = p.note(7 * BAR + i * EIGHTH, EIGHTH, 5, 0);
        t.tech.flags.insert(if i % 4 == 0 {
            TechFlags::ACCENT
        } else {
            TechFlags::GHOST
        });
    }
    p
}

fn root_and_fifth_bass() -> Part {
    // G C D G / Em C D G: root and fifth, then root, fifth, octave, fifth. Bass strings G D A E
    // from the top line.
    struct Shape {
        name: &'static str,
        root: (u8, u8),
        fifth: (u8, u8),
        octave: (u8, u8),
    }
    let g = Shape {
        name: "G",
        root: (3, 3),
        fifth: (2, 5),
        octave: (1, 5),
    };
    let c = Shape {
        name: "C",
        root: (2, 3),
        fifth: (1, 5),
        octave: (0, 5),
    };
    let d = Shape {
        name: "D",
        root: (2, 5),
        fifth: (1, 7),
        octave: (0, 7),
    };
    let em = Shape {
        name: "Em",
        root: (3, 0),
        fifth: (2, 2),
        octave: (1, 2),
    };
    let song = [&g, &c, &d, &g, &em, &c, &d, &g];
    let mut p = Part::new();
    for (bar, s) in song.iter().enumerate() {
        let t0 = bar as i64 * BAR;
        p.chords.push((t0, s.name));
        if bar == 0 {
            p.markers.push((0, "Root and fifth"));
        }
        if bar == 4 {
            p.markers.push((t0, "Root, fifth, octave, fifth"));
        }
        let line = if bar < 4 {
            [s.root, s.fifth, s.root, s.fifth]
        } else {
            [s.root, s.fifth, s.octave, s.fifth]
        };
        for (beat, (string, fret)) in line.into_iter().enumerate() {
            p.note(t0 + beat as i64 * PPQ, PPQ, string, fret);
        }
    }
    p
}

fn rolls_banjo() -> Part {
    // Three rolls over G, C and D7 and back to G, four bars each. The roll lists the strings
    // (1 to 5) a bar of eight eighths picks.
    let rolls: [(&'static str, [u8; 8]); 3] = [
        ("Forward roll", [3, 2, 1, 3, 2, 1, 3, 2]),
        ("Backward roll", [2, 1, 5, 2, 1, 5, 2, 1]),
        ("Alternating thumb roll", [3, 2, 5, 1, 3, 2, 5, 1]),
    ];
    let chords: [(&'static str, [i8; 5]); 4] = [
        ("G", G_BANJO),
        ("C", C_BANJO),
        ("D7", D7_BANJO),
        ("G", G_BANJO),
    ];
    let mut p = Part::new();
    for (r, (name, roll)) in rolls.iter().enumerate() {
        p.markers.push((r as i64 * 4 * BAR, *name));
        for (c, (chord, shape)) in chords.iter().enumerate() {
            let bar = r as i64 * 4 + c as i64;
            p.chord(bar * BAR, chord, shape);
            for (i, string) in roll.iter().enumerate() {
                let index = usize::from(*string) - 1;
                let fret = u8::try_from(shape[index]).unwrap_or(0);
                p.note(bar * BAR + i as i64 * EIGHTH, EIGHTH, index as u8, fret);
            }
        }
    }
    p
}

fn strums_ukulele() -> Part {
    // C Am F G twice: four down strokes a bar, then down and up in the usual eighth-note rhythm.
    let chords: [(&'static str, [i8; 4]); 4] =
        [("C", C_UKE), ("Am", AM_UKE), ("F", F_UKE), ("G", G_UKE)];
    let mut p = Part::new();
    for bar in 0..8_i64 {
        let (name, shape) = chords[bar as usize % 4];
        p.chord(bar * BAR, name, &shape);
        if bar == 0 {
            p.markers.push((0, "Four downs"));
        }
        if bar == 4 {
            p.markers.push((bar * BAR, "Down and up"));
        }
        if bar < 4 {
            for beat in 0..4 {
                p.strum(bar * BAR + beat * PPQ, PPQ, &shape, StrumDirection::Down);
            }
        } else {
            // Down, down-up, up-down-up: eighth positions 0, 2, 3, 5, 6, 7.
            for (pos, dir) in [
                (0, StrumDirection::Down),
                (2, StrumDirection::Down),
                (3, StrumDirection::Up),
                (5, StrumDirection::Up),
                (6, StrumDirection::Down),
                (7, StrumDirection::Up),
            ] {
                p.strum(bar * BAR + pos * EIGHTH, EIGHTH, &shape, dir);
            }
        }
    }
    p
}

fn changes_ukulele() -> Part {
    // C Am F G: a chord a bar, then two beats each, one chord change after another.
    let chords: [(&'static str, [i8; 4]); 4] =
        [("C", C_UKE), ("Am", AM_UKE), ("F", F_UKE), ("G", G_UKE)];
    let mut p = Part::new();
    for bar in 0..4_i64 {
        let (name, shape) = chords[bar as usize];
        p.chord(bar * BAR, name, &shape);
        p.strum(bar * BAR, 2 * PPQ, &shape, StrumDirection::Down);
        p.strum(bar * BAR + 2 * PPQ, 2 * PPQ, &shape, StrumDirection::Down);
    }
    p.markers.push((0, "A chord a bar"));
    p.markers.push((4 * BAR, "A chord every two beats"));
    for half in 0..8_i64 {
        let (name, shape) = chords[half as usize % 4];
        let t = 4 * BAR + half * 2 * PPQ;
        p.chord(t, name, &shape);
        p.strum(t, PPQ, &shape, StrumDirection::Down);
        p.strum(t + PPQ, PPQ, &shape, StrumDirection::Up);
    }
    p
}

fn tremolo_mandolin() -> Part {
    // Sixteenths on one note a bar (the strokes written out), then the same notes held with a
    // tremolo mark, on the strings G D A E from the bottom line up.
    let mut p = Part::new();
    let line: [(u8, u8); 8] = [
        (3, 0),
        (2, 0),
        (1, 0),
        (0, 0),
        (1, 2),
        (2, 2),
        (3, 2),
        (3, 0),
    ];
    p.markers.push((0, "Strokes written out"));
    for (bar, (string, fret)) in line.iter().enumerate() {
        for i in 0..16 {
            let t = p.note(bar as i64 * BAR + i * SIXTEENTH, SIXTEENTH, *string, *fret);
            if i % 4 == 0 {
                t.tech.flags.insert(TechFlags::ACCENT);
            }
        }
    }
    p.markers.push((8 * BAR, "Tremolo mark"));
    for (bar, (string, fret)) in line.iter().enumerate() {
        let t = p.note(8 * BAR + bar as i64 * BAR, BAR, *string, *fret);
        t.tech.tremolo_division = 16;
    }
    p
}

fn major_scale_mandolin() -> Part {
    // G major from G3 to D5, up and down: two notes a string in the first position.
    let mut p = Part::new();
    let up = [
        (3, 0),
        (3, 2),
        (3, 4),
        (3, 5),
        (2, 0),
        (2, 2),
        (2, 4),
        (2, 5),
        (1, 0),
        (1, 2),
        (1, 3),
        (1, 5),
    ];
    eighths(&up_and_down(&up), &mut p);
    p.markers.push((0, "G major, first position"));
    p
}

/// The exercises, in the order of the manifest.
pub(in crate::app_core) const EXERCISES: &[ExerciseDef] = &[
    ExerciseDef {
        name: "Guitar C Major Scale",
        instrument: "guitar6",
        tags: &["guitar", "scale", "open position", "C major"],
        trains: "The C major scale in open position from C3 to G4 and back, in eighth notes at a \
                 slow tempo: one finger a fret, even picking, the notes ringing into each other.",
        bpm: 80.0,
        part: major_scale_guitar,
        sheet: "",
    },
    ExerciseDef {
        name: "Guitar Minor Pentatonic Box",
        instrument: "guitar6",
        tags: &["guitar", "scale", "pentatonic", "A minor", "box"],
        trains: "The first box of the A minor pentatonic scale, frets 5 to 8, up and down in \
                 eighth notes. Loop it, then raise the practice speed a step at a time.",
        bpm: 72.0,
        part: minor_pentatonic_guitar,
        sheet: "",
    },
    ExerciseDef {
        name: "Guitar Fingerpicking Arpeggios",
        instrument: "guitar6",
        tags: &["guitar", "arpeggio", "fingerpicking", "chords"],
        trains: "A bass note and the strings above it, eight eighth notes to a bar, over Am, C, \
                 G and Em, twice. Thumb on the bass strings, fingers on the three top ones.",
        bpm: 76.0,
        part: fingerpicking_guitar,
        sheet: "{title: Fingerpicking Arpeggios}\n{key: C}\n[Am]One two three four five \
                six seven eight\n[C]One two three four five six seven eight\n[G]One two \
                three four five six seven eight\n[Em]One two three four five six seven \
                eight\n",
    },
    ExerciseDef {
        name: "Guitar Chord Changes G C D Em",
        instrument: "guitar6",
        tags: &["guitar", "chords", "chord change", "strumming", "drill"],
        trains: "Four open chords, G, C, D and Em: one strum a bar, then four. Look at the \
                 diagram, change on the beat, and loop the bars you miss.",
        bpm: 70.0,
        part: changes_guitar,
        sheet: "{title: Chord Changes G C D Em}\n{key: G}\n[G]One two three four [C]five six \
                seven eight\n[D]One two three four [Em]five six seven eight\n",
    },
    ExerciseDef {
        name: "Guitar Technique Sampler",
        instrument: "guitar6",
        tags: &[
            "guitar",
            "technique",
            "palm mute",
            "bend",
            "slide",
            "harmonic",
        ],
        trains: "Eight bars, one technique each: palm mutes, dead notes, hammer-ons and \
                 pull-offs, slides, a bend and a vibrato, natural harmonics, let ring, and \
                 accents against ghost notes. Plucked Strings plays each of them.",
        bpm: 90.0,
        part: technique_sampler_guitar,
        sheet: "",
    },
    ExerciseDef {
        name: "Bass Root and Fifth Lines",
        instrument: "bass4",
        tags: &["bass", "root", "fifth", "octave", "line"],
        trains: "Root and fifth over G, C, D and G, then Em, C, D and G with the octave in the \
                 line: the shapes under one hand, no looking at the neck.",
        bpm: 84.0,
        part: root_and_fifth_bass,
        sheet: "{title: Root and Fifth Lines}\n{key: G}\n[G]One two [C]three four [D]five six \
                [G]seven eight\n[Em]One two [C]three four [D]five six [G]seven eight\n",
    },
    ExerciseDef {
        name: "Banjo Rolls",
        instrument: "banjo5",
        tags: &["banjo", "roll", "forward roll", "backward roll", "open G"],
        trains: "Three rolls, four bars each, over G, C, D7 and G in open G tuning: the forward \
                 roll, the backward roll and the alternating thumb roll. The short fifth string \
                 is on the bottom line.",
        bpm: 88.0,
        part: rolls_banjo,
        sheet: "{title: Banjo Rolls}\n{key: G}\n[G]One two three four [C]five six seven eight\n\
                [D7]One two three four [G]five six seven eight\n",
    },
    ExerciseDef {
        name: "Ukulele Strum Patterns",
        instrument: "ukulele_soprano",
        tags: &["ukulele", "strum", "pattern", "chords"],
        trains: "C, Am, F and G twice: four down strokes a bar, then the down-and-up pattern. \
                 The stroke direction is marked on every note.",
        bpm: 84.0,
        part: strums_ukulele,
        sheet: "{title: Strum Patterns}\n{key: C}\n[C]One two three four [Am]five six seven \
                eight\n[F]One two three four [G]five six seven eight\n",
    },
    ExerciseDef {
        name: "Ukulele Chord Changes C Am F G",
        instrument: "ukulele_soprano",
        tags: &["ukulele", "chords", "chord change", "drill"],
        trains: "Four chords you can play a whole evening on, a bar each, then a change every \
                 two beats. Keep the strum going while the fingers move.",
        bpm: 68.0,
        part: changes_ukulele,
        sheet: "{title: Chord Changes C Am F G}\n{key: C}\n[C]One two [Am]three four [F]five \
                six [G]seven eight\n",
    },
    ExerciseDef {
        name: "Mandolin Tremolo Study",
        instrument: "mandolin",
        tags: &["mandolin", "tremolo", "picking", "study"],
        trains: "Sixteenth notes on one note a bar with the downbeat accented, then the same \
                 notes as long tremolo notes. Pick from the wrist and let the pairs of strings \
                 ring.",
        bpm: 72.0,
        part: tremolo_mandolin,
        sheet: "",
    },
    ExerciseDef {
        name: "Mandolin G Major Scale",
        instrument: "mandolin",
        tags: &["mandolin", "scale", "G major", "first position"],
        trains: "G major from G3 to D5 and back in eighth notes: two notes to a string, down \
                 and up strokes alternating.",
        bpm: 80.0,
        part: major_scale_mandolin,
        sheet: "",
    },
];

// --------------------------------------------------------------------------------- the files

/// Where an exercise lives in the factory folder, relative to it.
pub(in crate::app_core) fn exercise_path(name: &str) -> String {
    format!("Projects/Exercises/{name}/{name}.atomik")
}

/// The tab score of an exercise: one track on the default tuning of its instrument.
pub(in crate::app_core) fn exercise_score(def: &ExerciseDef) -> TabScore {
    let part = (def.part)();
    let instrument = find_instrument(def.instrument).expect("a catalogue instrument");
    let tuning: Tuning = instrument.default_tuning().tuning.clone();
    let mut track = TabTrack::new(instrument.name, def.instrument, tuning);
    track.midi_program = match instrument.family {
        atomik_tab::instrument::Family::Bass => 33,
        atomik_tab::instrument::Family::Banjo => 105,
        _ => 25,
    };
    track.notes = part.notes;
    track.chords = part
        .chords
        .iter()
        .map(|(tick, name)| (*tick, ChordSymbol::parse(name).expect("a chord name")))
        .collect();
    track.diagrams = part
        .diagrams
        .iter()
        .map(|(tick, name, shape)| DiagramRef {
            tick: *tick,
            name: (*name).to_owned(),
            voicing: voicing(shape),
        })
        .collect();
    let mut score = TabScore::new();
    score.title = def.name.to_owned();
    score.tab_by = AUTHOR.to_owned();
    score.tempo = vec![(0, def.bpm)];
    score.markers = part
        .markers
        .iter()
        .map(|(t, n)| (*t, (*n).to_owned()))
        .collect();
    score.tracks = vec![track];
    score
}

/// Builds the project text of an exercise. `root` is the factory folder whose `Patches` hold the
/// Plucked Strings patches the track loads.
///
/// # Panics
/// The exercise cannot be imported or saved (a mistake in the table).
pub(in crate::app_core) fn build_exercise(def: &ExerciseDef, root: &Path) -> String {
    let mut core = AppCore::with_registry(
        RecordingSink::new(),
        default_registry(),
        new_project_doc(),
        None,
    );
    core.set_factory_root(Some(root.to_path_buf()));
    core.apply_tab(&exercise_score(def), Vec::new(), true, def.name)
        .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    if !def.sheet.is_empty() {
        let parsed = parse_sheet(def.sheet).unwrap_or_else(|e| panic!("{}: {e}", def.name));
        let sheets =
            atomik_model::chord_sheets::with_sheet(&[], parsed.sheet).expect("room for one sheet");
        core.apply_command(Command::Song(SongCommand::SetSheets(sheets)))
            .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    }
    core.apply_command(Command::SetSongInfo(SongInfo {
        title: def.name.to_owned(),
        author: AUTHOR.to_owned(),
        notes: format!(
            "{} Open the clip in the Tab Editor (View > Editors > Tab Editor), loop a few bars \
             and slow it down with the practice bar.",
            def.trains
        ),
        ..SongInfo::default()
    }))
    .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    save_project_checked(core.doc()).unwrap_or_else(|e| panic!("{}: {e}", def.name))
}

/// Writes every exercise below `root/Projects/Exercises` (the folder is made fresh).
///
/// # Panics
/// A file cannot be written or an exercise cannot be built.
pub(in crate::app_core) fn write_exercises(root: &Path) {
    let folder = root.join("Projects").join("Exercises");
    let _ = std::fs::remove_dir_all(&folder);
    for def in EXERCISES {
        let text = build_exercise(def, root);
        let file = root.join(exercise_path(def.name));
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("a project folder");
        std::fs::write(&file, text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    }
}
