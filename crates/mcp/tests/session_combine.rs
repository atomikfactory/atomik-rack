//! Combine, Uncombine and the container bypass in the session: the Edit menu commands run on
//! the selection the pointer made, `session_status` lists what is inside a container and whether
//! it is bypassed, and `session_set_param` switches the bypass, which a save keeps.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use atomik_model::{Command, DeviceKey, Document, empty_doc, save_project};
use atomik_render::default_registry;
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

fn ok(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, name, args);
    assert!(!r.is_error, "{name}: {}", r.first_text());
    r.parsed()
}

fn fresh(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("mcp-session-combine")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A project with a delay `echo` and a reverb `verb`, uncabled.
fn project(dir: &Path) -> String {
    let r = default_registry();
    let mut d = Document::new(empty_doc());
    for (i, (type_id, k)) in [("atomik.stereo-delay", "echo"), ("atomik.reverb", "verb")]
        .into_iter()
        .enumerate()
    {
        d.apply(
            Command::AddDevice {
                index: i,
                type_id: type_id.to_owned(),
                key: Some(DeviceKey(k.to_owned())),
            },
            &r,
        )
        .unwrap();
    }
    let path = dir.join("Rack.atomik");
    std::fs::write(&path, save_project(d.doc())).unwrap();
    path.to_string_lossy().into_owned()
}

fn command_row(s: &mut Server, id: &str) -> Value {
    let v = ok(s, "session_commands", json!({"query": id}));
    v["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no command {id}: {v}"))
        .clone()
}

fn text_pos(s: &mut Server, text: &str) -> (f64, f64) {
    let v = ok(s, "session_texts", json!({"contains": text, "limit": 1000}));
    let t = v["texts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["text"] == text)
        .unwrap_or_else(|| panic!("no text {text:?} on screen: {v}"))
        .clone();
    (t["x"].as_f64().unwrap(), t["y"].as_f64().unwrap())
}

fn devices(s: &mut Server) -> Vec<Value> {
    let v = ok(s, "session_status", json!({}));
    v["document"]["devices"].as_array().unwrap().clone()
}

fn container(s: &mut Server) -> Option<Value> {
    devices(s).into_iter().find(|d| d["container"] == true)
}

fn select(s: &mut Server, name: &str) {
    let (x, y) = text_pos(s, name);
    ok(
        s,
        "session_pointer",
        json!({"action": "click", "x": x + 10.0, "y": y - 4.0}),
    );
}

#[test]
fn combine_uncombine_and_bypass_run_as_menu_commands_of_the_session() {
    let dir = fresh("menu");
    let mut s = server();
    ok(&mut s, "session_open", json!({"path": project(&dir)}));
    // Edit menu items, off while nothing is selected.
    for id in ["rack.combine", "rack.uncombine"] {
        let row = command_row(&mut s, id);
        assert_eq!(row["menu"], "Edit", "{id}");
        assert_eq!(row["enabled"], false, "{id}: nothing is selected");
    }
    assert!(
        command_row(&mut s, "rack.combine")["shortcut"]
            .as_str()
            .is_some_and(|k| k.ends_with("Shift+G"))
    );
    select(&mut s, "Reverb");
    assert_eq!(command_row(&mut s, "rack.combine")["enabled"], true);
    assert_eq!(command_row(&mut s, "rack.uncombine")["enabled"], false);
    ok(&mut s, "session_command", json!({"id": "rack.combine"}));
    let c = container(&mut s).expect("a container");
    assert_eq!(c["inside"].as_array().unwrap().len(), 1);
    assert_eq!(c["inside"][0]["type_id"], "atomik.reverb");
    assert!(c["bypassed"].is_null(), "off until switched");
    assert_eq!(
        c["unfolded"], true,
        "a combine leaves the container unfolded"
    );
    // The container is selected now: Uncombine is on, and it is one undo step each way.
    assert_eq!(command_row(&mut s, "rack.uncombine")["enabled"], true);
    ok(&mut s, "session_command", json!({"id": "rack.uncombine"}));
    assert!(container(&mut s).is_none());
    assert_eq!(devices(&mut s).len(), 2);
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert!(container(&mut s).is_some(), "undo of the uncombine");
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert!(container(&mut s).is_none(), "undo of the combine");
}

#[test]
fn the_bypass_is_set_by_session_set_param_listed_in_the_status_and_kept_by_a_save() {
    let dir = fresh("bypass");
    let mut s = server();
    ok(&mut s, "session_open", json!({"path": project(&dir)}));
    select(&mut s, "Reverb");
    ok(&mut s, "session_command", json!({"id": "rack.combine"}));
    let key = container(&mut s).unwrap()["key"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        &mut s,
        "session_set_param",
        json!({"device": key, "param": "bypass", "value": 1}),
    );
    assert_eq!(container(&mut s).unwrap()["bypassed"], true);
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert!(container(&mut s).unwrap()["bypassed"].is_null());
    // The bypass survives a save and a reload.
    ok(
        &mut s,
        "session_set_param",
        json!({"device": key, "param": "bypass", "value": 1}),
    );
    let saved = dir.join("Saved.atomik").to_string_lossy().into_owned();
    ok(&mut s, "session_save", json!({"path": saved}));
    let inspected = ok(&mut s, "project_inspect", json!({"path": saved}));
    let listed = inspected["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["container"] == true)
        .expect("the container");
    assert_eq!(listed["bypassed"], true);
    assert_eq!(listed["inside"][0]["key"], "verb");
    ok(&mut s, "session_close", json!({}));
}
