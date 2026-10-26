//! The MIDI control tools: learn, mappings, presets and the surface driver, driven through the
//! server on a headless session, with bounces proving that mappings never change a render.

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

fn tmp(name: &str) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-midi");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

fn open_demo(s: &mut Server) {
    ok(
        s,
        "session_open",
        json!({"path": "examples/projects/demo.atomik"}),
    );
}

fn fader(s: &mut Server, device: &str) -> f64 {
    let v = ok(s, "session_status", json!({"include_params": true}));
    let d = v["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["key"] == device)
        .unwrap()
        .clone();
    d["params"]["fader"].as_f64().unwrap_or(0.75)
}

fn learn(s: &mut Server, args: Value) -> Value {
    ok(s, "session_midi_learn", args)
}

fn input(s: &mut Server, messages: &str) -> Value {
    ok(s, "session_midi_input", json!({"messages": messages}))
}

#[test]
fn the_midi_tools_need_an_open_session_and_say_what_is_wrong() {
    let mut s = server();
    for (tool, args) in [
        ("session_midi_input", json!({"messages": "B0 07 40"})),
        ("session_midi_learn", json!({"action": "clear"})),
        ("session_surface", json!({"action": "frame"})),
    ] {
        let r = call(&mut s, tool, args);
        assert!(
            r.is_error && r.first_text().contains("session_open"),
            "{tool}"
        );
    }
    open_demo(&mut s);
    let bad = |s: &mut Server, tool: &str, args: Value| call(s, tool, args).first_text().to_owned();
    assert!(bad(&mut s, "session_midi_input", json!({"messages": "B0 zz"})).contains("zz"));
    assert!(bad(&mut s, "session_midi_input", json!({"messages": "C0 05"})).contains("not a note"));
    assert!(bad(&mut s, "session_midi_input", json!({"messages": " ; "})).contains("no message"));
    assert!(bad(&mut s, "session_midi_learn", json!({"action": "dance"})).contains("dance"));
    assert!(bad(&mut s, "session_midi_learn", json!({"action": "arm"})).contains("target"));
    assert!(
        bad(
            &mut s,
            "session_midi_learn",
            json!({"action": "arm", "target": "nope fader"})
        )
        .contains("no device")
    );
    assert!(
        bad(
            &mut s,
            "session_midi_learn",
            json!({"action": "remove", "id": 9})
        )
        .contains("no mapping")
    );
    assert!(
        bad(
            &mut s,
            "session_surface",
            json!({"action": "send", "messages": "90 00 7F"})
        )
        .contains("attach")
    );
    assert!(
        bad(
            &mut s,
            "session_surface",
            json!({"action": "attach", "driver": "nope"})
        )
        .contains("driver")
    );
}

#[test]
fn learning_by_arming_a_target_and_moving_a_control_makes_a_mapping() {
    let mut s = server();
    open_demo(&mut s);
    let v = learn(&mut s, json!({"action": "arm", "target": "channel fader"}));
    assert_eq!(v["armed"], "Channel Fader");
    // The first control moved is the source: CC 74 is above 63, so it decides at once.
    let v = input(&mut s, "B2 4A 40");
    assert_eq!(v["mappings"], 1);
    assert_eq!(v["armed"], Value::Null);
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["document"]["midi_maps"][0]["source"], "CC 74 ch 3");
    assert_eq!(
        status["document"]["midi_maps"][0]["target"],
        "channel fader"
    );
    assert_eq!(status["document"]["midi_maps"][0]["mode"], "absolute");
    assert_eq!(status["midi"]["mappings"], 1);
    // A lone low controller waits for a possible pair, then a `wait` decides it.
    learn(&mut s, json!({"action": "arm", "target": "channel pan"}));
    ok(
        &mut s,
        "session_midi_input",
        json!({"messages": "B0 07 10", "wait": 0.5}),
    );
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["document"]["midi_maps"].as_array().unwrap().len(), 2);
    // The learn is undone by the menu's Undo like any edit.
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["document"]["midi_maps"].as_array().unwrap().len(), 1);
}

#[test]
fn a_mapped_control_moves_the_fader_with_range_invert_and_takeover() {
    let mut s = server();
    open_demo(&mut s);
    learn(&mut s, json!({"action": "arm", "target": "channel fader"}));
    input(&mut s, "B0 4A 40");
    // Soft takeover: the fader is at 0.62 in the demo; a control far from it does nothing...
    let start = fader(&mut s, "channel");
    let v = input(&mut s, "B0 4A 00; B0 4A 08");
    assert_eq!(v["document_changed"], false);
    assert!((fader(&mut s, "channel") - start).abs() < 1e-6);
    // ...until it reaches the value, and then it follows.
    input(&mut s, "B0 4A 4F");
    input(&mut s, "B0 4A 7F");
    assert!((fader(&mut s, "channel") - 1.0).abs() < 1e-4);
    // Without takeover, with a range and inverted.
    learn(
        &mut s,
        json!({"action": "edit", "id": 1, "pickup": false, "min": 0.25, "max": 0.75, "invert": true}),
    );
    input(&mut s, "B0 4A 00");
    assert!((fader(&mut s, "channel") - 0.75).abs() < 1e-4);
    input(&mut s, "B0 4A 7F");
    assert!((fader(&mut s, "channel") - 0.25).abs() < 1e-4);
    // Wrong options are refused with the reason.
    let r = call(
        &mut s,
        "session_midi_learn",
        json!({"action": "edit", "id": 1, "max": 3}),
    );
    assert!(
        r.is_error && r.first_text().contains("between 0 and 1"),
        "{}",
        r.first_text()
    );
    let r = call(
        &mut s,
        "session_midi_learn",
        json!({"action": "edit", "id": 1, "mode": "spin"}),
    );
    assert!(r.is_error && r.first_text().contains("spin"));
}

#[test]
fn buttons_toggle_and_a_relative_encoder_turns_and_a_transport_key_plays() {
    let mut s = server();
    open_demo(&mut s);
    learn(&mut s, json!({"action": "arm", "target": "channel mute"}));
    input(&mut s, "90 24 7F");
    learn(&mut s, json!({"action": "arm", "target": "channel pan"}));
    input(&mut s, "B0 50 01");
    learn(
        &mut s,
        json!({"action": "edit", "id": 2, "mode": "relative_twos", "step": 0.05}),
    );
    learn(&mut s, json!({"action": "arm", "target": "transport play"}));
    input(&mut s, "90 30 7F");
    let status = ok(&mut s, "session_status", json!({"include_params": true}));
    assert_eq!(status["document"]["midi_maps"][0]["mode"], "toggle");
    assert_eq!(status["document"]["midi_maps"][2]["target"], "Play");
    let channel = status["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["key"] == "channel")
        .unwrap()
        .clone();
    let mute = |c: &Value| c["params"]["mute"].as_f64().unwrap_or(0.0);
    assert!(mute(&channel) < 0.5);
    input(&mut s, "90 24 7F; 80 24 00");
    let status = ok(&mut s, "session_status", json!({"include_params": true}));
    let channel = status["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["key"] == "channel")
        .unwrap()
        .clone();
    assert!(mute(&channel) > 0.5, "the note toggled the mute");
    // Two clockwise detents move the pan by 0.1 of its range.
    let before = channel["params"]["pan"].as_f64().unwrap_or(0.0);
    input(&mut s, "B0 50 02");
    let status = ok(&mut s, "session_status", json!({"include_params": true}));
    let after = status["document"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["key"] == "channel")
        .unwrap()["params"]["pan"]
        .as_f64()
        .unwrap();
    assert!((after - before).abs() > 1e-4, "{before} -> {after}");
    // The play key: the status text of the app does not matter, the transport does not fail.
    input(&mut s, "90 30 00; 90 30 7F");
}

#[test]
fn a_mapping_never_changes_a_bounce_and_the_file_keeps_it() {
    let mut s = server();
    open_demo(&mut s);
    let reference = tmp("maps-reference.wav");
    ok(&mut s, "session_bounce", json!({"output": reference}));
    learn(&mut s, json!({"action": "arm", "target": "channel fader"}));
    input(&mut s, "B0 4A 40");
    learn(&mut s, json!({"action": "arm", "target": "channel-2 pan"}));
    input(&mut s, "B0 4B 40");
    learn(&mut s, json!({"action": "arm", "target": "tempo"}));
    input(&mut s, "B0 4C 40");
    let b = ok(
        &mut s,
        "session_bounce",
        json!({"output": tmp("maps-with.wav"), "reference": reference}),
    );
    assert_eq!(
        b["comparison"]["bit_identical"], true,
        "mappings are not sound"
    );
    // Save, reopen, and the mappings are there and still silent.
    let file = tmp("maps.atomik");
    ok(&mut s, "session_save", json!({"path": file}));
    let inspected = ok(&mut s, "project_inspect", json!({"path": file}));
    assert_eq!(inspected["midi_maps"].as_array().unwrap().len(), 3);
    assert_eq!(inspected["midi_maps"][2]["target"], "Tempo");
    ok(&mut s, "session_open", json!({"path": file}));
    let b = ok(
        &mut s,
        "session_bounce",
        json!({"output": tmp("maps-reopened.wav"), "reference": reference}),
    );
    assert_eq!(b["comparison"]["bit_identical"], true);
    // A project of the previous format loads unchanged and bounces the same.
    let old = std::fs::read_to_string(&file).unwrap();
    let mut lines: Vec<&str> = old.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains("\"midi_maps\""))
        .unwrap();
    let end = lines[start..]
        .iter()
        .position(|l| l.trim() == "],")
        .unwrap()
        + start;
    lines.drain(start..=end);
    let v27 = lines
        .join("\n")
        .replace("\"version\": 31", "\"version\": 27");
    let v27_path = tmp("maps-v27.atomik");
    std::fs::write(&v27_path, v27).unwrap();
    ok(&mut s, "session_open", json!({"path": v27_path}));
    let b = ok(
        &mut s,
        "session_bounce",
        json!({"output": tmp("maps-v27.wav"), "reference": reference}),
    );
    assert_eq!(b["comparison"]["bit_identical"], true);
}

#[test]
fn a_preset_exported_from_the_session_applies_by_role_and_edits_and_clears_are_undoable() {
    let mut s = server();
    open_demo(&mut s);
    learn(
        &mut s,
        json!({"action": "arm", "target": "lead filter.cutoff"}),
    );
    input(&mut s, "B0 4A 40");
    learn(
        &mut s,
        json!({"action": "arm", "target": "channel-2 fader"}),
    );
    input(&mut s, "B0 4B 40");
    let file = tmp("preset.json");
    let v = learn(&mut s, json!({"action": "export", "file": file}));
    assert!(
        v["message"].as_str().unwrap().starts_with("Exported 2"),
        "{v}"
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("atomik-midi-preset") && text.contains("atomik.poly-synth"));
    // Clear (one undo step), import (one undo step, by role), undo both.
    let v = learn(&mut s, json!({"action": "clear"}));
    assert_eq!(v["mappings"], 0);
    let v = learn(&mut s, json!({"action": "import", "file": file}));
    assert_eq!(v["mappings"], 2);
    assert_eq!(v["midi_maps"][0]["target"], "Lead Cutoff");
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let v = learn(&mut s, json!({"action": "mode", "on": true}));
    assert_eq!(v["mappings"], 0);
    assert_eq!(v["learn_mode"], true);
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["midi"]["learn_mode"], true);
    assert!(
        status["status_text"]
            .as_str()
            .unwrap()
            .contains("MIDI LEARN")
    );
    // The Mixer menu's item is the same switch.
    ok(&mut s, "session_command", json!({"id": "midi.learn"}));
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["midi"]["learn_mode"], false);
    // A preset with nothing to export says so.
    let r = call(
        &mut s,
        "session_midi_learn",
        json!({"action": "export", "file": tmp("none.json")}),
    );
    assert!(
        r.is_error && r.first_text().contains("no MIDI mappings"),
        "{}",
        r.first_text()
    );
}

#[test]
fn the_surface_driver_answers_faders_keys_and_sysex_through_the_session() {
    let mut s = server();
    open_demo(&mut s);
    let v = ok(&mut s, "session_surface", json!({"action": "attach"}));
    let sent: Vec<String> = v["sent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        sent[0], "F0 00 00 66 14 00 F7",
        "the device query comes first"
    );
    assert!(sent.len() > 20, "the whole surface follows: {}", sent.len());
    // The scribble strip of the first channel carries the instrument's name.
    assert!(
        sent.iter()
            .any(|m| m.starts_with("F0 00 00 66 14 12 00 42 61 73 73")),
        "Bass: {sent:?}"
    );
    // A fader move changes the project and is not echoed back.
    let before = fader(&mut s, "channel");
    let v = ok(
        &mut s,
        "session_surface",
        json!({"action": "send", "messages": "E0 7F 7F"}),
    );
    assert!((fader(&mut s, "channel") - 1.0).abs() < 1e-4, "{before}");
    assert!(
        v["sent"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| !m.as_str().unwrap().starts_with("E0 ")),
        "no echo: {v}"
    );
    // Undo moves the motor back.
    ok(&mut s, "session_command", json!({"id": "edit.undo"}));
    let v = ok(&mut s, "session_surface", json!({"action": "frame"}));
    assert!(
        v["sent"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m.as_str().unwrap().starts_with("E0 "))
    );
    // The mute key flips the mute and lights its LED.
    let v = ok(
        &mut s,
        "session_surface",
        json!({"action": "send", "messages": "90 10 7F"}),
    );
    assert!(
        v["sent"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "90 10 7F"),
        "{v}"
    );
    // A host connection query is answered with the reply and the surface is shown again.
    let v = ok(
        &mut s,
        "session_surface",
        json!({"action": "send", "messages": "F0 00 00 66 14 01 41 42 43 44 45 46 47 01 02 03 04 F7"}),
    );
    assert!(
        v["sent"][0]
            .as_str()
            .unwrap()
            .starts_with("F0 00 00 66 14 02 41 42 43 44 45 46 47 05 05 7B 2F F7"),
        "{v}"
    );
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["midi"]["surface"]["driver"], "mcu");
    let v = ok(&mut s, "session_surface", json!({"action": "detach"}));
    assert_eq!(
        v["sent"].as_array().unwrap().last().unwrap(),
        "F0 00 00 66 14 0F F7"
    );
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["midi"]["surface"], Value::Null);
}
