//! The `master-demo` example project:
//! the starter project, two instruments made by the creation chain, a reverb patched into send
//! and return 1 and a send level on the master.
//!
//! `examples/projects/master-demo/master-demo.atomik` (no audio files, so no bundle) is built
//! from [`atomik_model::new_project_doc`] by model commands only:
//!
//! * **Audio I/O** on top and the locked **Master** under it, `main` cabled to outputs 1 and 2
//!   (the starter).
//! * **Lead** and **Bass**, poly synths made by `CreateInstrument` (device, mixer channel
//!   created folded, armed track; the tracks are disarmed again so the file opens quietly).
//! * **Hall**, a reverb made by `CreateEffect` with the master selected: it sits on send and
//!   return 1 (`fx_send1 -> Hall -> return1`). Lead sends to bus 1 at -6 dB, Bass does not.
//! * The master's **send 1 level** is -4 dB, the new trim on the way to the effect.
//! * A four-bar song: a chord line on Lead and a bass line on Bass.
//!
//! Regenerate the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test master_demo regenerate
//! ```

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, CreateTarget, DeviceKey,
    Document, Note, RackDoc, SongCommand, WorkflowCommand, load_project, new_project_doc,
    save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BAR: i64 = 4 * PPQ;
const BARS: i64 = 4;
const SYNTH: &str = "atomik.poly-synth";
const REVERB: &str = "atomik.reverb";

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/master-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("master-demo.atomik")
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn apply(doc: &mut Document, c: Command) {
    let registry = default_registry();
    doc.apply(c.clone(), &registry)
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn set(doc: &mut Document, device: &DeviceKey, param: &str, value: f32) {
    apply(
        doc,
        Command::SetParam {
            key: device.clone(),
            param: param.into(),
            value,
            merge: None,
        },
    );
}

fn clip(notes: Vec<(i64, i64, u8)>, velocity: f32) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: BARS * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .into_iter()
                .map(|(tick, length, k)| Note::new(tick, length, k, velocity))
                .collect(),
        ),
    }
}

/// The demo project.
pub fn demo_doc() -> RackDoc {
    let mut d = Document::new(new_project_doc());
    for (k, name, wave, cutoff, level) in [
        ("lead", "Lead", 1.0, 3000.0, -10.0),
        ("bass", "Bass", 0.0, 700.0, -8.0),
    ] {
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key(k)),
                options: CreateOptions::default(),
            }),
        );
        apply(
            &mut d,
            Command::Rename {
                key: key(k),
                name: name.into(),
            },
        );
        for (p, v) in [
            ("osc1.wave", wave),
            ("osc.mix", 0.0),
            ("voices", 4.0),
            ("filter.cutoff", cutoff),
            ("env.amp.attack", 8.0),
            ("env.amp.decay", 250.0),
            ("env.amp.sustain", 0.7),
            ("env.amp.release", 220.0),
            ("master.level", level),
        ] {
            set(&mut d, &key(k), p, v);
        }
        // The chain armed the new track: the file opens disarmed.
        apply(
            &mut d,
            Command::Song(SongCommand::SetTrackFlags {
                device: key(k),
                muted: false,
                solo: false,
                armed: false,
            }),
        );
    }
    // The reverb: created with the master selected, so it takes send and return 1.
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateEffect {
            source: CreateSource::Type(REVERB.into()),
            target: CreateTarget::Master,
            key: Some(key("hall")),
            options: CreateOptions::default(),
        }),
    );
    apply(
        &mut d,
        Command::Rename {
            key: key("hall"),
            name: "Hall".into(),
        },
    );
    set(&mut d, &key("hall"), "mix", 1.0);
    let master = d.doc().console.master.clone().expect("the starter master");
    set(&mut d, &master, "send1.level", -4.0);
    let lead_channel = d
        .doc()
        .console
        .channel_of(&key("lead"))
        .expect("Lead's channel")
        .channel
        .clone();
    set(&mut d, &lead_channel, "send1.on", 1.0);
    set(&mut d, &lead_channel, "send1.level", -6.0);
    // The song: Am F C G, a held chord on Lead and the roots on Bass.
    let progression = [
        (57u8, [69u8, 72, 76]),
        (53, [65, 69, 72]),
        (48, [64, 67, 72]),
        (55, [62, 67, 71]),
    ];
    let lead: Vec<(i64, i64, u8)> = progression
        .iter()
        .enumerate()
        .flat_map(|(bar, (_, chord))| {
            chord
                .iter()
                .map(move |k| (bar as i64 * BAR, BAR - PPQ / 2, *k))
        })
        .collect();
    let bass: Vec<(i64, i64, u8)> = progression
        .iter()
        .enumerate()
        .flat_map(|(bar, (root, _))| {
            (0..4).map(move |beat| (bar as i64 * BAR + beat * PPQ, PPQ - PPQ / 4, root - 12))
        })
        .collect();
    for (k, notes, velocity) in [("lead", lead, 0.7), ("bass", bass, 0.85)] {
        apply(
            &mut d,
            Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: clip(notes, velocity),
            }),
        );
    }
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(BARS * BAR)));
    apply(
        &mut d,
        Command::Song(SongCommand::SetLoop {
            range: (0, BARS * BAR),
            enabled: false,
        }),
    );
    d.doc().clone()
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

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> atomik_render::Rendered {
    render_doc_in(doc, &default_registry(), &options(block, workers), None).unwrap()
}

fn bits(r: &atomik_render::Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn peak(r: &atomik_render::Rendered, channel: usize) -> f32 {
    r.channels[channel].iter().fold(0.0, |m, x| m.max(x.abs()))
}

#[test]
fn the_committed_project_is_the_generated_one_and_canonical() {
    let (registry, doc) = registry_and_doc();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    assert_eq!(text, save_project(&demo_doc()), "regenerate the demo");
    assert_eq!(save_project(&doc), text);
    assert_eq!(load_project(&save_project(&doc), &registry).unwrap(), doc);
    assert!(
        text.contains("\"version\": 18"),
        "saved in the current format"
    );
}

#[test]
fn the_demo_is_the_starter_with_two_instruments_a_send_effect_and_a_master_send_level() {
    let (_, doc) = registry_and_doc();
    let keys: Vec<&str> = doc.devices.iter().map(|d| d.key.0.as_str()).collect();
    assert_eq!(
        keys.first(),
        Some(&"master"),
        "the master is the first device"
    );
    for k in ["lead", "bass", "hall"] {
        assert!(keys.contains(&k), "{k}");
    }
    assert_eq!(doc.console.master, Some(key("master")));
    assert_eq!(doc.console.channels.len(), 2);
    assert_eq!(
        doc.console.returns[0],
        Some(key("hall")),
        "send and return 1"
    );
    assert!(doc.console.returns[1..].iter().all(Option::is_none));
    // The two starter cables are still there and the effect is cabled in both directions.
    let cable = |from: &str, to: &str| {
        doc.cables.iter().any(|c| {
            format!("{}:{}", c.from.device, c.from.socket) == from
                && format!("{}:{}", c.to.device, c.to.socket) == to
        })
    };
    assert!(cable("master:main_l", "interface:out_1"));
    assert!(cable("master:main_r", "interface:out_2"));
    assert!(cable("master:fx_send1_l", "hall:in_l"));
    assert!(cable("hall:out_l", "master:return1_l"));
    // The channels were created folded; the master was not.
    assert!(doc.device(&key("master")).is_some_and(|d| !d.folded));
    for ch in &doc.console.channels {
        assert!(
            doc.device(&ch.channel).is_some_and(|d| d.folded),
            "{}",
            ch.channel
        );
    }
    // The file opens quietly: no armed track.
    assert!(doc.song.tracks.iter().all(|t| !t.armed));
}

#[test]
fn the_demo_sounds_on_outputs_one_and_two_and_the_send_effect_is_heard() {
    let (_, doc) = registry_and_doc();
    let wet = bounce(&doc, 512, 0);
    assert!(
        peak(&wet, 0) > 0.02 && peak(&wet, 1) > 0.02,
        "both outputs sound"
    );
    // Without Lead's send the reverb gets nothing and the mix is another one.
    let mut dry = Document::new(doc.clone());
    let lead = doc
        .console
        .channel_of(&key("lead"))
        .unwrap()
        .channel
        .clone();
    set(&mut dry, &lead, "send1.on", 0.0);
    assert!(
        bits(&bounce(dry.doc(), 512, 0)) != bits(&wet),
        "the send effect is heard"
    );
    // The master's send level scales what reaches the effect (-60 dB is off).
    let mut off = Document::new(doc.clone());
    let master = doc.console.master.clone().unwrap();
    set(&mut off, &master, "send1.level", -60.0);
    assert!(
        bits(&bounce(off.doc(), 512, 0)) != bits(&wet),
        "the master send level works"
    );
}

#[test]
fn the_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let (_, doc) = registry_and_doc();
    let reference = bits(&bounce(&doc, 512, 0));
    for (block, workers) in [(64, 0), (480, 2), (512, 4)] {
        assert!(
            bits(&bounce(&doc, block, workers)) == reference,
            "{block} {workers}"
        );
    }
}

#[test]
fn the_example_bounces_from_its_file_and_matches_the_document_bounce() {
    let (registry, doc) = registry_and_doc();
    let from_file = render_project_file(&demo_file(), &registry, &options(512, 0)).unwrap();
    assert!(bits(&from_file) == bits(&bounce(&doc, 512, 0)));
    assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
}
