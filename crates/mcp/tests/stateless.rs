//! The stateless tools against the example patches and projects, and the path policy.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

fn server() -> Server {
    Server::new(Paths::new(repo(), &[]))
}

fn call(s: &mut Server, name: &str, args: Value) -> ToolOutput {
    s.call_tool(name, &Args::new(Some(&args)))
}

/// A path under `target/tmp` (inside the allowed root), made fresh.
fn tmp(name: &str) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-stateless");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

#[test]
fn list_devices_lists_and_describes() {
    let mut s = server();
    let all = call(&mut s, "list_devices", json!({}));
    assert!(!all.is_error);
    let v = all.parsed();
    assert!(v["count"].as_u64().unwrap() > 10);
    let one = call(
        &mut s,
        "list_devices",
        json!({"type_id": "atomik.poly-synth"}),
    )
    .parsed();
    assert_eq!(one["kind"], "instrument");
    let cutoff = one["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == "filter.cutoff")
        .expect("a cutoff parameter");
    assert!(cutoff["min"].as_f64().unwrap() < cutoff["max"].as_f64().unwrap());
    assert!(
        one["sockets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["direction"] == "output")
    );
    let fx = call(&mut s, "list_devices", json!({"kind": "effect"})).parsed();
    assert!(
        fx["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["kind"] == "effect")
    );
    let q = call(&mut s, "list_devices", json!({"query": "POLY"})).parsed();
    assert!(q["count"].as_u64().unwrap() >= 1);
    let unknown = call(&mut s, "list_devices", json!({"type_id": "nope"}));
    assert!(unknown.is_error);
}

#[test]
fn project_inspect_lists_the_controller_lanes_of_a_note_lane() {
    let mut s = server();
    // The demo project with a pitch bend and a mod wheel lane added to its first note clip.
    let text = std::fs::read_to_string(repo().join("examples/projects/demo.atomik")).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let clip = &mut v["song"]["tracks"][0]["lanes"][0]["clips"][0];
    clip["controllers"] = json!([
        {"kind": "pitch_bend", "points": [[0, 0.0, "linear"], [100, 0.5, "step"]]},
        {"kind": "cc", "number": 1, "points": [[50, 1.0, "step"]]},
    ]);
    let path = tmp("controllers.atomik");
    std::fs::write(&path, v.to_string()).unwrap();
    let r = call(&mut s, "project_inspect", json!({"path": path}));
    assert!(!r.is_error, "{}", r.first_text());
    let lane = &r.parsed()["song"]["tracks"][0]["lanes"][0];
    assert_eq!(
        lane["controllers"],
        json!([
            {"kind": "pitch_bend", "points": 2},
            {"kind": "cc", "number": 1, "points": 1},
        ])
    );
    // A project without controller data has no such field, and a bad lane names its path.
    let plain = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik"}),
    )
    .parsed();
    assert!(
        plain["song"]["tracks"][0]["lanes"][0]
            .get("controllers")
            .is_none()
    );
    v["song"]["tracks"][0]["lanes"][0]["clips"][0]["controllers"][1]["number"] = json!(200);
    let bad = tmp("controllers-bad.atomik");
    std::fs::write(&bad, v.to_string()).unwrap();
    let e = call(&mut s, "project_inspect", json!({"path": bad}));
    assert!(e.is_error);
    assert!(
        e.first_text().contains("controllers[1].number"),
        "{}",
        e.first_text()
    );
}

#[test]
fn project_inspect_lists_the_take_groups_of_a_lane_with_their_comp() {
    use atomik_model::{Clip, ClipContent, ClipId, Command, Document, Note, SongCommand};
    let mut s = server();
    let reg = atomik_render::default_registry();
    let text = std::fs::read_to_string(repo().join("examples/projects/demo.atomik")).unwrap();
    let mut d = Document::new(atomik_model::load_project(&text, &reg).unwrap());
    let device = d.doc().song.tracks[0].device.clone();
    let ppq = atomik_core::PPQ;
    let take = |start: i64, length: i64, key: u8| Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        content: ClipContent::Notes(vec![Note::new(0, ppq, key, 0.8)]),
    };
    d.apply(
        Command::Song(SongCommand::NewTakeGroup {
            device,
            lane: 0,
            takes: vec![
                take(0, 8 * ppq, 60),
                take(0, 8 * ppq, 62),
                take(0, 4 * ppq, 64),
            ],
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: 2 * ppq,
            end: 3 * ppq,
        }),
        &reg,
    )
    .unwrap();
    let path = tmp("takes.atomik");
    std::fs::write(&path, atomik_model::save_project(d.doc())).unwrap();
    let r = call(&mut s, "project_inspect", json!({"path": path}));
    assert!(!r.is_error, "{}", r.first_text());
    let lane = &r.parsed()["song"]["tracks"][0]["lanes"][0];
    let groups = lane["take_groups"].as_array().expect("take_groups");
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert_eq!((g["id"].as_u64(), g["takes"].as_u64()), (Some(1), Some(3)));
    assert_eq!(
        (g["start"].as_i64(), g["end"].as_i64()),
        (Some(0), Some(8 * ppq))
    );
    assert_eq!(g["picked"], Value::Null, "the comp is not one whole take");
    assert_eq!(
        g["comp"],
        json!([
            {"take": 2, "start": 0, "end": 2 * ppq},
            {"take": 0, "start": 2 * ppq, "end": 3 * ppq},
            {"take": 2, "start": 3 * ppq, "end": 4 * ppq},
            {"take": 1, "start": 4 * ppq, "end": 8 * ppq},
        ])
    );
    assert_eq!(
        g["clips"].as_array().unwrap().len(),
        4,
        "the compiled lane clips"
    );
    // The plain demo has no such field.
    let plain = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik"}),
    )
    .parsed();
    assert!(
        plain["song"]["tracks"][0]["lanes"][0]
            .get("take_groups")
            .is_none()
    );
}

#[test]
fn project_inspect_describes_a_project_and_a_patch() {
    let mut s = server();
    let p = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    assert!(!p.is_error, "{}", p.first_text());
    let v = p.parsed();
    assert_eq!(v["kind"], "project");
    assert_eq!(v["song"]["tracks"].as_array().unwrap().len(), 2);
    assert!(
        v["song"]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["chase"] == true),
        "every track reports its chase flag, on for a project that never set it"
    );
    let lanes: Vec<&serde_json::Value> = v["song"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["lanes"].as_array().unwrap())
        .filter(|l| l["kind"].as_str().unwrap().starts_with("automation:"))
        .collect();
    assert!(!lanes.is_empty());
    assert!(
        lanes.iter().all(|l| l["write"] == "touch"),
        "an automation lane reports its write mode (touch unless the project says otherwise)"
    );
    assert!(
        v["song"]["tracks"][0]["lanes"][0].get("write").is_none(),
        "a note lane has no write mode"
    );
    assert!(
        v["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["key"] == "bass")
    );
    assert!(
        v["cables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "bass:out_l -> channel:in_l")
    );
    assert_eq!(v["song"]["ticks_per_beat"], 15360);
    assert!(v["devices"][0].get("params").is_none());
    let with = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik", "include_params": true}),
    )
    .parsed();
    assert!(with["devices"][0]["params"].is_object());

    let patch = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/patches/minimal.json"}),
    );
    assert!(!patch.is_error, "{}", patch.first_text());
    assert_eq!(patch.parsed()["kind"], "patch");

    // A bundle folder resolves to its project file.
    let folder = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/audio-demo"}),
    );
    assert!(!folder.is_error, "{}", folder.first_text());
}

#[test]
fn validate_reports_data_not_failures() {
    let mut s = server();
    let ok = call(
        &mut s,
        "validate",
        json!({"path": "examples/patches/minimal.json"}),
    );
    assert!(!ok.is_error);
    assert_eq!(ok.parsed()["valid"], true);

    let bad = tmp("bad.json");
    std::fs::write(
        &bad,
        r#"{"format":"atomik-patch","version":1,"sample_rate":48000,"tempo_bpm":120,
            "length_beats":1,"output_channels":2,
            "devices":[{"id":"a","type":"atomik.no-such-device"}],"cables":[]}"#,
    )
    .unwrap();
    let r = call(&mut s, "validate", json!({"path": bad}));
    assert!(!r.is_error, "an invalid file is a result");
    let v = r.parsed();
    assert_eq!(v["valid"], false);
    assert!(v["error"].as_str().unwrap().contains("devices[0]"), "{v}");

    let proj = call(
        &mut s,
        "validate",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    assert_eq!(proj.parsed()["valid"], true);
}

#[test]
fn a_bounce_is_deterministic_across_blocks_and_workers() {
    let mut s = server();
    let a = tmp("demo-a.wav");
    let b = tmp("demo-b.wav");
    let ra = call(
        &mut s,
        "bounce",
        json!({"path": "examples/projects/demo.atomik", "output": a, "block": 512}),
    );
    assert!(!ra.is_error, "{}", ra.first_text());
    let rb = call(
        &mut s,
        "bounce",
        json!({"path": "examples/projects/demo.atomik", "output": b, "block": 333, "workers": 2}),
    );
    assert!(!rb.is_error, "{}", rb.first_text());
    let audio = ra.parsed()["audio"].clone();
    assert_eq!(audio["silent"], false);
    assert_eq!(audio["channels"], 2);
    assert_eq!(audio["non_finite_samples"], 0);
    assert!(audio["duration_s"].as_f64().unwrap() > 10.0);
    assert!(audio["window_rms_dbfs"].as_array().unwrap().len() >= 10);
    assert_eq!(audio["spectrum"]["bands"].as_array().unwrap().len(), 8);

    let cmp = call(&mut s, "compare_audio", json!({"a": a, "b": b})).parsed();
    assert_eq!(cmp["bit_identical"], true, "{cmp}");
    assert_eq!(cmp["matches"], true);

    // Analysing the written file agrees with the report of the bounce.
    let an = call(&mut s, "analyze_audio", json!({"path": a})).parsed();
    assert_eq!(an["frames"], audio["frames"]);
    assert_eq!(an["peak_dbfs"], audio["peak_dbfs"]);
}

#[test]
fn a_bounce_can_compare_itself_with_a_reference() {
    let mut s = server();
    let reference = tmp("ref.wav");
    let first = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": reference}),
    );
    assert!(!first.is_error, "{}", first.first_text());
    let second = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": tmp("again.wav"),
               "reference": reference, "format": "f32"}),
    );
    let c = second.parsed()["comparison"].clone();
    assert_eq!(c["matches"], true, "{c}");
    assert_eq!(c["bit_identical"], true);
}

#[test]
fn a_changed_patch_is_caught_against_its_reference() {
    let mut s = server();
    let reference = tmp("before.wav");
    call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": reference}),
    );
    // The same patch with the filter nearly closed: darker, so quieter in the loudness windows.
    let text = std::fs::read_to_string(repo().join("examples/patches/minimal.json")).unwrap();
    let edited = tmp("edited.json");
    std::fs::write(
        &edited,
        text.replace("\"filter.cutoff\": 1500", "\"filter.cutoff\": 120"),
    )
    .unwrap();
    let after = tmp("after.wav");
    let r = call(
        &mut s,
        "bounce",
        json!({"path": edited, "output": after, "reference": reference}),
    );
    assert!(!r.is_error, "{}", r.first_text());
    let c = r.parsed()["comparison"].clone();
    assert_eq!(c["bit_identical"], false, "{c}");
    assert_eq!(c["same_shape"], true);
    assert!(c["max_abs_diff"].as_f64().unwrap() > 0.0);
    // A tolerance wide enough accepts it; the default (the goldens' 0.5 dB) does not.
    let loose = call(
        &mut s,
        "compare_audio",
        json!({"a": reference, "b": after, "tolerance_db": 60.0}),
    )
    .parsed();
    assert_eq!(loose["matches"], true, "{loose}");
    let strict = call(&mut s, "compare_audio", json!({"a": reference, "b": after})).parsed();
    assert_eq!(strict["matches"], false, "{strict}");
}

#[test]
fn bounce_refuses_what_it_cannot_do_with_a_clear_message() {
    let mut s = server();
    let plugins = call(
        &mut s,
        "bounce",
        json!({"path": "examples/projects/plugin-demo"}),
    );
    assert!(plugins.is_error);
    assert!(
        plugins.first_text().contains("--allow-plugins"),
        "{}",
        plugins.first_text()
    );
    let patch_with_loop = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "loop_count": 2}),
    );
    assert!(patch_with_loop.is_error);
    let bad_block = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "block": 0}),
    );
    assert!(bad_block.is_error);
    let bad_format = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "format": "mp3"}),
    );
    assert!(bad_format.is_error);
}

#[test]
fn compare_audio_flags_a_different_shape() {
    let mut s = server();
    let a = tmp("short.wav");
    let b = tmp("long.wav");
    call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": a}),
    );
    call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/slice-demo.json", "output": b}),
    );
    let c = call(&mut s, "compare_audio", json!({"a": a, "b": b})).parsed();
    assert_eq!(c["same_shape"], false);
    assert_eq!(c["matches"], false);
}

#[test]
fn list_plugins_reads_a_cache_and_survives_a_missing_one() {
    let mut s = server();
    let missing = tmp("no-cache.json");
    std::fs::write(&missing, "not a cache").unwrap();
    let r = call(&mut s, "list_plugins", json!({"cache": missing}));
    assert!(!r.is_error, "{}", r.first_text());
    let v = r.parsed();
    assert_eq!(v["count"], 0);
    assert!(v["cache"].as_str().unwrap().starts_with("Rebuilt"), "{v}");
}

#[test]
fn paths_outside_the_allowed_folders_are_refused() {
    let mut s = server();
    let outside = std::env::temp_dir().join("atomik-mcp-outside.json");
    std::fs::write(&outside, "{}").unwrap();
    let read = call(
        &mut s,
        "validate",
        json!({"path": outside.to_string_lossy()}),
    );
    assert!(read.is_error);
    assert!(
        read.first_text().contains("outside the allowed folders"),
        "{}",
        read.first_text()
    );
    let escape = call(
        &mut s,
        "validate",
        json!({"path": "../../../../../../../../../../../../windows/win.ini"}),
    );
    assert!(escape.is_error);
    let write = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json",
               "output": std::env::temp_dir().join("atomik-mcp-out.wav").to_string_lossy()}),
    );
    assert!(write.is_error);
    assert!(write.first_text().contains("outside the allowed folders"));
    // `..` through a folder that does not exist yet must not slip out either.
    let dotdot = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": "newdir/../../../escape.wav"}),
    );
    assert!(dotdot.is_error, "{}", dotdot.first_text());
    assert!(!repo().join("newdir").exists());
    let _ = std::fs::remove_file(outside);
}

#[test]
fn an_extra_root_allows_its_folder() {
    let extra = std::env::temp_dir().join("atomik-mcp-extra-root");
    std::fs::create_dir_all(&extra).unwrap();
    let file = extra.join("p.json");
    std::fs::copy(repo().join("examples/patches/minimal.json"), &file).unwrap();
    let mut s = Server::new(Paths::new(repo(), std::slice::from_ref(&extra)));
    let r = call(&mut s, "validate", json!({"path": file.to_string_lossy()}));
    assert!(!r.is_error, "{}", r.first_text());
    assert_eq!(r.parsed()["valid"], true);
    let _ = std::fs::remove_dir_all(extra);
}

#[test]
fn argument_types_are_checked() {
    let mut s = server();
    let r = call(&mut s, "bounce", json!({"path": 3}));
    assert!(r.is_error);
    assert!(r.first_text().contains("must be a string"));
    let r = call(
        &mut s,
        "analyze_audio",
        json!({"path": "x.wav", "window_s": "long"}),
    );
    assert!(r.is_error);
}

#[test]
fn project_inspect_lists_the_groove_library_of_a_song() {
    use atomik_model::{Command, Document, Groove, GrooveSlot, SongCommand, StepResolution};
    let mut s = server();
    let reg = atomik_render::default_registry();
    let text = std::fs::read_to_string(repo().join("examples/projects/demo.atomik")).unwrap();
    let mut d = Document::new(atomik_model::load_project(&text, &reg).unwrap());
    let groove = Groove {
        name: "Late 2nd".into(),
        step: StepResolution::Sixteenth,
        slots: vec![
            GrooveSlot::STRAIGHT,
            GrooveSlot {
                offset: 0.125,
                velocity: 0.5,
            },
        ],
    };
    d.apply(Command::Song(SongCommand::SetGroove(groove)), &reg)
        .unwrap();
    let path = tmp("grooves.atomik");
    std::fs::write(&path, atomik_model::save_project(d.doc())).unwrap();
    let r = call(&mut s, "project_inspect", json!({"path": path}));
    assert!(!r.is_error, "{}", r.first_text());
    assert_eq!(
        r.parsed()["song"]["grooves"],
        json!([{"name": "Late 2nd", "step": "1/16", "slots": [[0.0, 1.0], [0.125, 0.5]]}])
    );
    // A song without a library has no field.
    let plain = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik"}),
    )
    .parsed();
    assert!(plain["song"].get("grooves").is_none());
}

#[test]
fn project_inspect_lists_the_folders_colours_comments_and_folds() {
    let mut s = server();
    let plain = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik"}),
    )
    .parsed();
    assert!(
        plain["song"].get("folders").is_none(),
        "a song without folders lists none"
    );
    for t in plain["song"]["tracks"].as_array().unwrap() {
        assert!(
            t.get("folder").is_none() && t.get("color").is_none() && t.get("comment").is_none()
        );
    }
    let v = call(
        &mut s,
        "project_inspect",
        json!({"path": "crates/model/tests/golden/organised_project.json"}),
    )
    .parsed();
    assert_eq!(
        v["song"]["folders"],
        json!([
            {"id": 1, "name": "Band", "collapsed": false, "tracks": ["synth", "lead", "a"],
             "color": "#ff8000"},
            {"id": 2, "name": "Rhythm \"main\"", "collapsed": true, "tracks": ["lead", "a"],
             "parent": 1}
        ])
    );
    let tracks = v["song"]["tracks"].as_array().unwrap();
    assert_eq!(tracks[0]["folder"], 1);
    assert_eq!(tracks[1]["folder"], 2);
    assert_eq!(tracks[1]["comment"], "Line one\nLine \"two\"");
    assert_eq!(tracks[3]["color"], "#12abef");
    assert!(tracks[4].get("folder").is_none());
    assert_eq!(tracks[1]["lanes"][1]["folded"], true);
    assert_eq!(tracks[1]["lanes"][0]["take_groups"][0]["open"], true);
}

#[test]
fn project_inspect_lists_the_audio_clips_with_their_editing() {
    use atomik_model::{
        AudioClipContent, ClipContent, Command, Document, FadeCurve, GainPoint, SongCommand,
    };
    let mut s = server();
    let reg = atomik_render::default_registry();
    let text = std::fs::read_to_string(repo().join("examples/projects/audio-demo.atomik")).unwrap();
    let mut d = Document::new(atomik_model::load_project(&text, &reg).unwrap());
    let (device, clip) = {
        let t = d.doc().song.tracks.iter().find(|t| t.is_audio()).unwrap();
        (t.device.clone(), t.lanes[0].clips[0].clone())
    };
    let ClipContent::Audio(mut a) = clip.content.clone() else {
        panic!("an audio clip")
    };
    // The plain clip first: no editing fields beyond the always-present ones.
    let path = tmp("audio-plain.atomik");
    std::fs::write(&path, atomik_model::save_project(d.doc())).unwrap();
    let plain = call(&mut s, "project_inspect", json!({"path": path})).parsed();
    let lane = plain["song"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["device"] == device.as_str())
        .unwrap()["lanes"][0]
        .clone();
    let first = &lane["audio_clips"][0];
    assert_eq!(first["id"].as_u64(), Some(clip.id.0));
    assert!(first.get("reverse").is_none() && first.get("envelope").is_none());
    assert_eq!(first["fade_curve"], a.fade_curve.id());
    // Reversed, with an envelope and another curve and crossfade.
    a.reverse = true;
    a.fade_curve = FadeCurve::SCurve;
    a.xfade = None;
    a.envelope = vec![
        GainPoint {
            frame: 0,
            gain_db: -6.0,
        },
        GainPoint {
            frame: 100,
            gain_db: 0.0,
        },
    ];
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id: clip.id,
            start: clip.start,
            length: clip.length,
            muted: false,
            content: AudioClipContent { ..a },
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let path = tmp("audio-edited.atomik");
    std::fs::write(&path, atomik_model::save_project(d.doc())).unwrap();
    let r = call(&mut s, "project_inspect", json!({"path": path}));
    assert!(!r.is_error, "{}", r.first_text());
    let v = r.parsed();
    let lane = &v["song"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["device"] == device.as_str())
        .unwrap()["lanes"][0];
    let c = &lane["audio_clips"][0];
    assert_eq!(c["reverse"], true);
    assert_eq!(c["envelope"], 2);
    assert_eq!(c["fade_curve"], "s_curve");
    assert_eq!(c["crossfade"], "off");
    assert!(c.get("crossfade_ticks").is_none());
}

#[test]
fn project_inspect_lists_the_warp_of_an_audio_clip() {
    use atomik_model::{
        AudioClipContent, ClipContent, Command, Document, SongCommand, WarpMarker, WarpMode,
        WarpSettings,
    };
    let mut s = server();
    let reg = atomik_render::default_registry();
    let text = std::fs::read_to_string(repo().join("examples/projects/audio-demo.atomik")).unwrap();
    let mut d = Document::new(atomik_model::load_project(&text, &reg).unwrap());
    let (device, clip) = {
        let t = d.doc().song.tracks.iter().find(|t| t.is_audio()).unwrap();
        (t.device.clone(), t.lanes[0].clips[0].clone())
    };
    let ClipContent::Audio(a) = clip.content.clone() else {
        panic!("an audio clip")
    };
    let lane_of = |s: &mut _, d: &Document, name: &str| {
        let path = tmp(name);
        std::fs::write(&path, atomik_model::save_project(d.doc())).unwrap();
        let v = call(s, "project_inspect", json!({"path": path})).parsed();
        v["song"]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["device"] == device.as_str())
            .unwrap()["lanes"][0]
            .clone()
    };
    // No warp, no field.
    let lane = lane_of(&mut s, &d, "warp-none.atomik");
    assert!(lane["audio_clips"][0].get("warp").is_none());
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id: clip.id,
            start: clip.start,
            length: clip.length,
            muted: false,
            content: AudioClipContent {
                warp: WarpSettings {
                    enabled: true,
                    mode: WarpMode::Tonal,
                    source_bpm: 96.5,
                    transpose_cents: -300,
                    markers: vec![WarpMarker { tick: 0, source: 0 }],
                },
                ..a
            },
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let lane = lane_of(&mut s, &d, "warp-on.atomik");
    let w = &lane["audio_clips"][0]["warp"];
    assert_eq!(w["enabled"], true);
    assert_eq!(w["mode"], "tonal");
    assert_eq!(w["bpm"], 96.5);
    assert_eq!(w["transpose_cents"], -300);
    assert_eq!(w["markers"], 1);
}
