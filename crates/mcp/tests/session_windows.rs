//! Detached windows in the headless session: `session_status` lists them, the input and
//! picture tools take a `window`, and `session_window` plays the part of the OS.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
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

/// A server with a session on the starter project, nothing detached.
fn attached() -> Server {
    let mut s = Server::new(Paths::new(repo(), &[]));
    ok(&mut s, "session_open", json!({}));
    s
}

/// A session with the rack and the console detached.
fn detached() -> Server {
    let mut s = attached();
    ok(&mut s, "session_command", json!({"id": "view.detach_rack"}));
    ok(
        &mut s,
        "session_command",
        json!({"id": "view.detach_console"}),
    );
    s
}

/// The `windows` entry of `kind` in the status (null when it is not open).
fn window(s: &mut Server, kind: &str) -> Value {
    let status = ok(s, "session_status", json!({}));
    status["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["kind"] == kind)
        .cloned()
        .unwrap_or(Value::Null)
}

fn texts_of(v: &Value) -> Vec<String> {
    v["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["text"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn status_lists_windows_after_detach() {
    let mut s = attached();
    let status = ok(&mut s, "session_status", json!({}));
    let windows = status["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["kind"], "main");
    assert_eq!(windows[0]["detached"], false);
    assert_eq!(windows[0]["focused"], true);

    ok(&mut s, "session_command", json!({"id": "view.detach_rack"}));
    let rack = window(&mut s, "rack");
    assert_eq!(rack["detached"], true);
    assert_eq!(rack["logical"], json!({"width": 1280.0, "height": 720.0}));
    assert_eq!(rack["scale"], 1.0);
    assert_eq!(rack["physical"], json!({"width": 1280, "height": 720}));
    assert_eq!(window(&mut s, "console"), Value::Null);
    assert_eq!(window(&mut s, "main")["detached"], false);
}

#[test]
fn screenshot_rack_window_has_rack_size() {
    let mut s = detached();
    let shot = call(&mut s, "session_screenshot", json!({"window": "rack"}));
    assert!(!shot.is_error, "{}", shot.first_text());
    let info = shot.parsed();
    assert_eq!(info["width"], 1280);
    assert_eq!(info["height"], 720);
    assert_eq!(info["window"], "rack");
    let console = ok(&mut s, "session_screenshot", json!({"window": "console"}));
    assert_eq!(console["height"], 520);
    let main = ok(&mut s, "session_screenshot", json!({}));
    assert_eq!(main["height"], 860);
}

#[test]
fn pointer_in_console_window_reaches_console() {
    let mut s = detached();
    let r = ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": 400, "y": 260, "window": "console"}),
    );
    assert_eq!(r["pane_under_pointer"], "Console");
    assert_eq!(ok(&mut s, "session_status", json!({}))["focus"], "Console");
    let rack = ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": 400, "y": 300, "window": "rack"}),
    );
    assert_eq!(rack["pane_under_pointer"], "Rack");
    assert_eq!(ok(&mut s, "session_status", json!({}))["focus"], "Rack");
}

#[test]
fn key_from_rack_window_reaches_rack_context() {
    let mut s = detached();
    ok(
        &mut s,
        "session_pointer",
        json!({"action": "click", "x": 400, "y": 260, "window": "console"}),
    );
    ok(
        &mut s,
        "session_key",
        json!({"key": "Escape", "window": "rack"}),
    );
    assert_eq!(ok(&mut s, "session_status", json!({}))["focus"], "Rack");
    assert_eq!(window(&mut s, "rack")["focused"], true);
    assert_eq!(window(&mut s, "console")["focused"], false);
}

#[test]
fn texts_window_filter_console_only() {
    let mut s = detached();
    let main = texts_of(&ok(&mut s, "session_texts", json!({"limit": 2000})));
    let console = ok(
        &mut s,
        "session_texts",
        json!({"window": "console", "limit": 2000}),
    );
    assert!(console["total"].as_u64().unwrap() > 0);
    let console = texts_of(&console);
    // The transport is only in the main window; the mixer is only in the console's.
    assert!(main.iter().any(|t| t.contains("BPM")), "{main:?}");
    assert!(!console.iter().any(|t| t.contains("BPM")), "{console:?}");
    assert!(
        !main.iter().any(|t| t.to_lowercase().contains("master")),
        "{main:?}"
    );
    assert!(
        console.iter().any(|t| t.to_lowercase().contains("master")),
        "{console:?}"
    );
}

#[test]
fn session_window_close_attaches() {
    let mut s = detached();
    let r = ok(
        &mut s,
        "session_window",
        json!({"action": "close", "window": "rack"}),
    );
    assert_eq!(r["windows"].as_array().unwrap().len(), 2);
    assert_eq!(window(&mut s, "rack"), Value::Null);
    assert_eq!(window(&mut s, "console")["detached"], true);
}

#[test]
fn session_window_resize_sets_the_window_size() {
    let mut s = detached();
    ok(
        &mut s,
        "session_window",
        json!({"action": "resize", "window": "console", "width": 900, "height": 400}),
    );
    assert_eq!(
        window(&mut s, "console")["logical"],
        json!({"width": 900.0, "height": 400.0})
    );
    let shot = ok(&mut s, "session_screenshot", json!({"window": "console"}));
    assert_eq!(
        (shot["width"].as_u64(), shot["height"].as_u64()),
        (Some(900), Some(400))
    );
    let m = fails(
        &mut s,
        "session_window",
        json!({"action": "resize", "window": "console", "width": 900}),
    );
    assert!(m.contains("height"), "{m}");
    let m = fails(
        &mut s,
        "session_window",
        json!({"action": "resize", "window": "console", "width": 10, "height": 400}),
    );
    assert!(m.contains("200..=8192"), "{m}");
}

#[test]
fn session_window_focus_moves_the_keys() {
    let mut s = detached();
    ok(
        &mut s,
        "session_window",
        json!({"action": "focus", "window": "console"}),
    );
    assert_eq!(window(&mut s, "console")["focused"], true);
    assert_eq!(ok(&mut s, "session_status", json!({}))["focus"], "Console");
    ok(
        &mut s,
        "session_window",
        json!({"action": "focus", "window": "main"}),
    );
    assert_eq!(window(&mut s, "main")["focused"], true);
}

#[test]
fn window_not_open_is_refused_by_name() {
    let mut s = attached();
    let want = "the rack window is not open: run view.detach_rack";
    for (tool, args) in [
        ("session_screenshot", json!({"window": "rack"})),
        ("session_texts", json!({"window": "rack"})),
        (
            "session_pointer",
            json!({"action": "move", "x": 1, "y": 1, "window": "rack"}),
        ),
        ("session_key", json!({"key": "a", "window": "rack"})),
        (
            "session_window",
            json!({"action": "focus", "window": "rack"}),
        ),
        (
            "session_window",
            json!({"action": "close", "window": "rack"}),
        ),
    ] {
        assert_eq!(fails(&mut s, tool, args), want, "{tool}");
    }
    assert_eq!(
        fails(&mut s, "session_texts", json!({"window": "console"})),
        "the console window is not open: run view.detach_console"
    );
}

#[test]
fn window_unknown_name_is_refused() {
    let mut s = attached();
    let m = fails(&mut s, "session_texts", json!({"window": "sidebar"}));
    assert!(
        m.contains("main, rack, console or spectrum-eq") && m.contains("sidebar"),
        "{m}"
    );
    let m = fails(
        &mut s,
        "session_window",
        json!({"action": "spin", "window": "main"}),
    );
    assert!(m.contains("focus, close or resize"), "{m}");
}

#[test]
fn session_window_close_main_is_refused() {
    let mut s = detached();
    let m = fails(
        &mut s,
        "session_window",
        json!({"action": "close", "window": "main"}),
    );
    assert!(m.contains("session_close"), "{m}");
}
