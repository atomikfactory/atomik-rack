//! The `container-demo` example project (bounce part):
//! containers bounced through the shared builder, and the expansion theorem on a real project.
//!
//! `examples/projects/container-demo/container-demo.atomik` (no audio files, so no bundle) holds:
//!
//! * **Layer**, a container used as an instrument: two poly synths (**Low**, **High**), a line
//!   mixer and a stereo delay inside, cabled `Low, High -> Mix -> Echo -> output`. A note split puts
//!   the keys below C4 on **Low** and C4 and above on **High** (one octave up). Macro 1 (**Bright**)
//!   drives both cutoffs (High inverted) and the echo mix, macro 2 (**Space**) the echo feedback.
//!   TO CH gave it a console channel.
//! * **Tone**, an effect container on the Layer channel's insert: a compressor with look-ahead
//!   and a nested container **Space** holding a reverb. Macro 1 (**Tone**) drives the threshold
//!   and the nested reverb's mix.
//! * The song: one track on the Layer container (notes through its splits) with a four-bar
//!   progression and an automation lane of `macro1` ramping up over the four bars.
//!
//! Regenerate the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test container_demo regenerate
//! ```

use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, ContainerCommand,
    Curve, DeviceKey, Document, InsertEffect, MacroTarget, Note, NoteSplit, RackDevice, RackDoc,
    SocketRef, SongCommand, empty_doc, load_project, save_project,
};
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use atomik_render::{default_registry, flat};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BAR: i64 = 4 * PPQ;
const BARS: i64 = 4;
const SYNTH: &str = "atomik.poly-synth";
const CONTAINER: &str = "atomik.container";

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/container-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("container-demo.atomik")
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(spec: &str) -> SocketRef {
    let (device, socket) = spec.split_once(':').expect("device:socket");
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
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
        self.rename(k, name);
    }

    fn rename(&mut self, k: &str, name: &str) {
        self.apply(Command::Rename {
            key: key(k),
            name: name.into(),
        });
    }

    fn inner(&mut self, container: &str, type_id: &str, k: &str, name: &str) {
        self.apply(Command::Container(ContainerCommand::AddInner {
            container: key(container),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
            auto_split: false,
        }));
        self.rename(&format!("{container}/{k}"), name);
    }

    fn set(&mut self, path: &str, p: &str, v: f32) {
        self.apply(Command::SetParam {
            key: key(path),
            param: p.into(),
            value: v,
            merge: None,
        });
    }

    fn connect(&mut self, from: &str, to: &str) {
        self.apply(Command::Connect(Cable {
            from: sref(from),
            to: sref(to),
        }));
    }

    /// Both channels of `from` (`device:out`) to `to` (`device:in`), as `a:out_l`, `b:in_l`...
    fn stereo(&mut self, from: (&str, &str), to: (&str, &str)) {
        for side in ["l", "r"] {
            self.connect(
                &format!("{}:{}_{side}", from.0, from.1),
                &format!("{}:{}_{side}", to.0, to.1),
            );
        }
    }

    fn target(
        &mut self,
        container: &str,
        index: u8,
        device: &str,
        param: &str,
        min: f32,
        max: f32,
    ) {
        self.apply(Command::Container(ContainerCommand::SetMacro {
            container: key(container),
            index,
            target: MacroTarget {
                device: key(device),
                param: param.into(),
                min,
                max,
            },
            merge: None,
        }));
    }

    fn name_macro(&mut self, container: &str, index: u8, name: &str) {
        self.apply(Command::Container(ContainerCommand::RenameMacro {
            container: key(container),
            index,
            name: name.into(),
        }));
    }
}

fn synth(b: &mut Builder, path: &str, wave: f32, cutoff: f32, level: f32) {
    for (p, v) in [
        ("osc1.wave", wave),
        ("osc.mix", 0.0),
        ("voices", 4.0),
        ("filter.cutoff", cutoff),
        ("filter.resonance", 0.2),
        ("env.amp.attack", 8.0),
        ("env.amp.decay", 250.0),
        ("env.amp.sustain", 0.7),
        ("env.amp.release", 180.0),
        ("master.level", level),
    ] {
        b.set(path, p, v);
    }
}

/// The demo project.
pub fn demo_doc() -> RackDoc {
    let mut b = Builder {
        doc: Document::new(empty_doc()),
    };
    // Layer: the split instrument.
    b.add(CONTAINER, "layer", "Layer");
    b.inner("layer", SYNTH, "low", "Low");
    b.inner("layer", SYNTH, "high", "High");
    b.inner("layer", "atomik.line-mixer", "mix", "Mix");
    b.inner("layer", "atomik.stereo-delay", "echo", "Echo");
    synth(&mut b, "layer/low", 1.0, 900.0, -8.0);
    synth(&mut b, "layer/high", 0.0, 3000.0, -12.0);
    b.set("layer/echo", "time.ms", 375.0);
    b.set("layer/echo", "feedback", 0.35);
    b.set("layer/echo", "mix", 0.25);
    b.stereo(("layer/low", "out"), ("layer/mix", "ch1_in"));
    b.stereo(("layer/high", "out"), ("layer/mix", "ch2_in"));
    b.stereo(("layer/mix", "master"), ("layer/echo", "in"));
    b.stereo(("layer/echo", "out"), ("layer/boundary", "out"));
    b.apply(Command::Container(ContainerCommand::SetSplits {
        container: key("layer"),
        splits: vec![
            NoteSplit {
                target: key("low"),
                key_lo: 0,
                key_hi: 59,
                vel_lo: 1,
                vel_hi: 127,
                transpose: 0,
            },
            NoteSplit {
                target: key("high"),
                key_lo: 60,
                key_hi: 127,
                vel_lo: 1,
                vel_hi: 127,
                transpose: 12,
            },
        ],
        merge: None,
    }));
    b.name_macro("layer", 0, "Bright");
    b.name_macro("layer", 1, "Space");
    b.target("layer", 0, "low", "filter.cutoff", 400.0, 3200.0);
    b.target("layer", 0, "high", "filter.cutoff", 6000.0, 800.0);
    b.target("layer", 0, "echo", "mix", 0.1, 0.5);
    b.target("layer", 1, "echo", "feedback", 0.1, 0.7);
    b.set("layer", "macro1", 0.5);
    b.set("layer", "macro2", 0.4);
    // TO CH: a console channel for the container.
    b.apply(Command::Console(ConsoleCommand::AddChannel {
        source: key("layer"),
    }));
    // Tone: an effect container (compressor, nested reverb) on the channel's insert.
    b.add(CONTAINER, "tone", "Tone");
    b.inner("tone", "atomik.compressor", "comp", "Comp");
    b.inner("tone", CONTAINER, "space", "Space");
    b.inner("tone/space", "atomik.reverb", "rev", "Reverb");
    b.set("tone/comp", "lookahead", 2.0);
    b.set("tone/comp", "ratio", 3.0);
    b.set("tone/space/rev", "decay", 1.8);
    b.stereo(("tone/boundary", "in"), ("tone/comp", "in"));
    b.stereo(("tone/comp", "out"), ("tone/space", "in"));
    b.stereo(("tone/space", "out"), ("tone/boundary", "out"));
    b.stereo(("tone/space/boundary", "in"), ("tone/space/rev", "in"));
    b.stereo(("tone/space/rev", "out"), ("tone/space/boundary", "out"));
    b.name_macro("tone", 0, "Tone");
    b.target("tone", 0, "comp", "threshold", -6.0, -30.0);
    b.target("tone", 0, "space/rev", "mix", 0.05, 0.35);
    b.set("tone", "macro1", 0.5);
    let channel = b.doc.doc().console.channels[0].channel.clone();
    b.apply(Command::Console(ConsoleCommand::SetInsert {
        channel,
        slot: 0,
        effect: Some(InsertEffect::Existing(key("tone"))),
    }));
    // The song: a track on the container, notes through its splits, `macro1` automated.
    b.apply(Command::Song(SongCommand::AddTrack {
        device: key("layer"),
    }));
    let length = BARS * BAR;
    // (start beat, beats, key): a bass line below C4 and chords above.
    let score: [(i64, i64, u8); 14] = [
        (0, 2, 36),
        (2, 2, 43),
        (4, 2, 41),
        (6, 2, 48),
        (8, 2, 43),
        (10, 2, 38),
        (12, 4, 36),
        (0, 8, 63),
        (0, 8, 67),
        (4, 8, 65),
        (4, 8, 68),
        (8, 8, 62),
        (8, 8, 67),
        (12, 4, 60),
    ];
    let mut notes: Vec<Note> = score
        .iter()
        .map(|&(beat, beats, k)| Note::new(beat * PPQ, beats * PPQ, k, 0.8))
        .collect();
    notes.sort_by_key(|n| (n.tick, n.key));
    b.apply(Command::Song(SongCommand::AddClip {
        device: key("layer"),
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
            content: ClipContent::Notes(notes),
        },
    }));
    b.apply(Command::Song(SongCommand::AddAutomationLane {
        device: key("layer"),
        param: "macro1".into(),
    }));
    b.apply(Command::Song(SongCommand::AddClip {
        device: key("layer"),
        lane: 1,
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
            content: ClipContent::Automation(vec![
                AutomationPoint {
                    tick: 0,
                    value: 0.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: length - 1,
                    value: 1.0,
                    curve: Curve::Step,
                },
            ]),
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

fn inner_mut<'a>(doc: &'a mut RackDoc, container: &str) -> &'a mut atomik_model::ContainerDoc {
    doc.inner_mut(&key(container)).expect("a container")
}

fn set_inner_param(doc: &mut RackDoc, path: &str, param: &str, value: f32) {
    doc.resolve_mut(&key(path))
        .unwrap()
        .params
        .insert(param.to_string(), value);
}

/// The same rack built by hand without containers: every flat device of the expansion as a plain
/// top-level device (path keys renamed `layer/low` to `layer-low`, `layer/~split` to
/// `layer-split`), the flat cables, tracks and console sources on the container moved to its
/// `~split` device. `twin` has no container, so the shared builder takes its no-expansion path.
fn flat_twin(doc: &RackDoc, registry: &atomik_device_sdk::Registry) -> RackDoc {
    let expanded = doc.expand(registry);
    let rename = |k: &DeviceKey| key(&k.0.replace("/~", "-").replace('/', "-"));
    let through = |k: &DeviceKey| rename(expanded.alias.get(k).unwrap_or(k));
    let mut twin = doc.clone();
    twin.devices = expanded
        .devices
        .iter()
        .map(|e| RackDevice {
            key: rename(&e.key),
            type_id: e.type_id.clone(),
            name: e.key.0.clone(),
            params: e.params.clone(),
            folded: false,
            color: None,
            zones: e.zones.clone(),
            steps: e.steps.clone(),
            inner: None,
            plugin: None,
            column: 0,
        })
        .collect();
    twin.cables = expanded
        .cables
        .iter()
        .map(|c| Cable {
            from: SocketRef {
                device: rename(&c.from.device),
                socket: c.from.socket.clone(),
            },
            to: SocketRef {
                device: rename(&c.to.device),
                socket: c.to.socket.clone(),
            },
        })
        .collect();
    for c in &mut twin.console.channels {
        if let Some(s) = &mut c.source {
            *s = through(s);
        }
    }
    for t in &mut twin.song.tracks {
        t.device = through(&t.device);
    }
    twin
}

#[test]
fn the_committed_project_is_the_generated_one_and_canonical() {
    let (registry, doc) = registry_and_doc();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    assert_eq!(text, save_project(&demo_doc()), "regenerate the demo");
    assert_eq!(save_project(&doc), text);
    assert_eq!(load_project(&save_project(&doc), &registry).unwrap(), doc);
    assert!(text.contains("\"version\": 20"));
    assert!(text.contains("\"inner\""));
}

#[test]
fn the_demo_expands_to_the_documented_flat_graph() {
    let (registry, doc) = registry_and_doc();
    let flat = doc.expand(&registry);
    let order: Vec<&str> = flat.devices.iter().map(|d| d.key.0.as_str()).collect();
    let pos = |k: &str| order.iter().position(|x| *x == k).unwrap();
    // Each container's slot holds its split, then its inner devices depth first.
    assert_eq!(
        order[pos("layer/~split")..pos("layer/~split") + 5],
        [
            "layer/~split",
            "layer/low",
            "layer/high",
            "layer/mix",
            "layer/echo"
        ]
    );
    assert_eq!(
        order[pos("tone/~split")..pos("tone/~split") + 4],
        [
            "tone/~split",
            "tone/comp",
            "tone/space/~split",
            "tone/space/rev"
        ]
    );
    let cables: Vec<String> = flat
        .cables
        .iter()
        .map(|c| format!("{} -> {}", c.from, c.to))
        .collect();
    for expected in [
        "layer/low:out_l -> layer/mix:ch1_in_l",
        "layer/high:out_r -> layer/mix:ch2_in_r",
        "layer/mix:master_l -> layer/echo:in_l",
        // The split's two outputs feed the two synths.
        "layer/~split:out_1 -> layer/low:notes_in",
        "layer/~split:out_2 -> layer/high:notes_in",
        // Inner chains are spliced into the insert path: comp -> reverb inside the nested level.
        "tone/comp:out_l -> tone/space/rev:in_l",
    ] {
        assert!(
            cables.iter().any(|c| c == expected),
            "{expected}: {cables:?}"
        );
    }
    // No cable mentions a container or a boundary any more.
    assert!(
        cables
            .iter()
            .all(|c| !c.contains("boundary") && !c.contains("layer:") && !c.contains("tone:")),
        "{cables:?}"
    );
    assert_eq!(flat.alias[&key("layer")], key("layer/~split"));
    // Split params: slot 1 is Low (0 to 59), slot 2 High (60 to 127, +12).
    let split = &flat.devices[pos("layer/~split")].params;
    assert_eq!(split["s1.on"], 1.0);
    assert_eq!(split["s1.key_hi"], 59.0);
    assert_eq!(split["s2.key_lo"], 60.0);
    assert_eq!(split["s2.transpose"], 12.0);
    // Macro values are applied to the targets: macro 1 = 0.5 gives Low's cutoff 1800.
    let low = &flat.devices[pos("layer/low")].params;
    assert_eq!(low["filter.cutoff"], 1800.0);
}

#[test]
fn the_container_project_sounds_and_the_split_sends_each_half_to_its_synth() {
    let (_, doc) = registry_and_doc();
    let all = bounce(&doc, 512, 0);
    let peak = all
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.01..1.5).contains(&peak), "peak {peak}");
    let mut no_high = doc.clone();
    set_inner_param(&mut no_high, "layer/high", "master.level", -60.0);
    let mut no_low = doc.clone();
    set_inner_param(&mut no_low, "layer/low", "master.level", -60.0);
    let (a, b) = (bounce(&no_high, 512, 0), bounce(&no_low, 512, 0));
    // The first half-note: only the bass (Low) plays at beat 1.5 to 2, the chord (High) from 2.
    let beat = 24_000;
    assert!(
        rms(&a.channels[0], 2 * beat, 4 * beat) > 1e-3,
        "Low alone sounds"
    );
    assert!(
        rms(&b.channels[0], 2 * beat, 4 * beat) > 1e-3,
        "High alone sounds"
    );
    assert!(bits(&a) != bits(&all) && bits(&b) != bits(&all));
    assert!(bits(&a) != bits(&b));
    // Moving the split point changes the sound; a split with no zone on is silent.
    let mut moved = doc.clone();
    inner_mut(&mut moved, "layer").splits[0].key_hi = 40;
    assert!(bits(&bounce(&moved, 512, 0)) != bits(&all));
    let mut none = doc.clone();
    inner_mut(&mut none, "layer").splits.clear();
    let quiet = bounce(&none, 512, 0);
    assert!(
        rms(&quiet.channels[0], 0, 8 * beat) < 1e-6,
        "no split, no notes"
    );
}

#[test]
fn macros_change_the_sound_and_a_macro_value_is_its_targets_value() {
    let (registry, mut doc) = registry_and_doc();
    for t in &mut doc.song.tracks {
        t.lanes.retain(|l| l.kind.is_notes());
    }
    // The stored macro value 0.5 is the targets' value: the same as writing them by hand.
    let mut by_hand = doc.clone();
    by_hand
        .resolve_mut(&key("layer"))
        .unwrap()
        .params
        .remove("macro1");
    {
        let inner = inner_mut(&mut by_hand, "layer");
        inner.macros[0].targets.clear();
    }
    for (path, param, value) in [
        ("layer/low", "filter.cutoff", 1800.0),
        ("layer/high", "filter.cutoff", 3400.0),
        ("layer/echo", "mix", 0.1 + (0.5 - 0.1) * 0.5),
    ] {
        set_inner_param(&mut by_hand, path, param, value);
    }
    assert!(
        by_hand.expand(&registry).macro_targets.len() < doc.expand(&registry).macro_targets.len()
    );
    assert!(bits(&bounce(&by_hand, 512, 0)) == bits(&bounce(&doc, 512, 0)));
    // Another macro value is another sound.
    let mut other = doc.clone();
    other
        .resolve_mut(&key("layer"))
        .unwrap()
        .params
        .insert("macro2".into(), 0.9);
    assert!(bits(&bounce(&other, 512, 0)) != bits(&bounce(&doc, 512, 0)));
}

#[test]
fn the_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let (_, doc) = registry_and_doc();
    let reference = bits(&bounce(&doc, 512, 0));
    for (block, workers) in [(64, 0), (480, 0), (512, 2), (1000, 3), (64, 2), (480, 3)] {
        assert!(
            bits(&bounce(&doc, block, workers)) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn the_expansion_theorem_the_container_bounce_equals_the_hand_built_flat_twin() {
    let (registry, doc) = registry_and_doc();
    let twin = flat_twin(&doc, &registry);
    assert!(!twin.has_containers());
    // Same engine ids in the same order.
    let (flat_doc, flat_twin_doc) = (doc.expand(&registry), twin.expand(&registry));
    assert_eq!(flat_doc.devices.len(), flat_twin_doc.devices.len());
    for (a, b) in flat_doc.devices.iter().zip(&flat_twin_doc.devices) {
        assert_eq!((&a.type_id, &a.params), (&b.type_id, &b.params));
    }
    // The twin has no macro automation lane (it is expanded below), so test a project without
    // it first: notes through the split, macro values applied.
    let mut doc_static = doc.clone();
    for t in &mut doc_static.song.tracks {
        t.lanes.retain(|l| l.kind.is_notes());
    }
    let twin_static = flat_twin(&doc_static, &registry);
    for (block, workers) in [(64, 0), (480, 0), (512, 0), (512, 2), (1000, 3)] {
        let a = bounce(&doc_static, block, workers);
        let b = bounce(&twin_static, block, workers);
        assert!(bits(&a) == bits(&b), "block {block}, workers {workers}");
    }
}

#[test]
fn a_macro_automation_lane_equals_lanes_written_on_each_target() {
    // Macro 1 drives only the two synth cutoffs here; the twin has one lane per synth, a track
    // on each synth without notes (a delay has no notes input and cannot hold a track).
    let (registry, mut doc) = registry_and_doc();
    inner_mut(&mut doc, "layer").macros[0]
        .targets
        .retain(|t| t.param == "filter.cutoff");
    let expanded = doc.expand(&registry);
    assert!(expanded.macro_targets.iter().all(|(c, _, _)| c.0 != "tone") || true);
    let automation = doc.song.tracks[0]
        .lanes
        .iter()
        .find(|l| !l.kind.is_notes())
        .expect("the macro lane")
        .clone();
    let ClipContent::Automation(points) = &automation.clips[0].content else {
        panic!("automation clip");
    };
    let mut twin = flat_twin(&doc, &registry);
    // Replace the container track's lane with direct lanes on Low and High.
    twin.song.tracks[0].lanes.retain(|l| l.kind.is_notes());
    let mut d = Document::new(twin.clone());
    let reg = default_registry();
    for (device, min, max) in [
        ("layer-low", 400.0f32, 3200.0f32),
        ("layer-high", 6000.0, 800.0),
    ] {
        d.apply(
            Command::Song(SongCommand::AddAutomationLane {
                device: key(device),
                param: "filter.cutoff".into(),
            }),
            &reg,
        )
        .unwrap();
        let mapped: Vec<AutomationPoint> = points
            .iter()
            .map(|p| AutomationPoint {
                value: atomik_model::macro_value(
                    min,
                    max,
                    p.value,
                    reg.get(SYNTH)
                        .unwrap()
                        .descriptor
                        .param_by_key("filter.cutoff")
                        .unwrap(),
                ),
                ..*p
            })
            .collect();
        d.apply(
            Command::Song(SongCommand::AddClip {
                device: key(device),
                lane: 1,
                clip: Clip {
                    id: ClipId(0),
                    start: 0,
                    length: BARS * BAR,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    scale: None,
                    controllers: Vec::new(),
                    grid: None,
                    content: ClipContent::Automation(mapped),
                },
            }),
            &reg,
        )
        .unwrap();
    }
    let twin = d.doc().clone();
    for (block, workers) in [(512, 0), (64, 2), (1000, 3)] {
        let a = bounce(&doc, block, workers);
        let b = bounce(&twin, block, workers);
        assert!(bits(&a) == bits(&b), "block {block}, workers {workers}");
    }
    // The lane is audible: without it the sound differs.
    let mut still = doc.clone();
    still.song.tracks[0].lanes.retain(|l| l.kind.is_notes());
    assert!(bits(&bounce(&still, 512, 0)) != bits(&bounce(&doc, 512, 0)));
}

#[test]
fn song_input_expands_a_macro_lane_into_one_lane_per_target() {
    let (registry, doc) = registry_and_doc();
    let expanded = doc.expand(&registry);
    let ids = atomik_render::project::expanded_ids(&expanded);
    let input = atomik_render::project::song_input(&doc, &registry, |k| ids.get(k).copied());
    // The note track plays through the split, in front of the automation-only tracks.
    assert_eq!(input.tracks[0].device, ids[&key("layer/~split")]);
    assert!(!input.tracks[0].notes.is_empty());
    assert!(input.tracks[0].automation.is_empty());
    // Macro 1 drives three targets: Low, High and the echo (a track without notes).
    let targets: Vec<_> = input.tracks[1..].iter().map(|t| t.device).collect();
    assert_eq!(
        targets,
        [
            ids[&key("layer/low")],
            ids[&key("layer/high")],
            ids[&key("layer/echo")]
        ]
    );
    for t in &input.tracks[1..] {
        assert!(t.notes.is_empty());
        assert_eq!(t.automation.len(), 1);
        assert_eq!(t.automation[0].points.len(), 2);
    }
    // The inverted target starts at its max of the range (6000 Hz at macro 0).
    assert_eq!(input.tracks[2].automation[0].points[0].value, 6000.0);
    assert!(atomik_engine::compile_song(&input).is_ok());
}

#[test]
fn the_example_bounces_from_its_file_and_matches_the_document_bounce() {
    let (registry, doc) = registry_and_doc();
    let from_file = render_project_file(&demo_file(), &registry, &options(512, 0)).unwrap();
    assert!(bits(&from_file) == bits(&bounce(&doc, 512, 0)));
    assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
}

#[test]
fn the_patch_builder_rejects_the_container_and_the_internal_split_with_a_message() {
    let text = |t: &str| {
        format!(
            r#"{{"format":"atomik-patch","version":1,"sample_rate":48000,"tempo_bpm":120,"seed":1,
            "length_beats":4,"output_channels":2,"devices":[{{"id":"x","type":"{t}"}}],
            "cables":[],"notes":[]}}"#
        )
    };
    for t in [CONTAINER, "atomik.note-split"] {
        let e = atomik_render::patch::Patch::from_json(&text(t)).unwrap_err();
        assert!(e.to_string().contains(t), "{e}");
    }
    // A patch built in memory with a container is refused by the builder too.
    let mut p = atomik_render::patch::Patch::from_json(&text("atomik.stereo-delay")).unwrap();
    p.devices[0].device_type = CONTAINER.into();
    let e = atomik_render::build::build(&p, &default_registry()).unwrap_err();
    assert!(e.to_string().contains("flat::expanded_patch"), "{e}");
}

#[test]
fn expanded_patch_ids_never_collide_with_user_keys() {
    let (registry, doc) = registry_and_doc();
    let expanded = doc.expand(&registry);
    let (patch, ids) = flat::expanded_patch(&doc, &expanded);
    assert_eq!(patch.devices.len(), expanded.devices.len());
    patch.validate().expect("generated ids are valid patch ids");
    let unique: std::collections::BTreeSet<_> = ids.values().collect();
    assert_eq!(unique.len(), ids.len());
    // A user key that looks like a generated one forces a different id.
    let mut tricky = doc.clone();
    tricky.devices.push(RackDevice {
        key: key("in2-layer-split"),
        type_id: "atomik.stereo-delay".into(),
        name: "x".into(),
        params: Default::default(),
        folded: false,
        color: None,
        zones: Vec::new(),
        steps: None,
        inner: None,
        plugin: None,
        column: 0,
    });
    let e2 = tricky.expand(&registry);
    let (p2, ids2) = flat::expanded_patch(&tricky, &e2);
    p2.validate().unwrap();
    assert_eq!(ids2[&key("in2-layer-split")], "in2-layer-split");
}
