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
//! which rewrites `factory/Patches/*.atomikpatch` and `factory/factory.json`.

use atomik_model::{
    Command, DeviceKey, DeviceSnapshot, Document, WorkflowCommand, empty_doc, save_device_item,
};
use atomik_render::default_registry;
use std::path::Path;

pub(super) const POLY: &str = "atomik.poly-synth";
pub(super) const BASS: &str = "atomik.mono-bass";
pub(super) const DUAL: &str = "atomik.dual-layer";
pub(super) const FM: &str = "atomik.fm-synth";
pub(super) const GRAIN: &str = "atomik.granular";
pub(super) const WAVE: &str = "atomik.wavetable-synth";
pub(super) const ORGAN: &str = "atomik.organ";

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
    },
];

/// The text of the item file of `def`: the device with `def`'s params, named for the patch.
///
/// # Panics
/// A key the device does not have, or a value outside its range.
pub(super) fn item_text(def: &Def) -> String {
    let registry = default_registry();
    let desc = registry
        .descriptor(def.device)
        .unwrap_or_else(|| panic!("{}: no device {}", def.name, def.device));
    let key = DeviceKey("synth".to_owned());
    let mut doc = Document::new(empty_doc());
    let mut apply = |command: Command| {
        doc.apply(command, &registry)
            .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    };
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: def.device.to_owned(),
        key: Some(key.clone()),
    });
    for &(param, value) in def.params {
        let p = desc
            .param_by_key(param)
            .unwrap_or_else(|| panic!("{}: {} has no param {param}", def.name, def.device));
        assert!(
            (p.min..=p.max).contains(&value),
            "{}: {param} = {value} is outside {}..{}",
            def.name,
            p.min,
            p.max
        );
        apply(Command::SetParam {
            key: key.clone(),
            param: param.to_owned(),
            value,
            merge: None,
        });
    }
    apply(Command::Workflow(WorkflowCommand::SetPatchName {
        device: key.clone(),
        name: def.name.to_owned(),
    }));
    let snapshot = DeviceSnapshot::capture(doc.doc(), &[key], false)
        .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    save_device_item(&snapshot, doc.doc())
}

/// The text of `factory.json` for [`DEFS`].
pub(super) fn manifest_text() -> String {
    let mut items = serde_json::Map::new();
    for def in DEFS {
        items.insert(
            format!("Patches/{}.atomikpatch", def.name),
            serde_json::json!({
                "author": AUTHOR,
                "categories": [def.category],
                "tags": def.tags,
            }),
        );
    }
    let manifest = serde_json::json!({ "version": 1, "items": items });
    let mut text = serde_json::to_string_pretty(&manifest).expect("a manifest is JSON");
    text.push('\n');
    text
}

/// Writes `Patches/*.atomikpatch` and `factory.json` into the folder `root`.
///
/// # Panics
/// A file cannot be written.
pub(super) fn write_all(root: &Path) {
    let patches = root.join("Patches");
    std::fs::create_dir_all(&patches).expect("the Patches folder");
    for def in DEFS {
        let path = patches.join(format!("{}.atomikpatch", def.name));
        std::fs::write(&path, item_text(def)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    std::fs::write(root.join("factory.json"), manifest_text()).expect("the manifest");
}
