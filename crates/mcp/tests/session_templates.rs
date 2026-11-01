//! Templates and song information in the session: the templates folder beside the session's
//! library, the template commands of the File menu, opening one as an untitled song that is never
//! overwritten, and the song information in `project_inspect` and `session_status`.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use atomik_model::{Command, DeviceKey, Document, SongInfo, empty_doc, save_project};
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

/// A fresh folder under `target/tmp` (inside the allowed root).
fn fresh(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("mcp-session-templates")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A project with a synth called `lead` and the given title (none: no information).
fn project(title: Option<&str>) -> String {
    let r = default_registry();
    let mut d = Document::new(empty_doc());
    d.apply(
        Command::AddDevice {
            index: 0,
            type_id: "atomik.poly-synth".to_owned(),
            key: Some(DeviceKey("lead".to_owned())),
        },
        &r,
    )
    .unwrap();
    if let Some(title) = title {
        d.apply(
            Command::SetSongInfo(SongInfo {
                title: title.to_owned(),
                author: "Example Author".to_owned(),
                notes: "line one\nline two".to_owned(),
                ..SongInfo::default()
            }),
            &r,
        )
        .unwrap();
    }
    save_project(d.doc())
}

fn device_keys(status: &Value) -> Vec<String> {
    status["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_session_lists_its_templates_and_opens_one_untitled_without_touching_it() {
    let dir = fresh("open");
    let templates = dir.join("Templates");
    std::fs::create_dir_all(&templates).unwrap();
    let template = templates.join("Lead Synth.atomik");
    std::fs::write(&template, project(None)).unwrap();
    let bytes = std::fs::read(&template).unwrap();

    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"library": text(&dir.join("Library"))}),
    );
    let status = ok(&mut s, "session_status", json!({}));
    assert!(
        status["templates_folder"]
            .as_str()
            .unwrap()
            .replace('\\', "/")
            .ends_with("mcp-session-templates/open/Templates"),
        "{status}"
    );
    let list = status["templates"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{status}");
    assert_eq!(list[0]["name"], "Lead Synth");
    assert_eq!(list[0]["index"], 0);

    // The menu lists it as a command, and running it opens an untitled song.
    let commands = ok(
        &mut s,
        "session_commands",
        json!({"query": "file.new_template"}),
    );
    let row = &commands["commands"].as_array().unwrap()[0];
    assert_eq!(row["label"], "Lead Synth", "{row}");
    assert_eq!(row["id"], "file.new_template:0");
    let done = ok(
        &mut s,
        "session_command",
        json!({"id": "file.new_template:0"}),
    );
    assert!(done["requests"].as_array().unwrap().is_empty(), "{done}");
    let status = ok(&mut s, "session_status", json!({}));
    assert!(
        device_keys(&status).contains(&"lead".to_owned()),
        "{status}"
    );
    assert_eq!(status["dirty"], false);
    assert!(
        status["title"].as_str().unwrap().ends_with("Untitled"),
        "{status}"
    );

    // Save asks for a name; Save As elsewhere leaves the template as it was.
    let done = ok(&mut s, "session_command", json!({"id": "file.save"}));
    assert!(!done["requests"].as_array().unwrap().is_empty(), "{done}");
    let out = dir.join("made.atomik");
    ok(&mut s, "session_save", json!({"path": text(&out)}));
    assert!(out.is_file());
    assert_eq!(std::fs::read(&template).unwrap(), bytes);
}

#[test]
fn a_session_without_a_library_has_no_templates_folder() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["templates_folder"], Value::Null);
    assert!(status["templates"].as_array().unwrap().is_empty());
    // The command that makes the folder is not a way to write outside the roots.
    let r = call(
        &mut s,
        "session_command",
        json!({"id": "file.show_templates"}),
    );
    assert!(!r.is_error, "{}", r.first_text());
}

#[test]
fn save_as_template_adds_to_the_list_of_the_session() {
    let dir = fresh("save");
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"library": text(&dir.join("Library"))}),
    );
    ok(
        &mut s,
        "session_command",
        json!({"id": "file.save_template"}),
    );
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["templates"].as_array().unwrap().len(), 1, "{status}");
    assert_eq!(status["templates"][0]["name"], "Untitled");
    assert!(dir.join("Templates").join("Untitled.atomik").is_file());
}

#[test]
fn project_inspect_and_the_session_show_the_song_information() {
    let dir = fresh("info");
    let with = dir.join("with.atomik");
    let without = dir.join("without.atomik");
    std::fs::write(&with, project(Some("Night Drive"))).unwrap();
    std::fs::write(&without, project(None)).unwrap();
    let mut s = server();
    let v = ok(&mut s, "project_inspect", json!({"path": text(&with)}));
    assert_eq!(v["info"]["title"], "Night Drive", "{v}");
    assert_eq!(v["info"]["author"], "Example Author");
    assert_eq!(v["info"]["notes"], "line one\nline two");
    assert!(
        v["info"].get("email").is_none(),
        "only the texts that are set: {v}"
    );
    let v = ok(&mut s, "project_inspect", json!({"path": text(&without)}));
    assert!(v.get("info").is_none(), "{v}");
    // An open session shows it in its status and in its window title.
    let opened = ok(&mut s, "session_open", json!({"path": text(&with)}));
    assert_eq!(
        opened["document"]["info"]["title"], "Night Drive",
        "{opened}"
    );
    assert!(
        opened["title"].as_str().unwrap().contains("Night Drive"),
        "{opened}"
    );
}
