//! The library in the stateless tools: `.atomikpatch`, `.atomikcontainer` and `.atomikclip`
//! files in `validate` and `project_inspect`, and `library_list` over a library folder.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use atomik_model::{
    ClipFile, Command, ContainerCommand, DeviceKey, DeviceSnapshot, Document, empty_doc,
    save_clip_file, save_device_item, save_project,
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

/// A fresh folder under `target/tmp` (inside the allowed root).
fn fresh(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("mcp-library")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) -> String {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    path.to_string_lossy().into_owned()
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// A synth into a delay, and a container with two devices inside.
fn rack() -> Document {
    let r = default_registry();
    let mut d = Document::new(empty_doc());
    let add = |index, type_id: &str, k: &str| Command::AddDevice {
        index,
        type_id: type_id.to_owned(),
        key: Some(key(k)),
    };
    let inner = |type_id: &str, k: &str, index| {
        Command::Container(ContainerCommand::AddInner {
            container: key("layer"),
            index,
            type_id: type_id.to_owned(),
            key: Some(key(k)),
            auto_split: false,
        })
    };
    for c in [
        add(0, "atomik.poly-synth", "lead"),
        add(1, "atomik.stereo-delay", "echo"),
        add(2, "atomik.container", "layer"),
        inner("atomik.poly-synth", "lo", 0),
        inner("atomik.stereo-delay", "fx", 1),
    ] {
        d.apply(c, &r).unwrap();
    }
    d
}

fn item(d: &Document, keys: &[&str]) -> String {
    let keys: Vec<DeviceKey> = keys.iter().map(|k| key(k)).collect();
    let s = DeviceSnapshot::capture(d.doc(), &keys, false).unwrap();
    save_device_item(&s, d.doc())
}

fn clip(bars: i64, notes: i64) -> String {
    let notes = (0..notes)
        .map(|i| atomik_model::Note::new(i * 3_840, 3_840, 60, 0.8))
        .collect();
    let clip = atomik_model::Clip {
        id: atomik_model::ClipId(0),
        start: 0,
        length: bars * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        content: atomik_model::ClipContent::Notes(notes),
    };
    save_clip_file(&ClipFile {
        tempo_bpm: 120.0,
        meter: (4, 4),
        clip,
    })
    .unwrap()
}

/// A library folder: two patches (one in a sub-folder), a container, a clip and a project.
fn library(name: &str) -> PathBuf {
    let root = fresh(name);
    let d = rack();
    write(
        &root.join("Patches/Synths/Lead Echo.atomikpatch"),
        &item(&d, &["lead", "echo"]),
    );
    write(&root.join("Patches/Solo.atomikpatch"), &item(&d, &["lead"]));
    write(
        &root.join("Containers/Layer.atomikcontainer"),
        &item(&d, &["layer"]),
    );
    write(&root.join("Clips/Riff.atomikclip"), &clip(4, 32));
    write(&root.join("Projects/Song.atomik"), &save_project(d.doc()));
    // Not library items.
    write(&root.join("Patches/readme.txt"), "hello");
    root
}

fn rel(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

fn names(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["path"].as_str().unwrap().to_owned())
        .collect()
}

// ---------------------------------------------------------------- validate and project_inspect

#[test]
fn validate_patch_item_is_valid_and_has_the_patch_item_kind() {
    let mut s = server();
    let root = fresh("validate-patch");
    let path = write(
        &root.join("a.atomikpatch"),
        &item(&rack(), &["lead", "echo"]),
    );
    let v = ok(&mut s, "validate", json!({"path": path}));
    assert_eq!(v["valid"], true, "{v}");
    assert_eq!(v["kind"], "patch_item");
}

#[test]
fn validate_container_item_is_valid_and_has_the_container_item_kind() {
    let mut s = server();
    let root = fresh("validate-container");
    let path = write(&root.join("A.ATOMIKCONTAINER"), &item(&rack(), &["layer"]));
    let v = ok(&mut s, "validate", json!({"path": path}));
    assert_eq!(v["valid"], true, "{v}");
    assert_eq!(v["kind"], "container_item");
}

#[test]
fn validate_container_item_with_two_devices_is_invalid() {
    let mut s = server();
    let root = fresh("validate-container-two");
    let path = write(
        &root.join("a.atomikcontainer"),
        &item(&rack(), &["lead", "echo"]),
    );
    let v = ok(&mut s, "validate", json!({"path": path}));
    assert_eq!(v["valid"], false);
    assert_eq!(v["kind"], "container_item");
    assert!(
        v["error"].as_str().unwrap().contains("one container"),
        "{v}"
    );
}

#[test]
fn validate_broken_patch_item_names_the_json_path() {
    let mut s = server();
    let root = fresh("validate-broken");
    let text = item(&rack(), &["lead"]).replace("atomik.poly-synth", "atomik.nope");
    let path = write(&root.join("a.atomikpatch"), &text);
    let v = ok(&mut s, "validate", json!({"path": path}));
    assert_eq!(v["valid"], false);
    assert_eq!(v["kind"], "patch_item");
    assert!(v["error"].as_str().unwrap().contains("devices[0]"), "{v}");
}

#[test]
fn validate_clip_is_valid_and_has_the_clip_kind() {
    let mut s = server();
    let root = fresh("validate-clip");
    let path = write(&root.join("a.atomikclip"), &clip(2, 3));
    let v = ok(&mut s, "validate", json!({"path": path}));
    assert_eq!(v["valid"], true, "{v}");
    assert_eq!(v["kind"], "clip");
}

#[test]
fn validate_broken_clip_names_the_json_path() {
    let mut s = server();
    let root = fresh("validate-clip-broken");
    let text = clip(2, 3).replace("\"ppq\": 15360", "\"ppq\": 480");
    let path = write(&root.join("a.atomikclip"), &text);
    let v = ok(&mut s, "validate", json!({"path": path}));
    assert_eq!(v["valid"], false);
    assert_eq!(v["kind"], "clip");
    assert!(v["error"].as_str().unwrap().contains("ppq"), "{v}");
}

#[test]
fn project_inspect_patch_item_describes_the_devices_with_the_item_kind() {
    let mut s = server();
    let root = fresh("inspect-patch");
    let path = write(
        &root.join("a.atomikpatch"),
        &item(&rack(), &["lead", "echo"]),
    );
    let v = ok(&mut s, "project_inspect", json!({"path": path}));
    assert_eq!(v["kind"], "patch_item");
    assert_eq!(v["devices"].as_array().unwrap().len(), 2);
    assert_eq!(v["summary"], "2 devices: Poly Synth, Stereo Delay");
}

#[test]
fn project_inspect_container_item_has_the_container_item_kind() {
    let mut s = server();
    let root = fresh("inspect-container");
    let path = write(&root.join("a.atomikcontainer"), &item(&rack(), &["layer"]));
    let v = ok(&mut s, "project_inspect", json!({"path": path}));
    assert_eq!(v["kind"], "container_item");
    assert_eq!(v["summary"], "Container, 2 inside");
}

#[test]
fn project_inspect_clip_describes_the_clip() {
    let mut s = server();
    let root = fresh("inspect-clip");
    let path = write(&root.join("a.atomikclip"), &clip(4, 32));
    let v = ok(&mut s, "project_inspect", json!({"path": path}));
    assert_eq!(v["kind"], "clip");
    assert_eq!(v["summary"], "4 bars, 32 notes");
    assert_eq!(v["tempo_bpm"], 120.0);
    assert_eq!(v["meter"], json!({"num": 4, "den": 4}));
    assert_eq!(v["clip"]["length"], 4 * BAR);
    assert_eq!(v["clip"]["notes"], 32);
    assert_eq!(v["clip"]["muted"], false);
}

#[test]
fn project_inspect_broken_clip_is_a_failed_call_naming_the_json_path() {
    let mut s = server();
    let root = fresh("inspect-clip-broken");
    let path = write(&root.join("a.atomikclip"), "{\"format\": \"atomik-clip\"}");
    let r = call(&mut s, "project_inspect", json!({"path": path}));
    assert!(r.is_error);
    assert!(r.first_text().contains("version"), "{}", r.first_text());
}

// ---------------------------------------------------------------- library_list

#[test]
fn library_list_lists_every_section_in_order_with_details() {
    let mut s = server();
    let root = library("list-all");
    let v = ok(&mut s, "library_list", json!({"folder": rel(&root)}));
    assert_eq!(
        names(&v),
        [
            "Patches/Solo.atomikpatch",
            "Patches/Synths/Lead Echo.atomikpatch",
            "Containers/Layer.atomikcontainer",
            "Clips/Riff.atomikclip",
            "Projects/Song.atomik",
        ]
    );
    assert_eq!(v["count"], 5);
    let items = v["items"].as_array().unwrap();
    let get = |name: &str| items.iter().find(|i| i["name"] == name).unwrap();
    assert_eq!(get("Solo")["section"], "patches");
    assert_eq!(get("Solo")["kind"], "patch");
    assert_eq!(get("Solo")["detail"], "1 device: Poly Synth");
    assert_eq!(
        get("Lead Echo")["detail"],
        "2 devices: Poly Synth, Stereo Delay"
    );
    assert_eq!(get("Layer")["kind"], "container");
    assert_eq!(get("Layer")["detail"], "Container, 2 inside");
    assert_eq!(get("Riff")["kind"], "clip");
    assert_eq!(get("Riff")["detail"], "4 bars, 32 notes");
    assert_eq!(get("Song")["kind"], "project");
    assert!(get("Song")["detail"].as_str().unwrap().contains("120 BPM"));
    assert!(get("Solo")["size"].as_u64().unwrap() > 0);
}

#[test]
fn library_list_section_lists_only_that_section() {
    let mut s = server();
    let root = library("list-section");
    let v = ok(
        &mut s,
        "library_list",
        json!({"folder": rel(&root), "section": "Clips"}),
    );
    assert_eq!(names(&v), ["Clips/Riff.atomikclip"]);
}

#[test]
fn library_list_query_matches_the_name_in_any_case() {
    let mut s = server();
    let root = library("list-query");
    let v = ok(
        &mut s,
        "library_list",
        json!({"folder": rel(&root), "query": "ECHO"}),
    );
    assert_eq!(names(&v), ["Patches/Synths/Lead Echo.atomikpatch"]);
    let none = ok(
        &mut s,
        "library_list",
        json!({"folder": rel(&root), "query": "zzz"}),
    );
    assert_eq!(none["count"], 0);
}

#[test]
fn library_list_missing_section_folders_are_empty_not_errors() {
    let mut s = server();
    let root = fresh("list-empty");
    let v = ok(&mut s, "library_list", json!({"folder": rel(&root)}));
    assert_eq!(v["count"], 0);
    assert!(v["items"].as_array().unwrap().is_empty());
}

#[test]
fn library_list_unreadable_item_is_listed_without_detail() {
    let mut s = server();
    let root = fresh("list-bad");
    write(&root.join("Patches/Bad.atomikpatch"), "not json");
    let v = ok(&mut s, "library_list", json!({"folder": rel(&root)}));
    assert_eq!(names(&v), ["Patches/Bad.atomikpatch"]);
    assert!(v["items"][0].get("detail").is_none());
}

#[test]
fn library_list_refuses_a_folder_outside_the_allowed_roots() {
    let mut s = server();
    let outside = std::env::temp_dir();
    let r = call(
        &mut s,
        "library_list",
        json!({"folder": outside.to_string_lossy()}),
    );
    assert!(r.is_error);
    assert!(r.first_text().contains("allowed"), "{}", r.first_text());
    let up = call(&mut s, "library_list", json!({"folder": "../.."}));
    assert!(up.is_error, "{}", up.first_text());
}

#[test]
fn library_list_a_file_or_a_missing_folder_is_a_failed_call() {
    let mut s = server();
    let root = library("list-notdir");
    let file = root.join("Clips/Riff.atomikclip");
    let r = call(&mut s, "library_list", json!({"folder": rel(&file)}));
    assert!(r.is_error);
    assert!(
        r.first_text().contains("not a folder"),
        "{}",
        r.first_text()
    );
    let gone = call(
        &mut s,
        "library_list",
        json!({"folder": rel(&root.join("nope"))}),
    );
    assert!(gone.is_error);
}

#[test]
fn library_list_unknown_section_is_a_failed_call_naming_the_choices() {
    let mut s = server();
    let root = library("list-badsection");
    for section in ["tunes", "devices"] {
        let r = call(
            &mut s,
            "library_list",
            json!({"folder": rel(&root), "section": section}),
        );
        assert!(r.is_error, "{section}");
        assert!(r.first_text().contains("patches"), "{}", r.first_text());
    }
}

#[test]
fn library_list_does_not_follow_symbolic_links_out_of_the_library() {
    // Making a link needs privileges on some systems: the test only runs where it can.
    let root = fresh("list-link");
    let outside = fresh("list-link-outside");
    write(
        &outside.join("Secret.atomikpatch"),
        &item(&rack(), &["lead"]),
    );
    std::fs::create_dir_all(root.join("Patches")).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, root.join("Patches/out")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&outside, root.join("Patches/out")).is_ok();
    if !linked {
        return;
    }
    let mut s = server();
    let v = ok(&mut s, "library_list", json!({"folder": rel(&root)}));
    assert_eq!(v["count"], 0, "{v}");
}
