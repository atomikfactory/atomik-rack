//! The drawn-table patches of the factory library (test code): Wavetable Synth patches whose
//! table was made with the table editor's maths, stored the way the editor stores a table.
//!
//! A patch lists its key frames as harmonic recipes (a level and a phase for each harmonic that
//! sounds), whether the editor's phase-safe morph aligns them, and the synth's params. The
//! builder rebuilds each frame by additive synthesis ([`atomik_dsp::wavetable::edit::rebuild`]),
//! bakes the frames into the sixteen the synth stores ([`atomik_dsp::wavetable::edit::bake`]),
//! and writes the item with that table as the device's one sample, named the way the editor names
//! it, so opening the editor on the patch shows the key frames again. The audio sits beside the
//! item in `<name>/Audio/<id>.wav` like a drum kit's.
//!
//! The bake is plain arithmetic on the harmonics (deterministic sine, no `atan2`), so the same
//! table gives the same bytes on every platform.

use super::kits::{Built, RATE};
use super::{WAVE, run, set_params};
use atomik_audio_file::{AudioData, content_hash};
use atomik_dsp::wavetable::edit::{BINS, Bin, Frame, bake, rebuild};
use atomik_model::{
    AssetMeta, Command, DeviceKey, DeviceSnapshot, Document, WorkflowCommand, ZoneDoc, empty_doc,
    save_device_item,
};
use atomik_render::default_registry;

/// One harmonic of a recipe: `(harmonic, level, phase in cycles)`.
pub(in crate::app_core) type Harmonic = (usize, f32, f32);

/// How one key frame is made.
pub(in crate::app_core) enum KeyDef {
    /// Harmonics `1..=n` at `1 / h`: a saw.
    Saw(usize),
    /// The odd harmonics up to `n` at `1 / h`: a square.
    Odd(usize),
    /// The listed harmonics, every other one silent.
    Sparse(&'static [Harmonic]),
}

/// One factory drawn-table patch (an item of the `Patches` folder with audio beside it).
pub(in crate::app_core) struct TableDef {
    /// The patch name (and the file name).
    pub name: &'static str,
    /// What kind of sound it is (the browser's category column).
    pub category: &'static str,
    /// Words to search and filter by.
    pub tags: &'static [&'static str],
    /// One line on how it sounds (for the notes of a release; not saved in the item).
    pub sound: &'static str,
    /// The key frames, first to last.
    pub keys: &'static [KeyDef],
    /// Whether the fundamentals of the frames are aligned (phase-safe morph).
    pub phase_safe: bool,
    /// `(param key, plain value)` of every param that differs from a new device.
    pub params: &'static [(&'static str, f32)],
}

/// The drawn-table patches, in the order the manifest lists them.
pub(in crate::app_core) const TABLES: &[TableDef] = &[
    TableDef {
        name: "Drawn Bell Morph",
        category: "Pluck",
        tags: &["wavetable", "drawn", "bell", "bright"],
        sound: "Four drawn frames from a soft sine to a glassy set of partials; every note \
                starts on the bright end and the position envelope settles it to the soft one.",
        keys: &[
            KeyDef::Sparse(&[(1, 1.0, 0.0), (2, 0.25, 0.0)]),
            KeyDef::Sparse(&[(1, 1.0, 0.0), (3, 0.5, 0.0), (5, 0.55, 0.0), (7, 0.35, 0.0)]),
            KeyDef::Sparse(&[(2, 0.8, 0.0), (5, 0.9, 0.0), (8, 0.6, 0.0), (11, 0.45, 0.0)]),
            KeyDef::Sparse(&[(1, 0.5, 0.0), (4, 0.7, 0.0), (9, 0.7, 0.0), (14, 0.5, 0.0)]),
        ],
        phase_safe: true,
        params: &[
            ("osc1.table", 12.0),
            ("osc1.position", 0.0),
            ("osc1.level", 0.8),
            ("osc2.level", 0.0),
            ("env.mod.to_pos1", 0.9),
            ("env.mod.attack", 1.0),
            ("env.mod.decay", 1100.0),
            ("env.mod.sustain", 0.0),
            ("filter.cutoff", 9000.0),
            ("env.amp.attack", 2.0),
            ("env.amp.decay", 1400.0),
            ("env.amp.sustain", 0.25),
            ("env.amp.release", 500.0),
            ("master.level", -2.0),
        ],
    },
    TableDef {
        name: "Aligned Saw Pad",
        category: "Pad",
        tags: &["wavetable", "drawn", "phase-safe", "wide"],
        sound: "A saw, a frame whose fundamental starts half a cycle later and a square, \
                aligned by the phase-safe morph so the position sweep keeps its level; three \
                unison voices and a slow position LFO.",
        keys: &[
            KeyDef::Saw(24),
            KeyDef::Sparse(&[
                (1, 1.0, 0.5),
                (3, 0.5, 0.5),
                (5, 0.4, 0.5),
                (7, 0.3, 0.5),
                (9, 0.2, 0.5),
            ]),
            KeyDef::Odd(20),
        ],
        phase_safe: true,
        params: &[
            ("osc1.table", 12.0),
            ("osc1.position", 0.3),
            ("osc1.level", 0.75),
            ("osc2.level", 0.0),
            ("lfo.rate", 0.3),
            ("lfo.to_pos1", 0.45),
            ("filter.cutoff", 5000.0),
            ("unison.voices", 3.0),
            ("unison.detune", 12.0),
            ("unison.spread", 0.8),
            ("env.amp.attack", 450.0),
            ("env.amp.sustain", 0.85),
            ("env.amp.release", 1200.0),
            ("master.level", -4.5),
        ],
    },
    TableDef {
        name: "Table Sub Morph",
        category: "Bass",
        tags: &["wavetable", "drawn", "sub", "mono"],
        sound: "Four drawn frames that all keep a strong fundamental: a sine, a sine with its \
                octave, a rounder set and an odd-harmonic one; mono, an octave down, with the \
                position moved by a short envelope.",
        keys: &[
            KeyDef::Sparse(&[(1, 1.0, 0.0)]),
            KeyDef::Sparse(&[(1, 1.0, 0.0), (2, 0.35, 0.0)]),
            KeyDef::Sparse(&[(1, 1.0, 0.0), (2, 0.3, 0.0), (3, 0.4, 0.0), (4, 0.15, 0.0)]),
            KeyDef::Sparse(&[(1, 1.0, 0.0), (3, 0.25, 0.0), (5, 0.15, 0.0)]),
        ],
        phase_safe: true,
        params: &[
            ("osc1.table", 12.0),
            ("osc1.octave", -1.0),
            ("osc1.position", 0.0),
            ("osc1.level", 0.9),
            ("osc2.level", 0.0),
            ("env.mod.to_pos1", 0.5),
            ("env.mod.attack", 1.0),
            ("env.mod.decay", 260.0),
            ("env.mod.sustain", 0.2),
            ("filter.cutoff", 2400.0),
            ("voices", 1.0),
            ("env.amp.attack", 3.0),
            ("env.amp.sustain", 0.9),
            ("env.amp.release", 120.0),
            ("master.level", 4.0),
        ],
    },
];

/// The key frames of `def`.
pub(in crate::app_core) fn table_keys(def: &TableDef) -> Vec<Frame> {
    def.keys
        .iter()
        .map(|key| {
            let mut bins = vec![Bin::default(); BINS];
            match key {
                KeyDef::Saw(n) => {
                    for h in 1..=(*n).min(BINS) {
                        bins[h - 1].level = 1.0 / h as f32;
                    }
                }
                KeyDef::Odd(n) => {
                    for h in (1..=(*n).min(BINS)).step_by(2) {
                        bins[h - 1].level = 1.0 / h as f32;
                    }
                }
                KeyDef::Sparse(list) => {
                    for &(h, level, phase) in *list {
                        assert!((1..=BINS).contains(&h), "{}: harmonic {h}", def.name);
                        bins[h - 1] = Bin { level, phase };
                    }
                }
            }
            rebuild(&bins)
        })
        .collect()
}

/// The audio of the table of `def`: the baked sixteen frames, mono.
pub(in crate::app_core) fn table_audio(def: &TableDef) -> AudioData {
    let table = bake(&table_keys(def), def.phase_safe);
    AudioData::new(RATE, vec![table]).unwrap_or_else(|e| panic!("{}: {e}", def.name))
}

/// The item text of `def` with `data` as the audio of its table.
///
/// # Panics
/// A key the device does not have, a value outside its range, or a zone the model refuses.
pub(in crate::app_core) fn table_item(def: &TableDef, data: AudioData) -> Built {
    let registry = default_registry();
    let item = def.name;
    let key = DeviceKey("table".to_owned());
    let mut doc = Document::new(empty_doc());
    run(
        &mut doc,
        &registry,
        item,
        Command::AddDevice {
            index: usize::MAX,
            type_id: WAVE.to_owned(),
            key: Some(key.clone()),
        },
    );
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
                original_name: super::super::wavetable_edit::table_name(
                    def.keys.len(),
                    def.phase_safe,
                ),
            },
        },
    );
    set_params(&mut doc, &registry, item, &key, WAVE, def.params);
    run(
        &mut doc,
        &registry,
        item,
        Command::SetZones {
            device: key.clone(),
            zones: vec![ZoneDoc::new(hash.clone(), frames)],
            merge: None,
        },
    );
    run(
        &mut doc,
        &registry,
        item,
        Command::Workflow(WorkflowCommand::SetPatchName {
            device: key.clone(),
            name: def.name.to_owned(),
        }),
    );
    let snapshot =
        DeviceSnapshot::capture(doc.doc(), &[key], false).unwrap_or_else(|e| panic!("{item}: {e}"));
    Built {
        text: save_device_item(&snapshot, doc.doc()),
        audio: vec![(hash, data)],
    }
}

/// Builds the item of `def`.
pub(in crate::app_core) fn build_table(def: &TableDef) -> Built {
    table_item(def, table_audio(def))
}
