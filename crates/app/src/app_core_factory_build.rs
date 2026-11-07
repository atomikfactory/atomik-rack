//! The sound design of the factory library (test code): every factory patch as a table of
//! params, and the writer that turns the tables into the files of the `factory` folder.
//!
//! A patch lists only the params that differ from a new device of its type; the writer builds
//! the device with the model (every value is checked against the device's descriptor, so a typo
//! in a key or a value out of range stops it), names the patch, and writes the item exactly as
//! Save to Library does. The sounds are this project's own, designed from the built-in devices
//! and named for what they sound like.
//!
//! To change a sound or add one, edit the table here and run
//!
//! ```text
//! ATOMIK_WRITE_FACTORY=1 cargo test -p atomik-app --lib write_the_factory_library -- --ignored
//! ```
//!
//! which rewrites every item of the `factory` folder (patches, drum kits with their audio,
//! containers and note clips) and `factory/factory.json`.

use atomik_device_sdk::Registry;
use atomik_model::{
    CONTAINER_TYPE, Cable, Command, ContainerCommand, DeviceKey, DeviceSnapshot, Document,
    MacroKind, MacroTarget, ModSlot, SocketRef, WorkflowCommand, amount_param, empty_doc,
    save_device_item,
};
use atomik_render::default_registry;
use std::path::Path;

#[path = "app_core_factory_clips.rs"]
pub(in crate::app_core) mod clips;
#[path = "app_core_factory_examples.rs"]
pub(in crate::app_core) mod examples;
#[path = "app_core_factory_kits.rs"]
pub(in crate::app_core) mod kits;

pub(in crate::app_core) use clips::{CLIPS, ClipDef, clip_notes, clip_text};
pub(in crate::app_core) use examples::{EXAMPLES, write_examples};
pub(in crate::app_core) use kits::{KITS, KitDef, build_kit, kit_item, write_audio};

pub(super) const POLY: &str = "atomik.poly-synth";
pub(super) const BASS: &str = "atomik.mono-bass";
pub(super) const DUAL: &str = "atomik.dual-layer";
pub(super) const FM: &str = "atomik.fm-synth";
pub(super) const GRAIN: &str = "atomik.granular";
pub(super) const WAVE: &str = "atomik.wavetable-synth";
pub(super) const ORGAN: &str = "atomik.organ";
pub(super) const SEMI: &str = "atomik.semi-modular";
pub(super) const DYNAMIC_EQ: &str = "atomik.dynamic-eq";
pub(super) const CLIPPER: &str = "atomik.clipper";
pub(super) const DUCK: &str = "atomik.duck";

/// The author every factory item carries.
pub(super) const AUTHOR: &str = "Atomik";

/// One factory patch.
pub(super) struct Def {
    /// The patch name (and the file name).
    pub name: &'static str,
    /// The device type the patch is for.
    pub device: &'static str,
    /// What kind of sound it is (the browser's category column).
    pub category: &'static str,
    /// Words to search and filter by.
    pub tags: &'static [&'static str],
    /// One line on how it sounds (for the notes of a release; not saved in the item).
    pub sound: &'static str,
    /// `(param key, plain value)` of every param that differs from a new device.
    pub params: &'static [(&'static str, f32)],
    /// The modulation slots in use, in slot order (devices with a matrix only).
    pub mods: &'static [ModLink],
}

/// One slot of a device's modulation matrix, by the keys of its source and target tables.
pub(super) struct ModLink {
    /// The source key (`lfo1`, `step`, ...).
    pub source: &'static str,
    /// The target key (`cutoff`, `filter1_cutoff`, ...).
    pub target: &'static str,
    /// The source that scales the amount (`mod_wheel`), or empty for none.
    pub via: &'static str,
    /// The amount, -1..1.
    pub amount: f32,
}

/// The factory patches, in the order the manifest lists them.
pub(super) const DEFS: &[Def] = &[
    Def {
        name: "Warm Drift Pad",
        device: POLY,
        category: "Pad",
        tags: &["analog", "warm", "slow", "wide"],
        sound: "Two slightly detuned saw oscillators behind a low-pass filter that opens slowly, \
                with a drifting pitch, a slow filter sweep and a soft chorus.",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 0.0),
            ("osc1.fine", -6.0),
            ("osc2.fine", 9.0),
            ("osc.mix", 0.5),
            ("filter.cutoff", 1400.0),
            ("filter.resonance", 0.15),
            ("filter.env_amount", 0.25),
            ("filter.key_track", 0.4),
            ("env.amp.attack", 600.0),
            ("env.amp.decay", 800.0),
            ("env.amp.sustain", 0.8),
            ("env.amp.release", 1400.0),
            ("env.filter.attack", 900.0),
            ("env.filter.decay", 1500.0),
            ("env.filter.sustain", 0.4),
            ("lfo.wave", 0.0),
            ("lfo.rate", 0.25),
            ("lfo.to_cutoff", 0.4),
            ("drift", 0.4),
            ("spread", 0.6),
            ("chorus.mix", 0.35),
            ("chorus.rate", 0.4),
            ("chorus.depth", 0.5),
            ("master.level", -4.0),
        ],
        mods: &[],
    },
    Def {
        name: "Copper Pluck",
        device: POLY,
        category: "Pluck",
        tags: &["analog", "short", "bright", "echo"],
        sound: "A square and a saw oscillator through a resonant filter that snaps shut in a \
                quarter of a second, with a dotted echo behind it.",
        params: &[
            ("osc1.wave", 1.0),
            ("osc2.wave", 0.0),
            ("osc2.fine", 5.0),
            ("osc.mix", 0.4),
            ("filter.cutoff", 900.0),
            ("filter.resonance", 0.3),
            ("filter.env_amount", 0.7),
            ("filter.key_track", 0.6),
            ("env.amp.attack", 1.0),
            ("env.amp.decay", 380.0),
            ("env.amp.sustain", 0.1),
            ("env.amp.release", 220.0),
            ("env.filter.decay", 260.0),
            ("env.filter.sustain", 0.0),
            ("spread", 0.2),
            ("delay.mix", 0.2),
            ("delay.time", 375.0),
            ("delay.feedback", 0.35),
            ("master.level", -3.5),
        ],
        mods: &[],
    },
    Def {
        name: "Neon Stab",
        device: POLY,
        category: "Lead",
        tags: &["analog", "bright", "stab", "echo"],
        sound: "Two saws a fifth apart through a driven ladder filter: a short, hard chord \
                stab that decays to a murmur, with chorus and a slap echo.",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 0.0),
            ("osc2.semi", 7.0),
            ("osc2.fine", 6.0),
            ("osc.mix", 0.45),
            ("filter.type", 1.0),
            ("filter.drive", 0.3),
            ("filter.cutoff", 2200.0),
            ("filter.resonance", 0.35),
            ("filter.env_amount", 0.6),
            ("env.amp.attack", 2.0),
            ("env.amp.decay", 260.0),
            ("env.amp.sustain", 0.35),
            ("env.amp.release", 180.0),
            ("env.filter.decay", 220.0),
            ("env.filter.sustain", 0.15),
            ("chorus.mix", 0.25),
            ("delay.mix", 0.18),
            ("delay.time", 250.0),
            ("delay.feedback", 0.3),
            ("master.level", 0.0),
        ],
        mods: &[],
    },
    Def {
        name: "Tidewater Strings",
        device: POLY,
        category: "Strings",
        tags: &["analog", "ensemble", "wide", "soft"],
        sound: "A wide saw ensemble with a slow bow-like attack, a gentle level shimmer and a \
                deep chorus.",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 0.0),
            ("osc1.fine", -9.0),
            ("osc2.fine", 11.0),
            ("osc.mix", 0.5),
            ("filter.cutoff", 2600.0),
            ("filter.resonance", 0.1),
            ("filter.env_amount", 0.1),
            ("filter.key_track", 0.6),
            ("env.amp.attack", 350.0),
            ("env.amp.decay", 600.0),
            ("env.amp.sustain", 0.9),
            ("env.amp.release", 700.0),
            ("lfo2.wave", 0.0),
            ("lfo2.rate", 5.5),
            ("lfo2.to_level", 0.06),
            ("drift", 0.3),
            ("spread", 0.8),
            ("chorus.mix", 0.5),
            ("chorus.rate", 0.5),
            ("chorus.depth", 0.6),
            ("master.level", 0.0),
        ],
        mods: &[],
    },
    Def {
        name: "Hollow Reed Lead",
        device: POLY,
        category: "Lead",
        tags: &["analog", "mono", "glide", "vibrato"],
        sound: "A square and a triangle oscillator that glide between notes played legato, \
                with a slow vibrato and a hollow, reedy filter.",
        params: &[
            ("osc1.wave", 1.0),
            ("osc2.wave", 2.0),
            ("osc2.fine", 4.0),
            ("osc.mix", 0.35),
            ("filter.cutoff", 1800.0),
            ("filter.resonance", 0.25),
            ("filter.env_amount", 0.3),
            ("env.amp.attack", 12.0),
            ("env.amp.decay", 200.0),
            ("env.amp.sustain", 0.8),
            ("env.amp.release", 180.0),
            ("lfo.wave", 0.0),
            ("lfo.rate", 5.2),
            ("lfo.to_pitch", 0.18),
            ("voices", 6.0),
            ("glide", 70.0),
            ("legato", 1.0),
            ("delay.mix", 0.15),
            ("master.level", 0.5),
        ],
        mods: &[],
    },
    Def {
        name: "Dusk Clavier",
        device: POLY,
        category: "Keys",
        tags: &["analog", "plucked", "reedy", "dry"],
        sound: "A narrow pulse with a square an octave up, through a resonant filter that \
                follows the keys: a dry, clicky, plucked keyboard tone.",
        params: &[
            ("osc1.wave", 1.0),
            ("osc1.table", 5.0),
            ("osc2.wave", 1.0),
            ("osc2.table", 3.0),
            ("osc2.octave", 1.0),
            ("osc.mix", 0.3),
            ("filter.cutoff", 1500.0),
            ("filter.resonance", 0.45),
            ("filter.env_amount", 0.5),
            ("filter.key_track", 0.8),
            ("env.amp.attack", 1.0),
            ("env.amp.decay", 450.0),
            ("env.amp.sustain", 0.0),
            ("env.amp.release", 150.0),
            ("env.filter.decay", 300.0),
            ("env.filter.sustain", 0.0),
            ("velocity.amp", 0.8),
            ("chorus.mix", 0.2),
            ("master.level", -3.0),
        ],
        mods: &[],
    },
    Def {
        name: "Basalt Sub",
        device: BASS,
        category: "Bass",
        tags: &["sub", "mono", "dark", "round"],
        sound: "A sine and a triangle an octave below the played key: a round, steady sub \
                with a short glide and almost no filter movement.",
        params: &[
            ("osc1.wave", 3.0),
            ("osc1.octave", -1.0),
            ("osc2.wave", 2.0),
            ("osc2.octave", -1.0),
            ("osc.mix", 0.25),
            ("filter.type", 0.0),
            ("filter.cutoff", 380.0),
            ("filter.resonance", 0.1),
            ("filter.env_amount", 0.2),
            ("filter.key_track", 0.3),
            ("env.amp.attack", 3.0),
            ("env.amp.decay", 300.0),
            ("env.amp.sustain", 0.9),
            ("env.amp.release", 120.0),
            ("glide", 25.0),
            ("chorus.mix", 0.0),
            ("master.level", 5.0),
        ],
        mods: &[],
    },
    Def {
        name: "Rubber Floor",
        device: BASS,
        category: "Bass",
        tags: &["acid", "mono", "resonant", "glide"],
        sound: "A saw through a driven, strongly resonant ladder filter that squelches shut on \
                every note, with a lazy glide between notes.",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 1.0),
            ("osc2.octave", -1.0),
            ("osc.mix", 0.25),
            ("filter.cutoff", 420.0),
            ("filter.resonance", 0.72),
            ("filter.env_amount", 0.85),
            ("filter.key_track", 0.4),
            ("filter.drive", 0.4),
            ("env.filter.decay", 180.0),
            ("env.filter.sustain", 0.05),
            ("env.amp.decay", 400.0),
            ("env.amp.sustain", 0.6),
            ("env.amp.release", 80.0),
            ("glide", 80.0),
            ("master.level", 4.5),
        ],
        mods: &[],
    },
    Def {
        name: "Gravel Drive",
        device: BASS,
        category: "Bass",
        tags: &["distorted", "mono", "gritty", "heavy"],
        sound: "A saw and a square an octave down with a little noise, hard into a ladder \
                filter and a second low-pass: a thick, gritty bass.",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 1.0),
            ("osc2.octave", -1.0),
            ("osc.mix", 0.5),
            ("noise.level", 0.04),
            ("filter.cutoff", 700.0),
            ("filter.resonance", 0.3),
            ("filter.env_amount", 0.5),
            ("filter.drive", 0.7),
            ("env.amp.sustain", 0.85),
            ("filter2.mode", 1.0),
            ("filter2.cutoff", 3000.0),
            ("filter2.resonance", 0.15),
            ("chorus.mix", 0.1),
            ("master.level", 6.0),
        ],
        mods: &[],
    },
    Def {
        name: "Aurora Stack",
        device: DUAL,
        category: "Pad",
        tags: &["layered", "bright", "evolving", "bell"],
        sound: "Two layers played together: a slow saw pad underneath and a soft bell-like \
                triangle and sine one and two octaves up that rings out and fades.",
        params: &[
            ("layer.mode", 0.0),
            ("spread", 0.5),
            ("master.level", -4.0),
            ("a.osc1.wave", 0.0),
            ("a.osc2.wave", 0.0),
            ("a.osc2.fine", 8.0),
            ("a.osc.mix", 0.5),
            ("a.filter.cutoff", 1800.0),
            ("a.filter.env_amount", 0.2),
            ("a.env.amp.attack", 500.0),
            ("a.env.amp.sustain", 0.85),
            ("a.env.amp.release", 1200.0),
            ("b.osc1.wave", 2.0),
            ("b.osc1.octave", 1.0),
            ("b.osc2.wave", 3.0),
            ("b.osc2.octave", 2.0),
            ("b.osc.mix", 0.4),
            ("b.filter.cutoff", 6000.0),
            ("b.env.amp.attack", 2.0),
            ("b.env.amp.decay", 900.0),
            ("b.env.amp.sustain", 0.15),
            ("b.env.amp.release", 900.0),
        ],
        mods: &[],
    },
    Def {
        name: "Slow Morph",
        device: DUAL,
        category: "Pad",
        tags: &["layered", "evolving", "soft", "drift"],
        sound: "Halfway between a bright saw layer and a dark sine-and-triangle layer, with \
                every voice drifting a little out of tune.",
        params: &[
            ("layer.mode", 1.0),
            ("layer.morph", 0.5),
            ("layer.age", 0.6),
            ("spread", 0.6),
            ("master.level", -4.0),
            ("a.osc1.wave", 0.0),
            ("a.osc2.wave", 0.0),
            ("a.osc.mix", 0.5),
            ("a.filter.cutoff", 5000.0),
            ("a.env.amp.attack", 700.0),
            ("a.env.amp.sustain", 0.85),
            ("a.env.amp.release", 1500.0),
            ("b.osc1.wave", 3.0),
            ("b.osc2.wave", 2.0),
            ("b.osc2.octave", -1.0),
            ("b.osc.mix", 0.4),
            ("b.filter.cutoff", 900.0),
            ("b.env.amp.attack", 900.0),
            ("b.env.amp.sustain", 0.9),
            ("b.env.amp.release", 1800.0),
        ],
        mods: &[],
    },
    Def {
        name: "Rust Bell",
        device: FM,
        category: "Bell",
        tags: &["fm", "metallic", "inharmonic", "ringing"],
        sound: "Two pairs of operators at ratios that do not line up (2.32, 3.7 and 6.1 over \
                the played pitch): a long, metallic, slightly out-of-tune bell.",
        params: &[
            ("algorithm", 1.0),
            ("op1.coarse", 1.0),
            ("op1.level", 0.7),
            ("op1.velocity", 0.3),
            ("op1.attack", 1.0),
            ("op1.decay", 4200.0),
            ("op1.sustain", 0.0),
            ("op1.release", 2200.0),
            ("op2.coarse", 2.0),
            ("op2.fine", 0.32),
            ("op2.level", 0.5),
            ("op2.velocity", 0.6),
            ("op2.attack", 1.0),
            ("op2.decay", 2400.0),
            ("op2.sustain", 0.0),
            ("op2.release", 1500.0),
            ("op3.coarse", 3.0),
            ("op3.fine", 0.7),
            ("op3.level", 0.4),
            ("op3.velocity", 0.3),
            ("op3.attack", 1.0),
            ("op3.decay", 3200.0),
            ("op3.sustain", 0.0),
            ("op3.release", 1800.0),
            ("op4.coarse", 6.0),
            ("op4.fine", 0.1),
            ("op4.level", 0.3),
            ("op4.velocity", 0.5),
            ("op4.attack", 1.0),
            ("op4.decay", 1100.0),
            ("op4.sustain", 0.0),
            ("op4.release", 800.0),
            ("master.level", -4.0),
        ],
        mods: &[],
    },
    Def {
        name: "Velvet Tines",
        device: FM,
        category: "Keys",
        tags: &["fm", "soft", "tremolo", "mellow"],
        sound: "A soft electric-keys tone: a mellow body, a detuned second voice and a short \
                bright tine on the attack, with a light tremolo and chorus.",
        params: &[
            ("algorithm", 1.0),
            ("spread", 0.35),
            ("op1.coarse", 1.0),
            ("op1.level", 0.75),
            ("op1.velocity", 0.3),
            ("op1.attack", 1.0),
            ("op1.decay", 2200.0),
            ("op1.sustain", 0.1),
            ("op1.release", 320.0),
            ("op2.coarse", 1.0),
            ("op2.level", 0.28),
            ("op2.velocity", 0.6),
            ("op2.attack", 1.0),
            ("op2.decay", 1400.0),
            ("op2.sustain", 0.05),
            ("op2.release", 300.0),
            ("op3.coarse", 1.0),
            ("op3.detune", 5.0),
            ("op3.level", 0.45),
            ("op3.velocity", 0.3),
            ("op3.attack", 1.0),
            ("op3.decay", 2300.0),
            ("op3.sustain", 0.1),
            ("op3.release", 320.0),
            ("op4.coarse", 7.0),
            ("op4.level", 0.14),
            ("op4.velocity", 0.8),
            ("op4.attack", 1.0),
            ("op4.decay", 120.0),
            ("op4.sustain", 0.0),
            ("op4.release", 100.0),
            ("lfo.rate", 4.0),
            ("lfo.to_amp", 0.1),
            ("chorus.mix", 0.25),
            ("master.level", 0.0),
        ],
        mods: &[],
    },
    Def {
        name: "Iron Bass",
        device: FM,
        category: "Bass",
        tags: &["fm", "mono", "glide", "punchy"],
        sound: "A mono three-modulator stack with feedback that is bright for an instant on \
                every note and settles to a solid tone, with a very short pitch drop.",
        params: &[
            ("algorithm", 0.0),
            ("feedback", 0.35),
            ("play.mode", 1.0),
            ("play.glide", 25.0),
            ("pitch.env_amount", 5.0),
            ("pitch.env_decay", 40.0),
            ("op1.coarse", 1.0),
            ("op1.level", 0.9),
            ("op1.velocity", 0.3),
            ("op1.attack", 1.0),
            ("op1.decay", 500.0),
            ("op1.sustain", 0.7),
            ("op1.release", 90.0),
            ("op2.coarse", 1.0),
            ("op2.level", 0.45),
            ("op2.velocity", 0.5),
            ("op2.attack", 1.0),
            ("op2.decay", 250.0),
            ("op2.sustain", 0.25),
            ("op2.release", 90.0),
            ("op3.coarse", 2.0),
            ("op3.level", 0.3),
            ("op3.velocity", 0.5),
            ("op3.attack", 1.0),
            ("op3.decay", 140.0),
            ("op3.sustain", 0.1),
            ("op3.release", 90.0),
            ("op4.coarse", 1.0),
            ("op4.level", 0.35),
            ("op4.velocity", 0.4),
            ("op4.attack", 1.0),
            ("op4.decay", 90.0),
            ("op4.sustain", 0.0),
            ("op4.release", 90.0),
            ("master.level", 3.5),
        ],
        mods: &[],
    },
    Def {
        name: "Glass Choir",
        device: FM,
        category: "Pad",
        tags: &["fm", "airy", "slow", "wide"],
        sound: "Two detuned sine carriers lightly coloured by two slow modulators, spread over \
                three unison voices: a breathy, glassy vocal pad that swells in.",
        params: &[
            ("algorithm", 8.0),
            ("index", 0.8),
            ("unison.voices", 3.0),
            ("unison.detune", 12.0),
            ("unison.spread", 0.8),
            ("op1.coarse", 1.0),
            ("op1.level", 0.55),
            ("op1.velocity", 0.2),
            ("op1.attack", 700.0),
            ("op1.decay", 1500.0),
            ("op1.sustain", 0.8),
            ("op1.release", 1400.0),
            ("op2.coarse", 1.0),
            ("op2.detune", 8.0),
            ("op2.level", 0.5),
            ("op2.velocity", 0.2),
            ("op2.attack", 800.0),
            ("op2.decay", 1500.0),
            ("op2.sustain", 0.8),
            ("op2.release", 1400.0),
            ("op3.coarse", 2.0),
            ("op3.level", 0.18),
            ("op3.attack", 900.0),
            ("op3.decay", 1800.0),
            ("op3.sustain", 0.7),
            ("op3.release", 1400.0),
            ("op4.coarse", 3.0),
            ("op4.level", 0.12),
            ("op4.attack", 1100.0),
            ("op4.decay", 1800.0),
            ("op4.sustain", 0.7),
            ("op4.release", 1400.0),
            ("lfo.rate", 0.3),
            ("lfo.to_index", 0.15),
            ("chorus.mix", 0.3),
            ("master.level", -1.5),
        ],
        mods: &[],
    },
    Def {
        name: "Pebble Mallet",
        device: FM,
        category: "Pluck",
        tags: &["fm", "wooden", "percussive", "dry"],
        sound: "A sine struck by a very quick modulator at four times its pitch: a short, \
                dry, wooden mallet note that dies at once.",
        params: &[
            ("algorithm", 0.0),
            ("spread", 0.3),
            ("op1.coarse", 1.0),
            ("op1.level", 0.8),
            ("op1.velocity", 0.4),
            ("op1.attack", 1.0),
            ("op1.decay", 700.0),
            ("op1.sustain", 0.0),
            ("op1.release", 200.0),
            ("op2.coarse", 4.0),
            ("op2.level", 0.35),
            ("op2.velocity", 0.8),
            ("op2.attack", 1.0),
            ("op2.decay", 80.0),
            ("op2.sustain", 0.0),
            ("op2.release", 80.0),
            ("master.level", -5.0),
        ],
        mods: &[],
    },
    Def {
        name: "Mist Bed",
        device: GRAIN,
        category: "Texture",
        tags: &["granular", "soft", "slow", "vocal"],
        sound: "Long, soft grains of the built-in vowel source, scattered wide and drifting \
                slowly through it: a breathy, shifting bed that swells in and fades out.",
        params: &[
            ("grain.density", 40.0),
            ("grain.size", 140.0),
            ("grain.window", 0.0),
            ("grain.timing", 0.4),
            ("grain.spread", 0.9),
            ("pos.position", 0.3),
            ("pos.spray", 0.15),
            ("pos.scan", 0.08),
            ("env.amp.attack", 1200.0),
            ("env.amp.decay", 400.0),
            ("env.amp.sustain", 1.0),
            ("env.amp.release", 1800.0),
            ("lfo.wave", 0.0),
            ("lfo.rate", 0.12),
            ("lfo.to_position", 0.1),
            ("filter.enabled", 1.0),
            ("filter.cutoff", 5000.0),
            ("master.level", 1.5),
        ],
        mods: &[],
    },
    Def {
        name: "Scatter Dust",
        device: GRAIN,
        category: "Texture",
        tags: &["granular", "glitchy", "bright", "random"],
        sound: "A fine spray of very short, percussive grains, some of them backwards, with \
                random pitch: a glittering cloud that thins out after each note.",
        params: &[
            ("grain.density", 90.0),
            ("grain.size", 14.0),
            ("grain.window", 2.0),
            ("grain.timing", 0.7),
            ("grain.reverse", 0.3),
            ("grain.spread", 1.0),
            ("pos.position", 0.4),
            ("pos.spray", 0.5),
            ("pitch.random", 300.0),
            ("env.amp.attack", 5.0),
            ("env.amp.decay", 300.0),
            ("env.amp.sustain", 0.4),
            ("env.amp.release", 500.0),
            ("master.level", 4.5),
        ],
        mods: &[],
    },
    Def {
        name: "Slow Tide",
        device: GRAIN,
        category: "Texture",
        tags: &["granular", "evolving", "legato", "warm"],
        sound: "Large, smooth grains that scan steadily through the source, played legato with \
                a slow glide and a gentle pitch wobble: a rolling, tidal drone.",
        params: &[
            ("grain.density", 18.0),
            ("grain.size", 300.0),
            ("grain.window", 1.0),
            ("pos.position", 0.1),
            ("pos.scan", 0.4),
            ("pos.spray", 0.05),
            ("voice.mode", 2.0),
            ("voice.glide", 120.0),
            ("lfo.wave", 0.0),
            ("lfo.rate", 0.4),
            ("lfo.to_pitch", 0.1),
            ("env.amp.attack", 800.0),
            ("env.amp.release", 1500.0),
            ("master.level", 6.0),
        ],
        mods: &[],
    },
    Def {
        name: "Vowel Drift",
        device: WAVE,
        category: "Pad",
        tags: &["wavetable", "vocal", "evolving", "wide"],
        sound: "The vowel table with its position moved slowly by an envelope and a slow \
                LFO, an octave of sine-to-saw below it, and three unison voices.",
        params: &[
            ("osc1.table", 5.0),
            ("osc1.position", 0.1),
            ("osc1.level", 0.7),
            ("osc2.table", 2.0),
            ("osc2.octave", -1.0),
            ("osc2.level", 0.25),
            ("env.mod.to_pos1", 0.6),
            ("env.mod.decay", 1800.0),
            ("lfo.rate", 0.35),
            ("lfo.to_pos1", 0.25),
            ("filter.cutoff", 5500.0),
            ("unison.voices", 3.0),
            ("unison.detune", 14.0),
            ("unison.spread", 0.7),
            ("env.amp.attack", 250.0),
            ("env.amp.sustain", 0.8),
            ("env.amp.release", 900.0),
            ("master.level", -2.5),
        ],
        mods: &[],
    },
    Def {
        name: "Sync Razor",
        device: WAVE,
        category: "Lead",
        tags: &["wavetable", "sharp", "bright", "sync"],
        sound: "The hard-sync sweep table, sharpest at the start of each note and softening as \
                the position envelope closes, with two detuned unison voices.",
        params: &[
            ("osc1.table", 7.0),
            ("osc1.position", 0.3),
            ("osc1.level", 0.7),
            ("osc2.level", 0.0),
            ("env.mod.to_pos1", 0.7),
            ("env.mod.decay", 500.0),
            ("env.mod.sustain", 0.3),
            ("filter.cutoff", 6500.0),
            ("filter.resonance", 0.2),
            ("unison.voices", 2.0),
            ("unison.detune", 10.0),
            ("env.amp.attack", 3.0),
            ("env.amp.sustain", 0.8),
            ("env.amp.release", 150.0),
            ("voices", 6.0),
            ("master.level", -2.0),
        ],
        mods: &[],
    },
    Def {
        name: "Harmonic Sunrise",
        device: WAVE,
        category: "Pad",
        tags: &["wavetable", "slow", "evolving", "bright"],
        sound: "The harmonic sweep table opening from dark to bright over several seconds, \
                with a hollow sine an octave below and four wide unison voices.",
        params: &[
            ("osc1.table", 6.0),
            ("osc1.position", 0.0),
            ("osc1.level", 0.7),
            ("osc2.table", 3.0),
            ("osc2.octave", -1.0),
            ("osc2.level", 0.4),
            ("env.mod.attack", 3000.0),
            ("env.mod.decay", 4000.0),
            ("env.mod.sustain", 0.6),
            ("env.mod.to_pos1", 0.9),
            ("filter.cutoff", 4200.0),
            ("unison.voices", 4.0),
            ("unison.detune", 18.0),
            ("unison.spread", 0.8),
            ("env.amp.attack", 1400.0),
            ("env.amp.sustain", 0.9),
            ("env.amp.release", 2200.0),
            ("master.level", -3.5),
        ],
        mods: &[],
    },
    Def {
        name: "Porch Light Organ",
        device: ORGAN,
        category: "Organ",
        tags: &["organ", "soft", "vibrato", "warm"],
        sound: "Soft drawbars (the low registers full, a gentle octave and a hint of the \
                higher ones) with a slow scanner vibrato and a little key click.",
        params: &[
            ("drawbar.16", 6.0),
            ("drawbar.5_1_3", 0.0),
            ("drawbar.8", 8.0),
            ("drawbar.4", 5.0),
            ("drawbar.2_2_3", 0.0),
            ("drawbar.2", 3.0),
            ("drawbar.1_3_5", 0.0),
            ("drawbar.1_1_3", 0.0),
            ("drawbar.1", 0.0),
            ("perc.mode", 0.0),
            ("key.click", 0.2),
            ("scanner.type", 1.0),
            ("scanner.rate", 5.8),
            ("out.drive", 0.15),
            ("out.level", -0.5),
        ],
        mods: &[],
    },
    Def {
        name: "Brass Rail",
        device: ORGAN,
        category: "Organ",
        tags: &["organ", "bright", "driven", "percussive"],
        sound: "Nearly every drawbar out, with a fast third-harmonic attack, a chorus-style \
                scanner and the output driven: a loud, bright, biting organ.",
        params: &[
            ("drawbar.16", 8.0),
            ("drawbar.5_1_3", 8.0),
            ("drawbar.8", 8.0),
            ("drawbar.4", 6.0),
            ("drawbar.2_2_3", 4.0),
            ("drawbar.2", 5.0),
            ("drawbar.1_3_5", 3.0),
            ("drawbar.1_1_3", 4.0),
            ("drawbar.1", 5.0),
            ("perc.mode", 2.0),
            ("perc.decay", 0.0),
            ("perc.level", 1.0),
            ("scanner.type", 5.0),
            ("out.drive", 0.45),
            ("out.level", 2.0),
        ],
        mods: &[],
    },
    Def {
        name: "Sequenced Pulse",
        device: SEMI,
        category: "Modular",
        tags: &["semi-modular", "sequence", "pulse", "rhythmic"],
        sound: "A narrow pulse and a saw through a resonant 24 dB filter, played by the built-in \
                sixteen-step sequencer from the keys: a short, hopping line whose cutoff \
                follows the step pitch and whose pulse width moves a little on every note.",
        params: &[
            ("play.mode", 1.0),
            ("master.level", 6.0),
            ("osc1.type", 1.0),
            ("osc1.shape", 0.3),
            ("osc1.level", 1.0),
            ("osc2.type", 0.0),
            ("osc2.octave", -1.0),
            ("osc2.fine", 6.0),
            ("osc2.level", 0.7),
            ("filter1.mode", 5.0),
            ("filter1.cutoff", 650.0),
            ("filter1.resonance", 0.35),
            ("filter1.env", 2.5),
            ("filter1.track", 0.5),
            ("env.filter.decay", 220.0),
            ("env.filter.sustain", 0.1),
            ("env.amp.attack", 1.0),
            ("env.amp.decay", 240.0),
            ("env.amp.sustain", 0.3),
            ("env.amp.release", 110.0),
            ("seq.run", 1.0),
            ("seq.rate", 3.0),
            ("seq.gate", 55.0),
            ("seq.pitch.1", 0.0),
            ("seq.pitch.2", 0.0),
            ("seq.pitch.3", 12.0),
            ("seq.pitch.4", 0.0),
            ("seq.pitch.5", 7.0),
            ("seq.pitch.6", 0.0),
            ("seq.pitch.7", 10.0),
            ("seq.pitch.8", 0.0),
            ("seq.pitch.9", 0.0),
            ("seq.pitch.10", 12.0),
            ("seq.pitch.11", 0.0),
            ("seq.pitch.12", 5.0),
            ("seq.pitch.13", 7.0),
            ("seq.pitch.14", 3.0),
            ("seq.pitch.15", 0.0),
            ("seq.pitch.16", -5.0),
            ("seq.on.4", 0.0),
            ("seq.on.8", 0.0),
            ("seq.on.12", 0.0),
        ],
        mods: &[
            ModLink {
                source: "step",
                target: "filter1_cutoff",
                via: "",
                amount: 0.18,
            },
            ModLink {
                source: "random",
                target: "osc1_shape",
                via: "",
                amount: 0.25,
            },
        ],
    },
    Def {
        name: "Fold Drone",
        device: SEMI,
        category: "Modular",
        tags: &["semi-modular", "drone", "fold", "evolving"],
        sound: "A sine, a triangle an octave down and a quiet saw two octaves down, played \
                legato with a long glide into a wave folder whose drive is moved by a very slow \
                LFO and by the mod wheel: a deep, slowly changing drone.",
        params: &[
            ("play.mode", 2.0),
            ("play.glide", 220.0),
            ("master.level", 6.0),
            ("osc1.type", 3.0),
            ("osc1.level", 0.7),
            ("osc2.type", 2.0),
            ("osc2.octave", -1.0),
            ("osc2.fine", 5.0),
            ("osc2.level", 0.5),
            ("osc3.type", 0.0),
            ("osc3.octave", -2.0),
            ("osc3.level", 0.25),
            ("filter1.mode", 1.0),
            ("filter1.cutoff", 1500.0),
            ("filter1.resonance", 0.15),
            ("filter1.env", 0.0),
            ("filter1.track", 0.4),
            ("shaper.type", 4.0),
            ("shaper.drive", 12.0),
            ("shaper.mix", 0.7),
            ("env.amp.attack", 1400.0),
            ("env.amp.decay", 800.0),
            ("env.amp.sustain", 1.0),
            ("env.amp.release", 2200.0),
            ("lfo1.wave", 0.0),
            ("lfo1.rate", 0.09),
            ("lfo2.wave", 1.0),
            ("lfo2.rate", 0.17),
        ],
        mods: &[
            ModLink {
                source: "lfo1",
                target: "shaper_drive",
                via: "",
                amount: 0.35,
            },
            ModLink {
                source: "lfo2",
                target: "osc1_shape",
                via: "",
                amount: 0.2,
            },
            ModLink {
                source: "mod_wheel",
                target: "shaper_drive",
                via: "",
                amount: 0.4,
            },
        ],
    },
    Def {
        name: "Comb Pluck",
        device: SEMI,
        category: "Modular",
        tags: &["semi-modular", "pluck", "comb", "plucked-string"],
        sound: "A burst of noise sent into a resonant comb filter that follows the keys, a \
                low-pass behind it and a short envelope: a plucked, string-like note that rings \
                for about a second.",
        params: &[
            ("master.level", 6.0),
            ("osc1.type", 7.0),
            ("osc1.level", 1.0),
            ("osc2.level", 0.0),
            ("filter1.mode", 6.0),
            ("filter1.cutoff", 262.0),
            ("filter1.resonance", 0.82),
            ("filter1.env", 0.0),
            ("filter1.track", 1.0),
            ("filter2.mode", 1.0),
            ("filter2.cutoff", 5000.0),
            ("filter2.resonance", 0.1),
            ("filter2.env", 0.0),
            ("filter2.track", 0.5),
            ("filter.routing", 0.0),
            ("shaper.type", 1.0),
            ("shaper.drive", 14.0),
            ("shaper.mix", 1.0),
            ("env.amp.attack", 0.5),
            ("env.amp.decay", 900.0),
            ("env.amp.sustain", 0.0),
            ("env.amp.release", 300.0),
        ],
        mods: &[ModLink {
            source: "velocity",
            target: "osc1_level",
            via: "",
            amount: 0.3,
        }],
    },
    Def {
        name: "Vocal Resonance Tamer",
        device: DYNAMIC_EQ,
        category: "Vocal",
        tags: &["dynamic eq", "vocal", "resonance", "smooth"],
        sound: "Three narrow bells at 300 Hz, 2.4 kHz and 5.2 kHz that cut by up to six decibels \
                only while a voice pushes a resonance there: a boxy note or a harsh syllable \
                is smoothed and the rest of the take is left alone.",
        params: &[
            ("band2.on", 1.0),
            ("band2.freq", 300.0),
            ("band2.q", 5.0),
            ("band2.threshold", -30.0),
            ("band2.range", -6.0),
            ("band2.attack", 8.0),
            ("band2.release", 120.0),
            ("band3.on", 1.0),
            ("band3.freq", 2400.0),
            ("band3.q", 6.0),
            ("band3.threshold", -28.0),
            ("band3.range", -5.0),
            ("band3.attack", 5.0),
            ("band3.release", 80.0),
            ("band4.on", 1.0),
            ("band4.freq", 5200.0),
            ("band4.q", 6.0),
            ("band4.threshold", -30.0),
            ("band4.range", -6.0),
            ("band4.attack", 3.0),
            ("band4.release", 60.0),
        ],
        mods: &[],
    },
    Def {
        name: "De-boxer",
        device: DYNAMIC_EQ,
        category: "Drums",
        tags: &["dynamic eq", "drums", "boxy", "room"],
        sound: "Two wide bells at 400 and 650 Hz that pull the cardboard-box resonance of a drum \
                or a room recording down by up to seven decibels on the loud hits, and let the \
                quiet tails keep their body.",
        params: &[
            ("band2.on", 1.0),
            ("band2.freq", 400.0),
            ("band2.q", 1.4),
            ("band2.threshold", -26.0),
            ("band2.range", -7.0),
            ("band2.ratio", 3.0),
            ("band2.attack", 15.0),
            ("band2.release", 150.0),
            ("band3.on", 1.0),
            ("band3.freq", 650.0),
            ("band3.q", 2.0),
            ("band3.threshold", -28.0),
            ("band3.range", -4.0),
            ("band3.ratio", 3.0),
            ("band3.attack", 15.0),
            ("band3.release", 150.0),
        ],
        mods: &[],
    },
    Def {
        name: "Air Lift",
        device: DYNAMIC_EQ,
        category: "Mix",
        tags: &["dynamic eq", "air", "bright", "lift"],
        sound: "A high shelf at 10.5 kHz with a small fixed lift that opens another three and a \
                half decibels as the top end gets louder: a dull mix gains air on its bright \
                moments without hiss in the gaps.",
        params: &[
            ("band5.on", 1.0),
            ("band5.freq", 10500.0),
            ("band5.q", 0.6),
            ("band5.gain", 1.0),
            ("band5.threshold", -32.0),
            ("band5.range", 3.5),
            ("band5.ratio", 3.0),
            ("band5.attack", 25.0),
            ("band5.release", 250.0),
        ],
        mods: &[],
    },
    Def {
        name: "Drum Bus De-harsh",
        device: DYNAMIC_EQ,
        category: "Drums",
        tags: &["dynamic eq", "drums", "bus", "smooth"],
        sound: "Two bells at 3.2 and 6.5 kHz that take up to five decibels off the bite of a \
                drum bus only while the hits are loud, so the snare crack and the hats stay \
                lively at low level and stop stinging when they are pushed.",
        params: &[
            ("band3.on", 1.0),
            ("band3.freq", 3200.0),
            ("band3.q", 2.0),
            ("band3.threshold", -24.0),
            ("band3.range", -5.0),
            ("band3.attack", 3.0),
            ("band3.release", 60.0),
            ("band4.on", 1.0),
            ("band4.freq", 6500.0),
            ("band4.q", 1.5),
            ("band4.threshold", -26.0),
            ("band4.range", -4.0),
            ("band4.attack", 2.0),
            ("band4.release", 50.0),
        ],
        mods: &[],
    },
    Def {
        name: "Master Peak Trim",
        device: CLIPPER,
        category: "Mastering",
        tags: &["clipper", "master", "peaks", "transparent"],
        sound: "A hard clipper with a ceiling one decibel under full scale and eight times \
                oversampling: it shaves the few tall peaks a mix has, a fraction of a decibel \
                at a time, so the limiter after it has less to do.",
        params: &[
            ("gain", 1.5),
            ("ceiling", -1.0),
            ("curve", 0.0),
            ("oversample", 3.0),
        ],
        mods: &[],
    },
    Def {
        name: "Drum Bus Soft Clip",
        device: CLIPPER,
        category: "Drums",
        tags: &["clipper", "drums", "soft", "density"],
        sound: "A soft-knee clipper driven six decibels into a ceiling three under full scale: \
                the snare and kick tops are rounded off and the bus gets denser and louder \
                without a hard edge.",
        params: &[
            ("gain", 6.0),
            ("ceiling", -3.0),
            ("curve", 0.7),
            ("oversample", 2.0),
            ("output", 1.0),
        ],
        mods: &[],
    },
    Def {
        name: "Bass Warm Clip",
        device: CLIPPER,
        category: "Bass",
        tags: &["clipper", "bass", "warm", "even harmonics"],
        sound: "A soft clipper with a little asymmetry that drives only the band under 160 Hz: \
                the bass picks up even harmonics and a thicker body while the highs pass \
                untouched.",
        params: &[
            ("gain", 0.0),
            ("ceiling", -4.0),
            ("curve", 0.9),
            ("symmetry", 0.25),
            ("oversample", 2.0),
            ("bands", 1.0),
            ("xover.low", 160.0),
            ("band1.drive", 8.0),
            ("band2.drive", 0.0),
            ("output", -1.0),
        ],
        mods: &[],
    },
    Def {
        name: "Kick Click Tamer",
        device: CLIPPER,
        category: "Drums",
        tags: &["clipper", "kick", "click", "tame"],
        sound: "A hard clipper that works on the click of a kick above 1.5 kHz: the beater is \
                driven into the ceiling and loses its sharp spike, the body below passes \
                under it untouched.",
        params: &[
            ("gain", 0.0),
            ("ceiling", -4.0),
            ("curve", 0.1),
            ("oversample", 2.0),
            ("bands", 1.0),
            ("xover.low", 1500.0),
            ("band1.drive", -6.0),
            ("band2.drive", 6.0),
        ],
        mods: &[],
    },
    Def {
        name: "Clean Kick Sub",
        device: POLY,
        category: "Bass",
        tags: &["kick", "sub", "sine", "reset phase"],
        sound: "Two sines an octave and two octaves down with a short pitch drop, started at \
                the same angle on every note: each hit begins with the identical first cycle, \
                so a layered kick adds up the same way every time.",
        params: &[
            ("osc1.wave", 3.0),
            ("osc1.octave", -1.0),
            ("osc2.wave", 3.0),
            ("osc2.octave", -2.0),
            ("osc.mix", 0.35),
            ("osc.phase", 0.0),
            ("osc.phase_start", 0.0),
            ("filter.cutoff", 420.0),
            ("filter.resonance", 0.05),
            ("filter.env_amount", 0.0),
            ("filter.key_track", 0.0),
            ("env.amp.attack", 1.0),
            ("env.amp.decay", 380.0),
            ("env.amp.sustain", 0.0),
            ("env.amp.release", 30.0),
            ("env.mod.attack", 1.0),
            ("env.mod.decay", 55.0),
            ("env.mod.sustain", 0.0),
            ("env.mod.release", 20.0),
            ("env.mod.amount", 0.45),
            ("env.mod.dest", 0.0),
            ("voices", 1.0),
            ("master.level", 3.0),
        ],
        mods: &[],
    },
    Def {
        name: "Mono Sub Bass",
        device: BASS,
        category: "Bass",
        tags: &["sub", "mono", "reset phase", "clean"],
        sound: "A sine an octave down with a trace of triangle above it, one voice, a short \
                twelve millisecond release and the filter following the key: a clean sub whose \
                notes all start at the same angle and end without a click.",
        params: &[
            ("osc1.wave", 3.0),
            ("osc1.octave", -1.0),
            ("osc2.wave", 2.0),
            ("osc.mix", 0.15),
            ("osc.phase", 0.0),
            ("osc.phase_start", 0.0),
            ("filter.cutoff", 500.0),
            ("filter.resonance", 0.08),
            ("filter.env_amount", 0.1),
            ("filter.key_track", 0.5),
            ("env.amp.attack", 2.0),
            ("env.amp.decay", 250.0),
            ("env.amp.sustain", 0.85),
            ("env.amp.release", 12.0),
            ("voices", 1.0),
            ("master.level", 5.0),
        ],
        mods: &[],
    },
    Def {
        name: "Reset Pluck Bass",
        device: WAVE,
        category: "Bass",
        tags: &["pluck", "bass", "wavetable", "reset phase"],
        sound: "The sine-to-saw table closing from bright to dark in a fifth of a second, one \
                voice, every note started on the top of the wave: a short plucked bass with \
                the same snap on each note.",
        params: &[
            ("osc1.table", 2.0),
            ("osc1.position", 0.2),
            ("osc1.level", 0.8),
            ("osc2.level", 0.0),
            ("osc.phase", 0.0),
            ("osc.phase_start", 90.0),
            ("env.mod.attack", 1.0),
            ("env.mod.decay", 220.0),
            ("env.mod.sustain", 0.0),
            ("env.mod.to_pos1", 0.7),
            ("filter.cutoff", 1800.0),
            ("filter.resonance", 0.15),
            ("filter.env_amount", 0.5),
            ("filter.key_track", 0.5),
            ("env.amp.attack", 1.0),
            ("env.amp.decay", 350.0),
            ("env.amp.sustain", 0.0),
            ("env.amp.release", 60.0),
            ("voices", 1.0),
            ("master.level", -1.0),
        ],
        mods: &[],
    },
    Def {
        name: "Four Floor Pump",
        device: DUCK,
        category: "Bass",
        tags: &["sidechain", "pump", "curve", "kick"],
        sound: "A drawn curve over a quarter note: the level drops to nothing within a few \
                milliseconds of every kick and swells back along a long, smooth line, so the \
                bass and pads breathe with a four on the floor kick. Put the kick on the \
                sidechain input.",
        params: &[
            ("mode", 3.0),
            ("depth", 0.85),
            ("length", 2.0),
            ("threshold", -24.0),
            ("hidden.curve_n", 4.0),
            ("hidden.curve_p1_x", 0.0),
            ("hidden.curve_p1_y", 0.0),
            ("hidden.curve_p2_x", 0.03),
            ("hidden.curve_p2_y", 1.0),
            ("hidden.curve_p2_bend", -24.0),
            ("hidden.curve_p3_x", 0.3),
            ("hidden.curve_p3_y", 0.35),
            ("hidden.curve_p3_bend", -16.0),
            ("hidden.curve_p4_x", 1.0),
            ("hidden.curve_p4_y", 0.0),
        ],
        mods: &[],
    },
    Def {
        name: "Sub Only Duck",
        device: DUCK,
        category: "Bass",
        tags: &["sidechain", "band", "low end", "kick"],
        sound: "The same quick drop and slow return, but only the part of the sound under \
                120 Hz is ducked: the kick keeps the low end to itself while the highs of the \
                bass line stay at full level.",
        params: &[
            ("mode", 3.0),
            ("depth", 1.0),
            ("length", 2.0),
            ("threshold", -24.0),
            ("band", 1.0),
            ("band_freq", 120.0),
            ("hidden.curve_n", 4.0),
            ("hidden.curve_p1_x", 0.0),
            ("hidden.curve_p1_y", 0.0),
            ("hidden.curve_p2_x", 0.025),
            ("hidden.curve_p2_y", 1.0),
            ("hidden.curve_p2_bend", -24.0),
            ("hidden.curve_p3_x", 0.35),
            ("hidden.curve_p3_y", 0.4),
            ("hidden.curve_p3_bend", -12.0),
            ("hidden.curve_p4_x", 1.0),
            ("hidden.curve_p4_y", 0.0),
        ],
        mods: &[],
    },
    Def {
        name: "Half Bar Breath",
        device: DUCK,
        category: "Pad",
        tags: &["sidechain", "pad", "slow", "curve"],
        sound: "A gentle duck over a half note: the level eases down over a tenth of the curve, \
                holds a little under full level and rises back slowly, so a pad leaves room for \
                a kick or a chord stab without an obvious pump.",
        params: &[
            ("mode", 3.0),
            ("depth", 0.55),
            ("length", 3.0),
            ("threshold", -26.0),
            ("hidden.curve_n", 4.0),
            ("hidden.curve_p1_x", 0.0),
            ("hidden.curve_p1_y", 0.0),
            ("hidden.curve_p2_x", 0.1),
            ("hidden.curve_p2_y", 1.0),
            ("hidden.curve_p2_bend", 8.0),
            ("hidden.curve_p3_x", 0.45),
            ("hidden.curve_p3_y", 0.6),
            ("hidden.curve_p3_bend", 12.0),
            ("hidden.curve_p4_x", 1.0),
            ("hidden.curve_p4_y", 0.0),
        ],
        mods: &[],
    },
    Def {
        name: "Eighth Chop",
        device: DUCK,
        category: "Keys",
        tags: &["sidechain", "chop", "rhythm", "curve"],
        sound: "A short curve over an eighth note that cuts the level hard on every hit and \
                lets it climb back in a straight line over the rest of the note: a chopped, \
                rhythmic movement for keys and stabs that follows the tempo.",
        params: &[
            ("mode", 3.0),
            ("depth", 0.9),
            ("length", 1.0),
            ("threshold", -24.0),
            ("hidden.curve_n", 3.0),
            ("hidden.curve_p1_x", 0.0),
            ("hidden.curve_p1_y", 0.0),
            ("hidden.curve_p2_x", 0.06),
            ("hidden.curve_p2_y", 1.0),
            ("hidden.curve_p3_x", 1.0),
            ("hidden.curve_p3_y", 0.0),
        ],
        mods: &[],
    },
];

/// Runs `command` on `doc`, naming the item when the model refuses it.
fn run(doc: &mut Document, registry: &Registry, item: &str, command: Command) {
    doc.apply(command, registry)
        .unwrap_or_else(|e| panic!("{item}: {e}"));
}

/// Sets `params` on the device `key` of `doc`, checking each value against the descriptor of
/// `type_id` first (a typo in a key or a value out of range stops the writer).
fn set_params(
    doc: &mut Document,
    registry: &Registry,
    item: &str,
    key: &DeviceKey,
    type_id: &str,
    params: &[(&str, f32)],
) {
    let desc = registry
        .descriptor(type_id)
        .unwrap_or_else(|| panic!("{item}: no device {type_id}"));
    for &(param, value) in params {
        let p = desc
            .param_by_key(param)
            .unwrap_or_else(|| panic!("{item}: {type_id} has no param {param}"));
        assert!(
            (p.min..=p.max).contains(&value),
            "{item}: {param} = {value} is outside {}..{}",
            p.min,
            p.max
        );
        run(
            doc,
            registry,
            item,
            Command::SetParam {
                key: key.clone(),
                param: param.to_owned(),
                value,
                merge: None,
            },
        );
    }
}

/// Fills the modulation slots of the device `key` from `links` (slot `n` is link `n`).
fn set_mods(
    doc: &mut Document,
    registry: &Registry,
    item: &str,
    key: &DeviceKey,
    type_id: &str,
    links: &[ModLink],
) {
    if links.is_empty() {
        return;
    }
    let tables = registry
        .mod_tables(type_id)
        .unwrap_or_else(|| panic!("{item}: {type_id} has no modulation matrix"));
    let source = |name: &str| {
        tables
            .sources
            .iter()
            .find(|s| s.key == name)
            .unwrap_or_else(|| panic!("{item}: {type_id} has no mod source {name}"))
            .id
    };
    let target = |name: &str| {
        tables
            .targets
            .iter()
            .find(|t| t.key == name)
            .unwrap_or_else(|| panic!("{item}: {type_id} has no mod target {name}"))
            .id
    };
    for (slot, link) in links.iter().enumerate() {
        let mut value = ModSlot::new(source(link.source), target(link.target));
        if !link.via.is_empty() {
            value = value.via(source(link.via));
        }
        let slot = u8::try_from(slot).expect("a few slots");
        run(
            doc,
            registry,
            item,
            Command::Workflow(WorkflowCommand::SetModSlot {
                device: key.clone(),
                slot,
                value: Some(value),
            }),
        );
        set_params(
            doc,
            registry,
            item,
            key,
            type_id,
            &[(&amount_param(usize::from(slot)), link.amount)],
        );
    }
}

/// The text of the item file of `def`: the device with `def`'s params, named for the patch.
///
/// # Panics
/// A key the device does not have, or a value outside its range.
pub(super) fn item_text(def: &Def) -> String {
    let registry = default_registry();
    let key = DeviceKey("synth".to_owned());
    let mut doc = Document::new(empty_doc());
    run(
        &mut doc,
        &registry,
        def.name,
        Command::AddDevice {
            index: usize::MAX,
            type_id: def.device.to_owned(),
            key: Some(key.clone()),
        },
    );
    set_params(&mut doc, &registry, def.name, &key, def.device, def.params);
    set_mods(&mut doc, &registry, def.name, &key, def.device, def.mods);
    run(
        &mut doc,
        &registry,
        def.name,
        Command::Workflow(WorkflowCommand::SetPatchName {
            device: key.clone(),
            name: def.name.to_owned(),
        }),
    );
    let snapshot = DeviceSnapshot::capture(doc.doc(), &[key], false)
        .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    save_device_item(&snapshot, doc.doc())
}

// ------------------------------------------------------------------------------- containers

/// One device inside a factory container.
pub(super) struct Part {
    /// The local key of the device inside the container.
    pub key: &'static str,
    /// The device type.
    pub device: &'static str,
    /// `(param key, plain value)` of every param that differs from a new device.
    pub params: &'static [(&'static str, f32)],
    /// Whether notes arriving at the container go to this device (a full-range split).
    pub split: bool,
    /// The modulation slots in use.
    pub mods: &'static [ModLink],
}

/// A cable inside a container: `(from device, from socket, to device, to socket)`. The device
/// `boundary` is the container's edge (`in_l`/`in_r` arrive, `out_l`/`out_r` leave).
pub(super) type Wire = (&'static str, &'static str, &'static str, &'static str);

/// What one container macro drives: `(part key, param key, value at 0, value at 1)`.
pub(super) type MacroTo = (&'static str, &'static str, f32, f32);

/// One macro of a factory container.
pub(super) struct MacroDef {
    /// The macro's name on the panel.
    pub name: &'static str,
    /// A switch (off below one half) instead of a knob.
    pub switch: bool,
    /// The params it drives.
    pub targets: &'static [MacroTo],
}

/// One factory container (an item of the `Containers` folder).
pub(super) struct ContainerDef {
    /// The patch name (and the file name).
    pub name: &'static str,
    /// What kind of sound it is (the browser's category column).
    pub category: &'static str,
    /// Words to search and filter by.
    pub tags: &'static [&'static str],
    /// One line on how it sounds (for the notes of a release; not saved in the item).
    pub sound: &'static str,
    /// The devices inside, in rack order.
    pub parts: &'static [Part],
    /// Cables between them. With `chain` they are made from the stereo audio sockets instead.
    pub wires: &'static [Wire],
    /// Wire the parts in series: the container's input to the first part, each part's stereo
    /// output to the next part's input, the last part to the container's output.
    pub chain: bool,
    /// The macros, from the first knob.
    pub macros: &'static [MacroDef],
}

/// The text of the item file of the container `def`.
///
/// # Panics
/// A key a device does not have, a value outside its range, or a cable the model refuses.
pub(super) fn container_text(def: &ContainerDef) -> String {
    let registry = default_registry();
    let item = def.name;
    let c = DeviceKey("c".to_owned());
    let inner = |k: &str| DeviceKey(format!("c/{k}"));
    let mut doc = Document::new(empty_doc());
    run(
        &mut doc,
        &registry,
        item,
        Command::AddDevice {
            index: usize::MAX,
            type_id: CONTAINER_TYPE.to_owned(),
            key: Some(c.clone()),
        },
    );
    for part in def.parts {
        run(
            &mut doc,
            &registry,
            item,
            Command::Container(ContainerCommand::AddInner {
                container: c.clone(),
                index: usize::MAX,
                type_id: part.device.to_owned(),
                key: Some(DeviceKey(part.key.to_owned())),
                auto_split: part.split,
            }),
        );
        set_params(
            &mut doc,
            &registry,
            item,
            &inner(part.key),
            part.device,
            part.params,
        );
        set_mods(
            &mut doc,
            &registry,
            item,
            &inner(part.key),
            part.device,
            part.mods,
        );
    }
    let mut wires: Vec<Wire> = def.wires.to_vec();
    if def.chain {
        let mut from = "boundary";
        for (i, part) in def.parts.iter().enumerate() {
            for side in ["l", "r"] {
                let (out, input): (&'static str, &'static str) = match side {
                    "l" => (if i == 0 { "in_l" } else { "out_l" }, "in_l"),
                    _ => (if i == 0 { "in_r" } else { "out_r" }, "in_r"),
                };
                wires.push((from, out, part.key, input));
            }
            from = part.key;
        }
        wires.push((from, "out_l", "boundary", "out_l"));
        wires.push((from, "out_r", "boundary", "out_r"));
    }
    for (from, out, to, input) in wires {
        run(
            &mut doc,
            &registry,
            item,
            Command::Connect(Cable {
                from: SocketRef {
                    device: inner(from),
                    socket: out.to_owned(),
                },
                to: SocketRef {
                    device: inner(to),
                    socket: input.to_owned(),
                },
            }),
        );
    }
    for (index, m) in def.macros.iter().enumerate() {
        let index = u8::try_from(index).expect("a few macros");
        for &(part, param, min, max) in m.targets {
            let device = def
                .parts
                .iter()
                .find(|p| p.key == part)
                .unwrap_or_else(|| panic!("{item}: macro {} drives no part {part}", m.name));
            let p = registry
                .descriptor(device.device)
                .and_then(|d| d.param_by_key(param))
                .unwrap_or_else(|| panic!("{item}: {part} has no param {param}"));
            assert!(
                (p.min..=p.max).contains(&min) && (p.min..=p.max).contains(&max),
                "{item}: macro {} leaves the range of {param}",
                m.name
            );
            run(
                &mut doc,
                &registry,
                item,
                Command::Container(ContainerCommand::SetMacro {
                    container: c.clone(),
                    index,
                    target: MacroTarget {
                        device: DeviceKey(part.to_owned()),
                        param: param.to_owned(),
                        min,
                        max,
                    },
                    merge: None,
                }),
            );
        }
        // A macro sets what it drives wherever the container is expanded, so the macro's own
        // position is the sound: it starts where the first target's value in the table sits.
        let &(part, param, min, max) = m.targets.first().expect("a macro drives something");
        let device = def.parts.iter().find(|p| p.key == part).expect("a part");
        let wanted = device
            .params
            .iter()
            .find(|(k, _)| *k == param)
            .map(|&(_, v)| v)
            .or_else(|| {
                registry
                    .descriptor(device.device)
                    .and_then(|d| d.param_by_key(param))
                    .map(|p| p.default)
            })
            .expect("a param");
        let position = ((wanted - min) / (max - min)).clamp(0.0, 1.0);
        set_params(
            &mut doc,
            &registry,
            item,
            &c,
            CONTAINER_TYPE,
            &[(format!("macro{}", index + 1).as_str(), position)],
        );
        run(
            &mut doc,
            &registry,
            item,
            Command::Container(ContainerCommand::RenameMacro {
                container: c.clone(),
                index,
                name: m.name.to_owned(),
            }),
        );
        if m.switch {
            run(
                &mut doc,
                &registry,
                item,
                Command::Container(ContainerCommand::SetMacroKind {
                    container: c.clone(),
                    index,
                    kind: MacroKind::Switch,
                }),
            );
        }
    }
    run(
        &mut doc,
        &registry,
        item,
        Command::Rename {
            key: c.clone(),
            name: def.name.to_owned(),
        },
    );
    run(
        &mut doc,
        &registry,
        item,
        Command::Workflow(WorkflowCommand::SetPatchName {
            device: c.clone(),
            name: def.name.to_owned(),
        }),
    );
    let snapshot = DeviceSnapshot::capture(doc.doc(), &[c], false)
        .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    save_device_item(&snapshot, doc.doc())
}

/// A stage of an effect chain: stereo in, stereo out, no notes.
const fn fx(
    key: &'static str,
    device: &'static str,
    params: &'static [(&'static str, f32)],
) -> Part {
    Part {
        key,
        device,
        params,
        split: false,
        mods: &[],
    }
}

const DISTORTION: &str = "atomik.distortion";
const EQ: &str = "atomik.eq";
const COMPRESSOR: &str = "atomik.compressor";
const DE_ESSER: &str = "atomik.de-esser";
const REVERB: &str = "atomik.reverb";
const SPACE_REVERB: &str = "atomik.space-reverb";
const ECHO: &str = "atomik.echo";
const MODULATION: &str = "atomik.modulation";
const AUTO_FILTER: &str = "atomik.auto-filter";
const PITCH_SHIFTER: &str = "atomik.pitch-shifter";
const STEREO_TOOL: &str = "atomik.stereo-tool";
const TRANSIENT: &str = "atomik.transient-shaper";
const ROTARY: &str = "atomik.rotary-speaker";
const MASTERING: &str = "atomik.mastering";
const MOD_SOURCE: &str = "atomik.mod-source";
const SAMPLE_HOLD: &str = "atomik.sample-hold";
const QUANTIZER: &str = "atomik.cv-quantizer";
const KEY_CHORD: &str = "atomik.key-chord";
const ARPEGGIATOR: &str = "atomik.arpeggiator";

/// The factory containers, in the order the manifest lists them: ten effect chains, then three
/// modular instruments.
pub(super) const CONTAINERS: &[ContainerDef] = &[
    ContainerDef {
        name: "Tape Warmth",
        category: "Saturation",
        tags: &["chain", "warm", "tape", "glue"],
        sound: "A tape-style saturator, a gentle top-end trim and a slow compressor: the \
                signal gets thicker and rounder without sounding distorted. The Drive macro \
                pushes the saturator, the Tone macro tilts it dark to bright.",
        parts: &[
            fx(
                "sat",
                DISTORTION,
                &[
                    ("curve", 5.0),
                    ("drive", 9.0),
                    ("tone", -0.2),
                    ("output", -5.0),
                    ("mix", 0.85),
                ],
            ),
            fx(
                "eq",
                EQ,
                &[
                    ("band1.on", 1.0),
                    ("band1.freq", 30.0),
                    ("band5.gain", -2.5),
                    ("band5.freq", 9000.0),
                ],
            ),
            fx(
                "comp",
                COMPRESSOR,
                &[
                    ("threshold", -14.0),
                    ("ratio", 2.5),
                    ("attack", 25.0),
                    ("release", 220.0),
                    ("auto_makeup", 1.0),
                ],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Drive",
                switch: false,
                targets: &[("sat", "drive", 3.0, 24.0)],
            },
            MacroDef {
                name: "Tone",
                switch: false,
                targets: &[("sat", "tone", -0.6, 0.6)],
            },
        ],
    },
    ContainerDef {
        name: "Vocal Air",
        category: "Vocal",
        tags: &["chain", "vocal", "plate", "smooth"],
        sound: "A de-esser to tame the harsh top, a compressor to even the level and a short \
                plate reverb behind it. The Space macro sets the reverb level.",
        parts: &[
            fx(
                "deess",
                DE_ESSER,
                &[("freq", 6800.0), ("threshold", -32.0), ("range", 10.0)],
            ),
            fx(
                "comp",
                COMPRESSOR,
                &[
                    ("threshold", -20.0),
                    ("ratio", 3.5),
                    ("attack", 8.0),
                    ("release", 120.0),
                    ("auto_makeup", 1.0),
                ],
            ),
            fx(
                "plate",
                REVERB,
                &[
                    ("algorithm", 2.0),
                    ("predelay", 18.0),
                    ("decay", 1.6),
                    ("damping", 0.5),
                    ("mix", 0.22),
                ],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Space",
                switch: false,
                targets: &[("plate", "mix", 0.0, 0.5)],
            },
            MacroDef {
                name: "Squeeze",
                switch: false,
                targets: &[("comp", "threshold", -8.0, -34.0)],
            },
        ],
    },
    ContainerDef {
        name: "Ping Pong Tide",
        category: "Delay",
        tags: &["chain", "delay", "stereo", "wide"],
        sound: "A dotted-eighth ping-pong echo with a darkened, wobbling repeat, a light \
                chorus to widen it and a small room behind. The Feedback macro lengthens the \
                trail and the Mix macro sets how much echo comes back.",
        parts: &[
            fx(
                "echo",
                ECHO,
                &[
                    ("time.sync", 6.0),
                    ("feedback", 0.45),
                    ("mix", 0.35),
                    ("color.high", 6000.0),
                    ("wobble.depth", 0.2),
                    ("ping_pong", 1.0),
                ],
            ),
            fx(
                "chorus",
                MODULATION,
                &[("mode", 0.0), ("rate", 0.4), ("depth", 0.4), ("mix", 0.3)],
            ),
            fx(
                "room",
                REVERB,
                &[("algorithm", 0.0), ("decay", 1.2), ("mix", 0.15)],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Feedback",
                switch: false,
                targets: &[("echo", "feedback", 0.1, 0.8)],
            },
            MacroDef {
                name: "Mix",
                switch: false,
                targets: &[("echo", "mix", 0.0, 0.6)],
            },
        ],
    },
    ContainerDef {
        name: "Shimmer Cathedral",
        category: "Reverb",
        tags: &["chain", "reverb", "shimmer", "huge"],
        sound: "A pitch shifter an octave up feeds a long cathedral reverb, and a stereo tool \
                opens the width: the tail glitters above the dry sound. The Shimmer macro sets \
                the octave-up level and the Decay macro the length.",
        parts: &[
            fx(
                "shift",
                PITCH_SHIFTER,
                &[("semitones", 12.0), ("quality", 2.0), ("mix", 0.35)],
            ),
            fx(
                "hall",
                SPACE_REVERB,
                &[
                    ("algorithm", 1.0),
                    ("predelay", 30.0),
                    ("decay", 8.0),
                    ("mix", 0.45),
                    ("highcut", 9000.0),
                ],
            ),
            fx("wide", STEREO_TOOL, &[("width", 1.3)]),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Shimmer",
                switch: false,
                targets: &[("shift", "mix", 0.0, 0.7)],
            },
            MacroDef {
                name: "Decay",
                switch: false,
                targets: &[("hall", "decay", 2.0, 14.0)],
            },
        ],
    },
    ContainerDef {
        name: "Radio Dust",
        category: "Lo-fi",
        tags: &["chain", "lo-fi", "crushed", "narrow"],
        sound: "A bit crusher and sample-rate reducer, a band-limiting equaliser that thins \
                the lows and rolls off the highs and a short slap echo: a small, dusty, \
                broadcast-from-another-room sound. The Crush macro lowers the bit depth.",
        parts: &[
            fx(
                "crush",
                DISTORTION,
                &[
                    ("curve", 4.0),
                    ("drive", 6.0),
                    ("crush.bits", 9.0),
                    ("crush.rate", 16000.0),
                    ("output", -2.0),
                    ("mix", 0.85),
                ],
            ),
            fx(
                "band",
                EQ,
                &[
                    ("band1.on", 1.0),
                    ("band1.freq", 320.0),
                    ("band5.gain", -9.0),
                    ("band5.freq", 4000.0),
                    ("output", 3.0),
                ],
            ),
            fx(
                "slap",
                ECHO,
                &[
                    ("time.mode", 0.0),
                    ("time.ms", 90.0),
                    ("feedback", 0.3),
                    ("mix", 0.2),
                ],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[MacroDef {
            name: "Crush",
            switch: false,
            targets: &[("crush", "crush.bits", 16.0, 4.0)],
        }],
    },
    ContainerDef {
        name: "Drum Punch",
        category: "Dynamics",
        tags: &["chain", "drums", "punchy", "tight"],
        sound: "A transient shaper that raises the attack and trims the sustain, a fast \
                compressor and an equaliser that adds weight at 90 Hz and air at 9 kHz: drums \
                hit harder and stay shorter. The Punch macro moves the attack from soft to \
                sharp.",
        parts: &[
            fx(
                "shape",
                TRANSIENT,
                &[("attack", 0.6), ("sustain", -0.2), ("output", -4.5)],
            ),
            fx(
                "comp",
                COMPRESSOR,
                &[
                    ("threshold", -16.0),
                    ("ratio", 5.0),
                    ("attack", 15.0),
                    ("release", 90.0),
                    ("auto_makeup", 1.0),
                ],
            ),
            fx(
                "eq",
                EQ,
                &[
                    ("band2.freq", 90.0),
                    ("band2.gain", 2.0),
                    ("band5.freq", 9000.0),
                    ("band5.gain", 2.5),
                ],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Punch",
                switch: false,
                targets: &[("shape", "attack", -0.3, 1.0)],
            },
            MacroDef {
                name: "Length",
                switch: false,
                targets: &[("shape", "sustain", -0.8, 0.5)],
            },
        ],
    },
    ContainerDef {
        name: "Slow Phaser Bed",
        category: "Modulation",
        tags: &["chain", "phaser", "evolving", "soft"],
        sound: "A six-stage phaser that sweeps once every eight seconds, a band-pass sweep \
                behind it and a plate reverb: a slowly breathing wash. The Rate macro speeds \
                the sweeps and the Depth macro deepens them.",
        parts: &[
            fx(
                "phase",
                MODULATION,
                &[
                    ("mode", 2.0),
                    ("rate", 0.12),
                    ("depth", 0.8),
                    ("feedback", 0.5),
                    ("stages", 6.0),
                    ("mix", 0.5),
                    ("output", 6.0),
                ],
            ),
            fx(
                "sweep",
                AUTO_FILTER,
                &[
                    ("mode", 1.0),
                    ("cutoff", 2400.0),
                    ("resonance", 0.3),
                    ("depth", 0.35),
                    ("rate", 0.2),
                    ("mix", 0.6),
                ],
            ),
            fx(
                "plate",
                REVERB,
                &[("algorithm", 2.0), ("decay", 2.5), ("mix", 0.2)],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Rate",
                switch: false,
                targets: &[("phase", "rate", 0.05, 2.0), ("sweep", "rate", 0.1, 3.0)],
            },
            MacroDef {
                name: "Depth",
                switch: false,
                targets: &[("phase", "depth", 0.2, 1.0), ("sweep", "depth", 0.1, 0.8)],
            },
        ],
    },
    ContainerDef {
        name: "Rotary Choir",
        category: "Modulation",
        tags: &["chain", "rotary", "organ", "swirl"],
        sound: "A rotary speaker on its slow speed with a little drive, a light chorus and a \
                small room: a swirling, vocal sheen for organs and pads. The Fast macro is a \
                switch that spins the horn and drum up.",
        parts: &[
            fx("rotor", ROTARY, &[("speed", 1.0), ("drive", 0.25)]),
            fx(
                "chorus",
                MODULATION,
                &[("mode", 0.0), ("rate", 0.3), ("depth", 0.4), ("mix", 0.25)],
            ),
            fx(
                "room",
                REVERB,
                &[("algorithm", 0.0), ("decay", 1.0), ("mix", 0.15)],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Fast",
                switch: true,
                targets: &[("rotor", "speed", 1.0, 2.0)],
            },
            MacroDef {
                name: "Drive",
                switch: false,
                targets: &[("rotor", "drive", 0.0, 0.8)],
            },
        ],
    },
    ContainerDef {
        name: "Wobble Gate",
        category: "Rhythm",
        tags: &["chain", "rhythmic", "filter", "pulsing"],
        sound: "A resonant filter that opens and shuts in time with the tempo, in eighth \
                notes, then a soft clipper and a compressor: the sound chops and pulses. The \
                Depth macro sets how far the filter moves and the Cutoff macro where it sits.",
        parts: &[
            fx(
                "gate",
                AUTO_FILTER,
                &[
                    ("cutoff", 600.0),
                    ("resonance", 0.45),
                    ("depth", 0.9),
                    ("sync", 1.0),
                    ("division", 5.0),
                    ("shape", 3.0),
                ],
            ),
            fx(
                "clip",
                DISTORTION,
                &[("curve", 0.0), ("drive", 8.0), ("output", -4.0)],
            ),
            fx(
                "comp",
                COMPRESSOR,
                &[("threshold", -14.0), ("ratio", 3.0), ("auto_makeup", 1.0)],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Depth",
                switch: false,
                targets: &[("gate", "depth", 0.1, 1.0)],
            },
            MacroDef {
                name: "Cutoff",
                switch: false,
                targets: &[("gate", "cutoff", 200.0, 3000.0)],
            },
        ],
    },
    ContainerDef {
        name: "Bus Glue",
        category: "Mastering",
        tags: &["chain", "bus", "glue", "master"],
        sound: "A low cut at 25 Hz and a touch of top lift, a two-to-one compressor with an \
                automatic release, a slightly wider image and a soft ceiling: a mix bus \
                finish that holds a mix together. The Glue macro lowers the threshold and the \
                Width macro opens or narrows the image.",
        parts: &[
            fx(
                "eq",
                EQ,
                &[
                    ("band1.on", 1.0),
                    ("band1.freq", 25.0),
                    ("band5.freq", 10000.0),
                    ("band5.gain", 1.5),
                ],
            ),
            fx(
                "comp",
                COMPRESSOR,
                &[
                    ("threshold", -12.0),
                    ("ratio", 2.0),
                    ("knee", 8.0),
                    ("attack", 30.0),
                    ("auto_release", 1.0),
                    ("auto_makeup", 1.0),
                ],
            ),
            fx("wide", STEREO_TOOL, &[("width", 1.1)]),
            fx(
                "limit",
                MASTERING,
                &[("multiband.on", 0.0), ("ceiling", -1.0), ("softclip", 0.2)],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Glue",
                switch: false,
                targets: &[("comp", "threshold", -4.0, -26.0)],
            },
            MacroDef {
                name: "Width",
                switch: false,
                targets: &[("wide", "width", 0.6, 1.5)],
            },
        ],
    },
    ContainerDef {
        name: "Double Clip",
        category: "Dynamics",
        tags: &["chain", "clipper", "loud", "density"],
        sound: "A soft clipper that rounds the tallest peaks, then a hard clipper two decibels \
                under full scale that catches what the soft one raised: a bus gets louder in \
                two small steps instead of one hard one. The Drive macro pushes both, the \
                Softness macro moves the first from a hard knee to a round one.",
        parts: &[
            fx(
                "soft",
                CLIPPER,
                &[
                    ("gain", 0.0),
                    ("ceiling", -8.0),
                    ("curve", 0.8),
                    ("oversample", 2.0),
                ],
            ),
            fx(
                "hard",
                CLIPPER,
                &[
                    ("gain", 0.0),
                    ("ceiling", -3.0),
                    ("curve", 0.0),
                    ("oversample", 2.0),
                ],
            ),
        ],
        wires: &[],
        chain: true,
        macros: &[
            MacroDef {
                name: "Drive",
                switch: false,
                targets: &[("soft", "gain", 0.0, 14.0), ("hard", "gain", 0.0, 6.0)],
            },
            MacroDef {
                name: "Softness",
                switch: false,
                targets: &[("soft", "curve", 0.2, 1.0)],
            },
        ],
    },
    ContainerDef {
        name: "Random Walk Lead",
        category: "Modular",
        tags: &["modular", "generative", "lead", "pentatonic"],
        sound: "A fast LFO sampled four times a second by a sample-and-hold, snapped to an \
                A minor pentatonic scale and sent to the pitch of a saw and square lead: held \
                keys wander through the scale on their own. The Range macro widens the walk \
                and the Cutoff macro brightens the lead.",
        parts: &[
            Part {
                key: "lfo",
                device: MOD_SOURCE,
                params: &[("lfo1_shape", 2.0), ("lfo1_rate", 7.3)],
                split: false,
                mods: &[],
            },
            Part {
                key: "hold",
                device: SAMPLE_HOLD,
                params: &[("clock", 1.0), ("rate", 4.0), ("level", 0.3)],
                split: false,
                mods: &[],
            },
            Part {
                key: "scale",
                device: QUANTIZER,
                params: &[("scale", 11.0), ("root", 9.0)],
                split: false,
                mods: &[],
            },
            Part {
                key: "lead",
                device: POLY,
                params: &[
                    ("osc1.wave", 0.0),
                    ("osc2.wave", 1.0),
                    ("osc2.fine", 6.0),
                    ("osc.mix", 0.4),
                    ("filter.cutoff", 1600.0),
                    ("filter.resonance", 0.25),
                    ("filter.env_amount", 0.4),
                    ("env.amp.attack", 4.0),
                    ("env.amp.decay", 300.0),
                    ("env.amp.sustain", 0.7),
                    ("env.amp.release", 200.0),
                    ("delay.mix", 0.2),
                    ("delay.time", 375.0),
                    ("delay.feedback", 0.35),
                    ("master.level", -7.5),
                ],
                split: true,
                mods: &[],
            },
        ],
        wires: &[
            ("lfo", "lfo1_cv_out", "hold", "cv_in"),
            ("hold", "cv_out", "scale", "pitch_cv_in"),
            ("scale", "pitch_cv_out", "lead", "pitch_cv_in"),
            ("lead", "out_l", "boundary", "out_l"),
            ("lead", "out_r", "boundary", "out_r"),
        ],
        chain: false,
        macros: &[
            MacroDef {
                name: "Range",
                switch: false,
                targets: &[("hold", "level", 0.05, 0.6)],
            },
            MacroDef {
                name: "Cutoff",
                switch: false,
                targets: &[("lead", "filter.cutoff", 400.0, 6000.0)],
            },
        ],
    },
    ContainerDef {
        name: "Chord Arp Stack",
        category: "Modular",
        tags: &["modular", "arpeggio", "chords", "pad"],
        sound: "Every key you hold becomes a seventh chord, which an arpeggiator plays up and \
                down over two octaves in sixteenth notes into a soft saw synth with a dotted \
                echo: one finger plays a whole phrase. The Gate macro shortens the notes and \
                the Cutoff macro opens the synth.",
        parts: &[
            Part {
                key: "chord",
                device: KEY_CHORD,
                params: &[("chord", 2.0)],
                split: true,
                mods: &[],
            },
            Part {
                key: "arp",
                device: ARPEGGIATOR,
                params: &[
                    ("mode", 2.0),
                    ("rate", 3.0),
                    ("octaves", 2.0),
                    ("gate", 70.0),
                    ("run", 1.0),
                ],
                split: false,
                mods: &[],
            },
            Part {
                key: "synth",
                device: POLY,
                params: &[
                    ("osc1.wave", 0.0),
                    ("osc2.wave", 0.0),
                    ("osc2.fine", 8.0),
                    ("osc.mix", 0.5),
                    ("filter.cutoff", 1800.0),
                    ("filter.resonance", 0.2),
                    ("filter.env_amount", 0.35),
                    ("env.amp.attack", 3.0),
                    ("env.amp.decay", 260.0),
                    ("env.amp.sustain", 0.3),
                    ("env.amp.release", 260.0),
                    ("env.filter.decay", 240.0),
                    ("env.filter.sustain", 0.1),
                    ("spread", 0.4),
                    ("delay.mix", 0.25),
                    ("delay.time", 375.0),
                    ("delay.feedback", 0.35),
                    ("master.level", -4.0),
                ],
                split: false,
                mods: &[],
            },
        ],
        wires: &[
            ("chord", "notes_out", "arp", "notes_in"),
            ("arp", "notes_out", "synth", "notes_in"),
            ("synth", "out_l", "boundary", "out_l"),
            ("synth", "out_r", "boundary", "out_r"),
        ],
        chain: false,
        macros: &[
            MacroDef {
                name: "Gate",
                switch: false,
                targets: &[("arp", "gate", 20.0, 100.0)],
            },
            MacroDef {
                name: "Cutoff",
                switch: false,
                targets: &[("synth", "filter.cutoff", 500.0, 7000.0)],
            },
        ],
    },
    ContainerDef {
        name: "Envelope Sweep Pad",
        category: "Modular",
        tags: &["modular", "pad", "sweep", "evolving"],
        sound: "A mod source with a slow envelope opens the filter of a saw pad over about \
                two seconds while a slow LFO moves the pan from side to side: each chord \
                swells in and drifts. The Sweep macro sets how far the envelope opens the \
                filter and the Drift macro how far the pan moves.",
        parts: &[
            Part {
                key: "mod",
                device: MOD_SOURCE,
                params: &[
                    ("env_attack", 1800.0),
                    ("env_decay", 3000.0),
                    ("env_sustain", 0.6),
                    ("env_release", 1500.0),
                    ("lfo1_shape", 1.0),
                    ("lfo1_rate", 0.18),
                    ("lfo1_amount", 0.4),
                ],
                split: true,
                mods: &[],
            },
            Part {
                key: "pad",
                device: POLY,
                params: &[
                    ("osc1.wave", 0.0),
                    ("osc2.wave", 0.0),
                    ("osc1.fine", -7.0),
                    ("osc2.fine", 9.0),
                    ("osc.mix", 0.5),
                    ("filter.cutoff", 350.0),
                    ("filter.resonance", 0.2),
                    ("filter.env_amount", 0.0),
                    ("cv.cutoff_amount", 3.0),
                    ("env.amp.attack", 900.0),
                    ("env.amp.decay", 600.0),
                    ("env.amp.sustain", 0.9),
                    ("env.amp.release", 1600.0),
                    ("spread", 0.5),
                    ("chorus.mix", 0.3),
                    ("master.level", -4.0),
                ],
                split: true,
                mods: &[],
            },
        ],
        wires: &[
            ("mod", "env_cv_out", "pad", "cutoff_cv_in"),
            ("mod", "lfo1_cv_out", "pad", "pan_cv_in"),
            ("pad", "out_l", "boundary", "out_l"),
            ("pad", "out_r", "boundary", "out_r"),
        ],
        chain: false,
        macros: &[
            MacroDef {
                name: "Sweep",
                switch: false,
                targets: &[("pad", "cv.cutoff_amount", 0.5, 4.0)],
            },
            MacroDef {
                name: "Drift",
                switch: false,
                targets: &[("pad", "pan_cv_trim", 0.0, 1.0)],
            },
        ],
    },
];

// ------------------------------------------------------------------------------- the folder

/// Where an item of each kind lives in the factory folder.
pub(super) fn patch_path(name: &str) -> String {
    format!("Patches/{name}.atomikpatch")
}

/// The path of a factory container item, relative to the factory folder.
pub(super) fn container_path(name: &str) -> String {
    format!("Containers/{name}.atomikcontainer")
}

/// The path of a factory clip item, relative to the factory folder.
pub(super) fn clip_path(name: &str) -> String {
    format!("Clips/{name}.atomikclip")
}

/// The marks of one item: its path below the factory folder, its category and its tags.
pub(super) struct Mark {
    /// The path below the factory folder, with `/`.
    pub path: String,
    /// The category.
    pub category: &'static str,
    /// The tags.
    pub tags: &'static [&'static str],
}

/// The marks of every factory item, in table order.
pub(super) fn marks() -> Vec<Mark> {
    let mut out = Vec::new();
    for d in DEFS {
        out.push(Mark {
            path: patch_path(d.name),
            category: d.category,
            tags: d.tags,
        });
    }
    for k in KITS {
        out.push(Mark {
            path: patch_path(k.name),
            category: k.category,
            tags: k.tags,
        });
    }
    for c in CONTAINERS {
        out.push(Mark {
            path: container_path(c.name),
            category: c.category,
            tags: c.tags,
        });
    }
    for c in CLIPS {
        out.push(Mark {
            path: clip_path(c.name),
            category: c.category,
            tags: c.tags,
        });
    }
    for e in EXAMPLES {
        out.push(Mark {
            path: examples::project_path(e.name),
            category: examples::CATEGORY,
            tags: e.tags,
        });
    }
    out
}

/// The text of `factory.json` for every table.
pub(super) fn manifest_text() -> String {
    let mut items = serde_json::Map::new();
    for mark in marks() {
        items.insert(
            mark.path,
            serde_json::json!({
                "author": AUTHOR,
                "categories": [mark.category],
                "tags": mark.tags,
            }),
        );
    }
    let manifest = serde_json::json!({ "version": 1, "items": items });
    let mut text = serde_json::to_string_pretty(&manifest).expect("a manifest is JSON");
    text.push('\n');
    text
}

/// Writes the `Patches`, `Containers`, `Clips` and `Projects` folders and `factory.json` into the
/// folder `root`: every item the tables describe, the audio of the drum kits beside theirs and
/// the example projects.
///
/// # Panics
/// A file cannot be written.
pub(super) fn write_all(root: &Path) {
    let write = |relative: String, text: String| {
        let path = root.join(&relative);
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("an item folder");
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    };
    for def in DEFS {
        write(patch_path(def.name), item_text(def));
    }
    let mut built_kits = Vec::new();
    for kit in KITS {
        let built = build_kit(kit);
        write_audio(&root.join("Patches"), kit, &built);
        write(patch_path(kit.name), built.text.clone());
        built_kits.push((kit.name, built));
    }
    for def in CONTAINERS {
        write(container_path(def.name), container_text(def));
    }
    for def in CLIPS {
        write(clip_path(def.name), clip_text(def));
    }
    write_examples(root, &built_kits);
    std::fs::write(root.join("factory.json"), manifest_text()).expect("the manifest");
}
