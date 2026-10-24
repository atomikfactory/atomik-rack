//! The session tools: a real headless shell driven through the server.

use atomik_mcp::session::{list_texts, parse_key};
use atomik_mcp::{Args, Content, Paths, Server, ToolOutput};
use atomik_ui::Key;
use atomik_ui::event::NumpadKey;
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

fn tmp(name: &str) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-session");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

fn command_row(s: &mut Server, query: &str) -> Value {
    let v = ok(s, "session_commands", json!({"query": query}));
    v["commands"].as_array().unwrap()[0].clone()
}

#[test]
fn every_session_tool_needs_an_open_session() {
    let mut s = server();
    for (tool, args) in [
        ("session_status", json!({})),
        ("session_commands", json!({})),
        ("session_command", json!({"id": "edit.undo"})),
        ("session_pointer", json!({"action": "move", "x": 1, "y": 1})),
        ("session_key", json!({"key": "a"})),
        ("session_texts", json!({})),
        ("session_screenshot", json!({})),
        ("session_save", json!({"path": "target/tmp/never.atomik"})),
        ("session_relink", json!({"folder": "examples"})),
        (
            "session_midi",
            json!({"action": "export", "file": "target/tmp/never.mid"}),
        ),
        ("session_audio_notes", json!({})),
        (
            "session_set_param",
            json!({"device": "channel", "param": "input.gain", "value": 1}),
        ),
        ("session_meters", json!({})),
        ("session_bounce", json!({})),
    ] {
        let r = call(&mut s, tool, args);
        assert!(r.is_error, "{tool}");
        assert!(
            r.first_text().contains("session_open"),
            "{tool}: {}",
            r.first_text()
        );
    }
    // Closing nothing is fine.
    assert_eq!(ok(&mut s, "session_close", json!({}))["closed"], false);
}

#[test]
fn a_session_opens_the_starter_project_and_closes() {
    let mut s = server();
    let v = ok(&mut s, "session_open", json!({}));
    assert_eq!(v["title"], "Atomik Rack — Untitled");
    assert_eq!(v["dirty"], false);
    assert_eq!(v["can_undo"], false);
    assert_eq!(v["document"]["devices"].as_array().unwrap().len(), 1);
    assert_eq!(v["window"]["logical"], json!([1280.0, 860.0]));
    assert!(v["status_text"].as_str().unwrap().contains("audio off"));
    assert_eq!(ok(&mut s, "session_close", json!({}))["closed"], true);
    assert!(call(&mut s, "session_status", json!({})).is_error);
}

#[test]
fn a_session_opens_a_file_and_refuses_a_bad_one_keeping_the_old_session() {
    let mut s = server();
    let v = ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    assert!(v["title"].as_str().unwrap().contains("demo"), "{v}");
    assert_eq!(v["document"]["song"]["tracks"].as_array().unwrap().len(), 2);

    let bad = tmp("bad.atomik");
    std::fs::write(
        &bad,
        r#"{"format":"atomik-project","version":10,"devices":"nope"}"#,
    )
    .unwrap();
    let r = call(&mut s, "session_open", json!({"path": bad}));
    assert!(r.is_error);
    // The earlier session is untouched.
    let still = ok(&mut s, "session_status", json!({}));
    assert!(still["title"].as_str().unwrap().contains("demo"));

    let outside = call(
        &mut s,
        "session_open",
        json!({"path": std::env::temp_dir().to_string_lossy()}),
    );
    assert!(outside.is_error);
    assert!(outside.first_text().contains("outside the allowed folders"));

    assert!(call(&mut s, "session_open", json!({"os": "beos"})).is_error);
    assert!(call(&mut s, "session_open", json!({"width": 10})).is_error);
    assert!(call(&mut s, "session_open", json!({"scale": 9})).is_error);
}

#[test]
fn commands_list_the_menus_and_toggles_flip() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    let all = ok(&mut s, "session_commands", json!({}));
    let ids: Vec<&str> = all["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    for id in [
        "file.new",
        "edit.undo",
        "edit.redo",
        "view.toggle_browser",
        "transport.add_marker",
    ] {
        assert!(ids.contains(&id), "{id} in {ids:?}");
    }
    let undo = command_row(&mut s, "edit.undo");
    assert_eq!(undo["enabled"], false);
    assert_eq!(undo["shortcut"], "Ctrl+Z");
    let enabled = ok(&mut s, "session_commands", json!({"enabled_only": true}));
    assert!(
        enabled["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["enabled"] == true)
    );
    let file = ok(&mut s, "session_commands", json!({"menu": "file"}));
    assert!(
        file["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["menu"] == "File")
    );

    assert_eq!(command_row(&mut s, "view.toggle_browser")["checked"], false);
    ok(
        &mut s,
        "session_command",
        json!({"id": "view.toggle_browser"}),
    );
    assert_eq!(command_row(&mut s, "view.toggle_browser")["checked"], true);
    ok(
        &mut s,
        "session_command",
        json!({"id": "view.toggle_browser"}),
    );
    assert_eq!(command_row(&mut s, "view.toggle_browser")["checked"], false);

    let missing = call(&mut s, "session_command", json!({"id": "no.such"}));
    assert!(missing.is_error);
    let disabled = call(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert!(disabled.is_error);
    assert!(disabled.first_text().contains("disabled"));
}

#[test]
fn an_edit_is_undone_by_command_and_by_chord_and_new_asks_before_discarding() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let before = ok(&mut s, "session_status", json!({}));
    assert_eq!(before["dirty"], false);
    assert!(
        before["document"]["song"]["markers"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    ok(
        &mut s,
        "session_command",
        json!({"id": "transport.add_marker"}),
    );
    let after = ok(&mut s, "session_status", json!({}));
    assert_eq!(after["dirty"], true);
    assert_eq!(after["can_undo"], true);
    assert_eq!(
        after["document"]["song"]["markers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let undone = ok(&mut s, "session_status", json!({}));
    assert_eq!(undone["dirty"], false);
    assert!(
        undone["document"]["song"]["markers"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Redo by its chord: the keymap, not the command table, decides.
    ok(
        &mut s,
        "session_key",
        json!({"key": "y", "modifiers": ["ctrl"]}),
    );
    let redone = ok(&mut s, "session_status", json!({}));
    assert_eq!(
        redone["document"]["song"]["markers"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{redone}"
    );

    // File, New over unsaved changes is a question, not an action.
    let asked = ok(&mut s, "session_command", json!({"id": "file.new"}));
    assert_eq!(
        asked["requests"][0]["request"], "confirm_discard",
        "{asked}"
    );
    assert_eq!(
        ok(&mut s, "session_status", json!({}))["document"]["song"]["tracks"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "nothing was discarded"
    );
    let done = ok(
        &mut s,
        "session_command",
        json!({"id": "file.new", "confirm": true}),
    );
    assert!(done["requests"].as_array().unwrap().is_empty(), "{done}");
    let fresh = ok(&mut s, "session_status", json!({}));
    assert_eq!(fresh["document"]["devices"].as_array().unwrap().len(), 1);
    assert_eq!(fresh["dirty"], false);
}

#[test]
fn a_command_that_needs_a_dialog_says_so() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    let r = ok(&mut s, "session_command", json!({"id": "file.open"}));
    assert_eq!(r["requests"][0]["request"], "pick_open_file", "{r}");
    assert!(
        r["requests"][0]["note"]
            .as_str()
            .unwrap()
            .contains("session_open")
    );
}

#[test]
fn texts_read_the_screen_and_follow_a_toggle() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let all = ok(&mut s, "session_texts", json!({"limit": 1000}));
    assert!(all["total"].as_u64().unwrap() > 20);
    let texts: Vec<&str> = all["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["text"].as_str().unwrap())
        .collect();
    for expected in ["File", "Bass", "Lead", "120.00"] {
        assert!(texts.contains(&expected), "{expected} in {texts:?}");
    }
    let bass = ok(&mut s, "session_texts", json!({"contains": "bass"}));
    assert!(bass["total"].as_u64().unwrap() >= 1);
    assert!(
        bass["texts"].as_array().unwrap().iter().all(|t| t["text"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("bass"))
    );
    let limited = ok(&mut s, "session_texts", json!({"limit": 3}));
    assert_eq!(limited["shown"], 3);
    assert!(limited["total"].as_u64().unwrap() > 3);

    // Positions are logical pixels, so they do not depend on the scale.
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik", "scale": 2}),
    );
    let hi = ok(
        &mut s,
        "session_texts",
        json!({"contains": "Bass", "limit": 1}),
    );
    let lo_pos = {
        ok(
            &mut s,
            "session_open",
            json!({"path": "examples/projects/demo.atomik", "scale": 1}),
        );
        ok(
            &mut s,
            "session_texts",
            json!({"contains": "Bass", "limit": 1}),
        )
    };
    assert_eq!(hi["texts"][0]["x"], lo_pos["texts"][0]["x"]);
    assert_eq!(hi["texts"][0]["y"], lo_pos["texts"][0]["y"]);
}

#[test]
fn a_screenshot_is_a_png_of_the_window_at_its_scale() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"width": 800, "height": 600, "scale": 2}),
    );
    let path = tmp("shot.png");
    let r = call(&mut s, "session_screenshot", json!({"save_to": path}));
    assert!(!r.is_error, "{}", r.first_text());
    let info = r.parsed();
    assert_eq!(info["width"], 1600);
    assert_eq!(info["height"], 1200);
    let png = r
        .content
        .iter()
        .find_map(|c| match c {
            Content::Png(b) => Some(b.clone()),
            Content::Text(_) => None,
        })
        .expect("an image block");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    // IHDR holds the size big-endian at bytes 16..24.
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 1600);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 1200);
    assert_eq!(std::fs::read(&path).unwrap(), png);
    // The picture is not blank: more than one distinct pixel value is in the data.
    assert!(png.len() > 5_000, "{} bytes", png.len());
}

#[test]
fn a_screenshot_changes_when_the_ui_changes() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let shot = |s: &mut Server| {
        call(s, "session_screenshot", json!({}))
            .content
            .iter()
            .find_map(|c| match c {
                Content::Png(b) => Some(b.clone()),
                Content::Text(_) => None,
            })
            .unwrap()
    };
    let a = shot(&mut s);
    assert_eq!(a, shot(&mut s), "painting is deterministic");
    ok(
        &mut s,
        "session_command",
        json!({"id": "view.toggle_browser"}),
    );
    assert_ne!(a, shot(&mut s), "the browser panel is drawn");
}

#[test]
fn pointer_and_key_input_reach_the_shell() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    for args in [
        json!({"action": "move", "x": 400, "y": 300}),
        json!({"action": "click", "x": 400, "y": 300}),
        json!({"action": "double_click", "x": 640, "y": 600}),
        json!({"action": "drag", "x": 300, "y": 600, "to_x": 500, "to_y": 600, "steps": 4}),
        json!({"action": "wheel", "x": 640, "y": 600, "dy": -1}),
        json!({"action": "click", "x": 640, "y": 600, "button": "secondary"}),
        json!({"action": "leave"}),
    ] {
        let v = ok(&mut s, "session_pointer", args.clone());
        assert!(v["requests"].as_array().is_some(), "{args}");
    }
    // A right click on a clip opened a context menu or closed it again; either way a click
    // elsewhere with Escape leaves none open.
    ok(&mut s, "session_key", json!({"key": "Escape"}));
    assert_eq!(
        ok(&mut s, "session_status", json!({}))["context_menu_open"],
        false
    );

    // Pane under the pointer: the transport bar at the top of the lower pane area.
    let over = ok(
        &mut s,
        "session_pointer",
        json!({"action": "move", "x": 640, "y": 435}),
    );
    assert!(over["pane_under_pointer"].is_string(), "{over}");

    assert!(
        call(
            &mut s,
            "session_pointer",
            json!({"action": "teleport", "x": 1, "y": 1})
        )
        .is_error
    );
    assert!(
        call(
            &mut s,
            "session_pointer",
            json!({"action": "click", "x": 1})
        )
        .is_error
    );
    assert!(
        call(
            &mut s,
            "session_pointer",
            json!({"action": "click", "x": 1, "y": 1, "button": "x"})
        )
        .is_error
    );
    assert!(
        call(
            &mut s,
            "session_pointer",
            json!({"action": "click", "x": 1, "y": 1, "modifiers": ["hyper"]})
        )
        .is_error
    );
}

#[test]
fn a_modified_click_selects_with_its_role() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    // Ctrl+A selects everything in the focused pane; the call must succeed with modifiers.
    let v = ok(
        &mut s,
        "session_key",
        json!({"key": "a", "modifiers": ["cmd"]}),
    );
    assert!(v["status_text"].is_string());
    let v = ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": 400, "y": 560, "modifiers": ["shift", "alt"]}),
    );
    assert!(v["status_text"].is_string());
}

#[test]
fn key_input_takes_a_key_or_text_not_both_or_neither() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    assert!(call(&mut s, "session_key", json!({})).is_error);
    assert!(call(&mut s, "session_key", json!({"key": "a", "text": "b"})).is_error);
    assert!(call(&mut s, "session_key", json!({"key": "NoSuchKey"})).is_error);
    ok(&mut s, "session_key", json!({"text": "abc"}));
    ok(&mut s, "session_key", json!({"key": "F3"}));
    assert_eq!(
        command_row(&mut s, "view.toggle_browser")["checked"],
        true,
        "F3 is its chord"
    );
}

#[test]
fn key_names_parse() {
    assert_eq!(parse_key("Escape").unwrap(), Key::Escape);
    assert_eq!(parse_key("esc").unwrap(), Key::Escape);
    assert_eq!(parse_key("Enter").unwrap(), Key::Enter);
    assert_eq!(parse_key("space").unwrap(), Key::Space);
    assert_eq!(parse_key("PageDown").unwrap(), Key::PageDown);
    assert_eq!(parse_key("F12").unwrap(), Key::F(12));
    assert_eq!(parse_key("f5").unwrap(), Key::F(5));
    assert_eq!(parse_key("Digit3").unwrap(), Key::Digit(3));
    assert_eq!(
        parse_key("Numpad7").unwrap(),
        Key::Numpad(NumpadKey::Digit(7))
    );
    assert_eq!(parse_key("z").unwrap(), Key::Char('z'));
    assert_eq!(parse_key("Z").unwrap(), Key::Char('Z'));
    assert_eq!(parse_key(",").unwrap(), Key::Char(','));
    for bad in ["F25", "F0", "Digit10", "Numpad", "ab", ""] {
        assert!(parse_key(bad).is_err(), "{bad}");
    }
}

#[test]
fn text_positions_follow_the_transform_stack() {
    use atomik_ui::kurbo::{Affine, Point};
    use atomik_ui::peniko::Color;
    use atomik_ui::{Canvas, DisplayList, TextRun};
    let mut list = DisplayList::new();
    list.push_transform(Affine::translate((100.0, 50.0)));
    list.push_transform(Affine::scale(2.0));
    list.text(&TextRun::new("a", Point::new(10.0, 5.0), 9.0, Color::WHITE));
    list.pop_transform();
    list.text(&TextRun::new("b", Point::new(1.0, 1.0), 9.0, Color::WHITE));
    list.pop_transform();
    list.text(&TextRun::new("c", Point::new(3.0, 3.0), 9.0, Color::WHITE));
    let t = list_texts(&list, 2.0);
    // Physical (100 + 2*10, 50 + 2*5) = (120, 60), halved for scale 2.
    assert_eq!((t[0].0.as_str(), t[0].1, t[0].2), ("a", 60.0, 30.0));
    assert_eq!((t[1].0.as_str(), t[1].1, t[1].2), ("b", 50.5, 25.5));
    assert_eq!((t[2].0.as_str(), t[2].1, t[2].2), ("c", 1.5, 1.5));
}

#[test]
fn save_writes_a_project_the_stateless_tools_read_back() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    ok(
        &mut s,
        "session_command",
        json!({"id": "transport.add_marker"}),
    );
    let out = tmp("saved.atomik");
    let saved = ok(&mut s, "session_save", json!({"path": out}));
    assert_eq!(saved["written"], true);
    assert_eq!(saved["dirty"], false);
    assert!(saved["bytes"].as_u64().unwrap() > 100);
    let inspected = ok(&mut s, "project_inspect", json!({"path": out}));
    assert_eq!(inspected["song"]["markers"].as_array().unwrap().len(), 1);
    assert_eq!(inspected["song"]["tracks"].as_array().unwrap().len(), 2);
    let outside = call(
        &mut s,
        "session_save",
        json!({"path": std::env::temp_dir().join("atomik-mcp-x.atomik").to_string_lossy()}),
    );
    assert!(outside.is_error);
}

#[test]
fn midi_files_export_from_one_session_and_import_into_another() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let out = tmp("demo.mid");
    let exported = ok(
        &mut s,
        "session_midi",
        json!({"action": "export", "file": out, "scope": "song"}),
    );
    assert!(exported["bytes"].as_u64().unwrap() > 40);
    assert!(
        exported["message"]
            .as_str()
            .unwrap()
            .starts_with("Exported the song"),
        "{exported}"
    );
    // A clip needs a selection, the song does not.
    let clip = call(
        &mut s,
        "session_midi",
        json!({"action": "export", "file": tmp("clip.mid"), "scope": "clip"}),
    );
    assert_eq!(
        clip.parsed()["message"],
        "select a note clip to export",
        "nothing is written without a selection"
    );
    assert!(clip.parsed()["bytes"].is_null() || clip.parsed()["bytes"] == 0);
    let tracks = ok(&mut s, "session_status", json!({}))["document"]["song"]["tracks"]
        .as_array()
        .unwrap()
        .len();

    // The starter project takes the file as a song: new tracks, tempo and markers, one undo.
    ok(&mut s, "session_open", json!({}));
    let imported = ok(
        &mut s,
        "session_midi",
        json!({"action": "import", "file": out, "as_song": true}),
    );
    assert_eq!(imported["document_changed"], true);
    assert_eq!(imported["can_undo"], true);
    assert!(imported["tracks"].as_u64().unwrap() >= 1);
    assert!(imported["tracks"].as_u64().unwrap() as usize <= tracks);
    assert!(
        imported["message"]
            .as_str()
            .unwrap()
            .starts_with("Imported"),
        "{imported}"
    );
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let after = ok(&mut s, "session_status", json!({}));
    assert_eq!(
        after["document"]["song"]["tracks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Errors: a missing file, a bad action, a scope that does not exist.
    for args in [
        json!({"action": "import", "file": "examples/none.mid"}),
        json!({"action": "import", "file": "README.md"}),
        json!({"action": "sing", "file": out}),
        json!({"action": "export", "file": out, "scope": "bar"}),
    ] {
        let r = call(&mut s, "session_midi", args);
        assert!(r.is_error || r.parsed()["document_changed"] == false);
    }
}

#[test]
fn a_session_bounce_equals_the_bounce_of_the_same_file() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let live = tmp("session.wav");
    let from_session = ok(
        &mut s,
        "session_bounce",
        json!({"output": live, "block": 480}),
    );
    assert_eq!(from_session["audio"]["silent"], false);
    let file = tmp("file.wav");
    ok(
        &mut s,
        "bounce",
        json!({"path": "examples/projects/demo.atomik", "output": file}),
    );
    let cmp = ok(&mut s, "compare_audio", json!({"a": live, "b": file}));
    assert_eq!(cmp["bit_identical"], true, "{cmp}");

    // The bounce follows the edits of the session, saved or not: a muted-everything edit would
    // differ; here a tempo-free edit (a marker) must not change a sample.
    ok(
        &mut s,
        "session_command",
        json!({"id": "transport.add_marker"}),
    );
    let r = ok(
        &mut s,
        "session_bounce",
        json!({"output": tmp("marked.wav"), "reference": live, "tolerance_db": 0.0}),
    );
    assert_eq!(r["comparison"]["bit_identical"], true, "{r}");
}

#[test]
fn a_session_bounce_hears_a_cable_edit() {
    // A cable removed in the session silences what it carried: the bounce reports it.
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let reference = tmp("with-cables.wav");
    ok(&mut s, "session_bounce", json!({"output": reference}));
    // Delete the bass device (select it by clicking its panel title, then Delete).
    let at = ok(
        &mut s,
        "session_texts",
        json!({"contains": "Bass", "limit": 1}),
    );
    let (x, y) = (
        at["texts"][0]["x"].as_f64().unwrap(),
        at["texts"][0]["y"].as_f64().unwrap(),
    );
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x, "y": y - 4.0}),
    );
    ok(&mut s, "session_key", json!({"key": "Delete"}));
    let status = ok(&mut s, "session_status", json!({}));
    let devices = status["document"]["devices"].as_array().unwrap();
    assert!(
        devices.iter().all(|d| d["key"] != "bass"),
        "the bass is gone: {devices:?}"
    );
    let after = ok(
        &mut s,
        "session_bounce",
        json!({"output": tmp("without-bass.wav"), "reference": reference}),
    );
    assert_eq!(after["comparison"]["matches"], false, "{after}");
}

// ------------------------------------------------------------------ relink

/// A saved project with one clip per name in `names` (a ramp each, distinct audio) whose
/// asset files were then deleted; the project file, relative to the repository when it can be.
fn missing_project(tag: &str, names: &[&str]) -> String {
    use atomik_app::AppCore;
    use atomik_app::session::{LiveSession, NullBackend};
    use atomik_app::settings::AudioSettings;
    use atomik_render::wav::{SampleFormat, write_wav_file};
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mcp-relink-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("source")).unwrap();
    let mut core = AppCore::with_doc(
        LiveSession::new(NullBackend, AudioSettings::default(), false),
        atomik_model::empty_doc(),
        None,
    );
    for (i, name) in names.iter().enumerate() {
        let seed = 1.0 / (i as f32 + 1.0);
        let ramp: Vec<f32> = (0..4_800).map(|n| n as f32 / 4_800.0 * seed).collect();
        let file = dir.join("source").join(name);
        write_wav_file(&file, &[ramp], 48_000, SampleFormat::F32).unwrap();
        core.import_audio(vec![file], atomik_ui::ImportTarget::NewTracks, 0);
        core.wait_background();
    }
    core.save_as(&dir.join("song.atomik")).unwrap();
    let audio = dir.join("song").join("Audio");
    for entry in std::fs::read_dir(&audio).unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "wav") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    dir.join("song")
        .join("song.atomik")
        .to_string_lossy()
        .into_owned()
}

/// A replacement ramp under the project's folder, by absolute path.
fn replacement(tag: &str, name: &str, seed: f32) -> String {
    use atomik_render::wav::{SampleFormat, write_wav_file};
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("mcp-relink-{tag}"))
        .join("new");
    std::fs::create_dir_all(&dir).unwrap();
    let ramp: Vec<f32> = (0..4_800).map(|n| n as f32 / 4_800.0 * seed).collect();
    let file = dir.join(name);
    write_wav_file(&file, &[ramp], 48_000, SampleFormat::F32).unwrap();
    file.to_string_lossy().into_owned()
}

#[test]
fn relink_validates_its_arguments() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    for args in [
        json!({}),
        json!({"file": "README.md", "folder": "examples"}),
        json!({"folder": "examples", "asset": "kick.wav"}),
    ] {
        let r = call(&mut s, "session_relink", args);
        assert!(r.is_error);
        assert!(
            r.first_text().contains("exactly one") || r.first_text().contains("`asset`"),
            "{}",
            r.first_text()
        );
    }
    // A file must be a file and a folder a folder; nothing missing is an error too.
    assert!(call(&mut s, "session_relink", json!({"file": "examples"})).is_error);
    assert!(call(&mut s, "session_relink", json!({"folder": "README.md"})).is_error);
    assert!(call(&mut s, "session_relink", json!({"file": 3})).is_error);
    let r = call(&mut s, "session_relink", json!({"folder": "examples"}));
    assert!(r.is_error);
    assert!(
        r.first_text().contains("no audio file is missing"),
        "{}",
        r.first_text()
    );
}

#[test]
fn relink_paths_stay_inside_the_allowed_folders() {
    let mut s = server();
    let project = missing_project("policy", &["kick.wav"]);
    ok(&mut s, "session_open", json!({"path": project}));
    let outside = std::env::temp_dir();
    for key in ["file", "folder"] {
        let r = call(
            &mut s,
            "session_relink",
            json!({ key: outside.to_string_lossy() }),
        );
        assert!(r.is_error, "{key}");
        assert!(
            r.first_text().contains("outside the allowed folders"),
            "{}",
            r.first_text()
        );
    }
    assert!(
        call(
            &mut s,
            "session_relink",
            json!({"file": "no/such/file.wav"})
        )
        .is_error
    );
}

#[test]
fn the_relink_command_lists_the_missing_files_and_relink_fixes_them() {
    let mut s = server();
    let project = missing_project("flow", &["kick.wav"]);
    ok(&mut s, "session_open", json!({"path": project}));
    let row = command_row(&mut s, "file.relink_audio");
    assert_eq!(row["enabled"], true);
    assert_eq!(row["label"], "Relink Missing Audio\u{2026}");
    let opened = ok(
        &mut s,
        "session_command",
        json!({"id": "file.relink_audio"}),
    );
    assert_eq!(opened["requests"], json!([]));
    let kick = ok(&mut s, "session_texts", json!({"contains": "kick.wav"}));
    assert!(!kick["texts"].as_array().unwrap().is_empty(), "{kick}");
    let locate = ok(&mut s, "session_texts", json!({"contains": "LOCATE"}));
    assert!(!locate["texts"].as_array().unwrap().is_empty());
    // Missing audio cannot bounce yet.
    assert!(call(&mut s, "session_bounce", json!({})).is_error);

    let file = replacement("flow", "snare.wav", 0.5);
    let r = ok(&mut s, "session_relink", json!({"file": file}));
    assert_eq!(r["relinked"], 1, "{r}");
    assert_eq!(r["still_missing"], json!([]));
    assert_eq!(r["document_changed"], true);
    assert_eq!(r["can_undo"], true);
    assert_eq!(r["dirty"], true);
    assert!(
        r["message"]
            .as_str()
            .unwrap()
            .contains("relinked 1, 0 still missing")
    );
    // The list closed with the last missing file, and the command is disabled again.
    assert_eq!(command_row(&mut s, "file.relink_audio")["enabled"], false);
    let gone = ok(&mut s, "session_texts", json!({"contains": "LOCATE"}));
    assert!(gone["texts"].as_array().unwrap().is_empty());
    // The session bounce now plays the relinked audio.
    let out = tmp("relinked.wav");
    let b = ok(&mut s, "session_bounce", json!({"output": out}));
    assert_eq!(b["audio"]["silent"], false, "{b}");
    // Undo brings the missing file back.
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(command_row(&mut s, "file.relink_audio")["enabled"], true);
}

#[test]
fn relink_picks_the_asset_by_name_or_id_and_searches_a_folder() {
    let mut s = server();
    let project = missing_project("pick", &["kick.wav", "snare.wav"]);
    ok(&mut s, "session_open", json!({"path": project}));
    let file = replacement("pick", "other.wav", 0.25);
    // Two are missing: the asset must be named.
    let r = call(&mut s, "session_relink", json!({"file": file}));
    assert!(r.is_error);
    assert!(
        r.first_text().contains("name one with `asset`"),
        "{}",
        r.first_text()
    );
    let r = call(
        &mut s,
        "session_relink",
        json!({"file": file, "asset": "hat.wav"}),
    );
    assert!(r.is_error);
    assert!(
        r.first_text().contains("not a missing asset"),
        "{}",
        r.first_text()
    );
    let v = ok(
        &mut s,
        "session_relink",
        json!({"file": file, "asset": "SNARE.WAV"}),
    );
    assert_eq!(v["relinked"], 1);
    assert_eq!(v["still_missing"].as_array().unwrap().len(), 1);
    assert_eq!(v["still_missing"][0]["name"], "kick.wav");

    // A folder search with a nested, differently cased file name relinks the other one.
    let deep = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("mcp-relink-pick")
        .join("pool")
        .join("deep");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::copy(
        replacement("pick", "found.wav", 0.75),
        deep.join("KICK.WAV"),
    )
    .unwrap();
    let pool = deep.parent().unwrap().to_string_lossy().into_owned();
    let v = ok(&mut s, "session_relink", json!({"folder": pool}));
    assert_eq!(v["relinked"], 1, "{v}");
    assert_eq!(v["still_missing"], json!([]));
    assert_eq!(command_row(&mut s, "file.relink_audio")["enabled"], false);
}

/// The logical position of the first drawn text equal to `text`.
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

#[test]
fn an_automation_lane_is_armed_and_its_write_mode_cycled_with_the_pointer() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let lane_write = |s: &mut Server| {
        let v = ok(s, "session_status", json!({}));
        let tracks = v["document"]["song"]["tracks"].as_array().unwrap().clone();
        let lanes = tracks
            .iter()
            .flat_map(|t| t["lanes"].as_array().unwrap().clone())
            .filter(|l| l["kind"] == "automation:filter.cutoff")
            .collect::<Vec<_>>();
        assert_eq!(lanes.len(), 1, "{v}");
        (lanes[0]["write"].as_str().unwrap().to_string(), v)
    };
    let (mode, status) = lane_write(&mut s);
    assert_eq!(mode, "touch");
    assert_eq!(status["armed_lanes"].as_array().unwrap().len(), 0);
    assert_eq!(status["recording"], false);

    // The mode button shows TCH; a click makes it Latch, the next Overwrite, one undo step each.
    let (x, y) = text_pos(&mut s, "TCH");
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x, "y": y - 3.0}),
    );
    assert_eq!(lane_write(&mut s).0, "latch");
    let (x, y) = text_pos(&mut s, "LTC");
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x, "y": y - 3.0}),
    );
    assert_eq!(lane_write(&mut s).0, "overwrite");
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(lane_write(&mut s).0, "latch", "one step undoes one change");

    // The arm button is session state: it shows in the status, is no undo step, and a take
    // can start with nothing else armed.
    let (x, y) = text_pos(&mut s, "W");
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x, "y": y - 3.0}),
    );
    let (_, status) = lane_write(&mut s);
    assert_eq!(
        status["armed_lanes"][0]["param"], "filter.cutoff",
        "{status}"
    );
    ok(&mut s, "session_command", json!({"id": "transport.record"}));
    assert_eq!(ok(&mut s, "session_status", json!({}))["recording"], true);
    ok(&mut s, "session_command", json!({"id": "transport.record"}));
    assert_eq!(ok(&mut s, "session_status", json!({}))["recording"], false);
}

#[test]
fn the_quantize_panel_opens_by_command_and_closes_with_escape() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let row = command_row(&mut s, "edit.quantize_options");
    assert_eq!(row["menu"], "Edit");
    assert_eq!(row["enabled"], true);
    ok(
        &mut s,
        "session_command",
        json!({"id": "edit.quantize_options"}),
    );
    let t = ok(&mut s, "session_texts", json!({"contains": "QUANTIZE"}));
    assert!(t["total"].as_u64().unwrap() >= 1, "{t}");
    for want in ["APPLY", "EXTRACT GROOVE FROM CLIP", "Humanize timing"] {
        let t = ok(&mut s, "session_texts", json!({"contains": want}));
        assert!(t["total"].as_u64().unwrap() >= 1, "{want}");
    }
    ok(&mut s, "session_key", json!({"key": "escape"}));
    let t = ok(&mut s, "session_texts", json!({"contains": "APPLY"}));
    assert_eq!(t["total"], 0);
}

#[test]
fn a_folder_is_selected_collapsed_and_ungrouped_with_the_pointer_and_commands() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "crates/model/tests/golden/organised_project.json"}),
    );
    let folders = |s: &mut Server| {
        let v = ok(s, "session_status", json!({}));
        v["document"]["song"]["folders"].as_array().unwrap().clone()
    };
    assert_eq!(folders(&mut s)[0]["name"], "Band");
    assert_eq!(folders(&mut s)[0]["collapsed"], false);
    // Nothing is selected: the folder commands are disabled.
    assert_eq!(
        command_row(&mut s, "sequencer.toggle_folder")["enabled"],
        false
    );
    assert_eq!(
        command_row(&mut s, "sequencer.group_tracks")["enabled"],
        false
    );
    // A click on the folder's name selects it; the Track menu follows.
    let (x, y) = text_pos(&mut s, "Band");
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x, "y": y - 3.0}),
    );
    assert_eq!(
        command_row(&mut s, "sequencer.toggle_folder")["enabled"],
        true
    );
    assert_eq!(
        command_row(&mut s, "sequencer.toggle_folder")["menu"],
        "Track"
    );
    ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.toggle_folder"}),
    );
    assert_eq!(folders(&mut s)[0]["collapsed"], true);
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(folders(&mut s)[0]["collapsed"], false);
    // The name is typed in place: the command opens the field, the keys fill it.
    ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.rename_folder"}),
    );
    for _ in 0..4 {
        ok(&mut s, "session_key", json!({"key": "Backspace"}));
    }
    ok(&mut s, "session_key", json!({"text": "Strings"}));
    ok(&mut s, "session_key", json!({"key": "Enter"}));
    assert_eq!(folders(&mut s)[0]["name"], "Strings");
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(folders(&mut s)[0]["name"], "Band");
    // Heights go through the same menu with an argument, and Ungroup keeps the tracks.
    ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.track_height:large"}),
    );
    ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.ungroup_tracks"}),
    );
    let v = ok(&mut s, "session_status", json!({}));
    assert_eq!(
        v["document"]["song"]["folders"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        v["document"]["song"]["folders"][0]["name"], "Rhythm \"main\"",
        "the outer folder is gone, the inner one stays at the top level"
    );
    assert_eq!(v["document"]["song"]["tracks"].as_array().unwrap().len(), 6);
}

#[test]
fn the_audio_editing_commands_are_menu_commands_that_need_an_audio_clip() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    let edit = ok(&mut s, "session_commands", json!({"menu": "edit"}));
    let rows = edit["commands"].as_array().unwrap();
    let row = |id: &str| {
        rows.iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("{id} in the Edit menu"))
    };
    for id in [
        "sequencer.audio_reverse",
        "sequencer.audio_normalise",
        "sequencer.audio_strip_silence",
    ] {
        assert_eq!(row(id)["enabled"], false, "{id} needs an audio clip");
    }
    let audio = ok(&mut s, "session_commands", json!({"menu": "audio"}));
    let ids: Vec<&str> = audio["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    for id in [
        "sequencer.audio_gain:up",
        "sequencer.audio_fade_shape:s_curve",
        "sequencer.audio_crossfade:off",
        "sequencer.audio_fades:declick",
        "sequencer.audio_envelope_clear",
    ] {
        assert!(ids.contains(&id), "{id} in {ids:?}");
    }
    // A disabled command is refused with its label.
    let r = call(
        &mut s,
        "session_command",
        json!({"id": "sequencer.audio_reverse"}),
    );
    assert!(r.is_error && r.first_text().contains("Reverse Audio"));
}

#[test]
fn the_warp_commands_are_menu_commands_that_need_an_audio_clip() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    let edit = ok(&mut s, "session_commands", json!({"menu": "edit"}));
    let quantize = edit["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "sequencer.audio_quantize")
        .expect("Quantize Audio in the Edit menu");
    assert_eq!(quantize["enabled"], false);
    let audio = ok(&mut s, "session_commands", json!({"menu": "audio"}));
    let rows = audio["commands"].as_array().unwrap();
    for id in [
        "sequencer.audio_warp",
        "sequencer.audio_warp_tempo",
        "sequencer.audio_warp_panel",
        "sequencer.audio_warp_mode:tonal",
        "sequencer.audio_warp_bars:4",
        "sequencer.audio_transpose:up",
        "sequencer.audio_transpose:octave_down",
    ] {
        let row = rows
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("{id} in the Audio menu"));
        assert_eq!(row["enabled"], false, "{id} needs an audio clip");
    }
    let r = call(
        &mut s,
        "session_command",
        json!({"id": "sequencer.audio_warp"}),
    );
    assert!(r.is_error && r.first_text().contains("Warp Audio"));
}

/// Opens a copy of the master demo that lives in the temp folder, so that the render a freeze
/// stores goes to a scratch bundle and never into the examples.
fn open_master_demo_copy(s: &mut Server, name: &str) {
    ok(
        s,
        "session_open",
        json!({"path": "examples/projects/master-demo/master-demo.atomik"}),
    );
    let copy = tmp(name);
    ok(s, "session_save", json!({"path": copy}));
    ok(s, "session_open", json!({"path": copy}));
}

#[test]
fn a_track_is_frozen_with_a_menu_command_and_the_bounce_stays_the_same() {
    let mut s = server();
    open_master_demo_copy(&mut s, "freeze-copy-1.atomik");
    // The freeze commands are Track menu commands, and need a selected track.
    let menu = ok(
        &mut s,
        "session_commands",
        json!({"menu": "Track", "query": "freeze"}),
    );
    let rows = menu["commands"].as_array().unwrap();
    for id in [
        "sequencer.freeze_track",
        "sequencer.freeze_track_to_device",
        "sequencer.unfreeze_track",
        "sequencer.unfreeze_track_keep",
        "sequencer.freeze_cancel",
        "sequencer.freeze_tail",
    ] {
        assert!(rows.iter().any(|c| c["id"] == id), "{id} in the Track menu");
    }
    assert_eq!(
        command_row(&mut s, "sequencer.freeze_track")["enabled"],
        false
    );
    let r = call(
        &mut s,
        "session_command",
        json!({"id": "sequencer.freeze_track"}),
    );
    assert!(r.is_error && r.first_text().contains("Freeze Track"));

    // Select Lead by its header (the lowest "Lead" on the screen is the arrangement's).
    let texts = ok(
        &mut s,
        "session_texts",
        json!({"contains": "Lead", "limit": 1000}),
    );
    let (x, y) = texts["texts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["text"] == "Lead")
        .map(|t| (t["x"].as_f64().unwrap(), t["y"].as_f64().unwrap()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("Lead is on screen");
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x + 2.0, "y": y - 3.0}),
    );
    assert_eq!(
        command_row(&mut s, "sequencer.freeze_track")["enabled"],
        true
    );

    let before = tmp("freeze-before.wav");
    ok(&mut s, "session_bounce", json!({"output": before}));
    let r = ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.freeze_track"}),
    );
    assert_eq!(r["waited"], true);
    assert!(
        r["message"].as_str().unwrap().starts_with("Froze Lead"),
        "{r}"
    );
    let v = ok(&mut s, "session_status", json!({}));
    let tracks = v["document"]["song"]["tracks"].as_array().unwrap().clone();
    let lead = tracks.iter().find(|t| t["device"] == "lead").unwrap();
    assert_eq!(lead["frozen"]["start"], 0);
    assert!(lead["frozen"]["length"].as_i64().unwrap() > 0);
    assert!(lead.get("lanes").is_some(), "its own clips stay listed");
    assert!(
        tracks
            .iter()
            .filter(|t| t["device"] != "lead")
            .all(|t| t.get("frozen").is_none())
    );
    assert_eq!(
        command_row(&mut s, "sequencer.unfreeze_track")["enabled"],
        true
    );
    assert_eq!(
        command_row(&mut s, "sequencer.freeze_track")["enabled"],
        false
    );

    // The frozen project sounds exactly like the project did.
    let after = tmp("freeze-after.wav");
    ok(&mut s, "session_bounce", json!({"output": after}));
    let same = ok(&mut s, "compare_audio", json!({"a": before, "b": after}));
    assert_eq!(same["bit_identical"], true, "{same}");

    // A saved frozen project (the render goes into its bundle) reads back, and the stateless
    // inspection lists the freeze too.
    let saved = tmp("freeze-saved.atomik");
    ok(&mut s, "session_save", json!({"path": saved}));
    // A project with audio is saved as a bundle folder next to the picked name.
    let bundle = saved.trim_end_matches(".atomik").to_owned();
    let inspected = ok(&mut s, "project_inspect", json!({"path": bundle}));
    let lead = inspected["song"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["device"] == "lead")
        .unwrap()
        .clone();
    assert_eq!(lead["frozen"]["start"], 0);
    assert!(lead["frozen"]["effects"].as_array().unwrap().is_empty());
    assert_eq!(inspected["assets"], 1, "the render");

    // One undo step brings the track back, and Unfreeze does the same from the menu.
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let v = ok(&mut s, "session_status", json!({}));
    assert!(
        v["document"]["song"]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t.get("frozen").is_none())
    );
    ok(&mut s, "session_command", json!({"id": "edit.redo"}));
    ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.unfreeze_track"}),
    );
    let v = ok(&mut s, "session_status", json!({}));
    assert!(
        v["document"]["song"]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t.get("frozen").is_none())
    );
}

#[test]
fn a_freeze_that_is_not_waited_for_can_be_cancelled() {
    let mut s = server();
    open_master_demo_copy(&mut s, "freeze-copy-2.atomik");
    let texts = ok(
        &mut s,
        "session_texts",
        json!({"contains": "Bass", "limit": 1000}),
    );
    let (x, y) = texts["texts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["text"] == "Bass")
        .map(|t| (t["x"].as_f64().unwrap(), t["y"].as_f64().unwrap()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("Bass is on screen");
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x + 2.0, "y": y - 3.0}),
    );
    let r = ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.freeze_track", "wait": false}),
    );
    assert!(r.get("waited").is_none());
    assert!(r["status_text"].as_str().unwrap().contains("Freezing Bass"));
    assert_eq!(
        command_row(&mut s, "sequencer.freeze_cancel")["enabled"],
        true
    );
    // Cancel returns once the render is over: it was stopped, or it had finished before the
    // stop was seen.
    let r = ok(
        &mut s,
        "session_command",
        json!({"id": "sequencer.freeze_cancel"}),
    );
    assert_eq!(r["waited"], true);
    let message = r["message"].as_str().unwrap();
    assert!(
        message.contains("cancelled") || message.starts_with("Froze Bass"),
        "{message}"
    );
    assert!(!r["status_text"].as_str().unwrap().contains("Freezing"));
}

/// A saved project with one audio track playing a sine melody (A3, E4 and C5 for 0.4 s each with
/// a gap) that the test generated, and the project file's path.
fn melody_project(tag: &str) -> String {
    use atomik_app::AppCore;
    use atomik_app::session::{LiveSession, NullBackend};
    use atomik_app::settings::AudioSettings;
    use atomik_render::wav::{SampleFormat, write_wav_file};
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mcp-notes-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("source")).unwrap();
    let mut core = AppCore::with_doc(
        LiveSession::new(NullBackend, AudioSettings::default(), false),
        atomik_model::empty_doc(),
        None,
    );
    let mut x = vec![0.0f32; 48_000 * 2];
    for (hz, from) in [(220.0f64, 0.1f64), (329.63, 0.7), (523.25, 1.3)] {
        let a = (from * 48_000.0) as usize;
        for i in 0..(0.4 * 48_000.0) as usize {
            let t = i as f64 / 48_000.0;
            let env = (i as f64 / 240.0)
                .min(((19_200 - i) as f64) / 240.0)
                .min(1.0);
            x[a + i] = (0.5 * env * (std::f64::consts::TAU * hz * t).sin()) as f32;
        }
    }
    let file = dir.join("source").join("tune.wav");
    write_wav_file(&file, &[x], 48_000, SampleFormat::F32).unwrap();
    core.import_audio(vec![file], atomik_ui::ImportTarget::NewTracks, 0);
    core.wait_background();
    core.save_as(&dir.join("song.atomik")).unwrap();
    dir.join("song")
        .join("song.atomik")
        .to_string_lossy()
        .into_owned()
}

/// Selects the audio clip the arrangement draws by clicking under its label (the smallest text
/// that reads `tune`; the larger ones are the rack's card and the track header).
fn select_audio_clip(s: &mut Server) {
    let v = ok(
        s,
        "session_texts",
        json!({"contains": "tune", "limit": 1000}),
    );
    let label = v["texts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["text"] == "tune")
        .min_by(|a, b| {
            a["size"]
                .as_f64()
                .unwrap()
                .total_cmp(&b["size"].as_f64().unwrap())
        })
        .expect("the clip label is on screen")
        .clone();
    ok(
        s,
        "session_pointer",
        json!({"action": "click", "x": label["x"].as_f64().unwrap() + 10.0, "y": label["y"].as_f64().unwrap() + 6.0}),
    );
}

#[test]
fn audio_to_notes_runs_in_a_session_on_a_generated_file_and_undoes_in_one_step() {
    let mut s = server();
    ok(&mut s, "session_open", json!({"path": melody_project("a")}));
    // The command opens the panel; it needs a selected audio clip.
    let row = command_row(&mut s, "sequencer.audio_to_notes");
    assert_eq!(row["enabled"], false);
    let r = call(&mut s, "session_audio_notes", json!({}));
    assert!(
        r.is_error || r.parsed()["started"] == false,
        "{}",
        r.first_text()
    );
    select_audio_clip(&mut s);
    assert_eq!(
        command_row(&mut s, "sequencer.audio_to_notes")["enabled"],
        true
    );
    let before = ok(&mut s, "session_status", json!({}));
    let tracks = before["document"]["song"]["tracks"]
        .as_array()
        .unwrap()
        .len();
    let r = ok(&mut s, "session_audio_notes", json!({}));
    assert_eq!(r["started"], true, "{r}");
    assert_eq!(r["document_changed"], true, "{r}");
    assert_eq!(r["count"], 3);
    let keys: Vec<i64> = r["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["key"].as_i64().unwrap())
        .collect();
    assert_eq!(keys, vec![57, 64, 72], "{r}");
    // 120 BPM: a second is 30 720 ticks. The first note starts 0.1 s in.
    let first = r["notes"][0]["tick"].as_i64().unwrap();
    assert!((first - 3_072).abs() < 800, "{first}");
    assert!(r["message"].as_str().unwrap().contains("3 notes"), "{r}");
    // The inspection shows the new track with its note clip.
    let after = ok(&mut s, "session_status", json!({}));
    let list = after["document"]["song"]["tracks"].as_array().unwrap();
    assert_eq!(list.len(), tracks + 1);
    let lane = &list.last().unwrap()["lanes"][0];
    assert_eq!(lane["kind"], "notes");
    assert_eq!(
        (lane["clips"].as_u64(), lane["items"].as_u64()),
        (Some(1), Some(3)),
        "{lane}"
    );
    // One undo step takes it all away.
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let undone = ok(&mut s, "session_status", json!({}));
    assert_eq!(
        undone["document"]["song"]["tracks"]
            .as_array()
            .unwrap()
            .len(),
        tracks
    );
    ok(&mut s, "session_close", json!({}));
}

#[test]
fn audio_to_notes_validates_its_arguments_and_reports_nothing_found() {
    let mut s = server();
    ok(&mut s, "session_open", json!({"path": melody_project("b")}));
    select_audio_clip(&mut s);
    for args in [
        json!({"mode": "vocal"}),
        json!({"grid": 3}),
        json!({"key_lo": 200}),
        json!({"key_lo": 90, "key_hi": 40}),
        json!({"sensitivity": "high"}),
    ] {
        let r = call(&mut s, "session_audio_notes", args.clone());
        assert!(r.is_error, "{args}");
    }
    // A range that holds none of the notes finds nothing, and the document stays as it was.
    let r = ok(
        &mut s,
        "session_audio_notes",
        json!({"key_lo": 100, "key_hi": 110}),
    );
    assert_eq!(r["started"], true);
    assert_eq!(r["document_changed"], false);
    assert!(
        r["message"].as_str().unwrap().contains("No notes found"),
        "{r}"
    );
    // Drums on a tonal clip still work as a command (it finds what hits it can).
    let r = ok(
        &mut s,
        "session_audio_notes",
        json!({"mode": "drums", "grid": 16}),
    );
    assert_eq!(r["started"], true);
    ok(&mut s, "session_close", json!({}));
}

/// A saved project with one audio track playing a generated 1 kHz stereo sine at -23 dBFS (the
/// level of the loudness reference) for three seconds, with a phase meter and a spectrum on its
/// channel's inserts, and the project file's path.
fn meter_project(tag: &str, right_gain: f32) -> String {
    use atomik_app::AppCore;
    use atomik_app::session::{LiveSession, NullBackend};
    use atomik_app::settings::AudioSettings;
    use atomik_model::{Command, ConsoleCommand, InsertEffect};
    use atomik_render::wav::{SampleFormat, write_wav_file};
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mcp-meters-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("source")).unwrap();
    let mut core = AppCore::with_doc(
        LiveSession::new(NullBackend, AudioSettings::default(), false),
        atomik_model::empty_doc(),
        None,
    );
    let amp = 10f64.powf(-23.0 / 20.0);
    let left: Vec<f32> = (0..48_000 * 3)
        .map(|i| (amp * (std::f64::consts::TAU * 1000.0 * i as f64 / 48_000.0).sin()) as f32)
        .collect();
    let right: Vec<f32> = left.iter().map(|v| v * right_gain).collect();
    let file = dir.join("source").join("tone.wav");
    write_wav_file(&file, &[left, right], 48_000, SampleFormat::F32).unwrap();
    core.import_audio(vec![file], atomik_ui::ImportTarget::NewTracks, 0);
    core.wait_background();
    let track = core.doc().song.tracks[0].device.clone();
    let channel = core
        .doc()
        .console
        .channel_of(&track)
        .unwrap()
        .channel
        .clone();
    for (slot, type_id) in ["atomik.phase-meter", "atomik.spectrum"]
        .into_iter()
        .enumerate()
    {
        core.apply_command(Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot,
            effect: Some(InsertEffect::New {
                type_id: type_id.into(),
            }),
        }))
        .unwrap();
    }
    core.save_as(&dir.join("song.atomik")).unwrap();
    dir.join("song")
        .join("song.atomik")
        .to_string_lossy()
        .into_owned()
}

fn device_of<'a>(v: &'a Value, type_id: &str) -> &'a Value {
    v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["type"] == type_id)
        .unwrap_or_else(|| panic!("no {type_id} in {v}"))
}

#[test]
fn session_meters_reads_the_reference_tone_without_an_audio_stream() {
    let mut s = server();
    // Nothing to read on the starter project.
    ok(&mut s, "session_open", json!({}));
    let r = call(&mut s, "session_meters", json!({}));
    assert!(r.is_error);
    assert!(
        r.first_text().contains("no spectrum or phase meter"),
        "{}",
        r.first_text()
    );
    ok(
        &mut s,
        "session_open",
        json!({"path": meter_project("a", 1.0), "height": 3600}),
    );
    let v = ok(&mut s, "session_meters", json!({"at_s": 2.9}));
    assert_eq!(v["frames"], 139_200);
    assert_eq!(v["devices"].as_array().unwrap().len(), 2);
    // The phase meter: a stereo sine at -23 dBFS reads -23 LUFS, mono, centred, narrow.
    let p = &device_of(&v, "atomik.phase-meter")["values"];
    let num = |name: &str| p[name].as_f64().unwrap_or_else(|| panic!("{name} in {p}"));
    assert!((num("momentary_lufs") + 23.0).abs() < 0.2, "{p}");
    // The 3 s window holds 2.9 s of tone: 0.15 dB under the level.
    assert!((num("short_term_lufs") + 23.15).abs() < 0.2, "{p}");
    assert!((num("integrated_lufs") + 23.0).abs() < 0.2, "{p}");
    assert!(num("correlation") > 0.999, "{p}");
    assert!(num("balance").abs() < 0.001 && num("width") < 0.001, "{p}");
    assert!((num("sample_peak_dbfs") + 23.0).abs() < 0.1, "{p}");
    assert!(
        (num("rms_l") - 10f64.powf(-23.0 / 20.0) / 2f64.sqrt()).abs() < 0.002,
        "{p}"
    );
    // The spectrum: a 1 kHz line at -23 dB, in the 1 kHz band.
    let sp = device_of(&v, "atomik.spectrum");
    let hz = sp["values"]["peak_hz"].as_f64().unwrap();
    assert!((hz - 1000.0).abs() < 12.0, "{hz}");
    assert!((sp["values"]["peak_db"].as_f64().unwrap() + 23.0).abs() < 1.5);
    let bands = sp["bands"].as_array().unwrap();
    assert_eq!(bands.len(), 31);
    let loudest = bands
        .iter()
        .max_by(|a, b| a[1].as_f64().unwrap().total_cmp(&b[1].as_f64().unwrap()))
        .unwrap();
    assert!(
        (loudest[0].as_f64().unwrap() - 1000.0).abs() < 1.0,
        "{loudest}"
    );
    assert!(
        (loudest[1].as_f64().unwrap() + 23.0).abs() < 1.0,
        "{loudest}"
    );
    // The panels show the readings: the texts the UI draws carry them.
    let t = ok(
        &mut s,
        "session_texts",
        json!({"contains": "LUFS", "limit": 100}),
    );
    assert!(!t["texts"].as_array().unwrap().is_empty(), "{t}");
    for want in ["CORRELATION", "+1.00", "MOMENTARY", "TRUE PEAK"] {
        let t = ok(
            &mut s,
            "session_texts",
            json!({"contains": want, "limit": 100}),
        );
        assert!(
            t["texts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x["text"] == want),
            "{want}: {t}"
        );
    }
    let t = ok(
        &mut s,
        "session_texts",
        json!({"contains": " dB", "limit": 100}),
    );
    assert!(
        t["texts"].as_array().unwrap().iter().any(|x| {
            x["text"]
                .as_str()
                .is_some_and(|s| s.starts_with("PEAK ") && s.contains("Hz") && s.ends_with("dB"))
        }),
        "the spectrum's readout of the loudest bin: {t}"
    );
    // The same call gives the same numbers, and a filter and a position are honoured.
    let again = ok(&mut s, "session_meters", json!({"at_s": 2.9}));
    assert_eq!(v, again);
    let one = ok(
        &mut s,
        "session_meters",
        json!({"at_s": 0.5, "device": device_of(&v, "atomik.phase-meter")["key"]}),
    );
    assert_eq!(one["devices"].as_array().unwrap().len(), 1);
    // At half a second the 3 s window is mostly empty: the short-term loudness is far lower.
    assert!(
        one["devices"][0]["values"]["short_term_lufs"]
            .as_f64()
            .unwrap()
            < -28.0
    );
    ok(&mut s, "session_close", json!({}));
}

#[test]
fn session_meters_follows_the_stereo_image_and_refuses_bad_arguments() {
    let mut s = server();
    // The right channel inverted and half as loud.
    ok(
        &mut s,
        "session_open",
        json!({"path": meter_project("b", -0.5)}),
    );
    let v = ok(&mut s, "session_meters", json!({"at_s": 2.0}));
    let p = &device_of(&v, "atomik.phase-meter")["values"];
    assert!(
        (p["correlation"].as_f64().unwrap() + 1.0).abs() < 0.001,
        "{p}"
    );
    assert!(
        p["balance"].as_f64().unwrap() < -0.3,
        "louder on the left: {p}"
    );
    assert!(p["width"].as_f64().unwrap() > 0.7, "{p}");
    for args in [
        json!({"at_s": -1}),
        json!({"at_s": 99999}),
        json!({"at_s": "soon"}),
        json!({"device": "nope"}),
        json!({"device": 3}),
    ] {
        let r = call(&mut s, "session_meters", args.clone());
        assert!(r.is_error, "{args}");
    }
    ok(&mut s, "session_close", json!({}));
}

#[test]
fn vca_groups_are_made_filled_and_deleted_with_menu_commands_and_a_group_id() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    ok(
        &mut s,
        "session_command",
        json!({"id": "view.toggle_console"}),
    );
    let vcas = |s: &mut Server| {
        let v = ok(s, "session_status", json!({}));
        v["document"]["console"]["vcas"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    // The four commands are in the Mixer menu; nothing selected, nothing to make.
    for id in [
        "mixer.create_vca",
        "mixer.add_to_vca",
        "mixer.remove_from_vca",
        "mixer.delete_vca",
    ] {
        let row = command_row(&mut s, id);
        assert_eq!(row["menu"], "Mixer", "{id}");
        assert_eq!(row["shortcut"], Value::Null, "{id}");
        assert_eq!(row["enabled"], false, "{id}");
    }
    assert!(vcas(&mut s).is_empty());
    // A click on a channel's name plate selects it, and the group is made from it.
    let plate = |s: &mut Server, caption: &str| {
        let (x, y) = text_pos(s, caption);
        ok(
            s,
            "session_pointer",
            json!({"action": "click", "x": x + 20.0, "y": y - 12.0}),
        );
    };
    plate(&mut s, "CH 1");
    assert_eq!(command_row(&mut s, "mixer.create_vca")["enabled"], true);
    ok(&mut s, "session_command", json!({"id": "mixer.create_vca"}));
    let groups = vcas(&mut s);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["name"], "VCA 1");
    assert_eq!(groups[0]["id"], 1);
    assert_eq!(groups[0]["level_db"], 0.0);
    assert_eq!(groups[0]["members"][0]["strip"], "channel");
    assert!(groups[0]["color"].as_str().unwrap().starts_with('#'));
    // The second channel joins by the group's id, and leaves with Remove from VCA.
    plate(&mut s, "CH 2");
    ok(
        &mut s,
        "session_command",
        json!({"id": "mixer.add_to_vca", "arg": "1"}),
    );
    assert_eq!(vcas(&mut s)[0]["members"].as_array().unwrap().len(), 2);
    ok(
        &mut s,
        "session_command",
        json!({"id": "mixer.remove_from_vca"}),
    );
    assert_eq!(vcas(&mut s)[0]["members"].as_array().unwrap().len(), 1);
    // A bad argument is an error, and delete by id removes the group; undo brings it back.
    let r = call(
        &mut s,
        "session_command",
        json!({"id": "mixer.add_to_vca", "arg": "x"}),
    );
    assert!(r.is_error, "{}", r.first_text());
    ok(
        &mut s,
        "session_command",
        json!({"id": "mixer.delete_vca", "arg": "1"}),
    );
    assert!(vcas(&mut s).is_empty());
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(vcas(&mut s).len(), 1);
}

#[test]
fn session_set_param_sets_the_strip_upgrades_and_the_bounce_hears_them() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    let strip = |s: &mut Server| {
        let v = ok(s, "session_status", json!({}));
        v["document"]["console"]["strips"]
            .as_array()
            .and_then(|all| all.iter().find(|x| x["strip"] == "channel"))
            .cloned()
    };
    let reference = tmp("strip-reference.wav");
    ok(&mut s, "session_bounce", json!({"output": reference}));
    let bounce = |s: &mut Server, name: &str| {
        ok(
            s,
            "session_bounce",
            json!({"output": tmp(name), "reference": reference}),
        )["comparison"]["bit_identical"]
            .clone()
    };
    // The trim and the polarity, as the console's GAIN knob and the polarity button.
    let r = ok(
        &mut s,
        "session_set_param",
        json!({"device": "channel", "param": "input.gain", "value": 6.0}),
    );
    assert_eq!(
        (r["value"].as_f64(), r["can_undo"].as_bool()),
        (Some(6.0), Some(true))
    );
    ok(
        &mut s,
        "session_set_param",
        json!({"device": "channel", "param": "input.phase", "value": 1}),
    );
    let v = strip(&mut s).expect("the strip is listed once it is not at its defaults");
    assert_eq!(v["input_gain_db"], 6.0);
    assert_eq!(v["polarity_inverted"], true);
    assert_eq!(bounce(&mut s, "strip-trim.wav"), false);
    // Back to the defaults the project bounces as it did, bit for bit.
    ok(
        &mut s,
        "session_set_param",
        json!({"device": "channel", "param": "input.gain", "value": 0.0}),
    );
    ok(
        &mut s,
        "session_set_param",
        json!({"device": "channel", "param": "input.phase", "value": 0}),
    );
    assert_eq!(bounce(&mut s, "strip-default.wav"), true);
    // The parallel compressor: at mix 0 (or off) it is bypassed bit for bit, at 1 it is heard.
    for (param, value) in [
        ("pcomp.on", 1.0),
        ("pcomp.mix", 0.0),
        ("pcomp.threshold", -50.0),
        ("pcomp.ratio", 20.0),
        ("pcomp.makeup", 0.0),
    ] {
        ok(
            &mut s,
            "session_set_param",
            json!({"device": "channel", "param": param, "value": value}),
        );
    }
    let v = strip(&mut s).expect("listed with the parallel compressor set");
    assert_eq!(v["parallel"]["on"], true);
    assert_eq!(v["parallel"]["mix"], 0.0);
    assert_eq!(v["parallel"]["ratio"], 20.0);
    assert_eq!(bounce(&mut s, "strip-mix0.wav"), true);
    ok(
        &mut s,
        "session_set_param",
        json!({"device": "channel", "param": "pcomp.mix", "value": 1.0}),
    );
    assert_eq!(bounce(&mut s, "strip-mix1.wav"), false);
    // One undo step per call, and what the call cannot do is an error.
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(strip(&mut s).unwrap()["parallel"]["mix"], 0.0);
    for args in [
        json!({"device": "nothing", "param": "input.gain", "value": 1}),
        json!({"device": "channel", "param": "nothing", "value": 1}),
        json!({"device": "channel", "param": "input.gain"}),
    ] {
        assert!(call(&mut s, "session_set_param", args).is_error);
    }
}

#[test]
fn polarity_and_bypass_all_inserts_are_mixer_commands_of_the_session() {
    let mut s = server();
    ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    ok(
        &mut s,
        "session_command",
        json!({"id": "view.toggle_console"}),
    );
    for id in ["mixer.toggle_polarity", "mixer.bypass_all_inserts"] {
        let row = command_row(&mut s, id);
        assert_eq!(row["menu"], "Mixer", "{id}");
        assert_eq!(row["shortcut"], Value::Null, "{id}");
        assert_eq!(row["enabled"], false, "{id}: nothing is selected");
    }
    let (x, y) = text_pos(&mut s, "CH 1");
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": x + 20.0, "y": y - 12.0}),
    );
    assert_eq!(
        command_row(&mut s, "mixer.toggle_polarity")["enabled"],
        true
    );
    ok(
        &mut s,
        "session_command",
        json!({"id": "mixer.toggle_polarity"}),
    );
    let v = ok(&mut s, "session_status", json!({}));
    let strips = v["document"]["console"]["strips"].as_array().unwrap();
    assert_eq!(strips[0]["strip"], "channel");
    assert_eq!(strips[0]["polarity_inverted"], true);
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let v = ok(&mut s, "session_status", json!({}));
    assert!(
        v["document"]["console"]["strips"].is_null(),
        "back to nothing to report"
    );
}
