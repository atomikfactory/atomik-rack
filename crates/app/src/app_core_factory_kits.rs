//! The drum kits of the factory library (test code): every sample is rendered here, from the
//! synthesised pad modules of the built-in drum machine, so the library holds no recording
//! made by anyone.
//!
//! A kit is a table of hits. A hit says which module plays it (kick, snare, hat, tom, tone or
//! noise), how the module is set (`body`, `snap`, `sweep`, tune, decay and the pad's own tone
//! controls) and, for a clap, how many times the pad is struck in the first few milliseconds.
//! The renderer builds a hidden project with one drum machine, strikes the pad, bounces the
//! project offline (through the kit's own effect chain, when it has one), mixes the result to
//! mono, trims the silence either side, fades the end out and levels the peak. The kit item is
//! then a drum machine whose pads play those samples: one zone per pad on the pad's key, the
//! pad's level, pan and choke group, and the audio beside the item in `<name>/Audio/`.
//!
//! The renders are deterministic (seeded noise, no clock), so the same table gives the same
//! bytes on the machine the library was written on; another platform's maths may differ in the
//! last bit of a sample, which is why the tests compare the audio exactly on one platform only.

use super::{COMPRESSOR, DISTORTION, EQ, Part, fx, run, set_params};
use atomik_audio_file::{AudioData, content_hash, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, DeviceSnapshot,
    Document, InsertEffect, Note, SongCommand, WorkflowCommand, ZoneDoc, empty_doc,
    new_project_doc, save_device_item,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::path::Path;

/// The drum machine.
pub(in crate::app_core) const DRUM: &str = "atomik.drum-machine";
/// The sample rate of every kit sample (the project rate of a library item).
pub(in crate::app_core) const RATE: u32 = 48_000;
/// Where the peak of every sample is levelled to.
const PEAK: f32 = 0.9;
/// The level, relative to the peak, under which the end of a sample is cut.
const CUT: f32 = 0.002;
/// The fade at the end of a sample, in seconds.
const FADE: f64 = 0.008;
/// The longest a sample may be, in seconds.
pub(in crate::app_core) const MAX_SECONDS: f64 = 0.6;

/// One hit of a kit: a sample on a pad.
pub(in crate::app_core) struct Hit {
    /// The name of the sound (and of its audio in the pool).
    pub name: &'static str,
    /// The pad, 1..=16; it plays on key 35 + pad.
    pub pad: u8,
    /// The module: 1 kick, 2 snare, 3 hat, 4 tom, 5 tone, 6 noise.
    pub module: u8,
    /// `(pad param without the `padN.` prefix, plain value)`: `body`, `snap`, `sweep`, `tune`,
    /// `decay`, `tone`, `lowcut`, `highcut` and `drive`.
    pub params: &'static [(&'static str, f32)],
    /// When the pad is struck, in milliseconds after the first strike (empty: once).
    pub strikes: &'static [f32],
    /// The pad's level in the kit, in dB.
    pub level: f32,
    /// The pad's pan in the kit.
    pub pan: f32,
    /// The pad's choke group (0 for none).
    pub choke: u8,
}

/// One factory drum kit (an item of the `Patches` folder with audio beside it).
pub(in crate::app_core) struct KitDef {
    /// The patch name (and the file name).
    pub name: &'static str,
    /// What kind of sound it is (the browser's category column).
    pub category: &'static str,
    /// Words to search and filter by.
    pub tags: &'static [&'static str],
    /// One line on how it sounds (for the notes of a release; not saved in the item).
    pub sound: &'static str,
    /// The hits, in pad order.
    pub hits: &'static [Hit],
    /// Effects every hit is rendered through, in order (not saved in the item).
    pub effects: &'static [Part],
}

const fn hit(
    name: &'static str,
    pad: u8,
    module: u8,
    params: &'static [(&'static str, f32)],
) -> Hit {
    Hit {
        name,
        pad,
        module,
        params,
        strikes: &[],
        level: 0.0,
        pan: 0.0,
        choke: 0,
    }
}

impl Hit {
    const fn level(mut self, level: f32) -> Self {
        self.level = level;
        self
    }
    const fn pan(mut self, pan: f32) -> Self {
        self.pan = pan;
        self
    }
    const fn choke(mut self, group: u8) -> Self {
        self.choke = group;
        self
    }
    const fn strikes(mut self, at: &'static [f32]) -> Self {
        self.strikes = at;
        self
    }
}

const KICK: u8 = 1;
const SNARE: u8 = 2;
const HAT: u8 = 3;
const TOM: u8 = 4;
const TONE: u8 = 5;
const NOISE: u8 = 6;

/// The factory drum kits, in the order the manifest lists them.
pub(in crate::app_core) const KITS: &[KitDef] = &[
    KitDef {
        name: "Basalt Kit",
        category: "Drum Kit",
        tags: &["drums", "deep", "round", "electronic"],
        sound: "Deep and round: a long, saturated sine kick with a slow pitch fall, a full \
                snare, low toms, a soft clap and dark hats. Nine pads on keys 36 to 46.",
        hits: &[
            hit(
                "Basalt Kick",
                1,
                KICK,
                &[
                    ("body", 0.65),
                    ("snap", 0.3),
                    ("sweep", 0.55),
                    ("tune", -2.0),
                    ("decay", 420.0),
                ],
            ),
            hit(
                "Basalt Rim",
                2,
                TONE,
                &[
                    ("body", 0.9),
                    ("snap", 0.95),
                    ("sweep", 0.0),
                    ("tune", 17.0),
                    ("decay", 70.0),
                ],
            )
            .level(-4.0),
            hit(
                "Basalt Snare",
                3,
                SNARE,
                &[
                    ("body", 0.65),
                    ("snap", 0.5),
                    ("tune", -3.0),
                    ("decay", 240.0),
                ],
            ),
            hit(
                "Basalt Clap",
                4,
                NOISE,
                &[
                    ("body", 0.45),
                    ("snap", 0.35),
                    ("sweep", 0.3),
                    ("decay", 170.0),
                ],
            )
            .strikes(&[0.0, 9.0, 19.0, 31.0])
            .level(-2.0),
            hit(
                "Basalt Low Tom",
                6,
                TOM,
                &[
                    ("body", 0.5),
                    ("snap", 0.3),
                    ("sweep", 0.5),
                    ("tune", -3.0),
                    ("decay", 320.0),
                ],
            )
            .pan(-0.25),
            hit(
                "Basalt Closed Hat",
                7,
                HAT,
                &[
                    ("body", 0.45),
                    ("snap", 0.5),
                    ("decay", 55.0),
                    ("highcut", 14000.0),
                ],
            )
            .level(-5.0)
            .pan(0.2)
            .choke(1),
            hit(
                "Basalt Blip",
                8,
                TONE,
                &[
                    ("body", 0.55),
                    ("snap", 0.4),
                    ("sweep", 0.1),
                    ("tune", 5.0),
                    ("decay", 190.0),
                ],
            )
            .level(-5.0)
            .pan(-0.2),
            hit(
                "Basalt Mid Tom",
                10,
                TOM,
                &[
                    ("body", 0.5),
                    ("snap", 0.3),
                    ("sweep", 0.5),
                    ("tune", 4.0),
                    ("decay", 280.0),
                ],
            )
            .pan(0.25),
            hit(
                "Basalt Open Hat",
                11,
                HAT,
                &[
                    ("body", 0.45),
                    ("snap", 0.55),
                    ("decay", 300.0),
                    ("highcut", 14000.0),
                ],
            )
            .level(-6.0)
            .pan(0.2)
            .choke(1),
        ],
        effects: &[],
    },
    KitDef {
        name: "Static Kit",
        category: "Drum Kit",
        tags: &["drums", "tight", "bright", "electronic"],
        sound: "Tight and bright: a short, clicky kick with a fast pitch drop, a snappy snare, \
                a sharp noise clap, short toms and crisp hats. Nine pads on keys 36 to 46.",
        hits: &[
            hit(
                "Static Kick",
                1,
                KICK,
                &[
                    ("body", 0.35),
                    ("snap", 0.85),
                    ("sweep", 0.8),
                    ("tune", 4.0),
                    ("decay", 220.0),
                ],
            ),
            hit(
                "Static Rim",
                2,
                TONE,
                &[
                    ("body", 0.7),
                    ("snap", 1.0),
                    ("sweep", 0.0),
                    ("tune", 22.0),
                    ("decay", 45.0),
                ],
            )
            .level(-4.0),
            hit(
                "Static Snare",
                3,
                SNARE,
                &[
                    ("body", 0.35),
                    ("snap", 0.85),
                    ("tune", 3.0),
                    ("decay", 150.0),
                ],
            ),
            hit(
                "Static Clap",
                4,
                NOISE,
                &[
                    ("body", 0.6),
                    ("snap", 0.5),
                    ("sweep", 0.4),
                    ("decay", 120.0),
                ],
            )
            .strikes(&[0.0, 7.0, 15.0])
            .level(-2.0),
            hit(
                "Static Low Tom",
                6,
                TOM,
                &[
                    ("body", 0.3),
                    ("snap", 0.6),
                    ("sweep", 0.7),
                    ("tune", 2.0),
                    ("decay", 180.0),
                ],
            )
            .pan(-0.3),
            hit(
                "Static Closed Hat",
                7,
                HAT,
                &[("body", 0.8), ("snap", 0.7), ("tune", 3.0), ("decay", 35.0)],
            )
            .level(-6.0)
            .pan(0.25)
            .choke(1),
            hit(
                "Static Clave",
                8,
                TONE,
                &[
                    ("body", 0.3),
                    ("snap", 0.7),
                    ("sweep", 0.0),
                    ("tune", 12.0),
                    ("decay", 90.0),
                ],
            )
            .level(-4.0)
            .pan(-0.25),
            hit(
                "Static Mid Tom",
                10,
                TOM,
                &[
                    ("body", 0.3),
                    ("snap", 0.6),
                    ("sweep", 0.7),
                    ("tune", 9.0),
                    ("decay", 160.0),
                ],
            )
            .pan(0.3),
            hit(
                "Static Open Hat",
                11,
                HAT,
                &[
                    ("body", 0.8),
                    ("snap", 0.7),
                    ("tune", 3.0),
                    ("decay", 180.0),
                ],
            )
            .level(-7.0)
            .pan(0.25)
            .choke(1),
        ],
        effects: &[],
    },
    KitDef {
        name: "Dust Kit",
        category: "Drum Kit",
        tags: &["drums", "lo-fi", "crushed", "dusty"],
        sound: "Low-fidelity: the same kind of sounds as the others, crushed to a few bits \
                and a low sample rate and rolled off at the top, so the hats hiss and the \
                snare crackles. Nine pads on keys 36 to 46.",
        hits: &[
            hit(
                "Dust Kick",
                1,
                KICK,
                &[
                    ("body", 0.8),
                    ("snap", 0.2),
                    ("sweep", 0.4),
                    ("tune", -1.0),
                    ("decay", 340.0),
                ],
            ),
            hit(
                "Dust Rim",
                2,
                TONE,
                &[
                    ("body", 0.8),
                    ("snap", 0.9),
                    ("sweep", 0.0),
                    ("tune", 15.0),
                    ("decay", 60.0),
                ],
            )
            .level(-3.0),
            hit(
                "Dust Snare",
                3,
                SNARE,
                &[
                    ("body", 0.5),
                    ("snap", 0.6),
                    ("tune", -1.0),
                    ("decay", 200.0),
                ],
            ),
            hit(
                "Dust Clap",
                4,
                NOISE,
                &[
                    ("body", 0.4),
                    ("snap", 0.3),
                    ("sweep", 0.2),
                    ("decay", 150.0),
                ],
            )
            .strikes(&[0.0, 11.0, 23.0])
            .level(-2.0),
            hit(
                "Dust Low Tom",
                6,
                TOM,
                &[
                    ("body", 0.6),
                    ("snap", 0.2),
                    ("sweep", 0.4),
                    ("tune", -5.0),
                    ("decay", 300.0),
                ],
            )
            .pan(-0.25),
            hit(
                "Dust Closed Hat",
                7,
                HAT,
                &[("body", 0.3), ("snap", 0.8), ("decay", 50.0)],
            )
            .level(-4.0)
            .pan(0.2)
            .choke(1),
            hit(
                "Dust Block",
                8,
                TONE,
                &[
                    ("body", 0.4),
                    ("snap", 0.5),
                    ("sweep", 0.1),
                    ("tune", 2.0),
                    ("decay", 140.0),
                ],
            )
            .level(-4.0)
            .pan(-0.2),
            hit(
                "Dust Mid Tom",
                10,
                TOM,
                &[
                    ("body", 0.6),
                    ("snap", 0.2),
                    ("sweep", 0.4),
                    ("tune", 1.0),
                    ("decay", 260.0),
                ],
            )
            .pan(0.25),
            hit(
                "Dust Open Hat",
                11,
                HAT,
                &[("body", 0.3), ("snap", 0.8), ("decay", 260.0)],
            )
            .level(-6.0)
            .pan(0.2)
            .choke(1),
        ],
        effects: &[
            fx(
                "crush",
                DISTORTION,
                &[
                    ("curve", 4.0),
                    ("drive", 3.0),
                    ("crush.bits", 7.0),
                    ("crush.rate", 12000.0),
                    ("cut.high", 7000.0),
                    ("mix", 1.0),
                ],
            ),
            fx("tilt", EQ, &[("band5.gain", -6.0), ("band5.freq", 5000.0)]),
        ],
    },
    KitDef {
        name: "Glass Kit",
        category: "Drum Kit",
        tags: &["drums", "tonal", "bright", "electronic"],
        sound: "Tonal and bright: a short, high-tuned kick, a ringing snare, toms tuned like \
                notes, a resonant clap and a bell-like percussion pad, with the hats pitched \
                high. Nine pads on keys 36 to 46.",
        hits: &[
            hit(
                "Glass Kick",
                1,
                KICK,
                &[
                    ("body", 0.2),
                    ("snap", 0.5),
                    ("sweep", 0.9),
                    ("tune", 5.0),
                    ("decay", 260.0),
                ],
            ),
            hit(
                "Glass Rim",
                2,
                TONE,
                &[
                    ("body", 1.0),
                    ("snap", 0.8),
                    ("sweep", 0.0),
                    ("tune", 24.0),
                    ("decay", 80.0),
                ],
            )
            .level(-5.0),
            hit(
                "Glass Snare",
                3,
                SNARE,
                &[
                    ("body", 0.25),
                    ("snap", 0.55),
                    ("tune", 6.0),
                    ("decay", 190.0),
                ],
            ),
            hit(
                "Glass Clap",
                4,
                NOISE,
                &[
                    ("body", 0.75),
                    ("snap", 0.7),
                    ("sweep", 0.5),
                    ("decay", 140.0),
                ],
            )
            .strikes(&[0.0, 8.0, 17.0, 27.0])
            .level(-3.0),
            hit(
                "Glass Low Tom",
                6,
                TOM,
                &[
                    ("body", 0.7),
                    ("snap", 0.4),
                    ("sweep", 0.3),
                    ("tune", 3.0),
                    ("decay", 340.0),
                ],
            )
            .pan(-0.3),
            hit(
                "Glass Closed Hat",
                7,
                HAT,
                &[("body", 0.9), ("snap", 0.4), ("tune", 7.0), ("decay", 40.0)],
            )
            .level(-7.0)
            .pan(0.3)
            .choke(1),
            hit(
                "Glass Bell",
                8,
                TONE,
                &[
                    ("body", 0.85),
                    ("snap", 0.3),
                    ("sweep", 0.0),
                    ("tune", 14.0),
                    ("decay", 360.0),
                ],
            )
            .level(-6.0)
            .pan(-0.25),
            hit(
                "Glass Mid Tom",
                10,
                TOM,
                &[
                    ("body", 0.7),
                    ("snap", 0.4),
                    ("sweep", 0.3),
                    ("tune", 10.0),
                    ("decay", 300.0),
                ],
            )
            .pan(0.3),
            hit(
                "Glass Open Hat",
                11,
                HAT,
                &[
                    ("body", 0.9),
                    ("snap", 0.4),
                    ("tune", 7.0),
                    ("decay", 240.0),
                ],
            )
            .level(-8.0)
            .pan(0.3)
            .choke(1),
        ],
        effects: &[fx(
            "glue",
            COMPRESSOR,
            &[("threshold", -20.0), ("ratio", 2.0), ("attack", 2.0)],
        )],
    },
    KitDef {
        name: "Timber Kit",
        category: "Drum Kit",
        tags: &["drums", "acoustic", "natural", "woody"],
        sound: "Acoustic-like: a short, round kick, a snare with a rattle, a soft clap, three tuned toms, a wood block and a bell, light hats and a long crash. Fourteen pads on keys 36 to 49.",
        hits: &[
            hit(
                "Timber Kick",
                1,
                KICK,
                &[
                    ("body", 0.55),
                    ("snap", 0.45),
                    ("sweep", 0.3),
                    ("tune", -1.0),
                    ("decay", 280.0),
                ],
            ),
            hit(
                "Timber Rim",
                2,
                TONE,
                &[
                    ("body", 0.6),
                    ("snap", 1.0),
                    ("sweep", 0.0),
                    ("tune", 19.0),
                    ("decay", 50.0),
                ],
            )
            .level(-4.0),
            hit(
                "Timber Snare",
                3,
                SNARE,
                &[
                    ("body", 0.55),
                    ("snap", 0.7),
                    ("tune", 0.0),
                    ("decay", 200.0),
                ],
            ),
            hit(
                "Timber Clap",
                4,
                NOISE,
                &[
                    ("body", 0.5),
                    ("snap", 0.4),
                    ("sweep", 0.3),
                    ("decay", 140.0),
                ],
            )
            .level(-2.0)
            .strikes(&[0.0, 10.0, 22.0]),
            hit(
                "Timber Ghost",
                5,
                SNARE,
                &[("body", 0.4), ("snap", 0.5), ("tune", 4.0), ("decay", 90.0)],
            )
            .level(-6.0),
            hit(
                "Timber Low Tom",
                6,
                TOM,
                &[
                    ("body", 0.55),
                    ("snap", 0.4),
                    ("sweep", 0.35),
                    ("tune", -2.0),
                    ("decay", 360.0),
                ],
            )
            .pan(-0.3),
            hit(
                "Timber Closed Hat",
                7,
                HAT,
                &[("body", 0.6), ("snap", 0.6), ("tune", 2.0), ("decay", 45.0)],
            )
            .pan(0.25)
            .level(-6.0)
            .choke(1),
            hit(
                "Timber Block",
                8,
                TONE,
                &[
                    ("body", 0.5),
                    ("snap", 0.8),
                    ("sweep", 0.0),
                    ("tune", 14.0),
                    ("decay", 80.0),
                ],
            )
            .pan(-0.2)
            .level(-5.0),
            hit(
                "Timber Pedal Hat",
                9,
                HAT,
                &[("body", 0.6), ("snap", 0.5), ("tune", 0.0), ("decay", 30.0)],
            )
            .pan(0.25)
            .level(-8.0)
            .choke(1),
            hit(
                "Timber Mid Tom",
                10,
                TOM,
                &[
                    ("body", 0.55),
                    ("snap", 0.4),
                    ("sweep", 0.35),
                    ("tune", 5.0),
                    ("decay", 320.0),
                ],
            ),
            hit(
                "Timber Open Hat",
                11,
                HAT,
                &[
                    ("body", 0.6),
                    ("snap", 0.6),
                    ("tune", 2.0),
                    ("decay", 260.0),
                ],
            )
            .pan(0.25)
            .level(-7.0)
            .choke(1),
            hit(
                "Timber High Tom",
                12,
                TOM,
                &[
                    ("body", 0.5),
                    ("snap", 0.45),
                    ("sweep", 0.3),
                    ("tune", 11.0),
                    ("decay", 280.0),
                ],
            )
            .pan(0.3),
            hit(
                "Timber Crash",
                13,
                HAT,
                &[
                    ("body", 0.7),
                    ("snap", 0.5),
                    ("tune", -2.0),
                    ("decay", 480.0),
                ],
            )
            .pan(-0.25)
            .level(-8.0),
            hit(
                "Timber Bell",
                14,
                TONE,
                &[
                    ("body", 0.8),
                    ("snap", 0.4),
                    ("sweep", 0.0),
                    ("tune", 16.0),
                    ("decay", 300.0),
                ],
            )
            .pan(0.2)
            .level(-7.0),
        ],
        effects: &[fx(
            "glue",
            COMPRESSOR,
            &[("threshold", -20.0), ("ratio", 2.0), ("attack", 2.0)],
        )],
    },
    KitDef {
        name: "Circuit Kit",
        category: "Drum Kit",
        tags: &["drums", "electronic", "synthetic", "punchy"],
        sound: "Electronic: a long kick with a deep pitch fall, a snappy snare, a layered clap, a noise snap, low toms that sweep, crisp hats, a blip, a zap and a sub boom. Fourteen pads on keys 36 to 49.",
        hits: &[
            hit(
                "Circuit Kick",
                1,
                KICK,
                &[
                    ("body", 0.75),
                    ("snap", 0.4),
                    ("sweep", 0.7),
                    ("tune", -3.0),
                    ("decay", 480.0),
                ],
            ),
            hit(
                "Circuit Rim",
                2,
                TONE,
                &[
                    ("body", 0.5),
                    ("snap", 0.9),
                    ("sweep", 0.0),
                    ("tune", 20.0),
                    ("decay", 40.0),
                ],
            )
            .level(-4.0),
            hit(
                "Circuit Snare",
                3,
                SNARE,
                &[
                    ("body", 0.45),
                    ("snap", 0.8),
                    ("tune", 2.0),
                    ("decay", 180.0),
                ],
            ),
            hit(
                "Circuit Clap",
                4,
                NOISE,
                &[
                    ("body", 0.55),
                    ("snap", 0.5),
                    ("sweep", 0.5),
                    ("decay", 150.0),
                ],
            )
            .level(-2.0)
            .strikes(&[0.0, 8.0, 17.0, 27.0]),
            hit(
                "Circuit Snap",
                5,
                NOISE,
                &[
                    ("body", 0.3),
                    ("snap", 0.9),
                    ("sweep", 0.2),
                    ("decay", 70.0),
                ],
            )
            .level(-4.0),
            hit(
                "Circuit Low Tom",
                6,
                TOM,
                &[
                    ("body", 0.45),
                    ("snap", 0.5),
                    ("sweep", 0.8),
                    ("tune", -4.0),
                    ("decay", 400.0),
                ],
            )
            .pan(-0.3),
            hit(
                "Circuit Closed Hat",
                7,
                HAT,
                &[
                    ("body", 0.85),
                    ("snap", 0.6),
                    ("tune", 5.0),
                    ("decay", 30.0),
                ],
            )
            .pan(0.25)
            .level(-6.0)
            .choke(1),
            hit(
                "Circuit Blip",
                8,
                TONE,
                &[
                    ("body", 0.6),
                    ("snap", 0.5),
                    ("sweep", 0.05),
                    ("tune", 9.0),
                    ("decay", 220.0),
                ],
            )
            .pan(-0.2)
            .level(-5.0),
            hit(
                "Circuit Shaker",
                9,
                NOISE,
                &[
                    ("body", 0.7),
                    ("snap", 0.2),
                    ("sweep", 0.0),
                    ("decay", 90.0),
                ],
            )
            .pan(0.3)
            .level(-9.0),
            hit(
                "Circuit Mid Tom",
                10,
                TOM,
                &[
                    ("body", 0.45),
                    ("snap", 0.5),
                    ("sweep", 0.8),
                    ("tune", 3.0),
                    ("decay", 360.0),
                ],
            ),
            hit(
                "Circuit Open Hat",
                11,
                HAT,
                &[
                    ("body", 0.85),
                    ("snap", 0.6),
                    ("tune", 5.0),
                    ("decay", 200.0),
                ],
            )
            .pan(0.25)
            .level(-7.0)
            .choke(1),
            hit(
                "Circuit High Tom",
                12,
                TOM,
                &[
                    ("body", 0.4),
                    ("snap", 0.5),
                    ("sweep", 0.8),
                    ("tune", 10.0),
                    ("decay", 300.0),
                ],
            )
            .pan(0.3),
            hit(
                "Circuit Zap",
                13,
                TONE,
                &[
                    ("body", 0.7),
                    ("snap", 0.6),
                    ("sweep", 1.0),
                    ("tune", 12.0),
                    ("decay", 380.0),
                ],
            )
            .pan(-0.25)
            .level(-5.0),
            hit(
                "Circuit Boom",
                14,
                KICK,
                &[
                    ("body", 1.0),
                    ("snap", 0.1),
                    ("sweep", 0.3),
                    ("tune", -9.0),
                    ("decay", 520.0),
                ],
            )
            .level(-2.0),
        ],
        effects: &[],
    },
    KitDef {
        name: "Cassette Kit",
        category: "Drum Kit",
        tags: &["drums", "lo-fi", "warm", "boom-bap"],
        sound: "Warm low-fidelity: soft, round sounds pushed through a bit crusher and a high cut, so the kick thumps, the snare is dull and the hats are a hush. Fourteen pads on keys 36 to 49.",
        hits: &[
            hit(
                "Cassette Kick",
                1,
                KICK,
                &[
                    ("body", 0.7),
                    ("snap", 0.25),
                    ("sweep", 0.35),
                    ("tune", -2.0),
                    ("decay", 300.0),
                ],
            ),
            hit(
                "Cassette Rim",
                2,
                TONE,
                &[
                    ("body", 0.7),
                    ("snap", 0.9),
                    ("sweep", 0.0),
                    ("tune", 16.0),
                    ("decay", 55.0),
                ],
            )
            .level(-4.0),
            hit(
                "Cassette Snare",
                3,
                SNARE,
                &[
                    ("body", 0.6),
                    ("snap", 0.45),
                    ("tune", -2.0),
                    ("decay", 220.0),
                ],
            ),
            hit(
                "Cassette Clap",
                4,
                NOISE,
                &[
                    ("body", 0.5),
                    ("snap", 0.35),
                    ("sweep", 0.3),
                    ("decay", 160.0),
                ],
            )
            .level(-2.0)
            .strikes(&[0.0, 11.0, 24.0]),
            hit(
                "Cassette Brush",
                5,
                NOISE,
                &[
                    ("body", 0.6),
                    ("snap", 0.2),
                    ("sweep", 0.0),
                    ("decay", 130.0),
                ],
            )
            .level(-7.0),
            hit(
                "Cassette Low Tom",
                6,
                TOM,
                &[
                    ("body", 0.6),
                    ("snap", 0.3),
                    ("sweep", 0.4),
                    ("tune", -3.0),
                    ("decay", 340.0),
                ],
            )
            .pan(-0.3),
            hit(
                "Cassette Closed Hat",
                7,
                HAT,
                &[
                    ("body", 0.5),
                    ("snap", 0.5),
                    ("tune", 0.0),
                    ("decay", 45.0),
                    ("highcut", 11000.0),
                ],
            )
            .pan(0.2)
            .level(-6.0)
            .choke(1),
            hit(
                "Cassette Keys Tick",
                8,
                TONE,
                &[
                    ("body", 0.6),
                    ("snap", 0.6),
                    ("sweep", 0.0),
                    ("tune", 12.0),
                    ("decay", 110.0),
                ],
            )
            .pan(-0.2)
            .level(-5.0),
            hit(
                "Cassette Pedal Hat",
                9,
                HAT,
                &[
                    ("body", 0.5),
                    ("snap", 0.4),
                    ("tune", -1.0),
                    ("decay", 28.0),
                    ("highcut", 11000.0),
                ],
            )
            .pan(0.2)
            .level(-8.0)
            .choke(1),
            hit(
                "Cassette Mid Tom",
                10,
                TOM,
                &[
                    ("body", 0.6),
                    ("snap", 0.3),
                    ("sweep", 0.4),
                    ("tune", 3.0),
                    ("decay", 300.0),
                ],
            ),
            hit(
                "Cassette Open Hat",
                11,
                HAT,
                &[
                    ("body", 0.5),
                    ("snap", 0.5),
                    ("tune", 0.0),
                    ("decay", 280.0),
                    ("highcut", 11000.0),
                ],
            )
            .pan(0.2)
            .level(-7.0)
            .choke(1),
            hit(
                "Cassette High Tom",
                12,
                TOM,
                &[
                    ("body", 0.55),
                    ("snap", 0.3),
                    ("sweep", 0.4),
                    ("tune", 9.0),
                    ("decay", 260.0),
                ],
            )
            .pan(0.3),
            hit(
                "Cassette Dust",
                13,
                NOISE,
                &[
                    ("body", 0.4),
                    ("snap", 0.1),
                    ("sweep", 0.0),
                    ("decay", 240.0),
                ],
            )
            .pan(-0.25)
            .level(-9.0),
            hit(
                "Cassette Pluck",
                14,
                TONE,
                &[
                    ("body", 0.8),
                    ("snap", 0.3),
                    ("sweep", 0.0),
                    ("tune", 14.0),
                    ("decay", 260.0),
                ],
            )
            .pan(0.2)
            .level(-7.0),
        ],
        effects: &[fx(
            "crush",
            DISTORTION,
            &[
                ("curve", 4.0),
                ("drive", 2.0),
                ("crush.bits", 8.0),
                ("crush.rate", 14000.0),
                ("cut.high", 6500.0),
                ("mix", 1.0),
            ],
        )],
    },
    KitDef {
        name: "Bazaar Kit",
        category: "Drum Kit",
        tags: &["drums", "percussion", "hand", "shakers"],
        sound: "Hand percussion: two congas and two bongos, a clave, a wood block, a cowbell, a triangle, a palm clap, shakers, a tambourine and a frame drum. Fifteen pads on keys 36 to 50.",
        hits: &[
            hit(
                "Bazaar Conga Low",
                1,
                TOM,
                &[
                    ("body", 0.6),
                    ("snap", 0.5),
                    ("sweep", 0.2),
                    ("tune", 0.0),
                    ("decay", 300.0),
                ],
            ),
            hit(
                "Bazaar Clave",
                2,
                TONE,
                &[
                    ("body", 0.3),
                    ("snap", 0.8),
                    ("sweep", 0.0),
                    ("tune", 13.0),
                    ("decay", 80.0),
                ],
            )
            .level(-4.0),
            hit(
                "Bazaar Conga High",
                3,
                TOM,
                &[
                    ("body", 0.55),
                    ("snap", 0.6),
                    ("sweep", 0.2),
                    ("tune", 8.0),
                    ("decay", 240.0),
                ],
            ),
            hit(
                "Bazaar Palm Clap",
                4,
                NOISE,
                &[
                    ("body", 0.4),
                    ("snap", 0.3),
                    ("sweep", 0.2),
                    ("decay", 110.0),
                ],
            )
            .level(-2.0)
            .strikes(&[0.0, 12.0]),
            hit(
                "Bazaar Conga Mute",
                5,
                TOM,
                &[
                    ("body", 0.5),
                    ("snap", 0.6),
                    ("sweep", 0.2),
                    ("tune", 6.0),
                    ("decay", 90.0),
                ],
            )
            .pan(0.15),
            hit(
                "Bazaar Bongo Low",
                6,
                TOM,
                &[
                    ("body", 0.5),
                    ("snap", 0.5),
                    ("sweep", 0.15),
                    ("tune", 6.0),
                    ("decay", 180.0),
                ],
            )
            .pan(-0.3),
            hit(
                "Bazaar Shaker",
                7,
                HAT,
                &[("body", 0.9), ("snap", 0.3), ("tune", 6.0), ("decay", 60.0)],
            )
            .pan(0.25)
            .level(-8.0)
            .choke(1),
            hit(
                "Bazaar Cowbell",
                8,
                TONE,
                &[
                    ("body", 0.55),
                    ("snap", 0.6),
                    ("sweep", 0.0),
                    ("tune", 11.0),
                    ("decay", 260.0),
                ],
            )
            .pan(-0.2)
            .level(-5.0),
            hit(
                "Bazaar Maraca",
                9,
                NOISE,
                &[
                    ("body", 0.8),
                    ("snap", 0.2),
                    ("sweep", 0.0),
                    ("decay", 50.0),
                ],
            )
            .pan(0.3)
            .level(-9.0),
            hit(
                "Bazaar Bongo High",
                10,
                TOM,
                &[
                    ("body", 0.45),
                    ("snap", 0.5),
                    ("sweep", 0.15),
                    ("tune", 12.0),
                    ("decay", 150.0),
                ],
            )
            .pan(0.3),
            hit(
                "Bazaar Cabasa",
                11,
                HAT,
                &[
                    ("body", 0.8),
                    ("snap", 0.3),
                    ("tune", 4.0),
                    ("decay", 140.0),
                ],
            )
            .pan(0.25)
            .level(-8.0)
            .choke(1),
            hit(
                "Bazaar Wood Block",
                12,
                TONE,
                &[
                    ("body", 0.4),
                    ("snap", 0.9),
                    ("sweep", 0.0),
                    ("tune", 18.0),
                    ("decay", 60.0),
                ],
            )
            .pan(-0.25)
            .level(-5.0),
            hit(
                "Bazaar Tambourine",
                13,
                HAT,
                &[
                    ("body", 0.6),
                    ("snap", 0.5),
                    ("tune", 7.0),
                    ("decay", 220.0),
                ],
            )
            .pan(0.2)
            .level(-8.0)
            .strikes(&[0.0, 6.0]),
            hit(
                "Bazaar Triangle",
                14,
                TONE,
                &[
                    ("body", 0.9),
                    ("snap", 0.2),
                    ("sweep", 0.0),
                    ("tune", 24.0),
                    ("decay", 480.0),
                ],
            )
            .pan(0.2)
            .level(-8.0),
            hit(
                "Bazaar Frame Drum",
                15,
                KICK,
                &[
                    ("body", 0.55),
                    ("snap", 0.5),
                    ("sweep", 0.2),
                    ("tune", 1.0),
                    ("decay", 280.0),
                ],
            )
            .level(-1.0),
        ],
        effects: &[],
    },
];

/// The key a pad plays on.
pub(in crate::app_core) const fn key_of(pad: u8) -> u8 {
    35 + pad
}

fn pad_param(pad: u8, name: &str) -> String {
    format!("pad{pad}.{name}")
}

/// Milliseconds to ticks at 120 BPM (the tempo of a new project).
#[allow(clippy::cast_possible_truncation)] // a few milliseconds
fn ms_ticks(ms: f32) -> i64 {
    (f64::from(ms) / 1000.0 * 2.0 * PPQ as f64).round() as i64
}

/// Renders `hit` of `kit` to the mono sample the kit holds.
///
/// # Panics
/// A param the drum machine does not have or the render fails.
pub(in crate::app_core) fn render_hit(kit: &KitDef, hit: &Hit) -> AudioData {
    let registry = default_registry();
    let item = hit.name;
    let key = DeviceKey("kit".to_owned());
    let mut doc = Document::new(new_project_doc());
    run(
        &mut doc,
        &registry,
        item,
        Command::AddDevice {
            index: usize::MAX,
            type_id: DRUM.to_owned(),
            key: Some(key.clone()),
        },
    );
    let mut params: Vec<(String, f32)> = vec![
        (pad_param(hit.pad, "module"), f32::from(hit.module)),
        (pad_param(hit.pad, "level"), 0.0),
    ];
    params.extend(
        hit.params
            .iter()
            .map(|&(name, value)| (pad_param(hit.pad, name), value)),
    );
    let borrowed: Vec<(&str, f32)> = params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    set_params(&mut doc, &registry, item, &key, DRUM, &borrowed);
    let channel = doc
        .doc()
        .console
        .channel_of(&key)
        .map(|c| c.channel.clone())
        .unwrap_or_else(|| panic!("{item}: the drum machine has no channel"));
    for (slot, part) in kit.effects.iter().enumerate() {
        let fx_key = DeviceKey(part.key.to_owned());
        run(
            &mut doc,
            &registry,
            item,
            Command::AddDevice {
                index: usize::MAX,
                type_id: part.device.to_owned(),
                key: Some(fx_key.clone()),
            },
        );
        set_params(&mut doc, &registry, item, &fx_key, part.device, part.params);
        run(
            &mut doc,
            &registry,
            item,
            Command::Console(ConsoleCommand::SetInsert {
                channel: channel.clone(),
                slot,
                effect: Some(InsertEffect::Existing(fx_key)),
            }),
        );
    }
    run(
        &mut doc,
        &registry,
        item,
        Command::Song(SongCommand::AddTrack {
            device: key.clone(),
        }),
    );
    let strikes: &[f32] = if hit.strikes.is_empty() {
        &[0.0]
    } else {
        hit.strikes
    };
    let notes: Vec<Note> = strikes
        .iter()
        .map(|&ms| Note::new(ms_ticks(ms), PPQ / 8, key_of(hit.pad), 1.0))
        .collect();
    let clip = Clip {
        id: ClipId(0),
        start: 0,
        length: PPQ,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(notes.into()),
    };
    run(
        &mut doc,
        &registry,
        item,
        Command::Song(SongCommand::AddClip {
            device: key,
            lane: 0,
            clip,
        }),
    );
    run(
        &mut doc,
        &registry,
        item,
        Command::Song(SongCommand::SetSongEnd(PPQ)),
    );
    let rendered = render_doc_in(doc.doc(), &registry, &ProjectOptions::default(), None)
        .unwrap_or_else(|e| panic!("{item}: {e}"));
    assert_eq!(rendered.sample_rate, RATE, "{item}");
    finish(item, &rendered.channels)
}

/// Mixes the rendered channels to mono, trims the silence either side, fades the end and
/// levels the peak.
fn finish(item: &str, channels: &[Vec<f32>]) -> AudioData {
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    let mut mono: Vec<f32> = (0..frames)
        .map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / channels.len() as f32)
        .collect();
    let peak = mono.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    assert!(peak > 1.0e-3, "{item}: the render is silent");
    let first = mono.iter().position(|s| s.abs() > peak * CUT).unwrap_or(0);
    let last = mono
        .iter()
        .rposition(|s| s.abs() > peak * CUT)
        .unwrap_or(frames - 1);
    let mut mono: Vec<f32> = mono.drain(first..=last).collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a short constant
    let longest = (MAX_SECONDS * f64::from(RATE)) as usize;
    mono.truncate(longest);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a short constant
    let fade = ((FADE * f64::from(RATE)) as usize).min(mono.len());
    let len = mono.len();
    for (i, s) in mono[len - fade..].iter_mut().enumerate() {
        #[allow(clippy::cast_precision_loss)] // a fade position
        let t = (i + 1) as f32 / fade as f32;
        *s *= 0.5 * (1.0 + (std::f32::consts::PI * t).cos());
    }
    let level = PEAK / mono.iter().fold(0.0f32, |p, s| p.max(s.abs())).max(1.0e-9);
    for s in &mut mono {
        *s *= level;
    }
    AudioData::new(RATE, vec![mono]).unwrap_or_else(|e| panic!("{item}: {e}"))
}

/// A rendered kit: the item text and the audio it names.
pub(in crate::app_core) struct Built {
    /// The item file text.
    pub text: String,
    /// `(asset id, audio)` of every sample, in pad order.
    pub audio: Vec<(String, AudioData)>,
}

/// Renders every hit of `kit` and writes the item text around them.
///
/// # Panics
/// A render fails or the model refuses a zone.
pub(in crate::app_core) fn build_kit(kit: &KitDef) -> Built {
    let samples: Vec<AudioData> = kit.hits.iter().map(|h| render_hit(kit, h)).collect();
    kit_item(kit, samples)
}

/// The item text of `kit` with `samples` (one per hit) as the audio of its pads.
pub(in crate::app_core) fn kit_item(kit: &KitDef, samples: Vec<AudioData>) -> Built {
    let registry = default_registry();
    let item = kit.name;
    let key = DeviceKey("kit".to_owned());
    let mut doc = Document::new(empty_doc());
    run(
        &mut doc,
        &registry,
        item,
        Command::AddDevice {
            index: usize::MAX,
            type_id: DRUM.to_owned(),
            key: Some(key.clone()),
        },
    );
    let mut zones = Vec::new();
    let mut audio = Vec::new();
    let mut params: Vec<(String, f32)> = vec![("master.level".to_owned(), -1.0)];
    for (hit, data) in kit.hits.iter().zip(samples) {
        let hash = content_hash(&data);
        let frames = data.frames() as u64;
        run(
            &mut doc,
            &registry,
            item,
            Command::RegisterAsset {
                hash: hash.clone(),
                meta: AssetMeta {
                    channels: 1,
                    sample_rate: data.sample_rate,
                    frames,
                    original_name: hit.name.to_owned(),
                },
            },
        );
        let mut zone = ZoneDoc::new(hash.clone(), frames);
        zone.key_lo = key_of(hit.pad);
        zone.key_hi = key_of(hit.pad);
        zone.root_key = key_of(hit.pad);
        zones.push(zone);
        params.push((pad_param(hit.pad, "level"), hit.level));
        params.push((pad_param(hit.pad, "pan"), hit.pan));
        params.push((pad_param(hit.pad, "decay"), 10_000.0));
        params.push((pad_param(hit.pad, "choke"), f32::from(hit.choke)));
        audio.push((hash, data));
    }
    let borrowed: Vec<(&str, f32)> = params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    set_params(&mut doc, &registry, item, &key, DRUM, &borrowed);
    run(
        &mut doc,
        &registry,
        item,
        Command::SetZones {
            device: key.clone(),
            zones,
            merge: None,
        },
    );
    run(
        &mut doc,
        &registry,
        item,
        Command::Workflow(WorkflowCommand::SetPatchName {
            device: key.clone(),
            name: kit.name.to_owned(),
        }),
    );
    let snapshot =
        DeviceSnapshot::capture(doc.doc(), &[key], false).unwrap_or_else(|e| panic!("{item}: {e}"));
    Built {
        text: save_device_item(&snapshot, doc.doc()),
        audio,
    }
}

/// Writes the audio of a built kit to `<patches>/<name>/Audio/<id>.wav` (the folder is made
/// fresh, so a sample the tables no longer use does not stay behind).
///
/// # Panics
/// A file cannot be written.
pub(in crate::app_core) fn write_audio(patches: &Path, kit: &KitDef, built: &Built) {
    write_audio_for(patches, kit.name, built);
}

/// [`write_audio`] for any patch with audio beside it (a drum kit, a drawn table): `<patches>/
/// <name>/Audio/<id>.wav`.
///
/// # Panics
/// A file cannot be written.
pub(in crate::app_core) fn write_audio_for(patches: &Path, name: &str, built: &Built) {
    let folder = patches.join(name);
    let _ = std::fs::remove_dir_all(&folder);
    let audio = folder.join("Audio");
    std::fs::create_dir_all(&audio).expect("the audio folder of a patch");
    for (hash, data) in &built.audio {
        write_wav_f32(&audio.join(format!("{hash}.wav")), data)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}
