//! The genre templates of the factory library (test code): six songs as tables, and the writer
//! that turns each into a project `Templates/<name>/<name>.atomik`, exactly as Save As writes one
//! (no audio folder: a template holds none). See `app_core_factory_build.rs` for how the tables
//! become files.
//!
//! A template is a song to start from, not a finished one. It is made only of the program's own
//! devices and factory content: the drums are the drum machine with the synthesised pads of a
//! factory kit, the other tracks play factory patches, the notes are factory clips (a drum clip
//! feeds the kick track with its kick and the drums track with the rest) plus a few short
//! patterns of its own, and the two send effects are factory patches of the space reverb and the
//! echo. Every template has:
//!
//! * a tempo, a song key and eight sections on an 8-bar grid, 64 bars in all, as ranges (Intro,
//!   Verse or Build, Chorus or Drop, Break, Outro);
//! * six named, coloured tracks (kick, drums, bass, chords, lead and a texture) with clips that
//!   play from the first bar to the last, some sections left sparse for the user to fill;
//! * a launcher with four or five scenes holding the main loops;
//! * a light mix: every channel near -18 dBFS RMS, the strip EQ on the bass, the bass ducked from
//!   the kick by the quick sidechain, a send to the reverb and the echo, a glue compressor and a
//!   limiter at -1 dBTP on the master, set to land near -14 LUFS;
//! * the song information: the title, a note on how it is built and what to try, the author.

use super::clips::{ClipDef, Pattern, clip_of};
use super::examples::{Build, Keys, Source, Voice, bar_tick, clip_named, kick, place, rest, slug};
use super::{COMPRESSOR, DEFS, MASTERING, set_params};
use atomik_core::theory::ScaleKind;
use atomik_model::console::CHANNEL_TYPE;
use atomik_model::{
    ClipContent, Command, ConsoleCommand, DeviceKey, Document, Harmony, InsertEffect, Key,
    LauncherClip, LauncherCommand, RangeMarker, SongCommand, SongInfo, WorkflowCommand,
    new_project_doc, save_project_checked,
};
use atomik_render::default_registry;
use std::path::Path;

/// The author of every template.
pub(in crate::app_core) const TEMPLATE_AUTHOR: &str = "Atomik Rack factory";

/// A track of a template: its colour and what plays on it.
pub(in crate::app_core) struct Track {
    pub color: [u8; 3],
    /// The channel's input gain in dB: with the fader (`voice.level`) it sets the level of the
    /// track, so that every track sounds near -18 dBFS RMS with the fader at or below unity.
    pub trim: f32,
    pub voice: Voice,
}

/// One scene of the launcher: a name, a colour and `(track, clip, notes)` for every slot.
pub(in crate::app_core) struct SceneDef {
    pub name: &'static str,
    pub color: [u8; 3],
    pub slots: &'static [(&'static str, &'static str, Keys)],
}

/// One template.
pub(in crate::app_core) struct TemplateDef {
    /// The name (the folder and file name, and the song title).
    pub name: &'static str,
    /// The pitch class of the song key (A is 9) and its scale.
    pub key: (u8, ScaleKind),
    pub bpm: f64,
    /// The sections in order, `(name, bars)`: they add up to 64 bars.
    pub sections: &'static [(&'static str, i64)],
    /// The tracks the sidechain is made between: the bass ducks from the kick.
    pub kick: &'static str,
    pub bass: &'static str,
    /// The factory patches of the two send effects: the space reverb and the echo.
    pub sends: (&'static str, &'static str),
    /// The bar where the 16 bars start that the levels and the loudness are judged on: the
    /// second half of them has every track playing.
    pub loud: i64,
    /// The limiter's input gain in dB: the one number that sets the loudness of the master.
    pub master_input: f32,
    /// How it is built, and what to try (the notes of the song).
    pub built: &'static str,
    pub try_it: &'static str,
    /// The patterns of its own, besides the factory clips.
    pub clips: &'static [ClipDef],
    pub tracks: &'static [Track],
    pub scenes: &'static [SceneDef],
}

/// The bars of every template.
pub(in crate::app_core) const BARS: i64 = 64;

/// The strip EQ of the bass: a little weight below, less mud and a softer top.
const BASS_EQ: &[(&str, f32)] = &[
    ("eq.on", 1.0),
    ("eq.lf.freq", 60.0),
    ("eq.lf.gain", 1.5),
    ("eq.lmf.freq", 260.0),
    ("eq.lmf.gain", -3.0),
    ("eq.hf.freq", 4500.0),
    ("eq.hf.gain", -4.0),
];

/// The colour of a section, by its name.
fn section_color(name: &str) -> [u8; 3] {
    match name {
        "Intro" | "Outro" => [96, 125, 160],
        "Verse" | "Build" => [88, 150, 120],
        "Chorus" | "Drop" | "Swell" => [200, 120, 80],
        _ => [150, 110, 170],
    }
}

const KICK_COLOR: [u8; 3] = [214, 96, 84];
const DRUMS_COLOR: [u8; 3] = [226, 150, 70];
const BASS_COLOR: [u8; 3] = [70, 130, 200];
const CHORDS_COLOR: [u8; 3] = [112, 170, 110];
const LEAD_COLOR: [u8; 3] = [200, 110, 170];
const FX_COLOR: [u8; 3] = [130, 140, 150];

const SCENE_A: [u8; 3] = [96, 125, 160];
const SCENE_B: [u8; 3] = [88, 150, 120];
const SCENE_C: [u8; 3] = [200, 120, 80];
const SCENE_D: [u8; 3] = [150, 110, 170];

// ------------------------------------------------------------------------------------ the songs

pub(in crate::app_core) const TEMPLATES: &[TemplateDef] = &[
    TemplateDef {
        name: "Hip-Hop Boom Bap",
        key: (9, ScaleKind::NaturalMinor),
        bpm: 88.0,
        sections: &[
            ("Intro", 8),
            ("Verse", 16),
            ("Chorus", 8),
            ("Break", 8),
            ("Chorus", 16),
            ("Outro", 8),
        ],
        kick: "Kick",
        bass: "Sub",
        sends: ("Tight Chamber", "Tape Repeat"),
        loud: 17,
        master_input: 9.6,
        built: "A laid-back boom-bap beat in A minor at 88 BPM: a warm, crushed drum kit with the \
                kick on its own track, a round sub that ducks from the kick, jazzy seventh \
                chords, a mallet hook and a dusty texture, with a small room and a tape echo.",
        try_it: "The verses have no lead: write a melody or a vocal there. Mute the kick track to \
                 hear the bass breathe back in, and launch scenes from the launcher to try the \
                 loops in a different order.",
        clips: &[
            ClipDef {
                name: "Boom Bap Hook",
                category: "Lead",
                tags: &[],
                sound: "A pentatonic hook in A minor, four bars.",
                bpm: 88.0,
                bars: 4,
                pattern: Pattern::Line {
                    step: 0.5,
                    gate: 0.9,
                    keys: &[
                        69, 0, 72, 0, 74, 0, 72, 69, 0, 0, 67, 0, 69, 0, 0, 0, 72, 0, 74, 0, 76, 0,
                        74, 72, 0, 0, 69, 0, 67, 0, 0, 0,
                    ],
                    accents: "Xxxx",
                },
            },
            ClipDef {
                name: "Dust Holds",
                category: "Texture",
                tags: &[],
                sound: "Two long notes for the texture to colour.",
                bpm: 88.0,
                bars: 4,
                pattern: Pattern::Chords {
                    velocity: 0.5,
                    changes: &[(0.0, 8.0, &[69]), (8.0, 8.0, &[67])],
                },
            },
        ],
        tracks: &[
            Track {
                color: KICK_COLOR,
                trim: 4.8,
                voice: Voice {
                    name: "Kick",
                    source: Source::Pads("Cassette Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        kick("Boom Bap Swing", 9, 24),
                        kick("Boom Bap Swing", 41, 16),
                    ],
                },
            },
            Track {
                color: DRUMS_COLOR,
                trim: 2.8,
                voice: Voice {
                    name: "Drums",
                    source: Source::Pads("Cassette Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &["Drum Punch"],
                    sends: &[(1, -22.0)],
                    clips: &[
                        rest("Boom Bap Swing", 5, 4),
                        rest("Boom Bap Swing", 9, 24),
                        rest("Boom Bap Swing", 39, 2),
                        rest("Boom Bap Swing", 41, 16),
                        rest("Boom Bap Swing", 57, 4),
                    ],
                },
            },
            Track {
                color: BASS_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Sub",
                    source: Source::Patch("Basalt Sub"),
                    level: -5.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        place("Dub Sub Roots", 9, 24),
                        place("Dub Sub Roots", 41, 8),
                        place("Slow Fifths", 49, 8),
                    ],
                },
            },
            Track {
                color: CHORDS_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Chords",
                    source: Source::Patch("Dusk Clavier"),
                    level: -3.7,
                    pan: -0.2,
                    inserts: &["Tape Warmth"],
                    sends: &[(1, -14.0)],
                    clips: &[place("Seventh Changes", 1, 64)],
                },
            },
            Track {
                color: LEAD_COLOR,
                trim: 10.4,
                voice: Voice {
                    name: "Lead",
                    source: Source::Patch("Pebble Mallet"),
                    level: 0.0,
                    pan: 0.25,
                    inserts: &[],
                    sends: &[(1, -10.0), (2, -9.0)],
                    clips: &[
                        place("Boom Bap Hook", 25, 8),
                        place("Boom Bap Hook", 37, 4),
                        place("Boom Bap Hook", 41, 16),
                    ],
                },
            },
            Track {
                color: FX_COLOR,
                trim: 13.2,
                voice: Voice {
                    name: "Dust",
                    source: Source::Patch("Scatter Dust"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -8.0)],
                    clips: &[place("Dust Holds", 1, 64)],
                },
            },
        ],
        scenes: &[
            SceneDef {
                name: "Intro",
                color: SCENE_A,
                slots: &[
                    ("Drums", "Boom Bap Swing", Keys::Rest),
                    ("Chords", "Seventh Changes", Keys::All),
                    ("Dust", "Dust Holds", Keys::All),
                ],
            },
            SceneDef {
                name: "Groove",
                color: SCENE_B,
                slots: &[
                    ("Kick", "Boom Bap Swing", Keys::Kick),
                    ("Drums", "Boom Bap Swing", Keys::Rest),
                    ("Sub", "Dub Sub Roots", Keys::All),
                    ("Chords", "Seventh Changes", Keys::All),
                ],
            },
            SceneDef {
                name: "Hook",
                color: SCENE_C,
                slots: &[
                    ("Kick", "Boom Bap Swing", Keys::Kick),
                    ("Drums", "Boom Bap Swing", Keys::Rest),
                    ("Sub", "Slow Fifths", Keys::All),
                    ("Chords", "Seventh Changes", Keys::All),
                    ("Lead", "Boom Bap Hook", Keys::All),
                ],
            },
            SceneDef {
                name: "Break",
                color: SCENE_D,
                slots: &[
                    ("Chords", "Seventh Changes", Keys::All),
                    ("Lead", "Boom Bap Hook", Keys::All),
                    ("Dust", "Dust Holds", Keys::All),
                ],
            },
        ],
    },
    TemplateDef {
        name: "House",
        key: (9, ScaleKind::NaturalMinor),
        bpm: 124.0,
        sections: &[
            ("Intro", 8),
            ("Build", 8),
            ("Drop", 16),
            ("Break", 8),
            ("Drop", 16),
            ("Outro", 8),
        ],
        kick: "Kick",
        bass: "Bass",
        sends: ("Tight Chamber", "Dotted Slap"),
        loud: 17,
        master_input: 0.2,
        built: "A four-on-the-floor house track in A minor at 124 BPM: a deep kick on its own \
                track with clap and hats on another, an octave bass that ducks from the kick, \
                a two-chord organ vamp, a running pentatonic lead and a shimmering texture, with \
                a tight chamber and a dotted echo.",
        try_it: "The first drop has no lead: add a hook there. Draw a filter sweep on the lead \
                 across the build, or move the break to change where the energy falls.",
        clips: &[ClipDef {
            name: "Shimmer Holds",
            category: "Texture",
            tags: &[],
            sound: "Two long high notes for the texture to colour.",
            bpm: 124.0,
            bars: 4,
            pattern: Pattern::Chords {
                velocity: 0.55,
                changes: &[(0.0, 8.0, &[81]), (8.0, 8.0, &[76])],
            },
        }],
        tracks: &[
            Track {
                color: KICK_COLOR,
                trim: 3.9,
                voice: Voice {
                    name: "Kick",
                    source: Source::Pads("Basalt Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        kick("Four On The Floor", 1, 32),
                        kick("Four On The Floor", 41, 24),
                    ],
                },
            },
            Track {
                color: DRUMS_COLOR,
                trim: 15.2,
                voice: Voice {
                    name: "Drums",
                    source: Source::Pads("Basalt Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -22.0)],
                    clips: &[
                        rest("Four On The Floor", 5, 28),
                        rest("Four On The Floor", 39, 26),
                    ],
                },
            },
            Track {
                color: BASS_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Bass",
                    source: Source::Patch("Mono Sub Bass"),
                    level: -1.9,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        place("Disco Octaves", 13, 4),
                        place("Disco Octaves", 17, 8),
                        place("Octave Pump", 25, 8),
                        place("Octave Pump", 41, 8),
                        place("Disco Octaves", 49, 8),
                    ],
                },
            },
            Track {
                color: CHORDS_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Chords",
                    source: Source::Patch("Combo Drive"),
                    level: -2.6,
                    pan: -0.2,
                    inserts: &[],
                    sends: &[(1, -14.0)],
                    clips: &[
                        place("Dorian Vamp", 9, 24),
                        place("Suspended Drift", 33, 8),
                        place("Dorian Vamp", 41, 16),
                        place("Dorian Vamp", 57, 4),
                    ],
                },
            },
            Track {
                color: LEAD_COLOR,
                trim: 7.1,
                voice: Voice {
                    name: "Lead",
                    source: Source::Patch("Copper Pluck"),
                    level: 0.0,
                    pan: 0.25,
                    inserts: &[],
                    sends: &[(1, -10.0), (2, -8.0)],
                    clips: &[
                        place("Fifths Bounce", 15, 2),
                        place("Pentatonic Run", 25, 8),
                        place("Pentatonic Run", 41, 16),
                    ],
                },
            },
            Track {
                color: FX_COLOR,
                trim: 3.2,
                voice: Voice {
                    name: "Shimmer",
                    source: Source::Patch("Fine Dust Shimmer"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -6.0)],
                    clips: &[place("Shimmer Holds", 1, 64)],
                },
            },
        ],
        scenes: &[
            SceneDef {
                name: "Intro",
                color: SCENE_A,
                slots: &[
                    ("Kick", "Four On The Floor", Keys::Kick),
                    ("Drums", "Four On The Floor", Keys::Rest),
                    ("Shimmer", "Shimmer Holds", Keys::All),
                ],
            },
            SceneDef {
                name: "Build",
                color: SCENE_B,
                slots: &[
                    ("Kick", "Four On The Floor", Keys::Kick),
                    ("Drums", "Four On The Floor", Keys::Rest),
                    ("Chords", "Dorian Vamp", Keys::All),
                    ("Lead", "Fifths Bounce", Keys::All),
                ],
            },
            SceneDef {
                name: "Drop",
                color: SCENE_C,
                slots: &[
                    ("Kick", "Four On The Floor", Keys::Kick),
                    ("Drums", "Four On The Floor", Keys::Rest),
                    ("Bass", "Octave Pump", Keys::All),
                    ("Chords", "Dorian Vamp", Keys::All),
                    ("Lead", "Pentatonic Run", Keys::All),
                ],
            },
            SceneDef {
                name: "Break",
                color: SCENE_D,
                slots: &[
                    ("Chords", "Suspended Drift", Keys::All),
                    ("Shimmer", "Shimmer Holds", Keys::All),
                ],
            },
        ],
    },
    TemplateDef {
        name: "Techno",
        key: (9, ScaleKind::NaturalMinor),
        bpm: 132.0,
        sections: &[
            ("Intro", 8),
            ("Build", 16),
            ("Drop", 16),
            ("Break", 8),
            ("Drop", 8),
            ("Outro", 8),
        ],
        kick: "Kick",
        bass: "Acid Bass",
        sends: ("Tight Chamber", "Dotted Slap"),
        loud: 25,
        master_input: 1.5,
        built: "A driving techno track in A minor at 132 BPM: a long punchy kick on its own track \
                with clap and hats on another, an acid bass that ducks from the kick, off-beat \
                minor stabs, a modular pulse and a bouncing fifths lead, with a tight chamber \
                and a dotted echo.",
        try_it: "The intro, the break and the outro are bare: fill them. Open the pulse device \
                 and change its sequence, or automate the acid bass cutoff across the second \
                 drop.",
        clips: &[
            ClipDef {
                name: "Minor Stabs",
                category: "Chords",
                tags: &[],
                sound: "Short A minor and F stabs on the off-beats, two bars.",
                bpm: 132.0,
                bars: 2,
                pattern: Pattern::Chords {
                    velocity: 0.78,
                    changes: &[
                        (0.5, 0.5, &[57, 60, 64]),
                        (1.5, 0.5, &[57, 60, 64]),
                        (2.5, 0.5, &[57, 60, 64]),
                        (3.5, 0.5, &[57, 60, 64]),
                        (4.5, 0.5, &[53, 57, 60]),
                        (5.5, 0.5, &[53, 57, 60]),
                        (6.5, 0.5, &[53, 57, 60]),
                        (7.5, 0.5, &[53, 57, 60]),
                    ],
                },
            },
            ClipDef {
                name: "Pulse Roots",
                category: "Bass",
                tags: &[],
                sound: "One root per bar: A, F, C and G, held.",
                bpm: 132.0,
                bars: 4,
                pattern: Pattern::Chords {
                    velocity: 0.8,
                    changes: &[
                        (0.0, 4.0, &[45]),
                        (4.0, 4.0, &[41]),
                        (8.0, 4.0, &[48]),
                        (12.0, 4.0, &[43]),
                    ],
                },
            },
        ],
        tracks: &[
            Track {
                color: KICK_COLOR,
                trim: 3.7,
                voice: Voice {
                    name: "Kick",
                    source: Source::Pads("Circuit Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[kick("Techno Drive", 1, 40), kick("Techno Drive", 49, 16)],
                },
            },
            Track {
                color: DRUMS_COLOR,
                trim: 0.5,
                voice: Voice {
                    name: "Drums",
                    source: Source::Pads("Circuit Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &["Drum Punch"],
                    sends: &[(1, -22.0)],
                    clips: &[
                        rest("Techno Drive", 5, 36),
                        rest("Techno Drive", 47, 2),
                        rest("Techno Drive", 49, 12),
                    ],
                },
            },
            Track {
                color: BASS_COLOR,
                trim: 9.5,
                voice: Voice {
                    name: "Acid Bass",
                    source: Source::Patch("Acid Squelch"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        place("Two Note Rocker", 9, 8),
                        place("Acid Climb", 17, 24),
                        place("Acid Climb", 49, 8),
                    ],
                },
            },
            Track {
                color: CHORDS_COLOR,
                trim: 4.9,
                voice: Voice {
                    name: "Stabs",
                    source: Source::Patch("Neon Stab"),
                    level: 0.0,
                    pan: -0.2,
                    inserts: &[],
                    sends: &[(1, -14.0), (2, -12.0)],
                    clips: &[
                        place("Minor Stabs", 17, 24),
                        place("Suspended Drift", 41, 8),
                        place("Minor Stabs", 49, 8),
                    ],
                },
            },
            Track {
                color: LEAD_COLOR,
                trim: 8.5,
                voice: Voice {
                    name: "Lead",
                    source: Source::Patch("Sync Razor"),
                    level: 0.0,
                    pan: 0.25,
                    inserts: &[],
                    sends: &[(1, -12.0), (2, -9.0)],
                    clips: &[
                        place("Fifths Bounce", 9, 16),
                        place("Pentatonic Run", 25, 16),
                        place("Pentatonic Run", 49, 8),
                    ],
                },
            },
            Track {
                color: FX_COLOR,
                trim: 2.9,
                voice: Voice {
                    name: "Pulse",
                    source: Source::Patch("Dark Sequence"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -8.0)],
                    clips: &[
                        place("Pulse Roots", 9, 32),
                        place("Pulse Roots", 41, 8),
                        place("Pulse Roots", 49, 8),
                    ],
                },
            },
        ],
        scenes: &[
            SceneDef {
                name: "Intro",
                color: SCENE_A,
                slots: &[
                    ("Kick", "Techno Drive", Keys::Kick),
                    ("Drums", "Techno Drive", Keys::Rest),
                ],
            },
            SceneDef {
                name: "Build",
                color: SCENE_B,
                slots: &[
                    ("Kick", "Techno Drive", Keys::Kick),
                    ("Drums", "Techno Drive", Keys::Rest),
                    ("Acid Bass", "Two Note Rocker", Keys::All),
                    ("Lead", "Fifths Bounce", Keys::All),
                    ("Pulse", "Pulse Roots", Keys::All),
                ],
            },
            SceneDef {
                name: "Drop",
                color: SCENE_C,
                slots: &[
                    ("Kick", "Techno Drive", Keys::Kick),
                    ("Drums", "Techno Drive", Keys::Rest),
                    ("Acid Bass", "Acid Climb", Keys::All),
                    ("Stabs", "Minor Stabs", Keys::All),
                    ("Lead", "Pentatonic Run", Keys::All),
                    ("Pulse", "Pulse Roots", Keys::All),
                ],
            },
            SceneDef {
                name: "Break",
                color: SCENE_D,
                slots: &[
                    ("Stabs", "Suspended Drift", Keys::All),
                    ("Pulse", "Pulse Roots", Keys::All),
                ],
            },
        ],
    },
    TemplateDef {
        name: "Synthwave",
        key: (9, ScaleKind::NaturalMinor),
        bpm: 108.0,
        sections: &[
            ("Intro", 8),
            ("Verse", 16),
            ("Chorus", 8),
            ("Break", 8),
            ("Chorus", 16),
            ("Outro", 8),
        ],
        kick: "Kick",
        bass: "Bass",
        sends: ("Frozen Hall", "Dotted Slap"),
        loud: 17,
        master_input: 1.0,
        built: "A night-drive synthwave track in A minor at 108 BPM: bright tonal drums with the \
                kick on its own track, a pumping octave bass that ducks from the kick, the \
                four-chord loop on a wide string pad, a plucked arpeggio and a neon hook, with \
                a big hall and a dotted echo.",
        try_it: "The first verse has no arpeggio: bring it in earlier or later. Swap the pad for \
                 another factory patch, or add a second hook in the break.",
        clips: &[
            ClipDef {
                name: "Neon Backbeat",
                category: "Drums",
                tags: &[],
                sound: "A kick that pushes the third beat, a snare on two and four, eighth hats \
                        and an open hat before each bar, two bars.",
                bpm: 108.0,
                bars: 2,
                pattern: Pattern::Drums {
                    step: 0.25,
                    lanes: &[
                        (36, "X.......X..x....X.......X..x...."),
                        (38, "....X.......X.......X.......X..."),
                        (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x."),
                        (46, "..............x...............x."),
                    ],
                },
            },
            ClipDef {
                name: "Neon Hook",
                category: "Lead",
                tags: &[],
                sound: "A singing hook over A minor, F, C and G, four bars.",
                bpm: 108.0,
                bars: 4,
                pattern: Pattern::Line {
                    step: 0.5,
                    gate: 0.95,
                    keys: &[
                        69, 0, 72, 0, 76, 0, 72, 0, 77, 0, 72, 0, 69, 0, 0, 0, 72, 0, 76, 0, 79, 0,
                        76, 72, 74, 0, 71, 0, 67, 0, 0, 0,
                    ],
                    accents: "Xxxx",
                },
            },
            ClipDef {
                name: "Pad Roots",
                category: "Texture",
                tags: &[],
                sound: "One high root per bar: A, F, C and G, held.",
                bpm: 108.0,
                bars: 4,
                pattern: Pattern::Chords {
                    velocity: 0.6,
                    changes: &[
                        (0.0, 4.0, &[69]),
                        (4.0, 4.0, &[65]),
                        (8.0, 4.0, &[72]),
                        (12.0, 4.0, &[67]),
                    ],
                },
            },
        ],
        tracks: &[
            Track {
                color: KICK_COLOR,
                trim: 4.5,
                voice: Voice {
                    name: "Kick",
                    source: Source::Pads("Glass Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        kick("Neon Backbeat", 9, 16),
                        kick("Four On The Floor", 25, 8),
                        kick("Four On The Floor", 41, 16),
                        kick("Neon Backbeat", 57, 4),
                    ],
                },
            },
            Track {
                color: DRUMS_COLOR,
                trim: 11.4,
                voice: Voice {
                    name: "Drums",
                    source: Source::Pads("Glass Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -18.0)],
                    clips: &[
                        rest("Neon Backbeat", 9, 16),
                        rest("Four On The Floor", 25, 8),
                        rest("Neon Backbeat", 39, 2),
                        rest("Four On The Floor", 41, 16),
                        rest("Neon Backbeat", 57, 8),
                    ],
                },
            },
            Track {
                color: BASS_COLOR,
                trim: 1.7,
                voice: Voice {
                    name: "Bass",
                    source: Source::Patch("Iron Bass"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        place("Octave Pump", 9, 16),
                        place("Disco Octaves", 25, 8),
                        place("Octave Pump", 41, 16),
                    ],
                },
            },
            Track {
                color: CHORDS_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Pad",
                    source: Source::Patch("Tidewater Strings"),
                    level: -1.8,
                    pan: -0.2,
                    inserts: &[],
                    sends: &[(1, -10.0)],
                    clips: &[place("Minor Four Chords", 1, 64)],
                },
            },
            Track {
                color: LEAD_COLOR,
                trim: 12.7,
                voice: Voice {
                    name: "Lead",
                    source: Source::Patch("Copper Pluck"),
                    level: 0.0,
                    pan: 0.25,
                    inserts: &[],
                    sends: &[(1, -10.0), (2, -8.0)],
                    clips: &[
                        place("Wide Minor Arp", 5, 4),
                        place("Wide Minor Arp", 17, 8),
                        place("Neon Hook", 25, 8),
                        place("Wide Minor Arp", 33, 8),
                        place("Wide Minor Arp", 41, 8),
                        place("Neon Hook", 49, 8),
                    ],
                },
            },
            Track {
                color: FX_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Haze",
                    source: Source::Patch("Slow Tide"),
                    level: -1.8,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -6.0)],
                    clips: &[place("Pad Roots", 1, 64)],
                },
            },
        ],
        scenes: &[
            SceneDef {
                name: "Intro",
                color: SCENE_A,
                slots: &[
                    ("Pad", "Minor Four Chords", Keys::All),
                    ("Lead", "Wide Minor Arp", Keys::All),
                    ("Haze", "Pad Roots", Keys::All),
                ],
            },
            SceneDef {
                name: "Verse",
                color: SCENE_B,
                slots: &[
                    ("Kick", "Neon Backbeat", Keys::Kick),
                    ("Drums", "Neon Backbeat", Keys::Rest),
                    ("Bass", "Octave Pump", Keys::All),
                    ("Pad", "Minor Four Chords", Keys::All),
                ],
            },
            SceneDef {
                name: "Chorus",
                color: SCENE_C,
                slots: &[
                    ("Kick", "Four On The Floor", Keys::Kick),
                    ("Drums", "Four On The Floor", Keys::Rest),
                    ("Bass", "Disco Octaves", Keys::All),
                    ("Pad", "Minor Four Chords", Keys::All),
                    ("Lead", "Neon Hook", Keys::All),
                ],
            },
            SceneDef {
                name: "Break",
                color: SCENE_D,
                slots: &[
                    ("Pad", "Minor Four Chords", Keys::All),
                    ("Lead", "Wide Minor Arp", Keys::All),
                    ("Haze", "Pad Roots", Keys::All),
                ],
            },
        ],
    },
    TemplateDef {
        name: "Lo-fi",
        key: (9, ScaleKind::NaturalMinor),
        bpm: 82.0,
        sections: &[
            ("Intro", 8),
            ("Verse", 16),
            ("Chorus", 8),
            ("Break", 8),
            ("Verse", 16),
            ("Outro", 8),
        ],
        kick: "Kick",
        bass: "Bass",
        sends: ("Tight Chamber", "Tape Repeat"),
        loud: 17,
        master_input: 3.1,
        built: "A slow, dusty lo-fi loop in A minor at 82 BPM: a crushed drum kit with the kick on \
                its own track, a soft plucked bass that ducks from the kick, warm seventh chords \
                through tape warmth, a mellow tine arpeggio and a radio-dust texture, with a \
                small chamber and a tape echo.",
        try_it: "The first verse has no melody: play one over the chords. Turn the tape warmth \
                 up on the chords, or lengthen the break and let the arpeggio carry it.",
        clips: &[
            ClipDef {
                name: "Lazy Melody",
                category: "Lead",
                tags: &[],
                sound: "A slow, sparse melody in A minor, four bars.",
                bpm: 82.0,
                bars: 4,
                pattern: Pattern::Line {
                    step: 0.5,
                    gate: 0.9,
                    keys: &[
                        72, 0, 0, 71, 0, 69, 0, 0, 0, 0, 67, 0, 69, 0, 0, 0, 72, 0, 0, 74, 0, 72,
                        0, 69, 0, 0, 67, 0, 64, 0, 0, 0,
                    ],
                    accents: "Xxxx",
                },
            },
            ClipDef {
                name: "Dust Holds",
                category: "Texture",
                tags: &[],
                sound: "Two long notes for the texture to colour.",
                bpm: 82.0,
                bars: 4,
                pattern: Pattern::Chords {
                    velocity: 0.5,
                    changes: &[(0.0, 8.0, &[69]), (8.0, 8.0, &[67])],
                },
            },
        ],
        tracks: &[
            Track {
                color: KICK_COLOR,
                trim: 4.8,
                voice: Voice {
                    name: "Kick",
                    source: Source::Pads("Dust Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        kick("Boom Bap Swing", 9, 24),
                        kick("Boom Bap Swing", 41, 16),
                    ],
                },
            },
            Track {
                color: DRUMS_COLOR,
                trim: 12.4,
                voice: Voice {
                    name: "Drums",
                    source: Source::Pads("Dust Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -20.0)],
                    clips: &[
                        rest("Boom Bap Swing", 5, 4),
                        rest("Boom Bap Swing", 9, 24),
                        rest("Boom Bap Swing", 39, 2),
                        rest("Boom Bap Swing", 41, 16),
                        rest("Boom Bap Swing", 57, 4),
                    ],
                },
            },
            Track {
                color: BASS_COLOR,
                trim: 14.5,
                voice: Voice {
                    name: "Bass",
                    source: Source::Patch("Hollow Pluck Bass"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        place("Slow Fifths", 9, 16),
                        place("Walking Minor", 25, 8),
                        place("Slow Fifths", 41, 16),
                    ],
                },
            },
            Track {
                color: CHORDS_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Chords",
                    source: Source::Patch("Tine Hush"),
                    level: -4.7,
                    pan: -0.2,
                    inserts: &["Tape Warmth"],
                    sends: &[(1, -14.0)],
                    clips: &[place("Warm Minor Sevenths", 1, 64)],
                },
            },
            Track {
                color: LEAD_COLOR,
                trim: 10.4,
                voice: Voice {
                    name: "Lead",
                    source: Source::Patch("Soft Marimba"),
                    level: 0.0,
                    pan: 0.25,
                    inserts: &[],
                    sends: &[(1, -10.0), (2, -9.0)],
                    clips: &[
                        place("Descending Sevenths", 17, 8),
                        place("Lazy Melody", 25, 8),
                        place("Descending Sevenths", 33, 8),
                        place("Descending Sevenths", 41, 8),
                        place("Lazy Melody", 49, 8),
                    ],
                },
            },
            Track {
                color: FX_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Dust",
                    source: Source::Patch("Slow Spray"),
                    level: -0.2,
                    pan: 0.0,
                    inserts: &["Radio Dust"],
                    sends: &[(1, -8.0)],
                    clips: &[place("Dust Holds", 1, 64)],
                },
            },
        ],
        scenes: &[
            SceneDef {
                name: "Intro",
                color: SCENE_A,
                slots: &[
                    ("Drums", "Boom Bap Swing", Keys::Rest),
                    ("Chords", "Warm Minor Sevenths", Keys::All),
                    ("Dust", "Dust Holds", Keys::All),
                ],
            },
            SceneDef {
                name: "Groove",
                color: SCENE_B,
                slots: &[
                    ("Kick", "Boom Bap Swing", Keys::Kick),
                    ("Drums", "Boom Bap Swing", Keys::Rest),
                    ("Bass", "Slow Fifths", Keys::All),
                    ("Chords", "Warm Minor Sevenths", Keys::All),
                ],
            },
            SceneDef {
                name: "Melody",
                color: SCENE_C,
                slots: &[
                    ("Kick", "Boom Bap Swing", Keys::Kick),
                    ("Drums", "Boom Bap Swing", Keys::Rest),
                    ("Bass", "Walking Minor", Keys::All),
                    ("Chords", "Warm Minor Sevenths", Keys::All),
                    ("Lead", "Lazy Melody", Keys::All),
                ],
            },
            SceneDef {
                name: "Break",
                color: SCENE_D,
                slots: &[
                    ("Chords", "Warm Minor Sevenths", Keys::All),
                    ("Lead", "Descending Sevenths", Keys::All),
                    ("Dust", "Dust Holds", Keys::All),
                ],
            },
        ],
    },
    TemplateDef {
        name: "Ambient",
        key: (9, ScaleKind::NaturalMinor),
        bpm: 68.0,
        sections: &[
            ("Intro", 8),
            ("Build", 16),
            ("Swell", 16),
            ("Break", 8),
            ("Swell", 8),
            ("Outro", 8),
        ],
        kick: "Pulse",
        bass: "Drone",
        sends: ("Frozen Hall", "Tape Repeat"),
        loud: 25,
        master_input: 2.6,
        built: "A slow, open ambient piece in A minor at 68 BPM: long suspended chords on a layered \
                pad, a deep drone that ducks from a soft heartbeat kick, a few quiet drum hits, \
                a slow marimba spread and a drifting texture, with a huge hall and a tape echo.",
        try_it: "Almost nothing happens in the first bars: let the pad breathe, then add \
                 your own layers. Try a slow filter sweep on the pad, or stretch the break.",
        clips: &[ClipDef {
            name: "Tide Holds",
            category: "Texture",
            tags: &[],
            sound: "Two long dyads for the texture to colour.",
            bpm: 68.0,
            bars: 4,
            pattern: Pattern::Chords {
                velocity: 0.5,
                changes: &[(0.0, 8.0, &[57, 64]), (8.0, 8.0, &[55, 62])],
            },
        }],
        tracks: &[
            Track {
                color: KICK_COLOR,
                trim: 3.9,
                voice: Voice {
                    name: "Pulse",
                    source: Source::Pads("Basalt Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[kick("Dub Steppers", 17, 24), kick("Dub Steppers", 49, 8)],
                },
            },
            Track {
                color: DRUMS_COLOR,
                trim: 11.8,
                voice: Voice {
                    name: "Hush",
                    source: Source::Pads("Timber Kit"),
                    level: 0.0,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -12.0)],
                    clips: &[rest("Dub Steppers", 25, 16), rest("Dub Steppers", 49, 8)],
                },
            },
            Track {
                color: BASS_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Drone",
                    source: Source::Patch("Basalt Sub"),
                    level: -6.5,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[],
                    clips: &[
                        place("Dub Sub Roots", 17, 24),
                        place("Dub Sub Roots", 49, 8),
                        place("Dub Sub Roots", 57, 8),
                    ],
                },
            },
            Track {
                color: CHORDS_COLOR,
                trim: 2.2,
                voice: Voice {
                    name: "Pad",
                    source: Source::Patch("Slow Morph"),
                    level: 0.0,
                    pan: -0.15,
                    inserts: &[],
                    sends: &[(1, -8.0)],
                    clips: &[place("Suspended Drift", 1, 64)],
                },
            },
            Track {
                color: LEAD_COLOR,
                trim: 11.5,
                voice: Voice {
                    name: "Marimba",
                    source: Source::Patch("Soft Marimba"),
                    level: 0.0,
                    pan: 0.25,
                    inserts: &[],
                    sends: &[(1, -8.0), (2, -8.0)],
                    clips: &[
                        place("Slow Wide Spread", 25, 16),
                        place("Slow Wide Spread", 49, 8),
                    ],
                },
            },
            Track {
                color: FX_COLOR,
                trim: 0.0,
                voice: Voice {
                    name: "Tide",
                    source: Source::Patch("Slow Tide"),
                    level: -1.9,
                    pan: 0.0,
                    inserts: &[],
                    sends: &[(1, -6.0)],
                    clips: &[place("Tide Holds", 1, 64)],
                },
            },
        ],
        scenes: &[
            SceneDef {
                name: "Drift",
                color: SCENE_A,
                slots: &[
                    ("Pad", "Suspended Drift", Keys::All),
                    ("Tide", "Tide Holds", Keys::All),
                ],
            },
            SceneDef {
                name: "Pulse",
                color: SCENE_B,
                slots: &[
                    ("Pulse", "Dub Steppers", Keys::Kick),
                    ("Drone", "Dub Sub Roots", Keys::All),
                    ("Pad", "Suspended Drift", Keys::All),
                    ("Tide", "Tide Holds", Keys::All),
                ],
            },
            SceneDef {
                name: "Swell",
                color: SCENE_C,
                slots: &[
                    ("Pulse", "Dub Steppers", Keys::Kick),
                    ("Hush", "Dub Steppers", Keys::Rest),
                    ("Drone", "Dub Sub Roots", Keys::All),
                    ("Pad", "Suspended Drift", Keys::All),
                    ("Marimba", "Slow Wide Spread", Keys::All),
                ],
            },
        ],
    },
];

// --------------------------------------------------------------------------------- the builder

impl Build {
    /// The factory patch `name` as a device of its own, in the send slot `slot`.
    fn send_patch(&mut self, slot: usize, name: &str) {
        let patch = DEFS
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{}: no factory patch {name}", self.item));
        let key = DeviceKey(slug(name));
        self.patch(&key, patch);
        self.run(Command::Console(ConsoleCommand::SetReturn {
            slot,
            effect: Some(key),
        }));
    }

    /// A new `type_id` device `name` in the master's insert chain at `slot`, with `params`.
    fn master_device(&mut self, type_id: &str, name: &str, slot: usize, params: &[(&str, f32)]) {
        let key = DeviceKey(slug(name));
        self.run(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.to_owned(),
            key: Some(key.clone()),
        });
        set_params(
            &mut self.doc,
            &self.registry,
            self.item,
            &key,
            type_id,
            params,
        );
        self.run(Command::Rename {
            key: key.clone(),
            name: name.to_owned(),
        });
        let master = self.master();
        self.run(Command::Console(ConsoleCommand::SetInsert {
            channel: master,
            slot,
            effect: Some(InsertEffect::Existing(key)),
        }));
    }
}

/// The notes of the clip `name` that `keys` takes, as a clip that starts at tick 0.
fn slot_clip(def: &TemplateDef, name: &str, keys: Keys) -> atomik_model::Clip {
    let clip_def = clip_named(def.clips, def.name, name);
    let mut clip = clip_of(clip_def, 0);
    if let ClipContent::Notes(notes) = &mut clip.content {
        notes.retain(|n| keys.takes(n.key));
    }
    clip
}

/// The project text of `def`.
///
/// # Panics
/// A name the tables do not have, a value outside a param's range, or an edit the model refuses.
pub(in crate::app_core) fn build_template(def: &TemplateDef) -> String {
    let mix: Vec<(f32, f32)> = def.tracks.iter().map(|t| (t.voice.level, t.trim)).collect();
    build_with(def, &mix, def.master_input)
}

/// The project of `def` with other levels: `(fader dB, trim dB)` for each track and the input
/// gain of the limiter (what the calibration test tries).
///
/// # Panics
/// As [`build_template`].
pub(in crate::app_core) fn build_with(
    def: &TemplateDef,
    mix: &[(f32, f32)],
    master_input: f32,
) -> String {
    let mut build = Build {
        doc: Document::new(new_project_doc()),
        registry: default_registry(),
        item: def.name,
        audio: Vec::new(),
    };
    let b = &mut build;
    let total: i64 = def.sections.iter().map(|s| s.1).sum();
    assert_eq!(total, BARS, "{}: the sections fill {total} bars", def.name);
    b.run(Command::SetTempo(def.bpm));
    for (track, &(fader, trim)) in def.tracks.iter().zip(mix) {
        b.voice(def.clips, BARS, &track.voice, &[]);
        let channel = b.channel(&DeviceKey(slug(track.voice.name)));
        b.set_channel(&channel, fader, track.voice.pan);
        if trim != 0.0 {
            set_params(
                &mut b.doc,
                &b.registry,
                b.item,
                &channel,
                CHANNEL_TYPE,
                &[("input.gain", trim)],
            );
        }
        b.run(Command::Song(SongCommand::SetTrackColor {
            device: DeviceKey(slug(track.voice.name)),
            color: Some(track.color),
        }));
    }
    b.send_patch(0, def.sends.0);
    b.send_patch(1, def.sends.1);
    // The master: a glue compressor and a limiter at -1 dBTP.
    b.master_device(
        COMPRESSOR,
        "Glue Compressor",
        0,
        &[
            ("threshold", -14.0),
            ("ratio", 2.0),
            ("knee", 8.0),
            ("attack", 30.0),
            ("auto_release", 1.0),
            ("auto_makeup", 0.0),
        ],
    );
    b.master_device(
        MASTERING,
        "Limiter",
        1,
        &[
            ("multiband.on", 0.0),
            ("ceiling", -1.0),
            ("softclip", 0.0),
            ("input", master_input),
        ],
    );
    // The bass: the strip EQ, and a duck from the kick by the quick sidechain.
    let (kick_key, bass_key) = (b.key(def.kick), b.key(def.bass));
    let bass_channel = b.channel(&bass_key);
    set_params(
        &mut b.doc,
        &b.registry,
        b.item,
        &bass_channel,
        CHANNEL_TYPE,
        BASS_EQ,
    );
    b.run(Command::Workflow(WorkflowCommand::DuckFrom {
        target: bass_key,
        source: kick_key,
        pre_fader: false,
    }));
    // The sections, the song key and the end.
    let mut ranges = Vec::new();
    let mut bar = 1.0;
    for (i, (name, bars)) in def.sections.iter().enumerate() {
        ranges.push(RangeMarker {
            id: i as u32 + 1,
            start: bar_tick(bar),
            end: bar_tick(bar + *bars as f64),
            name: (*name).to_owned(),
            color: Some(section_color(name)),
        });
        bar += *bars as f64;
    }
    b.run(Command::Song(SongCommand::SetRanges(ranges)));
    let harmony = Harmony::default()
        .with_key(Some(Key::new(def.key.0, def.key.1)))
        .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    b.run(Command::Song(SongCommand::SetHarmony(harmony)));
    b.run(Command::Song(SongCommand::SetSongEnd(bar_tick(
        BARS as f64 + 1.0,
    ))));
    // The launcher: the main loops, a scene each.
    for (row, scene) in def.scenes.iter().enumerate() {
        for &(track, clip, keys) in scene.slots {
            let device = b.key(track);
            b.run(Command::Song(SongCommand::Launcher(
                LauncherCommand::SetSlot {
                    device,
                    row,
                    clip: LauncherClip {
                        clip: slot_clip(def, clip, keys),
                        quantize: None,
                    },
                },
            )));
        }
        b.run(Command::Song(SongCommand::Launcher(
            LauncherCommand::RenameScene {
                at: row,
                name: Some(scene.name.to_owned()),
            },
        )));
        b.run(Command::Song(SongCommand::Launcher(
            LauncherCommand::SetSceneColor {
                at: row,
                color: Some(scene.color),
            },
        )));
    }
    b.run(Command::SetSongInfo(SongInfo {
        title: def.name.to_owned(),
        author: TEMPLATE_AUTHOR.to_owned(),
        notes: format!("{} {}", def.built, def.try_it),
        ..SongInfo::default()
    }));
    save_project_checked(b.doc.doc()).unwrap_or_else(|e| panic!("{}: {e}", def.name))
}

/// Where a template lives in the factory folder, relative to it.
pub(in crate::app_core) fn template_path(name: &str) -> String {
    format!("Templates/{name}/{name}.atomik")
}

/// Writes every template below `root/Templates`. The folder is made fresh.
///
/// # Panics
/// A file cannot be written or a template cannot be built.
pub(in crate::app_core) fn write_templates(root: &Path) {
    let folder = root.join("Templates");
    let _ = std::fs::remove_dir_all(&folder);
    for def in TEMPLATES {
        let file = root.join(template_path(def.name));
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("a template folder");
        std::fs::write(&file, build_template(def))
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    }
}
