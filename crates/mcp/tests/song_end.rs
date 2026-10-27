//! The song End in the QA tools: `project_inspect` and `session_status` report it, and the
//! Set End Marker at Playhead command moves it (and undo puts it back).

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

fn ok(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, name, args);
    assert!(!r.is_error, "{name}: {}", r.first_text());
    r.parsed()
}

const DEMO: &str = "examples/projects/demo.atomik";

fn end_of_status(s: &mut Server) -> (i64, f64) {
    let v = ok(s, "session_status", json!({}));
    let song = &v["document"]["song"];
    (
        song["end_ticks"].as_i64().unwrap(),
        song["end_beats"].as_f64().unwrap(),
    )
}

#[test]
fn project_inspect_and_session_status_report_the_end() {
    let mut s = server();
    let inspected = ok(&mut s, "project_inspect", json!({"path": DEMO}));
    let ticks = inspected["song"]["end_ticks"].as_i64().unwrap();
    let beats = inspected["song"]["end_beats"].as_f64().unwrap();
    assert!(ticks > 0);
    assert_eq!(
        beats,
        ticks as f64 / inspected["song"]["ticks_per_beat"].as_f64().unwrap()
    );
    ok(&mut s, "session_open", json!({"path": DEMO}));
    assert_eq!(end_of_status(&mut s), (ticks, beats));
}

#[test]
fn the_set_end_marker_command_moves_the_end_to_the_playhead_and_undo_restores_it() {
    let mut s = server();
    ok(&mut s, "session_open", json!({"path": DEMO}));
    let (before, _) = end_of_status(&mut s);
    // Forward One Bar three times: the playhead is somewhere other than 0 and the End.
    for _ in 0..3 {
        ok(&mut s, "session_key", json!({"key": "Numpad8"}));
    }
    ok(
        &mut s,
        "session_command",
        json!({"id": "transport.set_end_marker"}),
    );
    let (after, _) = end_of_status(&mut s);
    assert_ne!(after, before);
    assert!(after > 0);
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    assert_eq!(end_of_status(&mut s).0, before);
}
