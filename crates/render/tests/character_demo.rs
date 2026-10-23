//! The `character-demo` example project:
//! the factory effect `atomik.character` on a synth phrase, a bass line and a pad.
//!
//! `examples/projects/character-demo/character-demo.atomik` (no audio files, so no bundle) is
//! built by model commands only, over [`atomik_model::empty_doc`] at 120 BPM, eight bars of Am F C
//! G twice:
//!
//! * **Lead**, a container holding a poly synth (`syn`) and a Character (`chr`) cabled in series,
//!   with a console channel and a song track. Its `macro1` (Preset) maps onto the Character's
//!   `preset` and `macro2` (Transform) onto `transform`, and the track automates both: the preset
//!   steps every two bars through `reel`, `swirl`, `dusty-groove` and `pocket-radio` (all short
//!   latency class) while Transform ramps up and back down. This is the only way a song lane can
//!   reach an effect's parameter.
//! * **Bass**, a poly synth played by a step sequencer, with a Character (`ion-sweep`) on its
//!   console insert. The sequencer's `pitch_cv_out` is cabled into the Character's
//!   `transform_cv_in`, so every note position sweeps the comb and the filter.
//! * **Pad**, a poly synth with long chords and a Character on `lossy` (the spectral, **long
//!   latency class**) on its insert: the rest of the mix is delayed by the engine's delay
//!   compensation to stay aligned with it.
//!
//! Regenerate the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test character_demo regenerate
//! ```

use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, ContainerCommand,
    Curve, DeviceKey, Document, MacroTarget, Note, PatternDoc, RackDoc, SeqResolution, SocketRef,
    SongCommand, StepDoc, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BAR: i64 = 4 * PPQ;
const BARS: i64 = 8;
const SYNTH: &str = "atomik.poly-synth";
const CHARACTER: &str = "atomik.character";
const CONTAINER: &str = "atomik.container";
/// Frames of one bar at 120 BPM and 48 kHz.
const BAR_FRAMES: usize = 96_000;

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/character-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("character-demo.atomik")
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(device: &str, socket: &str) -> SocketRef {
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
        self.apply(Command::Rename {
            key: key(k),
            name: name.into(),
        });
    }

    fn inner(&mut self, container: &str, type_id: &str, k: &str, name: &str, auto_split: bool) {
        self.apply(Command::Container(ContainerCommand::AddInner {
            container: key(container),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
            auto_split,
        }));
        self.apply(Command::Rename {
            key: key(&format!("{container}/{k}")),
            name: name.into(),
        });
    }

    fn set(&mut self, device: &str, param: &str, value: f32) {
        self.apply(Command::SetParam {
            key: key(device),
            param: param.into(),
            value,
            merge: None,
        });
    }

    fn set_all(&mut self, device: &str, params: &[(&str, f32)]) {
        for (p, v) in params {
            self.set(device, p, *v);
        }
    }

    fn connect(&mut self, from: (&str, &str), to: (&str, &str)) {
        self.apply(Command::Connect(Cable {
            from: sref(from.0, from.1),
            to: sref(to.0, to.1),
        }));
    }

    fn stereo(&mut self, from: (&str, &str), to: (&str, &str)) {
        for side in ["l", "r"] {
            self.connect(
                (from.0, &format!("{}_{side}", from.1)),
                (to.0, &format!("{}_{side}", to.1)),
            );
        }
    }

    fn channel(&self, source: &str) -> String {
        self.doc
            .doc()
            .console
            .channel_of(&key(source))
            .expect("the source has a channel")
            .channel
            .0
            .clone()
    }

    /// Cables the insert of `host` (a channel) through `fx`, both sides.
    fn insert(&mut self, host: &str, fx: &str) {
        for side in ["l", "r"] {
            self.connect(
                (host, &format!("insert_send_{side}")),
                (fx, &format!("in_{side}")),
            );
            self.connect(
                (fx, &format!("out_{side}")),
                (host, &format!("insert_return_{side}")),
            );
        }
    }

    /// A track with one note clip per `(start, length, notes)`; note ticks are relative to the clip.
    fn track(&mut self, device: &str, clips: Vec<(i64, i64, Vec<Note>)>) {
        self.apply(Command::Song(SongCommand::AddTrack {
            device: key(device),
        }));
        for (start, length, notes) in clips {
            self.apply(Command::Song(SongCommand::AddClip {
                device: key(device),
                lane: 0,
                clip: Clip {
                    start,
                    length,
                    ..clip(ClipContent::Notes(notes))
                },
            }));
        }
    }

    fn lane(&mut self, device: &str, param: &str, lane: usize, points: Vec<AutomationPoint>) {
        self.apply(Command::Song(SongCommand::AddAutomationLane {
            device: key(device),
            param: param.into(),
        }));
        self.apply(Command::Song(SongCommand::AddClip {
            device: key(device),
            lane,
            clip: clip(ClipContent::Automation(points)),
        }));
    }
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
        grid: None,
        content,
    }
}

fn note(tick: i64, length: i64, k: u8, velocity: f32) -> Note {
    Note::new(tick, length, k, velocity)
}

/// The chord tones per bar (Am F C G, twice).
const CHORDS: [[u8; 3]; 4] = [[57, 60, 64], [53, 57, 60], [55, 60, 64], [55, 59, 62]];

/// The presets the Lead steps through, one every two bars: `reel`, `swirl`, `dusty-groove`,
/// `pocket-radio`.
pub const LEAD_PRESETS: [u8; 4] = [0, 9, 7, 5];

/// The preset of the pad insert: `lossy`, the long latency class.
pub const PAD_PRESET: u8 = 8;
/// The preset of the bass insert: `ion-sweep`.
pub const BASS_PRESET: u8 = 14;

/// The bass line of the step sequencer (16 sixteenths, a little swing).
fn bass_pattern() -> PatternDoc {
    let mut p = PatternDoc::new("Bass Line");
    p.length = 16;
    p.resolution = SeqResolution::Sixteenth;
    p.swing = 10;
    for (step, k, velocity, gate) in [
        (0usize, 45u8, 110u8, 80u8),
        (3, 45, 90, 50),
        (6, 48, 100, 80),
        (8, 50, 100, 80),
        (10, 52, 95, 60),
        (12, 45, 105, 80),
        (14, 43, 90, 60),
    ] {
        p.steps[step] = StepDoc {
            on: true,
            key: k,
            velocity,
            gate,
            prob: 100,
            tie: false,
        };
    }
    p
}

fn point(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

/// The demo project.
pub fn demo_doc() -> RackDoc {
    let mut b = Builder {
        doc: Document::new(empty_doc()),
    };

    // Lead: a container with a poly synth and a Character, macros on the preset and Transform.
    b.add(CONTAINER, "lead", "Lead");
    b.inner("lead", SYNTH, "syn", "Synth", true);
    b.inner("lead", CHARACTER, "chr", "Character", false);
    b.set_all(
        "lead/syn",
        &[
            ("osc1.wave", 1.0),
            ("osc.mix", 0.0),
            ("voices", 4.0),
            ("filter.cutoff", 3500.0),
            ("filter.env_amount", 0.2),
            ("env.amp.attack", 4.0),
            ("env.amp.decay", 250.0),
            ("env.amp.sustain", 0.6),
            ("env.amp.release", 200.0),
            ("master.level", -14.0),
        ],
    );
    b.stereo(("lead/syn", "out"), ("lead/chr", "in"));
    b.stereo(("lead/chr", "out"), ("lead/boundary", "out"));
    for (index, name, param, max) in [
        (0u8, "Preset", "preset", 15.0f32),
        (1, "Transform", "transform", 1.0),
    ] {
        b.apply(Command::Container(ContainerCommand::RenameMacro {
            container: key("lead"),
            index,
            name: name.into(),
        }));
        b.apply(Command::Container(ContainerCommand::SetMacro {
            container: key("lead"),
            index,
            target: MacroTarget {
                device: key("chr"),
                param: param.into(),
                min: 0.0,
                max,
            },
            merge: None,
        }));
    }
    b.apply(Command::Console(ConsoleCommand::AddChannel {
        source: key("lead"),
    }));

    // Bass: a sequencer line, a Character on ion-sweep on the insert, note CV on Transform.
    b.add("atomik.step-sequencer", "seq", "Bass Line");
    b.apply(Command::SetPattern {
        device: key("seq"),
        index: 0,
        pattern: bass_pattern(),
        merge: None,
    });
    b.add(SYNTH, "bass", "Bass");
    b.set_all(
        "bass",
        &[
            ("osc1.wave", 0.0),
            ("osc.mix", 0.0),
            ("voices", 1.0),
            ("filter.cutoff", 900.0),
            ("filter.resonance", 0.15),
            ("env.amp.attack", 3.0),
            ("env.amp.decay", 220.0),
            ("env.amp.sustain", 0.8),
            ("env.amp.release", 90.0),
            ("master.level", -8.0),
        ],
    );
    b.connect(("seq", "notes_out"), ("bass", "notes_in"));
    b.add(CHARACTER, "bass-fx", "Bass Character");
    b.set_all(
        "bass-fx",
        &[
            ("preset", f32::from(BASS_PRESET)),
            ("transform", 0.75),
            ("transform_cv_trim", 0.5),
            ("mix", 0.8),
        ],
    );
    b.connect(("seq", "pitch_cv_out"), ("bass-fx", "transform_cv_in"));
    let bass_ch = b.channel("bass");
    b.insert(&bass_ch, "bass-fx");

    // Pad: long chords with a Character on lossy (the long latency class) on the insert.
    b.add(SYNTH, "pad", "Pad");
    b.set_all(
        "pad",
        &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 2.0),
            ("osc.mix", 0.5),
            ("voices", 6.0),
            ("filter.cutoff", 1800.0),
            ("filter.env_amount", 0.0),
            ("env.amp.attack", 450.0),
            ("env.amp.decay", 600.0),
            ("env.amp.sustain", 0.8),
            ("env.amp.release", 700.0),
            ("master.level", -15.0),
        ],
    );
    b.add(CHARACTER, "pad-fx", "Pad Character");
    b.set_all(
        "pad-fx",
        &[
            ("preset", f32::from(PAD_PRESET)),
            ("transform", 0.45),
            ("mix", 0.7),
        ],
    );
    let pad_ch = b.channel("pad");
    b.insert(&pad_ch, "pad-fx");

    // The song: an arpeggio on the Lead (a four-bar phrase in two clips, so no clip holds a long
    // run of note rows), held chords on the Pad.
    let mut phrase = Vec::new();
    let mut pad = Vec::new();
    for bar in 0..BARS {
        let chord = CHORDS[(bar % 4) as usize];
        for step in 0..8i64 {
            let k = chord[[0usize, 1, 2, 1, 2, 1, 2, 0][step as usize]] + 12;
            if bar < 4 {
                phrase.push(note(bar * BAR + step * PPQ / 2, PPQ / 2 - PPQ / 8, k, 0.75));
            }
        }
        for k in chord {
            pad.push(note(bar * BAR, BAR, k, 0.6));
        }
    }
    b.track(
        "lead",
        vec![(0, 4 * BAR, phrase.clone()), (4 * BAR, 4 * BAR, phrase)],
    );
    b.track("pad", vec![(0, BARS * BAR, pad)]);

    // Lead automation: the preset steps every two bars, Transform ramps up and down.
    let steps = LEAD_PRESETS
        .iter()
        .enumerate()
        .map(|(i, p)| point(i as i64 * 2 * BAR, f32::from(*p) / 15.0, Curve::Step))
        .collect();
    b.lane("lead", "macro1", 1, steps);
    b.lane(
        "lead",
        "macro2",
        2,
        vec![
            point(0, 0.2, Curve::Linear),
            point(4 * BAR, 0.9, Curve::Linear),
            point(BARS * BAR - 1, 0.4, Curve::Step),
        ],
    );
    b.apply(Command::Song(SongCommand::SetSongEnd(BARS * BAR)));
    b.apply(Command::Song(SongCommand::SetLoop {
        range: (0, BARS * BAR),
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

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> atomik_render::Rendered {
    render_doc_in(doc, &default_registry(), &options(block, workers), None).unwrap()
}

fn bits(r: &atomik_render::Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn peak(r: &atomik_render::Rendered, channel: usize) -> f32 {
    r.channels[channel].iter().fold(0.0, |m, x| m.max(x.abs()))
}

fn set_param(doc: &mut RackDoc, device: &str, param: &str, value: f32) {
    let d = doc
        .devices
        .iter_mut()
        .find(|d| d.key == key(device))
        .unwrap_or_else(|| panic!("{device}"));
    d.params.insert(param.to_string(), value);
}

#[test]
fn the_committed_project_is_the_generated_one_and_canonical() {
    let (registry, doc) = registry_and_doc();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    assert_eq!(text, save_project(&demo_doc()), "regenerate the demo");
    assert_eq!(save_project(&doc), text);
    assert_eq!(load_project(&save_project(&doc), &registry).unwrap(), doc);
    assert!(
        text.contains("\"version\": 16"),
        "saved in the current format"
    );
}

#[test]
fn the_demo_has_three_characters_a_preset_lane_a_transform_lane_and_a_cv_cable() {
    let (_, doc) = registry_and_doc();
    // The Lead container holds the synth and a Character; two macros map onto its params.
    let inner = doc.inner(&key("lead")).expect("the Lead container");
    let types: Vec<&str> = inner.devices.iter().map(|d| d.type_id.as_str()).collect();
    assert!(
        types.contains(&CHARACTER) && types.contains(&SYNTH),
        "{types:?}"
    );
    let targets = |i: usize| -> Vec<(String, String)> {
        inner.macros[i]
            .targets
            .iter()
            .map(|t| (t.device.0.clone(), t.param.clone()))
            .collect()
    };
    assert_eq!(targets(0), [("chr".to_string(), "preset".to_string())]);
    assert_eq!(targets(1), [("chr".to_string(), "transform".to_string())]);
    // Both lanes exist and the preset lane visits four distinct presets, none of them lossy.
    let track = doc.song.track(&key("lead")).expect("the Lead track");
    assert_eq!(track.lanes.len(), 3, "notes, macro1, macro2");
    let atomik_model::ClipContent::Automation(points) = &track.lanes[1].clips[0].content else {
        panic!("macro1 is an automation lane");
    };
    let presets: Vec<u8> = points
        .iter()
        .map(|p| (p.value * 15.0).round() as u8)
        .collect();
    assert_eq!(presets, LEAD_PRESETS);
    assert!(!presets.contains(&PAD_PRESET), "the Lead stays short class");
    assert!(matches!(
        &track.lanes[2].clips[0].content,
        atomik_model::ClipContent::Automation(p) if p.len() == 3
    ));
    // The bass sweep is a note-CV cable, the pad insert is the long class preset.
    assert!(doc.cables.iter().any(
        |c| c.from == sref("seq", "pitch_cv_out") && c.to == sref("bass-fx", "transform_cv_in")
    ));
    let preset = |k: &str| doc.device(&key(k)).unwrap().params["preset"];
    assert_eq!(preset("bass-fx"), f32::from(BASS_PRESET));
    assert_eq!(preset("pad-fx"), f32::from(PAD_PRESET));
}

#[test]
fn the_demo_sounds_on_both_outputs_and_every_character_is_heard() {
    let (_, doc) = registry_and_doc();
    let wet = bounce(&doc, 512, 0);
    assert!(
        peak(&wet, 0) > 0.02 && peak(&wet, 1) > 0.02,
        "both outputs sound"
    );
    assert!(
        wet.channels
            .iter()
            .flatten()
            .all(|x| x.is_finite() && x.abs() < 4.0),
        "finite and bounded"
    );
    // Dry/Wet at 0 on a Character changes the mix (the pad and bass ones; the Lead's is mapped).
    for k in ["bass-fx", "pad-fx"] {
        let mut dry = doc.clone();
        set_param(&mut dry, k, "mix", 0.0);
        assert!(bits(&bounce(&dry, 512, 0)) != bits(&wet), "{k} is heard");
    }
    // The note CV is heard: without the cable the bass sweep stands still.
    let mut uncabled = doc.clone();
    uncabled
        .cables
        .retain(|c| c.to != sref("bass-fx", "transform_cv_in"));
    assert!(
        bits(&bounce(&uncabled, 512, 0)) != bits(&wet),
        "the note CV moves Transform"
    );
}

#[test]
fn the_preset_and_transform_lanes_change_the_sound_from_their_first_step() {
    let (_, doc) = registry_and_doc();
    let with = bounce(&doc, 512, 0);
    // The same project with the lanes cleared: the Lead's Character stays on its first preset
    // and its mapped Transform at the knob.
    let mut flat = doc.clone();
    let track = flat
        .song
        .tracks
        .iter_mut()
        .find(|t| t.device == key("lead"))
        .unwrap();
    for lane in &mut track.lanes[1..] {
        lane.clips.clear();
    }
    let without = bounce(&flat, 512, 0);
    assert!(
        with.channels[0] != without.channels[0],
        "automation is heard"
    );
    // The Transform lane starts at 0.2 against the knob's 0.5, so the first bar differs; the
    // preset lane's first change (bar 2, a step to `swirl`) must not be needed for that.
    let mut only_transform = doc.clone();
    let track = only_transform
        .song
        .tracks
        .iter_mut()
        .find(|t| t.device == key("lead"))
        .unwrap();
    track.lanes[1].clips.clear();
    let t_only = bounce(&only_transform, 512, 0);
    let change = 2 * BAR_FRAMES;
    assert_eq!(
        &t_only.channels[0][..change - 256],
        &with.channels[0][..change - 256],
        "the preset lane does nothing before its first change"
    );
    assert!(
        t_only.channels[0][change..] != with.channels[0][change..],
        "the preset change at bar 2 is heard"
    );
}

#[test]
fn the_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let (_, doc) = registry_and_doc();
    let reference = bits(&bounce(&doc, 512, 0));
    for block in [64, 333, 480, 512] {
        for workers in [0, 2, 3] {
            assert!(
                bits(&bounce(&doc, block, workers)) == reference,
                "block {block}, workers {workers}"
            );
        }
    }
}

#[test]
fn the_example_bounces_from_its_file_and_matches_the_document_bounce() {
    let (registry, doc) = registry_and_doc();
    let from_file = render_project_file(&demo_file(), &registry, &options(512, 0)).unwrap();
    assert!(bits(&from_file) == bits(&bounce(&doc, 512, 0)));
    assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
}
