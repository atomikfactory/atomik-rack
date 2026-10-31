//! The library in the session: `session_open` takes a library folder, and `session_library`
//! saves the selected devices or a clip into it and loads items back, like the app's Save to
//! Library and its double click.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, SongCommand, empty_doc,
    save_project,
};
use atomik_render::default_registry;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const BAR: i64 = 61_440;

fn repo() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

fn server() -> Server {
    Server::new(Paths::new(repo(), &[]))
}

fn call(s: &mut Server, name: &str, args: Value) -> ToolOutput {
    s.call_tool(name, &Args::new(Some(&args)))
}

fn ok(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, name, args);
    assert!(!r.is_error, "{name}: {}", r.first_text());
    r.parsed()
}

fn fails(s: &mut Server, name: &str, args: Value) -> String {
    let r = call(s, name, args);
    assert!(r.is_error, "{name} should fail: {}", r.first_text());
    r.first_text().to_owned()
}

/// A fresh folder under `target/tmp` (inside the allowed root).
fn fresh(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("mcp-session-library")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// A project file with a synth "lead" and a delay "echo" and, on the synth's track, a note clip
/// of one bar at bar 3. Returns its path and the clip's id.
fn project(dir: &Path) -> (String, u64) {
    let r = default_registry();
    let mut d = Document::new(empty_doc());
    for (i, (type_id, k)) in [
        ("atomik.poly-synth", "lead"),
        ("atomik.stereo-delay", "echo"),
    ]
    .into_iter()
    .enumerate()
    {
        d.apply(
            Command::AddDevice {
                index: i,
                type_id: type_id.to_owned(),
                key: Some(key(k)),
            },
            &r,
        )
        .unwrap();
    }
    d.apply(
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
        &r,
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 2 * BAR,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, 3_840, 60, 0.8)].into()),
            },
        }),
        &r,
    )
    .unwrap();
    let id = d.doc().song.tracks[0].lanes[0].clips[0].id.0;
    let path = dir.join("Song.atomik");
    std::fs::write(&path, save_project(d.doc())).unwrap();
    (text(&path), id)
}

fn device_keys(status: &Value) -> Vec<String> {
    status["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap().to_owned())
        .collect()
}

fn clips_on_first_track(status: &Value) -> i64 {
    status["document"]["song"]["tracks"][0]["lanes"][0]["clips"]
        .as_i64()
        .unwrap()
}

// ---------------------------------------------------------------------------- session_open

#[test]
fn session_open_takes_a_library_folder_inside_the_allowed_roots() {
    let dir = fresh("open");
    let lib = dir.join("Library");
    let mut s = server();
    let v = ok(&mut s, "session_open", json!({"library": text(&lib)}));
    let shown = v["library"].as_str().unwrap().replace('\\', "/");
    assert!(
        shown.ends_with("mcp-session-library/open/Library"),
        "{shown}"
    );
    // Without one the session has none: nothing is saved anywhere by accident.
    let v = ok(&mut s, "session_open", json!({}));
    assert_eq!(v["library"], Value::Null);
    let outside = std::env::temp_dir().join("atomik-mcp-outside-library");
    let message = fails(&mut s, "session_open", json!({"library": text(&outside)}));
    assert!(message.contains("outside the allowed folders"), "{message}");
    assert!(!outside.exists(), "nothing was made outside the roots");
    let message = fails(&mut s, "session_open", json!({"library": ""}));
    assert!(!message.is_empty());
}

// ------------------------------------------------------------------------- session_library

#[test]
fn session_library_checks_its_arguments_and_needs_a_session_and_a_library() {
    let mut s = server();
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "save_selection"}),
    );
    assert!(message.contains("session_open"), "{message}");
    ok(&mut s, "session_open", json!({}));
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "save_selection"}),
    );
    assert!(message.contains("library"), "no library folder: {message}");
    let dir = fresh("args");
    ok(
        &mut s,
        "session_open",
        json!({"library": text(&dir.join("Library"))}),
    );
    assert!(fails(&mut s, "session_library", json!({})).contains("action"));
    assert!(fails(&mut s, "session_library", json!({"action": "zap"})).contains("save_selection"));
    assert!(fails(&mut s, "session_library", json!({"action": "load"})).contains("path"));
    assert!(
        fails(
            &mut s,
            "session_library",
            json!({"action": "save_clip", "path": "x"})
        )
        .contains("path")
    );
    assert!(
        fails(
            &mut s,
            "session_library",
            json!({"action": "load", "path": "atomik-device:atomik.arpeggiator", "slot": "one"})
        )
        .contains("slot")
    );
    assert!(
        fails(
            &mut s,
            "session_library",
            json!({"action": "save_selection", "devices": ["nobody"]})
        )
        .contains("nobody")
    );
}

#[test]
fn session_library_saves_the_selected_device_and_loads_it_back_as_one_undo_step() {
    let dir = fresh("device");
    let (file, _) = project(&dir);
    let lib = dir.join("Library");
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": file, "library": text(&lib)}),
    );
    let saved = ok(
        &mut s,
        "session_library",
        json!({"action": "save_selection", "devices": ["echo"]}),
    );
    assert_eq!(saved["saved"], "Patches/Stereo Delay.atomikpatch");
    assert_eq!(saved["section"], "patches");
    assert!(lib.join("Patches/Stereo Delay.atomikpatch").is_file());
    assert_eq!(saved["dirty"], false, "saving is not an edit");
    // The stateless tool lists it.
    let listed = ok(&mut s, "library_list", json!({"folder": text(&lib)}));
    assert_eq!(listed["count"], 1);

    let status = ok(&mut s, "session_status", json!({}));
    let before = device_keys(&status);
    let loaded = ok(
        &mut s,
        "session_library",
        json!({"action": "load", "path": saved["saved"]}),
    );
    assert_eq!(loaded["dirty"], true);
    assert_eq!(loaded["can_undo"], true);
    let after = device_keys(&ok(&mut s, "session_status", json!({})));
    assert_eq!(after.len(), before.len() + 1, "{after:?}");

    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let undone = device_keys(&ok(&mut s, "session_status", json!({})));
    assert_eq!(undone, before, "one undo step");
}

#[test]
fn session_library_saves_two_devices_as_one_patch_and_loads_it_into_a_slot() {
    let dir = fresh("patch");
    let (file, _) = project(&dir);
    let lib = dir.join("Library");
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": file, "library": text(&lib)}),
    );
    let saved = ok(
        &mut s,
        "session_library",
        json!({"action": "save_selection", "devices": ["lead", "echo"]}),
    );
    assert_eq!(saved["saved"], "Patches/Poly Synth+1.atomikpatch");
    let before = device_keys(&ok(&mut s, "session_status", json!({}))).len();
    ok(
        &mut s,
        "session_library",
        json!({"action": "load", "path": saved["saved"], "slot": "1:0"}),
    );
    let after = ok(&mut s, "session_status", json!({}));
    // The two devices, and the console channel the copied synth gets.
    assert!(device_keys(&after).len() >= before + 2, "{after}");
    let types: Vec<&str> = after["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["type_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        types
            .iter()
            .filter(|t| **t == "atomik.stereo-delay")
            .count(),
        2
    );
    let columns: Vec<i64> = after["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["column"].as_i64().unwrap())
        .collect();
    assert!(columns.iter().any(|c| *c >= 1), "a new column: {columns:?}");
}

#[test]
fn session_library_adds_a_built_in_device_by_its_row() {
    let dir = fresh("row");
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"library": text(&dir.join("Library"))}),
    );
    let before = device_keys(&ok(&mut s, "session_status", json!({}))).len();
    ok(
        &mut s,
        "session_library",
        json!({"action": "load", "path": "atomik-device:atomik.arpeggiator"}),
    );
    let after = device_keys(&ok(&mut s, "session_status", json!({}))).len();
    assert_eq!(after, before + 1);
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "load", "path": "atomik-device:atomik.nothing"}),
    );
    assert!(message.contains("atomik.nothing"), "{message}");
}

#[test]
fn session_library_saves_a_clip_and_loads_it_on_a_track_at_a_tick() {
    let dir = fresh("clip");
    let (file, clip_id) = project(&dir);
    let lib = dir.join("Library");
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": file, "library": text(&lib)}),
    );
    let saved = ok(
        &mut s,
        "session_library",
        json!({"action": "save_clip", "clip": clip_id}),
    );
    assert_eq!(saved["saved"], "Clips/Poly Synth bar 3.atomikclip");
    assert!(lib.join("Clips/Poly Synth bar 3.atomikclip").is_file());
    let listed = ok(
        &mut s,
        "library_list",
        json!({"folder": text(&lib), "section": "clips"}),
    );
    assert_eq!(listed["items"][0]["detail"], "1 bar, 1 note");

    let before = clips_on_first_track(&ok(&mut s, "session_status", json!({})));
    ok(
        &mut s,
        "session_library",
        json!({"action": "load", "path": saved["saved"], "track": "lead", "lane": 0, "tick": 8 * BAR}),
    );
    let after = clips_on_first_track(&ok(&mut s, "session_status", json!({})));
    assert_eq!(after, before + 1);
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(
        clips_on_first_track(&ok(&mut s, "session_status", json!({}))),
        before
    );
}

#[test]
fn session_library_a_clip_with_no_track_asks_for_one_and_a_project_is_left_to_session_open() {
    let dir = fresh("refuse");
    let (file, clip_id) = project(&dir);
    let lib = dir.join("Library");
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": file.clone(), "library": text(&lib)}),
    );
    let saved = ok(
        &mut s,
        "session_library",
        json!({"action": "save_clip", "clip": clip_id}),
    );
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "load", "path": saved["saved"]}),
    );
    assert!(message.contains("Select an instrument track"), "{message}");
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "load", "path": file}),
    );
    assert!(message.contains("session_open"), "{message}");
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "save_clip", "clip": 9999}),
    );
    assert!(message.contains("clip"), "{message}");
    // Nothing selected: exactly one clip is needed.
    ok(
        &mut s,
        "session_open",
        json!({"path": file, "library": text(&lib)}),
    );
    let message = fails(&mut s, "session_library", json!({"action": "save_clip"}));
    assert!(message.contains("exactly one"), "{message}");
}

#[test]
fn session_library_load_stays_inside_the_allowed_folders() {
    let dir = fresh("paths");
    let lib = dir.join("Library");
    let mut s = server();
    ok(&mut s, "session_open", json!({"library": text(&lib)}));
    let outside = std::env::temp_dir().join("atomik-mcp-outside.atomikpatch");
    std::fs::write(&outside, "{}").unwrap();
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "load", "path": text(&outside)}),
    );
    assert!(message.contains("outside the allowed folders"), "{message}");
    let _ = std::fs::remove_file(&outside);
    let message = fails(
        &mut s,
        "session_library",
        json!({"action": "load", "path": "Patches/Gone.atomikpatch"}),
    );
    assert!(message.contains("Gone"), "{message}");
}
