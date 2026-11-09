//! The audio loops of the factory library (test code): twelve short stereo loops rendered from
//! the factory note clips, each played by a factory drum kit or a factory instrument patch.
//!
//! A loop is a row of a table: which clip plays, who plays it, and the tags (the tempo and, for
//! pitched loops, the key) the browser filters by. The renderer builds a hidden project at the
//! clip's tempo, plays the clip twice in a row and keeps the second pass, so the tails of the
//! first pass ring into the start and the loop repeats without a gap. The result is levelled
//! and written as a 24-bit FLAC file next to the clips (`Clips/<name>.flac`).
//!
//! Like the kits, the renders are deterministic (seeded noise, no clock); only the bytes on the
//! platform the library was written on are guaranteed, which is why the tests compare the audio
//! with a tolerance.

use super::kits::{DRUM, Hit, KITS, KitDef, RATE, key_of};
use super::{CLIPS, ClipDef, DEFS, Def, clip_notes, run, set_mods, set_params};
use atomik_core::PPQ;
use atomik_dsp::DitherMode;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document, InsertEffect, Note,
    SongCommand, TempoEvent, new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::flac::write_flac_file;
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_render::wav::SampleFormat;
use std::path::Path;

/// Where the peak of every loop is levelled to.
const PEAK: f32 = 0.7;

/// Who plays a loop.
pub(in crate::app_core) enum Source {
    /// A factory drum kit, by name: its pads play the clip's keys (as synthesised pads).
    Kit(&'static str),
    /// A factory instrument patch, by name.
    Patch(&'static str),
}

/// One factory loop.
pub(in crate::app_core) struct LoopDef {
    /// The loop name (and the file name).
    pub name: &'static str,
    /// The factory clip it plays, by name.
    pub clip: &'static str,
    /// Who plays it.
    pub source: Source,
    /// What kind of loop it is (the browser's category column).
    pub category: &'static str,
    /// Words to search and filter by: the tempo (`<bpm> bpm`) and, for a pitched loop, the key.
    pub tags: &'static [&'static str],
    /// One line on how it sounds (for the notes of a release; not saved).
    pub sound: &'static str,
}

/// The factory loops, in the order the manifest lists them.
pub(in crate::app_core) const LOOPS: &[LoopDef] = &[
    LoopDef {
        name: "Timber Boom Bap Loop",
        clip: "Boom Bap Swing",
        source: Source::Kit("Timber Kit"),
        category: "Drum Loop",
        tags: &["loop", "drums", "88 bpm", "boom-bap"],
        sound: "A laid-back hip-hop groove on the acoustic-like kit.",
    },
    LoopDef {
        name: "Circuit Trap Loop",
        clip: "Trap Rolls",
        source: Source::Kit("Circuit Kit"),
        category: "Drum Loop",
        tags: &["loop", "drums", "140 bpm", "trap"],
        sound: "A half-time trap pattern with rolling hats on the electronic kit.",
    },
    LoopDef {
        name: "Cassette Chop Loop",
        clip: "Breakbeat Chop",
        source: Source::Kit("Cassette Kit"),
        category: "Drum Loop",
        tags: &["loop", "drums", "160 bpm", "breakbeat"],
        sound: "A fast chopped breakbeat through the crushed lo-fi kit.",
    },
    LoopDef {
        name: "Circuit Techno Loop",
        clip: "Techno Drive",
        source: Source::Kit("Circuit Kit"),
        category: "Drum Loop",
        tags: &["loop", "drums", "132 bpm", "techno"],
        sound: "A four-on-the-floor techno pattern on the electronic kit.",
    },
    LoopDef {
        name: "Bazaar Bossa Loop",
        clip: "Bossa Pulse",
        source: Source::Kit("Bazaar Kit"),
        category: "Drum Loop",
        tags: &["loop", "percussion", "120 bpm", "bossa"],
        sound: "A gentle bossa pulse played on hand percussion.",
    },
    LoopDef {
        name: "Basalt Dub Loop",
        clip: "Dub Steppers",
        source: Source::Kit("Basalt Kit"),
        category: "Drum Loop",
        tags: &["loop", "drums", "70 bpm", "dub"],
        sound: "A slow one-drop pattern on the deep kit.",
    },
    LoopDef {
        name: "Sub Roots Loop",
        clip: "Dub Sub Roots",
        source: Source::Patch("Mono Sub Bass"),
        category: "Bass Loop",
        tags: &["loop", "bass", "70 bpm", "A minor"],
        sound: "A sparse sub bass line in A minor, four bars.",
    },
    LoopDef {
        name: "Acid Climb Loop",
        clip: "Acid Climb",
        source: Source::Patch("Acid Squelch"),
        category: "Bass Loop",
        tags: &["loop", "bass", "132 bpm", "A minor"],
        sound: "A squelching sixteenth-note acid line in A minor.",
    },
    LoopDef {
        name: "Warm Sevenths Loop",
        clip: "Warm Minor Sevenths",
        source: Source::Patch("Warm Drift Pad"),
        category: "Chord Loop",
        tags: &["loop", "chords", "84 bpm", "A minor"],
        sound: "Four warm seventh chords on a slow analog pad, four bars.",
    },
    LoopDef {
        name: "Dorian Organ Loop",
        clip: "Dorian Vamp",
        source: Source::Patch("Gospel Full"),
        category: "Chord Loop",
        tags: &["loop", "chords", "104 bpm", "D dorian"],
        sound: "A rocking two-chord vamp on a full organ, four bars.",
    },
    LoopDef {
        name: "Pentatonic Pluck Loop",
        clip: "Pentatonic Run",
        source: Source::Patch("Copper Pluck"),
        category: "Arp Loop",
        tags: &["loop", "arpeggio", "118 bpm", "A minor"],
        sound: "A minor pentatonic run on a bright pluck, two bars.",
    },
    LoopDef {
        name: "Marimba Triplets Loop",
        clip: "Triplet Climb",
        source: Source::Patch("Soft Marimba"),
        category: "Arp Loop",
        tags: &["loop", "arpeggio", "110 bpm", "G minor"],
        sound: "A flowing triplet figure on a soft marimba, two bars.",
    },
];

/// The path of a factory loop, relative to the factory folder.
pub(in crate::app_core) fn loop_path(name: &str) -> String {
    format!("Clips/{name}.flac")
}

/// The clip a loop plays.
///
/// # Panics
/// A loop that names no factory clip.
pub(in crate::app_core) fn clip_of(def: &LoopDef) -> &'static ClipDef {
    CLIPS
        .iter()
        .find(|c| c.name == def.clip)
        .unwrap_or_else(|| panic!("{}: no factory clip {}", def.name, def.clip))
}

/// The length of one pass of a loop, in frames.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a few seconds of frames
pub(in crate::app_core) fn loop_frames(clip: &ClipDef) -> usize {
    (clip.bars as f64 * 4.0 * 60.0 / clip.bpm * f64::from(RATE)).round() as usize
}

/// Milliseconds to ticks at `bpm`.
#[allow(clippy::cast_possible_truncation)] // a few milliseconds
fn ms_ticks(ms: f32, bpm: f64) -> i64 {
    (f64::from(ms) / 1000.0 * bpm / 60.0 * PPQ as f64).round() as i64
}

fn kit_of(def: &LoopDef, name: &str) -> &'static KitDef {
    KITS.iter()
        .find(|k| k.name == name)
        .unwrap_or_else(|| panic!("{}: no factory kit {name}", def.name))
}

fn patch_of(def: &LoopDef, name: &str) -> &'static Def {
    DEFS.iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{}: no factory patch {name}", def.name))
}

/// Sets the pads of the drum machine `key` to the synthesised sounds of `kit`, with the kit's
/// effects as inserts, and returns the kit's hits.
fn kit_scene(doc: &mut Document, item: &str, key: &DeviceKey, kit: &KitDef) {
    let registry = default_registry();
    run(
        doc,
        &registry,
        item,
        Command::AddDevice {
            index: usize::MAX,
            type_id: DRUM.to_owned(),
            key: Some(key.clone()),
        },
    );
    let mut params: Vec<(String, f32)> = Vec::new();
    for hit in kit.hits {
        let pad = |name: &str| format!("pad{}.{name}", hit.pad);
        params.push((pad("module"), f32::from(hit.module)));
        params.push((pad("level"), hit.level));
        params.push((pad("pan"), hit.pan));
        params.push((pad("choke"), f32::from(hit.choke)));
        params.extend(hit.params.iter().map(|&(n, v)| (pad(n), v)));
    }
    let borrowed: Vec<(&str, f32)> = params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    set_params(doc, &registry, item, key, DRUM, &borrowed);
    let channel = doc
        .doc()
        .console
        .channel_of(key)
        .map(|c| c.channel.clone())
        .unwrap_or_else(|| panic!("{item}: the drum machine has no channel"));
    for (slot, part) in kit.effects.iter().enumerate() {
        let fx_key = DeviceKey(part.key.to_owned());
        run(
            doc,
            &registry,
            item,
            Command::AddDevice {
                index: usize::MAX,
                type_id: part.device.to_owned(),
                key: Some(fx_key.clone()),
            },
        );
        set_params(doc, &registry, item, &fx_key, part.device, part.params);
        run(
            doc,
            &registry,
            item,
            Command::Console(ConsoleCommand::SetInsert {
                channel: channel.clone(),
                slot,
                effect: Some(InsertEffect::Existing(fx_key)),
            }),
        );
    }
}

/// The notes a loop plays: the clip's, with every strike of a multi-strike pad (a clap) added.
fn played_notes(clip: &ClipDef, kit: Option<&KitDef>) -> Vec<Note> {
    let mut out = clip_notes(clip);
    if let Some(kit) = kit {
        let extra: Vec<Note> = out
            .iter()
            .flat_map(|n| {
                let hit: Option<&Hit> = kit.hits.iter().find(|h| key_of(h.pad) == n.key);
                hit.into_iter().flat_map(move |h| {
                    h.strikes.iter().skip(1).map(move |&ms| {
                        Note::new(n.tick + ms_ticks(ms, clip.bpm), n.length, n.key, n.velocity)
                    })
                })
            })
            .collect();
        out.extend(extra);
        out.sort_by_key(|n| (n.tick, n.key));
    }
    out
}

/// Renders `def` to the stereo planes of one loop pass, levelled.
///
/// # Panics
/// A name the tables do not have, a param the device does not have or a failed render.
pub(in crate::app_core) fn render_loop(def: &LoopDef) -> Vec<Vec<f32>> {
    let registry = default_registry();
    let item = def.name;
    let clip = clip_of(def);
    let key = DeviceKey("loop".to_owned());
    let mut doc = Document::new(new_project_doc());
    run(
        &mut doc,
        &registry,
        item,
        Command::Song(SongCommand::SetTempoMap(vec![TempoEvent::step(
            0, clip.bpm,
        )])),
    );
    let kit = match def.source {
        Source::Kit(name) => {
            let kit = kit_of(def, name);
            kit_scene(&mut doc, item, &key, kit);
            Some(kit)
        }
        Source::Patch(name) => {
            let patch = patch_of(def, name);
            run(
                &mut doc,
                &registry,
                item,
                Command::AddDevice {
                    index: usize::MAX,
                    type_id: patch.device.to_owned(),
                    key: Some(key.clone()),
                },
            );
            set_params(&mut doc, &registry, item, &key, patch.device, patch.params);
            set_mods(&mut doc, &registry, item, &key, patch.device, patch.mods);
            None
        }
    };
    run(
        &mut doc,
        &registry,
        item,
        Command::Song(SongCommand::AddTrack {
            device: key.clone(),
        }),
    );
    let notes = played_notes(clip, kit);
    let length = clip.bars * 4 * PPQ;
    for pass in 0..2u64 {
        let content = Clip {
            id: ClipId(pass),
            start: i64::try_from(pass).expect("two passes") * length,
            length,
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
            content: ClipContent::Notes(notes.clone().into()),
        };
        run(
            &mut doc,
            &registry,
            item,
            Command::Song(SongCommand::AddClip {
                device: key.clone(),
                lane: 0,
                clip: content,
            }),
        );
    }
    run(
        &mut doc,
        &registry,
        item,
        Command::Song(SongCommand::SetSongEnd(2 * length)),
    );
    let rendered = render_doc_in(doc.doc(), &registry, &ProjectOptions::default(), None)
        .unwrap_or_else(|e| panic!("{item}: {e}"));
    assert_eq!(rendered.sample_rate, RATE, "{item}");
    let frames = loop_frames(clip);
    let mut planes: Vec<Vec<f32>> = rendered
        .channels
        .iter()
        .map(|c| {
            assert!(c.len() >= 2 * frames, "{item}: the render is short");
            c[frames..2 * frames].to_vec()
        })
        .collect();
    let peak = planes.iter().flatten().fold(0.0f32, |p, s| p.max(s.abs()));
    assert!(peak > 1.0e-3, "{item}: the render is silent");
    let level = PEAK / peak;
    for s in planes.iter_mut().flatten() {
        *s *= level;
    }
    planes
}

/// Writes the loop `def` as `<root>/Clips/<name>.flac`.
///
/// # Panics
/// The file cannot be written.
pub(in crate::app_core) fn write_loop(root: &Path, def: &LoopDef) {
    let planes = render_loop(def);
    let path = root.join(loop_path(def.name));
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("the Clips folder");
    write_flac_file(
        &path,
        &planes,
        RATE,
        SampleFormat::Pcm24,
        (DitherMode::None, 0),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}
