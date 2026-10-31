//! Bounces of the note tools (players that map or generate notes between a clip and a synth).
//!
//! Each test builds its project in memory and bounces it through the shared renderer, so the
//! engine path (track input, note cable, device batching) is covered, not just the device.

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SocketRef,
    SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: key(d),
        socket: s.to_string(),
    }
}

struct Builder {
    doc: Document,
}

impl Builder {
    fn new() -> Self {
        Self {
            doc: Document::new(empty_doc()),
        }
    }

    fn apply(&mut self, c: Command) {
        self.doc
            .apply(c.clone(), &default_registry())
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }

    fn add(&mut self, type_id: &str, k: &str) {
        self.apply(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        });
    }

    fn set(&mut self, k: &str, p: &str, v: f32) {
        self.apply(Command::SetParam {
            key: key(k),
            param: p.into(),
            value: v,
            merge: None,
        });
    }

    fn connect(&mut self, from: (&str, &str), to: (&str, &str)) {
        self.apply(Command::Connect(Cable {
            from: sref(from.0, from.1),
            to: sref(to.0, to.1),
        }));
    }

    /// A one-bar clip of `notes` (`start`, `length`, `key`, `velocity`) on the track of `device`.
    fn clip(&mut self, device: &str, notes: &[(i64, i64, u8, f32)]) {
        self.apply(Command::Song(SongCommand::AddTrack {
            device: key(device),
        }));
        let notes: Vec<Note> = notes
            .iter()
            .map(|&(s, l, k, v)| Note::new(s, l, k, v))
            .collect();
        self.apply(Command::Song(SongCommand::AddClip {
            device: key(device),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(notes.into()),
            },
        }));
        self.apply(Command::Song(SongCommand::SetSongEnd(BAR)));
        self.apply(Command::Song(SongCommand::SetLoop {
            range: (0, BAR),
            enabled: false,
        }));
    }

    fn synth(&mut self) {
        self.add("atomik.poly-synth", "synth");
        self.set("synth", "env.amp.release", 40.0);
    }
}

const NOTES: [(i64, i64, u8, f32); 4] = [
    (0, PPQ, 48, 0.8),
    (PPQ, PPQ, 52, 0.6),
    (PPQ + PPQ / 2, 2 * PPQ, 60, 0.7),
    (2 * PPQ, PPQ, 55, 0.9),
];

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = render_doc_in(doc, &default_registry(), &options, None).unwrap();
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn through_filter(set: &[(&str, f32)]) -> RackDoc {
    let mut b = Builder::new();
    b.synth();
    b.add("atomik.note-filter", "filter");
    for (p, v) in set {
        b.set("filter", p, *v);
    }
    b.connect(("filter", "notes_out"), ("synth", "notes_in"));
    b.clip("filter", &NOTES);
    b.doc.doc().clone()
}

fn straight(notes: &[(i64, i64, u8, f32)]) -> RackDoc {
    let mut b = Builder::new();
    b.synth();
    b.clip("synth", notes);
    b.doc.doc().clone()
}

#[test]
fn note_filter_transpose_bounces_like_the_transposed_clip_into_the_synth() {
    let up: Vec<_> = NOTES
        .iter()
        .map(|&(s, l, k, v)| (s, l, k + 12, v))
        .collect();
    let direct = bounce(&straight(&up), 512, 0);
    assert!(
        direct.iter().any(|b| f32::from_bits(*b).abs() > 1e-3),
        "audible"
    );
    let filtered = through_filter(&[("transpose", 12.0)]);
    assert!(
        bounce(&filtered, 512, 0) == direct,
        "+12 equals the shifted clip"
    );
    // The device is not a no-op: unshifted notes sound different.
    assert!(bounce(&through_filter(&[]), 512, 0) != direct);
}

#[test]
fn note_filter_bounce_is_bit_identical_across_block_sizes_and_workers() {
    let doc = through_filter(&[
        ("transpose", 7.0),
        ("velocity.curve", 0.5),
        ("key.high", 58.0),
    ]);
    let reference = bounce(&doc, 512, 0);
    for (block, workers) in [(64, 0), (333, 0), (512, 2), (1000, 3)] {
        assert!(
            bounce(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn note_filter_key_range_bounces_like_the_clip_without_the_other_notes() {
    let kept: Vec<_> = NOTES.iter().copied().filter(|n| n.2 <= 55).collect();
    let direct = bounce(&straight(&kept), 512, 0);
    let filtered = through_filter(&[("key.high", 55.0)]);
    assert!(bounce(&filtered, 512, 0) == direct);
}

fn through_key_chord(set: &[(&str, f32)]) -> RackDoc {
    let mut b = Builder::new();
    b.synth();
    b.add("atomik.key-chord", "kc");
    for (p, v) in set {
        b.set("kc", p, *v);
    }
    b.connect(("kc", "notes_out"), ("synth", "notes_in"));
    b.clip("kc", &NOTES);
    b.doc.doc().clone()
}

#[test]
fn key_chord_snap_bounces_like_the_clip_snapped_into_the_scale() {
    // C natural minor: E (52) is between E flat and F and snaps down to E flat (51).
    let snapped: Vec<_> = NOTES
        .iter()
        .map(|&(s, l, k, v)| (s, l, if k == 52 { 51 } else { k }, v))
        .collect();
    let direct = bounce(&straight(&snapped), 512, 0);
    assert!(
        direct.iter().any(|b| f32::from_bits(*b).abs() > 1e-3),
        "audible"
    );
    let through = through_key_chord(&[("scale.kind", 2.0)]);
    assert!(bounce(&through, 512, 0) == direct, "snapped like the clip");
    // The chord is not a no-op.
    let chord = through_key_chord(&[("scale.kind", 2.0), ("chord", 1.0)]);
    assert!(bounce(&chord, 512, 0) != direct);
}

#[test]
fn key_chord_bounce_is_bit_identical_across_block_sizes_and_workers() {
    let doc = through_key_chord(&[
        ("scale.root", 2.0),
        ("scale.kind", 5.0),
        ("chord", 2.0),
        ("chord.voicing", 1.0),
        ("chord.velocity", 0.6),
    ]);
    let reference = bounce(&doc, 512, 0);
    for (block, workers) in [(64, 0), (333, 0), (512, 2), (1000, 3)] {
        assert!(
            bounce(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
}

fn through_device(type_id: &str, set: &[(&str, f32)]) -> RackDoc {
    let mut b = Builder::new();
    b.synth();
    b.add(type_id, "fx");
    for (p, v) in set {
        b.set("fx", p, *v);
    }
    b.connect(("fx", "notes_out"), ("synth", "notes_in"));
    b.clip("fx", &NOTES);
    b.doc.doc().clone()
}

#[test]
fn repeater_bounce_is_bit_identical_across_block_sizes() {
    let doc = through_device(
        "atomik.note-repeater",
        &[
            ("time.sync", 4.0),
            ("repeats", 5.0),
            ("velocity.decay", 0.3),
            ("pitch.step", 2.0),
            ("pitch.scale", 1.0),
            ("scale.kind", 1.0),
            ("gate", 0.7),
        ],
    );
    let reference = bounce(&doc, 512, 0);
    assert!(
        reference.iter().any(|b| f32::from_bits(*b).abs() > 1e-3),
        "audible"
    );
    // The echoes are not a no-op.
    assert!(bounce(&straight(&NOTES), 512, 0) != reference);
    for (block, workers) in [(64, 0), (333, 0), (512, 2), (1000, 3)] {
        assert!(
            bounce(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn delay_bounce_is_bit_identical_across_block_sizes_with_swing_and_humanize() {
    let doc = through_device(
        "atomik.note-delay",
        &[
            ("delay.sync", 3.0),
            ("swing", 60.0),
            ("humanize.time", 15.0),
            ("humanize.velocity", 20.0),
            ("random.seed", 42.0),
        ],
    );
    let reference = bounce(&doc, 512, 0);
    assert!(
        reference.iter().any(|b| f32::from_bits(*b).abs() > 1e-3),
        "audible"
    );
    assert!(bounce(&straight(&NOTES), 512, 0) != reference);
    for (block, workers) in [(64, 0), (333, 0), (512, 2), (1000, 3)] {
        assert!(
            bounce(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn delay_of_zero_bounces_like_the_clip_and_a_repeat_adds_to_it() {
    let direct = bounce(&straight(&NOTES), 512, 0);
    let none = through_device("atomik.note-delay", &[("delay.sync", 0.0)]);
    assert!(bounce(&none, 512, 0) == direct, "no delay, no change");
    let shifted = through_device("atomik.note-delay", &[("delay.sync", 7.0)]);
    assert!(bounce(&shifted, 512, 0) != direct);
}

// ---------------------------------------------------------------- drum generator

/// The `drum-demo` project (a drum machine loaded with five procedurally generated sounds on
/// the keys 36, 38, 39, 42 and 46) with its clip removed and a drum generator `gen` cabled to
/// the drum machine, playing a generated pattern with `set` applied.
fn generator_into_drums(set: &[(&str, f32)]) -> RackDoc {
    let reg = default_registry();
    let text = std::fs::read_to_string(demo_root().join("drum-demo.atomik")).unwrap();
    let mut doc = atomik_model::load_project(&text, &reg).unwrap();
    for track in &mut doc.song_mut().tracks {
        for lane in &mut track.lanes {
            lane.clips.clear();
        }
    }
    let mut b = Builder {
        doc: Document::new(doc),
    };
    b.add("atomik.drum-generator", "gen");
    b.connect(("gen", "notes_out"), ("drums", "notes_in"));
    let base = atomik_model::DrumPatternDoc::default();
    let written = atomik_devices::drum_generator::generate_pattern(
        &base.to_data(),
        atomik_devices::drum_generator::DrumStyle::BrokenBeat,
        0.0,
        3,
    );
    let mut pattern = atomik_model::DrumPatternDoc::from_data(&written, "Broken");
    pattern.swing = 20;
    b.apply(Command::SetDrumPattern {
        device: key("gen"),
        index: 0,
        pattern,
        merge: None,
    });
    for (p, v) in set {
        b.set("gen", p, *v);
    }
    b.doc.doc().clone()
}

fn demo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/drum-demo")
}

fn bounce_in_bundle(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = render_doc_in(doc, &default_registry(), &options, Some(&demo_root())).unwrap();
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn audible(bits: &[u32]) -> bool {
    bits.iter().any(|b| f32::from_bits(*b).abs() > 1e-3)
}

#[test]
fn drum_generator_bounce_into_a_drum_machine_is_bit_identical_across_blocks_and_workers() {
    let set = [
        ("density", 0.3),
        ("variation", 0.6),
        ("fill.every", 1.0),
        ("fill.amount", 0.5),
        ("accent", 0.4),
        ("random.seed", 11.0),
    ];
    let doc = generator_into_drums(&set);
    let reference = bounce_in_bundle(&doc, 512, 0);
    assert!(audible(&reference), "the drum machine sounds");
    for (block, workers) in [(64, 0), (333, 0), (512, 2)] {
        assert!(
            bounce_in_bundle(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn drum_generator_params_and_mute_change_what_the_drum_machine_plays() {
    let plain = bounce_in_bundle(&generator_into_drums(&[]), 512, 0);
    assert!(audible(&plain));
    // Density and a changed seed play other hits; a muted generator plays nothing.
    let denser = bounce_in_bundle(&generator_into_drums(&[("density", 0.8)]), 512, 0);
    assert!(denser != plain, "density adds hits");
    let seeded = bounce_in_bundle(
        &generator_into_drums(&[("density", 0.8), ("random.seed", 5.0)]),
        512,
        0,
    );
    assert!(seeded != denser, "the seed draws other hits");
    let muted = bounce_in_bundle(&generator_into_drums(&[("mute", 1.0)]), 512, 0);
    assert!(!audible(&muted), "mute plays nothing");
    // The same project bounces to the same samples twice.
    assert!(bounce_in_bundle(&generator_into_drums(&[]), 512, 0) == plain);
}
