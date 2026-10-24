//! A project with `atomik.character` in every preset saves,
//! loads and saves byte-identically, and the three automatable parameters
//! (`preset`, `transform`, `mix`) bounce identically across host blocks and workers.

use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, ContainerCommand,
    Curve, DeviceKey, Document, MacroTarget, Note, RackDoc, SocketRef, SongCommand, empty_doc,
    load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const BARS: i64 = 4;
const CHARACTER: &str = "atomik.character";
const SYNTH: &str = "atomik.poly-synth";
const CONTAINER: &str = "atomik.container";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(device: &str, socket: &str) -> SocketRef {
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
    }
}

fn apply(doc: &mut Document, c: Command) {
    doc.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn set(doc: &mut Document, device: &str, param: &str, value: f32) {
    apply(
        doc,
        Command::SetParam {
            key: key(device),
            param: param.into(),
            value,
            merge: None,
        },
    );
}

/// 16 Characters, one per preset, with non-default knobs.
fn sixteen_characters() -> RackDoc {
    let mut doc = Document::new(empty_doc());
    for p in 0..16u32 {
        let k = format!("chr{p}");
        apply(
            &mut doc,
            Command::AddDevice {
                index: usize::MAX,
                type_id: CHARACTER.into(),
                key: Some(key(&k)),
            },
        );
        set(&mut doc, &k, "preset", p as f32);
        set(&mut doc, &k, "transform", p as f32 / 15.0);
        set(&mut doc, &k, "mix", 1.0 - p as f32 / 30.0);
        set(&mut doc, &k, "gain", 0.3 + p as f32 / 40.0);
        set(&mut doc, &k, "volume", 0.45);
        set(&mut doc, &k, "transform_cv_trim", 0.25);
        set(&mut doc, &k, "mix_cv_trim", 0.75);
    }
    doc.doc().clone()
}

#[test]
fn a_project_with_every_preset_round_trips_save_load_save_byte_identically() {
    let registry = default_registry();
    let doc = sixteen_characters();
    let first = save_project(&doc);
    let loaded = load_project(&first, &registry).expect("a valid project");
    assert_eq!(loaded, doc, "load returns the saved document");
    assert_eq!(save_project(&loaded), first, "save, load, save is stable");
    // Every preset survived as its own value.
    for p in 0..16u32 {
        let d = loaded.device(&key(&format!("chr{p}"))).expect("device");
        assert_eq!(d.type_id, CHARACTER);
        assert_eq!(d.params["preset"], p as f32, "preset {p}");
    }
    // The preset is a plain numeric param, so no new project field appeared.
    assert!(
        first.contains("\"version\": 23"),
        "saved in the current format"
    );
}

fn point(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

fn clip(content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: BARS * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content,
    }
}

/// A container (synth into Character) whose three macros reach `preset`, `transform` and `mix`,
/// each automated on a song lane, over a held two-note phrase.
fn automated_doc() -> RackDoc {
    let mut doc = Document::new(empty_doc());
    apply(
        &mut doc,
        Command::AddDevice {
            index: usize::MAX,
            type_id: CONTAINER.into(),
            key: Some(key("box")),
        },
    );
    for (k, t) in [("syn", SYNTH), ("chr", CHARACTER)] {
        apply(
            &mut doc,
            Command::Container(ContainerCommand::AddInner {
                container: key("box"),
                index: usize::MAX,
                type_id: t.into(),
                key: Some(key(k)),
                auto_split: k == "syn",
            }),
        );
    }
    set(&mut doc, "box/syn", "osc1.wave", 1.0);
    set(&mut doc, "box/syn", "master.level", -12.0);
    for side in ["l", "r"] {
        for (a, b) in [
            (("box/syn", "out"), ("box/chr", "in")),
            (("box/chr", "out"), ("box/boundary", "out")),
        ] {
            apply(
                &mut doc,
                Command::Connect(Cable {
                    from: sref(a.0, &format!("{}_{side}", a.1)),
                    to: sref(b.0, &format!("{}_{side}", b.1)),
                }),
            );
        }
    }
    for (index, param, max) in [
        (0u8, "preset", 15.0f32),
        (1, "transform", 1.0),
        (2, "mix", 1.0),
    ] {
        apply(
            &mut doc,
            Command::Container(ContainerCommand::SetMacro {
                container: key("box"),
                index,
                target: MacroTarget {
                    device: key("chr"),
                    param: param.into(),
                    min: 0.0,
                    max,
                },
                merge: None,
            }),
        );
    }
    apply(
        &mut doc,
        Command::Console(ConsoleCommand::AddChannel { source: key("box") }),
    );
    apply(
        &mut doc,
        Command::Song(SongCommand::AddTrack { device: key("box") }),
    );
    let notes = (0..BARS)
        .flat_map(|bar| [57u8, 64].map(|k| Note::new(bar * BAR, BAR - PPQ / 4, k, 0.8)))
        .collect();
    apply(
        &mut doc,
        Command::Song(SongCommand::AddClip {
            device: key("box"),
            lane: 0,
            clip: clip(ClipContent::Notes(notes)),
        }),
    );
    let lanes: [(&str, Vec<AutomationPoint>); 3] = [
        (
            "macro1",
            vec![
                point(0, 0.0, Curve::Step),
                point(BAR, 9.0 / 15.0, Curve::Step),
                point(2 * BAR, 8.0 / 15.0, Curve::Step),
                point(3 * BAR, 14.0 / 15.0, Curve::Step),
            ],
        ),
        (
            "macro2",
            vec![
                point(0, 0.1, Curve::Linear),
                point(4 * BAR - 1, 0.9, Curve::Step),
            ],
        ),
        (
            "macro3",
            vec![
                point(0, 1.0, Curve::Linear),
                point(4 * BAR - 1, 0.3, Curve::Step),
            ],
        ),
    ];
    for (i, (param, points)) in lanes.into_iter().enumerate() {
        apply(
            &mut doc,
            Command::Song(SongCommand::AddAutomationLane {
                device: key("box"),
                param: param.into(),
            }),
        );
        apply(
            &mut doc,
            Command::Song(SongCommand::AddClip {
                device: key("box"),
                lane: i + 1,
                clip: clip(ClipContent::Automation(points)),
            }),
        );
    }
    apply(&mut doc, Command::Song(SongCommand::SetSongEnd(BARS * BAR)));
    apply(
        &mut doc,
        Command::Song(SongCommand::SetLoop {
            range: (0, BARS * BAR),
            enabled: false,
        }),
    );
    doc.doc().clone()
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = render_doc_in(doc, &default_registry(), &options, None).expect("renders");
    assert!(
        r.channels
            .iter()
            .flatten()
            .all(|x| x.is_finite() && x.abs() < 8.0),
        "finite and bounded"
    );
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn preset_transform_and_mix_automation_bounce_identically_across_blocks_and_workers() {
    let doc = automated_doc();
    let reference = bounce(&doc, 512, 0);
    assert!(
        reference.iter().any(|b| f32::from_bits(*b).abs() > 0.01),
        "the project sounds"
    );
    for block in [64, 333, 480, 512] {
        for workers in [0, 2, 4] {
            assert!(
                bounce(&doc, block, workers) == reference,
                "block {block}, workers {workers}"
            );
        }
    }
}

#[test]
fn each_automated_parameter_is_heard_in_the_bounce() {
    let doc = automated_doc();
    let with = bounce(&doc, 512, 0);
    for lane in 1..=3usize {
        let mut cleared = doc.clone();
        let track = cleared
            .song
            .tracks
            .iter_mut()
            .find(|t| t.device == key("box"))
            .unwrap();
        track.lanes[lane].clips.clear();
        assert!(bounce(&cleared, 512, 0) != with, "lane {lane} is heard");
    }
}

#[test]
fn the_bounce_is_stable_across_a_save_and_load() {
    let registry = default_registry();
    let doc = automated_doc();
    let text = save_project(&doc);
    let loaded = load_project(&text, &registry).expect("valid");
    assert_eq!(save_project(&loaded), text);
    assert!(bounce(&loaded, 480, 2) == bounce(&doc, 64, 0));
}
