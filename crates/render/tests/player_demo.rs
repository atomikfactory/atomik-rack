//! The `player-demo` example project (bounce part): a step sequencer
//! and an arpeggiator cabled to poly synths, bounced through the shared builder.
//!
//! `examples/projects/player-demo/player-demo.atomik` (no audio files, so no bundle) holds:
//!
//! * **Bass Line**, a step sequencer on its own two patterns (a 16-step 1/16 bass line with
//!   swing, a tied pair, short and 50 %/60 % probability steps; a 12-step 1/8 variation),
//!   its notes cabled to the **Bass** poly-synth;
//! * the sequencer's `pitch_cv_out` cabled to the **Arp Voice** synth's cutoff CV, so the
//!   filter follows the pattern;
//! * **Arp**, an arpeggiator fed by a note clip (an A minor chord for four bars) and cabled to
//!   the Arp Voice synth: up-down over two octaves at 1/16T.
//!
//! The song is four bars at 120 BPM. Regenerate the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test player_demo regenerate
//! ```

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, PatternDoc, RackDoc,
    SeqResolution, SocketRef, SongCommand, StepDoc, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BAR: i64 = 4 * PPQ;
const BARS: i64 = 4;

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/player-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("player-demo.atomik")
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(d: &DeviceKey, s: &str) -> SocketRef {
    SocketRef {
        device: d.clone(),
        socket: s.to_string(),
    }
}

struct Builder {
    doc: Document,
}

impl Builder {
    fn apply(&mut self, c: Command) {
        let registry = default_registry();
        self.doc
            .apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }

    fn add(&mut self, type_id: &str, k: &str, name: &str) {
        self.apply(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        });
        self.apply(Command::Rename {
            key: key(k),
            name: name.into(),
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
            from: sref(&key(from.0), from.1),
            to: sref(&key(to.0), to.1),
        }));
    }
}

/// `(step, key, velocity, gate %, probability %, tie)` of the bass line (16 x 1/16).
const BASS: [(usize, u8, u8, u8, u8, bool); 10] = [
    (0, 45, 110, 80, 100, false),
    (2, 45, 90, 50, 100, false),
    (4, 48, 100, 80, 100, true),
    (5, 50, 100, 80, 100, false),
    (7, 52, 85, 60, 50, false),
    (8, 45, 110, 80, 100, false),
    (10, 45, 90, 50, 100, false),
    (12, 43, 100, 80, 100, false),
    (14, 47, 95, 70, 100, false),
    (15, 48, 80, 60, 60, false),
];

fn pattern_of(
    name: &str,
    length: u8,
    resolution: SeqResolution,
    swing: u8,
    notes: &[(usize, u8, u8, u8, u8, bool)],
) -> PatternDoc {
    let mut p = PatternDoc::new(name);
    p.length = length;
    p.resolution = resolution;
    p.swing = swing;
    for &(step, k, velocity, gate, prob, tie) in notes {
        p.steps[step] = StepDoc {
            on: true,
            key: k,
            velocity,
            gate,
            prob,
            tie,
        };
    }
    p
}

/// The two patterns of the demo's sequencer.
pub fn demo_patterns() -> [PatternDoc; 2] {
    let variation = [
        (0, 45, 110, 90, 100, false),
        (3, 52, 90, 60, 100, false),
        (6, 50, 100, 80, 100, false),
        (8, 48, 100, 80, 70, false),
    ];
    [
        pattern_of("Bass Line", 16, SeqResolution::Sixteenth, 20, &BASS),
        pattern_of("Variation", 12, SeqResolution::Eighth, 0, &variation),
    ]
}

/// The demo project: sequencer and arpeggiator cabled to two synths.
pub fn demo_doc() -> RackDoc {
    let mut b = Builder {
        doc: Document::new(empty_doc()),
    };
    b.add("atomik.step-sequencer", "seq", "Bass Line");
    for (i, p) in demo_patterns().into_iter().enumerate() {
        if i > 0 {
            b.apply(Command::AddPattern { device: key("seq") });
        }
        b.apply(Command::SetPattern {
            device: key("seq"),
            index: i as u8,
            pattern: p,
            merge: None,
        });
    }
    b.add("atomik.poly-synth", "bass", "Bass");
    for (p, v) in [
        ("osc1.wave", 0.0),
        ("osc.mix", 0.0),
        ("voices", 1.0),
        ("filter.cutoff", 700.0),
        ("filter.resonance", 0.15),
        ("filter.env_amount", 0.2),
        ("env.amp.attack", 3.0),
        ("env.amp.decay", 200.0),
        ("env.amp.sustain", 0.85),
        ("env.amp.release", 60.0),
        ("master.level", -6.0),
    ] {
        b.set("bass", p, v);
    }
    b.add("atomik.poly-synth", "voice", "Arp Voice");
    for (p, v) in [
        ("osc1.wave", 1.0),
        ("osc.mix", 0.0),
        ("filter.cutoff", 900.0),
        ("cv.cutoff_amount", 24.0),
        ("env.amp.attack", 4.0),
        ("env.amp.decay", 150.0),
        ("env.amp.sustain", 0.5),
        ("env.amp.release", 120.0),
        ("master.level", -14.0),
    ] {
        b.set("voice", p, v);
    }
    b.add("atomik.arpeggiator", "arp", "Arp");
    for (p, v) in [
        ("mode", 2.0),
        ("rate", 4.0),
        ("octaves", 2.0),
        ("gate", 60.0),
        ("run", 0.0),
    ] {
        b.set("arp", p, v);
    }
    b.connect(("seq", "notes_out"), ("bass", "notes_in"));
    b.connect(("seq", "pitch_cv_out"), ("voice", "cutoff_cv_in"));
    b.connect(("arp", "notes_out"), ("voice", "notes_in"));
    b.apply(Command::Song(SongCommand::AddTrack { device: key("arp") }));
    let length = BARS * BAR;
    let chord = [57u8, 60, 64].map(|k| Note::new(0, length, k, 0.8));
    b.apply(Command::Song(SongCommand::AddClip {
        device: key("arp"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            grid: None,
            content: ClipContent::Notes(chord.to_vec()),
        },
    }));
    b.apply(Command::Song(SongCommand::SetSongEnd(length)));
    b.apply(Command::Song(SongCommand::SetLoop {
        range: (0, length),
        enabled: false,
    }));
    b.doc.doc().clone()
}

#[test]
fn regenerate() {
    if std::env::var_os(UPDATE_ENV).is_none() {
        return;
    }
    std::fs::create_dir_all(demo_root()).expect("project folder");
    std::fs::write(demo_file(), save_project(&demo_doc())).expect("project written");
}

fn registry_and_doc() -> (atomik_device_sdk::Registry, RackDoc) {
    let registry = default_registry();
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    (registry, doc)
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bits(r: &atomik_render::Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> atomik_render::Rendered {
    render_doc_in(doc, &default_registry(), &options(block, workers), None).unwrap()
}

fn rms(channel: &[f32], from: usize, to: usize) -> f64 {
    let w = &channel[from..to.min(channel.len())];
    (w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

fn set_param(doc: &mut RackDoc, device: &str, param: &str, value: f32) {
    let d = doc
        .devices
        .iter_mut()
        .find(|d| d.key == key(device))
        .unwrap();
    d.params.insert(param.to_string(), value);
}

#[test]
fn the_committed_project_is_the_generated_one_and_canonical() {
    let (registry, doc) = registry_and_doc();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    assert_eq!(text, save_project(&demo_doc()), "regenerate the demo");
    assert_eq!(save_project(&doc), text);
    let again = load_project(&save_project(&doc), &registry).unwrap();
    assert_eq!(again, doc);
    assert!(text.contains("\"version\": 28"));
}

#[test]
fn the_demo_has_both_players_cabled_to_synths_and_two_patterns() {
    let (_, doc) = registry_and_doc();
    let seq = doc.device(&key("seq")).unwrap();
    let patterns = &seq.steps.as_ref().expect("pattern data").patterns;
    assert_eq!(patterns.len(), 2);
    assert_eq!(patterns[0].length, 16);
    assert_eq!(patterns[1].resolution, SeqResolution::Eighth);
    let has = |from: &str, s: &str, to: &str, t: &str| {
        doc.cables
            .iter()
            .any(|c| c.from == sref(&key(from), s) && c.to == sref(&key(to), t))
    };
    assert!(has("seq", "notes_out", "bass", "notes_in"));
    assert!(has("seq", "pitch_cv_out", "voice", "cutoff_cv_in"));
    assert!(has("arp", "notes_out", "voice", "notes_in"));
}

#[test]
fn the_sequencer_patterns_play_in_the_bounce() {
    let (_, doc) = registry_and_doc();
    let all = bounce(&doc, 512, 0);
    let peak = all
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.02..1.0).contains(&peak), "peak {peak}");
    // Without the arp the bass line alone still sounds in the first beat.
    let mut no_arp = doc.clone();
    set_param(&mut no_arp, "voice", "master.level", -96.0);
    let bass_only = bounce(&no_arp, 512, 0);
    assert!(rms(&bass_only.channels[0], 0, 24_000) > 1e-3);
    // A sequencer pattern edit changes the sound: the data reaches the device.
    let mut edited = doc.clone();
    edited
        .devices
        .iter_mut()
        .find(|d| d.key == key("seq"))
        .unwrap()
        .steps
        .as_mut()
        .unwrap()
        .patterns[0]
        .steps[0]
        .key = 50;
    assert!(bits(&bounce(&edited, 512, 0)) != bits(&all));
}

#[test]
fn the_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let (_, doc) = registry_and_doc();
    let reference = bits(&bounce(&doc, 512, 0));
    for (block, workers) in [(64, 0), (480, 0), (512, 2), (1000, 3)] {
        assert!(
            bits(&bounce(&doc, block, workers)) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn seeded_probability_is_the_same_on_every_run_and_the_seed_changes_it() {
    let (_, doc) = registry_and_doc();
    let a = bits(&bounce(&doc, 512, 0));
    assert!(a == bits(&bounce(&doc, 256, 0)));
    let mut other = doc.clone();
    set_param(&mut other, "seq", "random.seed", 7.0);
    assert!(
        bits(&bounce(&other, 512, 0)) != a,
        "the seed re-rolls the 50 % steps"
    );
    let mut sure = doc.clone();
    set_param(&mut sure, "seq", "probability.amount", 0.0);
    assert!(
        bits(&bounce(&sure, 512, 0)) != a,
        "probability amount is audible"
    );
}

#[test]
fn a_sequencer_into_an_arpeggiator_into_a_synth_plays() {
    // The sequencer's notes through the arpeggiator: the Arp Voice plays the bass line arpeggiated.
    let (_, mut doc) = registry_and_doc();
    doc.cables
        .retain(|c| !(c.from.device == key("seq") && c.from.socket == "notes_out"));
    doc.cables.push(Cable {
        from: sref(&key("seq"), "notes_out"),
        to: sref(&key("arp"), "notes_in"),
    });
    doc.song.tracks.clear();
    set_param(&mut doc, "bass", "master.level", -96.0);
    set_param(&mut doc, "arp", "run", 1.0);
    let heard = bounce(&doc, 512, 0);
    assert!(rms(&heard.channels[0], 0, 96_000) > 1e-3);
    assert!(bits(&heard) == bits(&bounce(&doc, 64, 2)));
}

#[test]
fn the_example_bounces_from_its_file_and_matches_the_document_bounce() {
    let (registry, doc) = registry_and_doc();
    let from_file = render_project_file(&demo_file(), &registry, &options(512, 0)).unwrap();
    assert!(bits(&from_file) == bits(&bounce(&doc, 512, 0)));
    assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
}

#[test]
fn the_arpeggiated_chord_sounds_on_its_own_and_the_bass_line_stays_quiet_without_data() {
    let (_, doc) = registry_and_doc();
    let mut arp_only = doc.clone();
    set_param(&mut arp_only, "bass", "master.level", -96.0);
    let heard = bounce(&arp_only, 512, 0);
    assert!(rms(&heard.channels[0], 0, 96_000) > 1e-3, "the arp plays");
    // A sequencer whose patterns are all empty plays nothing: only the arp remains.
    let mut empty = doc.clone();
    let seq = empty
        .devices
        .iter_mut()
        .find(|d| d.key == key("seq"))
        .unwrap();
    for p in &mut seq.steps.as_mut().unwrap().patterns {
        p.steps = vec![StepDoc::default(); 64];
    }
    let quiet = bounce(&empty, 512, 0);
    let mut muted = empty.clone();
    set_param(&mut muted, "bass", "master.level", -96.0);
    let only_arp = bounce(&muted, 512, 0);
    assert!(rms(&quiet.channels[0], 0, 96_000) > 1e-3);
    assert!(rms(&only_arp.channels[0], 0, 96_000) > 1e-3);
    assert!(bits(&quiet) != bits(&bounce(&doc, 512, 0)));
}
