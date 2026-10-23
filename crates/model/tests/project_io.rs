//! `.atomik` project files (model part for the console): format,
//! bit-exact round trips, patch 1/v2 compatibility, the v1 → v2 migration, JSON-path
//! errors, and the demo project.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, ClipId, Command, ConsoleChannel, ConsoleCommand, Curve, Document, FX_SLOTS,
    ModelError, RackDoc, SongCommand, empty_doc, empty_song, load_patch, load_project,
    save_project,
};
use common::audio::{
    assert_audio_invariants, audio_op, audio_registry, pre_register, resolve_audio,
};
use common::console_ops::*;
use common::song::*;
use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn repo_file(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_file(rel)).unwrap()
}

fn edited(text: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    edit(&mut v);
    v.to_string()
}

/// A JSON edit applied to a good project.
type Edit = Box<dyn Fn(&mut Value)>;

fn assert_rejects(text: &str, path: &str, needle: &str) {
    let e = load_project(text, &console_registry()).expect_err("project must be rejected");
    let ModelError::Patch(msg) = &e else {
        panic!("expected a patch error, got {e:?}")
    };
    assert!(
        msg.starts_with(&format!("{path}: ")),
        "{msg} (wanted path {path})"
    );
    assert!(msg.contains(needle), "{msg} lacks {needle:?}");
}

/// The demo project: the slice-demo rack, a bass track (8 bars looping a 4-bar pattern, a
/// cutoff automation lane) and a lead track, mixed on the console: a channel per synth, the
/// echo on return 1 (fully wet) fed by both channels' send 1, and no line mixer.
fn demo(reg: &Registry) -> RackDoc {
    let mut d =
        Document::new(load_patch(&read("examples/patches/slice-demo.v2.json"), reg).unwrap());
    let mut apply = |c: Command| {
        d.apply(c, reg).unwrap();
    };
    apply(add_track("bass"));
    apply(add_track("lead"));
    apply(sc(SongCommand::AddAutomationLane {
        device: key("bass"),
        param: "filter.cutoff".into(),
    }));
    // Bass: A minor, F, C, G roots, one bar each, eighth-note pulse with accents.
    let mut bass = Vec::new();
    for (bar, root) in [45u8, 41, 48, 43].into_iter().enumerate() {
        let b = bar as i64 * BAR;
        bass.push(note(b, 3 * PPQ / 4, root, 0.95));
        bass.push(note(b + 3 * PPQ / 2, PPQ / 4, root, 0.7));
        bass.push(note(b + 2 * PPQ, 3 * PPQ / 4, root, 0.9));
        bass.push(note(b + 3 * PPQ, PPQ / 2, root - 5, 0.75));
        bass.push(note(b + 7 * PPQ / 2, PPQ / 4, root, 0.6));
    }
    let mut c = notes_clip(0, 8 * BAR, bass);
    c.loop_length = Some(4 * BAR);
    apply(add_clip("bass", 0, c));
    let mut sweep = auto_clip(
        0,
        8 * BAR,
        vec![
            point(0, 400.0, Curve::Linear),
            point(2 * BAR, 2400.0, Curve::Linear),
            point(4 * BAR - PPQ, 500.0, Curve::Step),
        ],
    );
    sweep.loop_length = Some(4 * BAR);
    apply(add_clip("bass", 1, sweep));
    let melody = [
        (2, 2, 76u8),
        (4, 2, 79),
        (6, 4, 81),
        (10, 2, 79),
        (12, 4, 76),
        (18, 2, 72),
        (20, 2, 74),
        (22, 6, 76),
        (28, 4, 72),
    ];
    let lead: Vec<_> = melody
        .iter()
        .map(|&(at, len, k)| note(at * PPQ / 2, len * PPQ / 2, k, 0.8))
        .collect();
    apply(add_clip("lead", 0, notes_clip(4 * BAR, 4 * BAR, lead)));
    apply(sc(SongCommand::SetLoop {
        range: (0, 8 * BAR),
        enabled: true,
    }));
    apply(sc(SongCommand::SetSongEnd(8 * BAR)));
    // The console replaces the line mixer and the insert chain.
    apply(Command::RemoveDevice { key: key("mixer") });
    for source in ["bass", "lead"] {
        apply(Command::Console(ConsoleCommand::AddChannel {
            source: key(source),
        }));
    }
    apply(Command::Console(ConsoleCommand::SetReturn {
        slot: 0,
        effect: Some(key("echo")),
    }));
    for (k, param) in [
        ("echo", "mix"),
        ("channel", "send1.on"),
        ("channel-2", "send1.on"),
    ] {
        apply(Command::SetParam {
            key: key(k),
            param: param.into(),
            value: 1.0,
            merge: None,
        });
    }
    d.doc().clone()
}

#[test]
fn demo_project_file_is_the_canonical_save_of_the_demo() {
    let reg = console_registry();
    let doc = demo(&reg);
    let text = save_project(&doc);
    let path = repo_file("examples/projects/demo.atomik");
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let file = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(file, text, "regenerate with ATOMIK_UPDATE_GOLDEN=1");
    let loaded = load_project(&file, &reg).unwrap();
    assert_eq!(loaded, doc);
    assert_eq!(loaded.song.tracks.len(), 2);
    assert_eq!(loaded.song.tracks[0].lanes.len(), 2);
    assert_eq!(loaded.console.channels.len(), 2);
    assert_eq!(loaded.console.returns[0], Some(key("echo")));
}

/// The asset id of the audio demo: a placeholder hash. It is replaced (together with the
/// frame count) by the real content hash when the bundle's `Audio/` folder is generated.
const AUDIO_DEMO_HASH: &str = "999788b9c9b84842ee8f9e66920f94954bce19bb62dcc350796323b11cf18341";

/// The demo rack plus a stereo audio track with three clips of one 2-bar asset (4 s at 48 kHz
/// and 120 BPM): the whole asset, the whole asset at -3 dB with fades, and its second half.
fn audio_demo(reg: &Registry) -> RackDoc {
    let mut d = Document::new(demo(reg));
    let mut apply = |c: Command| {
        d.apply(c, reg).unwrap();
    };
    apply(Command::SetInputChannels(2));
    apply(Command::RegisterAsset {
        hash: AUDIO_DEMO_HASH.into(),
        meta: atomik_model::AssetMeta {
            channels: 1,
            sample_rate: 48_000,
            frames: 100_000,
            original_name: "audio-demo.wav".into(),
        },
    });
    apply(sc(SongCommand::AddAudioTrack {
        name: Some("Audio Demo".into()),
    }));
    let clip = |start: i64, length: i64, offset: u64, gain_db: f32, fade: (u64, u64)| {
        sc(SongCommand::AddAudioClip {
            device: key("audio-track"),
            start,
            length,
            asset: AUDIO_DEMO_HASH.into(),
            source_offset: offset,
            gain_db,
            fade_in: fade.0,
            fade_out: fade.1,
            fade_curve: atomik_model::FadeCurve::EqualPower,
        })
    };
    apply(clip(0, 2 * BAR, 0, 0.0, (0, 0)));
    apply(clip(4 * BAR, 2 * BAR, 0, -3.0, (480, 9_600)));
    apply(clip(6 * BAR, BAR, 96_000, 0.0, (240, 2_400)));
    d.doc().clone()
}

#[test]
fn audio_demo_project_file_is_the_canonical_save_of_the_audio_demo() {
    let reg = audio_registry();
    let doc = audio_demo(&reg);
    let text = save_project(&doc);
    let path = repo_file("examples/projects/audio-demo.atomik");
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let file = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(file, text, "regenerate with ATOMIK_UPDATE_GOLDEN=1");
    let loaded = load_project(&file, &reg).unwrap();
    assert_eq!(loaded, doc);
    assert_eq!(loaded.input_channels, 2);
    assert_eq!(loaded.assets.len(), 1);
    assert_eq!(loaded.song.tracks.len(), 3);
    let t = loaded.song.track(&key("audio-track")).unwrap();
    assert!(t.is_audio());
    assert_eq!(t.lanes[0].clips.len(), 3);
    assert_eq!(
        loaded.referenced_assets(),
        [AUDIO_DEMO_HASH.to_owned()].into()
    );
    // The track has a console channel like the instruments.
    assert!(loaded.console.channel_of(&key("audio-track")).is_some());
}

#[test]
fn save_writes_project_header_and_song() {
    let reg = console_registry();
    let doc = demo(&reg);
    let v: Value = serde_json::from_str(&save_project(&doc)).unwrap();
    assert_eq!(v["format"], "atomik-project");
    assert_eq!(v["version"], 17);
    assert_eq!(v["input_channels"], 0);
    assert_eq!(v["assets"], json!({}));
    assert_eq!(v["tempo_bpm"], 120);
    assert_eq!(v["devices"][0]["id"], "bass");
    assert_eq!(v["devices"][2]["folded"], true);
    assert_eq!(v["song"]["next_clip_id"], 4);
    assert_eq!(v["song"]["tempo"], json!([{"tick": 0, "bpm": 120}]));
    assert_eq!(v["song"]["meter"], json!([{"bar": 1, "num": 4, "den": 4}]));
    assert_eq!(
        v["song"]["loop"],
        json!({"start": 0, "end": 8 * BAR, "enabled": true})
    );
    let bass = &v["song"]["tracks"][0];
    assert_eq!(bass["device"], "bass");
    assert_eq!(bass["lanes"][0]["kind"], "notes");
    assert_eq!(bass["lanes"][1]["kind"], "automation");
    assert_eq!(bass["lanes"][1]["param"], "filter.cutoff");
    let clip = &bass["lanes"][0]["clips"][0];
    assert_eq!(clip["id"], 1);
    assert_eq!(clip["loop_length"], 4 * BAR);
    assert_eq!(clip["notes"][0], json!([0, 3 * PPQ / 4, 45, 0.95]));
    assert_eq!(
        bass["lanes"][1]["clips"][0]["points"][0],
        json!([0, 400, "linear"])
    );
    assert!(v.get("length_beats").is_none());
}

#[test]
fn empty_doc_round_trips() {
    let reg = console_registry();
    let doc = empty_doc();
    assert_eq!(load_project(&save_project(&doc), &reg).unwrap(), doc);
}

#[test]
fn patch_v1_and_v2_load_as_song_less_projects() {
    let reg = console_registry();
    for name in ["slice-demo.json", "slice-demo.v2.json", "minimal.json"] {
        let text = read(&format!("examples/patches/{name}"));
        let project = load_project(&text, &reg).unwrap();
        let patch = load_patch(&text, &reg).unwrap();
        assert_eq!(project, patch, "{name}");
        assert_eq!(project.song, empty_song(project.tempo_bpm));
    }
    let fast = edited(&read("examples/patches/minimal.json"), |v| {
        v["tempo_bpm"] = json!(133.5);
    });
    let doc = load_project(&fast, &reg).unwrap();
    assert_tempo_invariant(&doc);
    assert_eq!(doc.song.tempo[0].bpm, 133.5);
    // Patch errors keep their paths.
    let bad = edited(&read("examples/patches/minimal.json"), |v| {
        v["tempo_bpm"] = json!(5);
    });
    assert_rejects(&bad, "tempo_bpm", "20..=999");
}

#[test]
fn ac_s6_save_load_is_bit_exact_for_awkward_floats() {
    let reg = console_registry();
    let mut d = Document::new(demo(&reg));
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            atomik_model::TempoEvent::step(0, 188.110_848_031_224_26),
            atomik_model::TempoEvent::step(3, 20.000_000_000_000_004),
        ])),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetClipContent {
            id: ClipId(1),
            content: ClipContent::Notes(vec![
                note(0, 1, 0, f32::MIN_POSITIVE),
                note(i64::from(u32::MAX) * 7, 3, 127, 0.1 + 0.2),
            ]),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetClipContent {
            id: ClipId(2),
            content: ClipContent::Automation(vec![
                point(0, 20.000_002, Curve::Step),
                point(1, 19_999.998, Curve::Linear),
            ]),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let text = save_project(d.doc());
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text, "save is canonical");
    let ClipContent::Notes(n) = &back.song.clip(ClipId(1)).unwrap().2.content else {
        panic!()
    };
    assert_eq!(n[1].velocity.to_bits(), (0.1f32 + 0.2).to_bits());
    assert_eq!(
        back.song.tempo[0].bpm.to_bits(),
        188.110_848_031_224_26f64.to_bits()
    );
}

#[test]
fn load_rejects_bad_projects_with_paths() {
    let reg = console_registry();
    let good = save_project(&demo(&reg));
    assert!(load_project(&good, &reg).is_ok());
    let t0 = "song.tracks[0]";
    let clip0 = "song.tracks[0].lanes[0].clips[0]";
    let auto0 = "song.tracks[0].lanes[1].clips[0]";
    let cases: Vec<(Edit, String, &str)> = vec![
        (
            Box::new(|v| v["format"] = json!("nope")),
            "format".into(),
            "atomik-project",
        ),
        (
            Box::new(|v| v["version"] = json!(18)),
            "version".into(),
            "unsupported version",
        ),
        (
            Box::new(|v| v["extra"] = json!(1)),
            "extra".into(),
            "unknown field",
        ),
        (
            Box::new(|v| {
                v.as_object_mut().unwrap().remove("song");
            }),
            "song".into(),
            "missing field",
        ),
        (
            Box::new(|v| v["song"]["bogus"] = json!(1)),
            "song.bogus".into(),
            "unknown field",
        ),
        (
            Box::new(|v| v["song"]["tempo"][0]["tick"] = json!(5)),
            "song.tempo[0].tick".into(),
            "0",
        ),
        (
            Box::new(|v| v["song"]["tempo"][0]["bpm"] = json!(121)),
            "song.tempo[0].bpm".into(),
            "tempo_bpm",
        ),
        (
            Box::new(|v| v["song"]["tempo"] = json!([])),
            "song.tempo".into(),
            "at least one",
        ),
        (
            Box::new(|v| v["song"]["meter"] = json!([4, 3])),
            "song.meter[1]".into(),
            "2, 4, 8 or 16",
        ),
        (
            Box::new(|v| v["song"]["loop"]["end"] = json!(0)),
            "song.loop".into(),
            "start < end",
        ),
        (
            Box::new(|v| v["song"]["end"] = json!(0)),
            "song.end".into(),
            "greater than 0",
        ),
        (
            Box::new(|v| v["song"]["next_clip_id"] = json!(1)),
            format!("{t0}.lanes[0].clips[0].id"),
            "next_clip_id",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["device"] = json!("ghost")),
            format!("{t0}.device"),
            "no device",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["device"] = json!("echo")),
            format!("{t0}.device"),
            "Notes input",
        ),
        (
            Box::new(|v| v["song"]["tracks"][1]["device"] = json!("bass")),
            "song.tracks[1].device".into(),
            "already has a track",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["kind"] = json!("automation")),
            format!("{t0}.lanes[0].kind"),
            "notes",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][1]["param"] = json!("nope")),
            format!("{t0}.lanes[1].param"),
            "no param",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][1]["kind"] = json!("notes")),
            format!("{t0}.lanes[1].kind"),
            "automation",
        ),
        (
            Box::new(|v| v["song"]["tracks"][1]["lanes"][0]["clips"][0]["id"] = json!(1)),
            "song.tracks[1].lanes[0].clips[0].id".into(),
            "duplicate clip id",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["length"] = json!(0)),
            format!("{clip0}.length"),
            "greater than 0",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["start"] = json!(-1)),
            format!("{clip0}.start"),
            "0..=",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["loop_length"] = json!(0)),
            format!("{clip0}.loop_length"),
            "greater than 0",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["points"] = json!([])),
            format!("{clip0}.points"),
            "unknown field",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["notes"][1][0] = json!(0)),
            format!("{clip0}.notes[1]"),
            "sorted",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["notes"][0][2] = json!(128)
            }),
            format!("{clip0}.notes[0][2]"),
            "0..=127",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["notes"][0][3] = json!(0)),
            format!("{clip0}.notes[0][3]"),
            "(0, 1]",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["notes"][0] = json!([0, 1, 2])
            }),
            format!("{clip0}.notes[0]"),
            "[tick, length, key, velocity]",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][1]["clips"][0]["points"][0][2] = json!("cubic")
            }),
            format!("{auto0}.points[0][2]"),
            "\"linear\" or \"step\"",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][1]["clips"][0]["points"][1][0] = json!(0)),
            format!("{auto0}.points[1]"),
            "sorted",
        ),
    ];
    for (edit, path, needle) in cases {
        assert_rejects(&edited(&good, edit), &path, needle);
    }
}

#[test]
fn load_clamps_automation_values_and_sorts_clips() {
    let reg = console_registry();
    let good = save_project(&demo(&reg));
    let text = edited(&good, |v| {
        v["song"]["tracks"][0]["lanes"][1]["clips"][0]["points"][0][1] = json!(1e9);
    });
    let doc = load_project(&text, &reg).unwrap();
    let ClipContent::Automation(p) = &doc.song.clip(ClipId(2)).unwrap().2.content else {
        panic!()
    };
    assert_eq!(p[0].value, 20_000.0);
    // Clips are stored sorted by (start, id) whatever the file order.
    let text = edited(&good, |v| {
        v["song"]["next_clip_id"] = json!(5);
        let clips = v["song"]["tracks"][0]["lanes"][0]["clips"]
            .as_array_mut()
            .unwrap();
        let mut extra = clips[0].clone();
        extra["id"] = json!(4);
        clips.insert(0, extra);
    });
    let doc = load_project(&text, &reg).unwrap();
    let ids: Vec<_> = doc.song.tracks[0].lanes[0]
        .clips
        .iter()
        .map(|c| c.id.0)
        .collect();
    assert_eq!(ids, [1, 4]);
}

/// A song start with a console: the chain and `lead`, each synth with a channel.
fn console_start(reg: &Registry) -> Document {
    let mut d = song_start(&registry());
    for s in ["synth", "lead"] {
        d.apply(
            Command::Console(ConsoleCommand::AddChannel { source: key(s) }),
            reg,
        )
        .unwrap();
    }
    Document::new(d.doc().clone())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Any song built by commands survives save → load bit-exactly, and saving is canonical.
    #[test]
    fn project_round_trips(ops in prop::collection::vec(song_op(), 0..60), seed in any::<u64>()) {
        let reg = registry();
        let mut d = song_start(&reg);
        for op in &ops {
            if let Some(cmd) = resolve_song(op, d.doc(), &reg) {
                let _ = d.apply(cmd, &reg);
            }
        }
        let mut doc = d.doc().clone();
        doc.seed = seed;
        let text = save_project(&doc);
        let back = load_project(&text, &reg).unwrap();
        prop_assert_eq!(&back, &doc);
        prop_assert_eq!(save_project(&back), text);
    }

    /// The same with a console: song, rack and console commands (automatic channels
    /// included) give documents that save → load bit-exactly and canonically.
    #[test]
    fn project_with_console_round_trips(
        ops in prop::collection::vec(prop_oneof![
            2 => song_op().prop_map(Ok),
            1 => console_op().prop_map(Err),
        ], 0..60),
        seed in any::<u64>(),
    ) {
        let reg = console_registry();
        let mut d = console_start(&reg);
        for op in &ops {
            let cmd = match op {
                Ok(op) => resolve_song(op, d.doc(), &reg),
                Err(op) => resolve_console(op, d.doc()),
            };
            if let Some(cmd) = cmd {
                let _ = d.apply(cmd, &reg);
            }
        }
        assert_console_invariants(d.doc());
        let mut doc = d.doc().clone();
        doc.seed = seed;
        let text = save_project(&doc);
        let back = load_project(&text, &reg).unwrap();
        prop_assert_eq!(&back, &doc);
        prop_assert_eq!(save_project(&back), text);
    }

    /// The same with audio tracks, clips, trims, inputs and a sample pool.
    #[test]
    fn project_with_audio_round_trips(
        ops in prop::collection::vec(prop_oneof![
            2 => song_op().prop_map(|o| (Some(o), None, None)),
            1 => console_op().prop_map(|o| (None, Some(o), None)),
            3 => audio_op().prop_map(|o| (None, None, Some(o))),
        ], 0..60),
        seed in any::<u64>(),
    ) {
        let reg = audio_registry();
        let mut d = console_start(&reg);
        pre_register(&mut d, &reg);
        for (song, console, audio) in &ops {
            let cmd = match (song, console, audio) {
                (Some(op), _, _) => resolve_song(op, d.doc(), &reg),
                (_, Some(op), _) => resolve_console(op, d.doc()),
                (_, _, Some(op)) => resolve_audio(op, d.doc()),
                _ => None,
            };
            if let Some(cmd) = cmd {
                let _ = d.apply(cmd, &reg);
            }
        }
        assert_console_invariants(d.doc());
        assert_audio_invariants(d.doc());
        let mut doc = d.doc().clone();
        doc.seed = seed;
        let text = save_project(&doc);
        let back = load_project(&text, &reg).unwrap();
        prop_assert_eq!(&back, &doc);
        prop_assert_eq!(save_project(&back), text);
        // Pruning the pool keeps the document loadable and the song intact.
        doc.prune_unreferenced_assets();
        let pruned = load_project(&save_project(&doc), &reg).unwrap();
        prop_assert_eq!(&pruned.song, &back.song);
        prop_assert_eq!(pruned.assets.keys().cloned().collect::<std::collections::BTreeSet<_>>(), back.referenced_assets());
    }
}

// ---------------------------------------------------------------------------------------
// The console section.

#[test]
fn save_writes_the_console_section() {
    let reg = console_registry();
    let mut d = Document::new(demo(&reg));
    let v: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    let mut returns = vec![json!(null); FX_SLOTS];
    returns[0] = json!("echo");
    assert_eq!(
        v["console"],
        json!({
            "channels": [
                {"channel": "channel", "source": "bass"},
                {"channel": "channel-2", "source": "lead"}
            ],
            "master": "master",
            "returns": returns,
            "compensation": true
        })
    );
    // A source-less channel, no master and compensation off round-trip too.
    d.apply(Command::RemoveDevice { key: key("lead") }, &reg)
        .unwrap();
    d.apply(add(0, DELAY, Some("verb")), &reg).unwrap();
    d.apply(
        Command::Console(ConsoleCommand::AddChannel {
            source: key("verb"),
        }),
        &reg,
    )
    .unwrap();
    d.apply(Command::RemoveDevice { key: key("verb") }, &reg)
        .unwrap();
    // The commands never remove the master; a file can lack it.
    let mut doc = d.doc().clone();
    let master = doc.console.master.take().expect("a master");
    doc.console.returns = Default::default();
    doc.devices.retain(|x| x.key != master);
    doc.cables
        .retain(|c| c.from.device != master && c.to.device != master);
    let mut d = Document::new(doc);
    d.apply(
        Command::Console(ConsoleCommand::SetCompensation(false)),
        &reg,
    )
    .unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["console"],
        json!({
            "channels": [
                {"channel": "channel", "source": "bass"},
                {"channel": "channel-2", "source": null}
            ],
            "master": null,
            "returns": vec![json!(null); FX_SLOTS],
            "compensation": false
        })
    );
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text);
}

#[test]
fn load_rejects_bad_consoles_with_paths() {
    let reg = console_registry();
    let good = save_project(&demo(&reg));
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| {
                v.as_object_mut().unwrap().remove("console");
            }),
            "console",
            "missing field",
        ),
        (
            Box::new(|v| v["console"]["extra"] = json!(1)),
            "console.extra",
            "unknown field",
        ),
        (
            Box::new(|v| v["console"]["channels"][0]["channel"] = json!("ghost")),
            "console.channels[0].channel",
            "no device",
        ),
        (
            Box::new(|v| v["console"]["channels"][0]["channel"] = json!("bass")),
            "console.channels[0].channel",
            "atomik.channel",
        ),
        (
            Box::new(|v| v["console"]["channels"][1]["channel"] = json!("channel")),
            "console.channels[1].channel",
            "listed twice",
        ),
        (
            Box::new(|v| v["console"]["channels"][0]["source"] = json!("master")),
            "console.channels[0].source",
            "console device",
        ),
        (
            Box::new(|v| v["console"]["channels"][0]["source"] = json!("ghost")),
            "console.channels[0].source",
            "no device",
        ),
        (
            Box::new(|v| v["console"]["channels"][0]["source"] = json!("echo")),
            "console.returns[0]",
            "feeds channel",
        ),
        (
            Box::new(|v| v["console"]["master"] = json!("channel")),
            "console.master",
            "atomik.master",
        ),
        (
            Box::new(|v| v["console"]["master"] = json!(null)),
            "console.returns[0]",
            "no master",
        ),
        (
            Box::new(|v| v["console"]["returns"] = json!([null])),
            "console.returns",
            "8",
        ),
        (
            Box::new(|v| v["console"]["returns"][1] = json!("echo")),
            "console.returns[1]",
            "already on return 1",
        ),
        (
            Box::new(|v| v["console"]["returns"][2] = json!("channel-2")),
            "console.returns[2]",
            "console device",
        ),
        (
            Box::new(|v| v["console"]["returns"][2] = json!(6)),
            "console.returns[2]",
            "string",
        ),
        (
            Box::new(|v| v["console"]["compensation"] = json!(1)),
            "console.compensation",
            "boolean",
        ),
        (
            Box::new(|v| v["devices"][1]["params"] = json!({"internal.solo_silence": 1})),
            r#"devices[1].params."internal.solo_silence""#,
            "never stored",
        ),
    ];
    for (edit, path, needle) in cases {
        assert_rejects(&edited(&good, edit), path, needle);
    }
    // Version 1 files have no console section.
    let v1 = edited(&good, |v| {
        v["version"] = json!(1);
        let o = v.as_object_mut().unwrap();
        o.remove("assets");
        o.remove("input_channels");
    });
    assert_rejects(&v1, "console", "unknown field");
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
    .replace("\r\n", "\n")
}

fn fixture_v1() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-v1.atomik"),
    )
    .unwrap()
}

/// A version 1 project gets a channel per instrument (in rack order) and a master,
/// wired per the auto-wiring rules; nothing else is lost, and the result is deterministic.
#[test]
fn ac_x6_v1_projects_migrate_losslessly() {
    let reg = console_registry();
    let text = fixture_v1();
    let raw: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["version"], 1);
    // Without console devices in the registry there is no migration.
    let plain = load_project(&text, &registry()).unwrap();
    assert_eq!(plain.console, empty_doc().console);
    let doc = load_project(&text, &reg).unwrap();
    assert_eq!(load_project(&text, &reg).unwrap(), doc, "deterministic");
    // Rack fields and song are untouched.
    assert_eq!(
        (
            doc.sample_rate,
            doc.tempo_bpm,
            doc.seed,
            doc.output_channels
        ),
        (
            plain.sample_rate,
            plain.tempo_bpm,
            plain.seed,
            plain.output_channels
        )
    );
    assert_eq!(doc.song, plain.song);
    // Every v1 device survives unchanged and in order; the console adds its own.
    let kept: Vec<_> = doc
        .devices
        .iter()
        .filter(|d| ![CHANNEL, MASTER].contains(&d.type_id.as_str()))
        .cloned()
        .collect();
    assert_eq!(kept, plain.devices);
    assert_eq!(
        keys(&Document::new(doc.clone())),
        [
            "bass",
            "channel",
            "echo",
            "lead",
            "channel-2",
            "mixer",
            "master"
        ]
    );
    assert_eq!(
        doc.console.channels,
        [
            ConsoleChannel {
                channel: key("channel"),
                source: Some(key("bass"))
            },
            ConsoleChannel {
                channel: key("channel-2"),
                source: Some(key("lead"))
            },
        ]
    );
    assert_eq!(doc.console.master, Some(key("master")));
    assert!(doc.console.returns.iter().all(Option::is_none));
    assert!(doc.console.compensation);
    // Every v1 cable survives except those into the interface outs the master takes over.
    for c in &plain.cables {
        let replaced = c.to.device.0 == "interface";
        assert_eq!(doc.cables.contains(c), !replaced, "{c:?}");
    }
    for c in [
        cable("bass:out_l", "channel:in_l"),
        cable("lead:out_r", "channel-2:in_r"),
        cable("channel-2:send8_r", "master:send_bus8_r"),
        cable("master:main_l", "interface:out_1"),
        cable("master:main_r", "interface:out_2"),
    ] {
        assert!(doc.cables.contains(&c), "{c:?}");
    }
    // 2 replaced; per channel 2 inputs + 2 outs + 16 sends; 2 mains.
    assert_eq!(doc.cables.len(), plain.cables.len() - 2 + 2 * 20 + 2);
    assert_console_invariants(&doc);
    // Saved as version 6, it reads back identically.
    let v3 = save_project(&doc);
    assert!(v3.contains("\"version\": 17,"));
    assert_eq!(load_project(&v3, &reg).unwrap(), doc);
}

#[test]
fn patches_load_without_a_console_even_with_console_devices() {
    let reg = console_registry();
    for name in ["slice-demo.json", "slice-demo.v2.json"] {
        let doc = load_project(&read(&format!("examples/patches/{name}")), &reg).unwrap();
        assert_eq!(doc.console, empty_doc().console, "{name}");
        assert_eq!(doc.devices.len(), 4);
    }
}

// ---- version 13: tempo ramps and the meter map ------------------------------------------

fn with_maps(reg: &Registry) -> RackDoc {
    let mut d = Document::new(demo(reg));
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            atomik_model::TempoEvent {
                tick: 0,
                bpm: 120.0,
                ramp: true,
            },
            atomik_model::TempoEvent::step(2 * BAR, 90.5),
            atomik_model::TempoEvent {
                tick: 5 * BAR + 17,
                bpm: 60.0,
                ramp: true,
            },
            atomik_model::TempoEvent::step(9 * BAR, 175.0),
        ])),
        reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetMeterMap(vec![
            atomik_model::MeterEvent {
                bar: 1,
                num: 4,
                den: 4,
            },
            atomik_model::MeterEvent {
                bar: 3,
                num: 7,
                den: 8,
            },
            atomik_model::MeterEvent {
                bar: 9,
                num: 3,
                den: 4,
            },
        ])),
        reg,
    )
    .unwrap();
    d.doc().clone()
}

#[test]
fn tempo_ramps_and_meter_changes_round_trip_canonically() {
    let reg = console_registry();
    let doc = with_maps(&reg);
    let text = save_project(&doc);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 17);
    assert_eq!(
        v["song"]["tempo"],
        json!([
            {"tick": 0, "bpm": 120, "ramp": true},
            {"tick": 2 * BAR, "bpm": 90.5},
            {"tick": 5 * BAR + 17, "bpm": 60, "ramp": true},
            {"tick": 9 * BAR, "bpm": 175}
        ])
    );
    assert_eq!(
        v["song"]["meter"],
        json!([
            {"bar": 1, "num": 4, "den": 4},
            {"bar": 3, "num": 7, "den": 8},
            {"bar": 9, "num": 3, "den": 4}
        ])
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded, doc);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_project_without_ramps_writes_no_ramp_field() {
    let reg = console_registry();
    let text = save_project(&demo(&reg));
    assert!(!text.contains("ramp"));
}

#[test]
fn a_version_12_project_loads_unchanged_and_resaves_as_13() {
    let reg = console_registry();
    let file = fixture("demo-v12.atomik");
    assert!(file.contains("\"version\": 12,") && file.contains("\"meter\": [4, 4],"));
    let doc = load_project(&file, &reg).unwrap();
    assert_eq!(doc.song.meter(), (4, 4));
    assert_eq!(doc.song.meter.len(), 1);
    assert!(doc.song.tempo.iter().all(|e| !e.ramp));
    assert_eq!(save_project(&doc), as_current(&file, 12));
}

#[test]
fn older_files_keep_their_single_meter_pair() {
    let reg = console_registry();
    let file =
        as_old(&save_project(&demo(&reg)), 12).replace("\"meter\": [4, 4],", "\"meter\": [7, 8],");
    assert!(file.contains("\"meter\": [7, 8],"));
    let doc = load_project(&file, &reg).unwrap();
    assert_eq!(
        doc.song.meter,
        vec![atomik_model::MeterEvent {
            bar: 1,
            num: 7,
            den: 8
        }]
    );
}

#[test]
fn ramp_and_meter_errors_name_their_json_path() {
    let reg = console_registry();
    let good = save_project(&with_maps(&reg));
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| v["song"]["tempo"][3]["ramp"] = json!(true)),
            "song.tempo[3].ramp",
            "cannot ramp",
        ),
        (
            Box::new(|v| v["song"]["tempo"][1]["ramp"] = json!(1)),
            "song.tempo[1].ramp",
            "boolean",
        ),
        (
            Box::new(|v| v["song"]["meter"][0]["bar"] = json!(2)),
            "song.meter[0].bar",
            "bar 1",
        ),
        (
            Box::new(|v| v["song"]["meter"][2]["bar"] = json!(3)),
            "song.meter[2].bar",
            "strictly",
        ),
        (
            Box::new(|v| v["song"]["meter"][1]["den"] = json!(3)),
            "song.meter[1].den",
            "beat unit",
        ),
        (
            Box::new(|v| v["song"]["meter"][1]["num"] = json!(33)),
            "song.meter[1].num",
            "beats per bar",
        ),
        (
            Box::new(|v| {
                v["song"]["meter"][1].as_object_mut().unwrap().remove("den");
            }),
            "song.meter[1].den",
            "missing",
        ),
        (
            Box::new(|v| v["song"]["meter"] = json!([])),
            "song.meter",
            "at least one",
        ),
    ];
    for (edit, path, needle) in cases {
        assert_rejects(&edited(&good, edit), path, needle);
    }
}

#[test]
fn ramps_are_unknown_before_version_13() {
    let reg = console_registry();
    let text = edited(&save_project(&with_maps(&reg)), |v| {
        v["version"] = json!(12)
    });
    assert_rejects(&text, "song.tempo[0].ramp", "unknown field");
}
