//! Patch browsing in the session: Browse puts the file browser on a device, stepping through 50
//! patches with the keys makes no undo entry, Return accepts one, and Escape puts the device
//! back.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use atomik_model::{
    Command, DeviceKey, DeviceSnapshot, Document, empty_doc, save_device_item, save_project,
};
use atomik_render::default_registry;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SYNTH: &str = "atomik.poly-synth";

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

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A project with the synth `lead` and a library whose Patches folder holds `P1..P50` (cutoff
/// `100 * i`).
fn setup(name: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("mcp-session-patches")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let patches = dir.join("Library").join("Patches");
    std::fs::create_dir_all(&patches).unwrap();
    let r = default_registry();
    let mut d = Document::new(empty_doc());
    d.apply(
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.to_owned(),
            key: Some(key("src")),
        },
        &r,
    )
    .unwrap();
    for i in 1..=50 {
        d.apply(
            Command::SetParam {
                key: key("src"),
                param: "filter.cutoff".to_owned(),
                value: 100.0 * i as f32,
                merge: None,
            },
            &r,
        )
        .unwrap();
        let snapshot = DeviceSnapshot::capture(d.doc(), &[key("src")], false).unwrap();
        std::fs::write(
            patches.join(format!("P{i}.atomikpatch")),
            save_device_item(&snapshot, d.doc()),
        )
        .unwrap();
    }
    let mut p = Document::new(empty_doc());
    p.apply(
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.to_owned(),
            key: Some(key("lead")),
        },
        &r,
    )
    .unwrap();
    let project = dir.join("Song.atomik");
    std::fs::write(&project, save_project(p.doc())).unwrap();
    (project, dir.join("Library"))
}

fn lead(s: &mut Server) -> (Option<f64>, bool, Value) {
    let v = ok(s, "session_status", json!({"include_params": true}));
    let device = v["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["key"] == "lead")
        .unwrap()
        .clone();
    (
        device["params"]["filter.cutoff"].as_f64(),
        v["can_undo"].as_bool().unwrap(),
        device,
    )
}

fn press(s: &mut Server, k: &str, modifiers: &[&str]) {
    ok(s, "session_key", json!({"key": k, "modifiers": modifiers}));
}

fn on_screen(s: &mut Server, what: &str) -> bool {
    let v = ok(s, "session_texts", json!({"contains": what, "limit": 20}));
    !v["texts"].as_array().unwrap().is_empty()
}

/// Opens the session on the project and aims the patch browser at `lead` (selected through the
/// library tool, which does not edit anything, then Browse Patches by its chord).
fn browse(name: &str) -> (Server, Option<f64>) {
    let (project, library) = setup(name);
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": text(&project), "library": text(&library)}),
    );
    let (original, can_undo, _) = lead(&mut s);
    assert!(!can_undo);
    ok(
        &mut s,
        "session_library",
        json!({"action": "save_selection", "devices": ["lead"]}),
    );
    // The tool saved the selection as a patch of its own: only P1..P50 are wanted here.
    for entry in std::fs::read_dir(library.join("Patches")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        if !(name.starts_with('P') && name[1..].parse::<u32>().is_ok()) {
            std::fs::remove_file(path).unwrap();
        }
    }
    press(&mut s, "b", &["ctrl", "shift"]);
    assert!(
        on_screen(&mut s, "PATCHES FOR"),
        "the band names the device"
    );
    (s, original)
}

#[test]
fn stepping_fifty_patches_makes_no_undo_entry_and_return_accepts_one() {
    let (mut s, original) = browse("commit");
    for i in 1..=50 {
        press(&mut s, "Right", &["ctrl"]);
        let (cutoff, can_undo, _) = lead(&mut s);
        assert_eq!(cutoff, Some(100.0 * f64::from(i)), "step {i}");
        assert!(!can_undo, "step {i} made an undo entry");
    }
    let (_, _, device) = lead(&mut s);
    assert_eq!(device["patch"], "P50");

    press(&mut s, "Enter", &[]);
    let (cutoff, can_undo, _) = lead(&mut s);
    assert_eq!(cutoff, Some(5000.0));
    assert!(can_undo, "Return made the one undo step");
    assert!(!on_screen(&mut s, "PATCHES FOR"), "the browse ended");
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let (cutoff, can_undo, device) = lead(&mut s);
    assert_eq!(cutoff, original);
    assert!(!can_undo, "one undo undid the whole browse");
    assert!(device.get("patch").is_none());
}

#[test]
fn escape_puts_the_device_back() {
    let (mut s, original) = browse("escape");
    for _ in 0..7 {
        press(&mut s, "Right", &["ctrl"]);
    }
    assert_eq!(lead(&mut s).0, Some(700.0));
    press(&mut s, "Escape", &[]);
    let (cutoff, can_undo, device) = lead(&mut s);
    assert_eq!(cutoff, original);
    assert!(!can_undo);
    assert!(device.get("patch").is_none());
    assert!(!on_screen(&mut s, "PATCHES FOR"));
    // Previous goes back through the folder from the start and wraps to the last patch.
    press(&mut s, "b", &["ctrl", "shift"]);
    press(&mut s, "Left", &["ctrl"]);
    assert_eq!(
        lead(&mut s).0,
        Some(5000.0),
        "previous wraps to the last patch"
    );
}
