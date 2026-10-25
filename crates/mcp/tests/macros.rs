//! The macro and parameter tools: banks and assignments in a headless session, the parameter
//! search on a file and in the session, the inspection of a project with macros, and a bounce that
//! macros without effect leave bit for bit as it was.

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
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-macros");
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

fn macros(s: &mut Server, args: Value) -> Value {
    ok(s, "session_macros", args)
}

fn assignments(v: &Value) -> Vec<Value> {
    v["banks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["macros"].as_array().unwrap().iter())
        .flat_map(|m| m["assigns"].as_array().cloned().unwrap_or_default())
        .collect()
}

#[test]
fn the_tools_need_a_session_and_say_what_is_wrong() {
    let mut s = server();
    for (tool, args) in [
        ("session_macros", json!({"action": "new_bank"})),
        ("session_param_search", json!({"query": "cutoff"})),
    ] {
        let r = call(&mut s, tool, args);
        assert!(
            r.is_error && r.first_text().contains("session_open"),
            "{tool}"
        );
    }
    open_demo(&mut s);
    let bad = |s: &mut Server, args: Value| call(s, "session_macros", args).first_text().to_owned();
    assert!(bad(&mut s, json!({"action": "dance"})).contains("dance"));
    assert!(
        bad(
            &mut s,
            json!({"action": "assign", "device": "lead", "param": "filter.cutoff"})
        )
        .contains("macro")
    );
    assert!(
        bad(
            &mut s,
            json!({"action": "assign", "device": "lead", "param": "filter.cutoff", "macro": 9})
        )
        .contains("1 to 8")
    );
    assert!(bad(&mut s, json!({"action": "range", "id": 4, "min": 0})).contains("no assignment"));
    assert!(
        bad(&mut s, json!({"action": "set", "macro": 1, "value": 1})).contains("no macro bank")
    );
    let v = macros(&mut s, json!({"action": "clear", "bank": "x", "macro": 1}));
    assert_eq!(v["done"], false, "nothing to clear");
    let bad = |s: &mut Server, args: Value| {
        call(s, "session_param_search", args)
            .first_text()
            .to_owned()
    };
    assert!(bad(&mut s, json!({"action": "dance"})).contains("dance"));
    // A refusal of the model is data: the call succeeds and says `done` false with the message.
    let v = macros(
        &mut s,
        json!({"action": "assign", "device": "nope", "param": "filter.cutoff", "macro": 1}),
    );
    assert_eq!(v["done"], false);
    assert!(v["message"].as_str().unwrap().contains("nope"), "{v}");
}

#[test]
fn assigning_a_param_makes_a_bank_and_the_session_lists_it() {
    let mut s = server();
    open_demo(&mut s);
    let v = macros(
        &mut s,
        json!({"action": "assign", "device": "lead", "param": "filter.cutoff", "macro": 2,
               "min": 300, "max": 3000, "invert": true}),
    );
    assert_eq!(v["done"], true);
    assert_eq!(v["can_undo"], true);
    let bank = v["current_bank"].as_str().unwrap().to_owned();
    let list = assignments(&v);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["device"], "lead");
    assert_eq!(list[0]["param"], "filter.cutoff");
    assert_eq!(list[0]["min"], 300.0);
    assert_eq!(list[0]["max"], 3000.0);
    assert_eq!(list[0]["invert"], true);
    assert_eq!(v["banks"][0]["macros"][1]["macro"], 2);
    // session_status carries the same under macros.
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(status["document"]["macros"]["banks"][0]["bank"], bank);
    // A second param on the same macro, then rename, set, range and the refusal of a direct edit.
    macros(
        &mut s,
        json!({"action": "assign", "device": "bass", "param": "filter.resonance", "macro": 2}),
    );
    macros(
        &mut s,
        json!({"action": "rename", "bank": bank, "macro": 2, "name": "Brightness"}),
    );
    let v = macros(
        &mut s,
        json!({"action": "set", "bank": bank, "macro": 2, "value": 0.5}),
    );
    assert_eq!(v["banks"][0]["macros"][1]["name"], "Brightness");
    assert_eq!(v["banks"][0]["macros"][1]["value"], 0.5);
    assert_eq!(assignments(&v).len(), 2);
    let id = assignments(&v)[0]["id"].as_u64().unwrap();
    let v = macros(&mut s, json!({"action": "range", "id": id, "min": 500}));
    assert_eq!(assignments(&v)[0]["min"], 500.0);
    assert_eq!(
        assignments(&v)[0]["invert"],
        true,
        "what the call leaves out stays"
    );
    // The driven param refuses a direct edit: the session's set_param says why.
    let r = call(
        &mut s,
        "session_set_param",
        json!({"device": "lead", "param": "filter.cutoff", "value": 900}),
    );
    assert!(
        r.is_error || r.first_text().contains("driven"),
        "{}",
        r.first_text()
    );
    // Remove by device and param, then clear the rest.
    // A param with automation of its own cannot be driven: the lane would fight the macro.
    let v = macros(
        &mut s,
        json!({"action": "assign", "device": "bass", "param": "filter.cutoff", "macro": 3}),
    );
    assert_eq!(v["done"], false);
    assert!(v["message"].as_str().unwrap().contains("automation"), "{v}");
    let v = macros(
        &mut s,
        json!({"action": "remove", "device": "lead", "param": "filter.cutoff"}),
    );
    assert_eq!(assignments(&v).len(), 1);
    let v = macros(&mut s, json!({"action": "clear", "bank": bank, "macro": 2}));
    assert!(assignments(&v).is_empty());
}

#[test]
fn a_bounce_with_a_bank_and_no_assignment_is_bit_identical() {
    let mut s = server();
    open_demo(&mut s);
    let reference = tmp("reference.wav");
    ok(&mut s, "session_bounce", json!({"output": reference}));
    macros(&mut s, json!({"action": "new_bank"}));
    let b = ok(
        &mut s,
        "session_bounce",
        json!({"output": tmp("with-bank.wav"), "reference": reference}),
    );
    assert_eq!(b["comparison"]["bit_identical"], true);
}

#[test]
fn a_saved_project_keeps_its_macros_and_the_stateless_tools_read_them() {
    let mut s = server();
    open_demo(&mut s);
    macros(
        &mut s,
        json!({"action": "assign", "device": "lead", "param": "filter.cutoff", "macro": 1,
               "min": 300, "max": 3000}),
    );
    macros(&mut s, json!({"action": "set", "macro": 1, "value": 0.5}));
    let file = tmp("macros.atomik");
    ok(&mut s, "session_save", json!({"path": file}));
    let inspected = ok(&mut s, "project_inspect", json!({"path": file}));
    let list = assignments(&inspected["macros"]);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["device"], "lead");
    // The stateless search sees the driven value, the macro and the track.
    let v = ok(
        &mut s,
        "param_search",
        json!({"path": file, "query": "lead cutoff", "limit": 3}),
    );
    assert!(v["indexed"].as_u64().unwrap() > 50);
    let first = &v["results"][0];
    assert_eq!(first["track"], "Lead");
    assert_eq!(first["device"], "Lead");
    assert_eq!(first["target"]["device"], "lead");
    assert_eq!(first["target"]["param"], "filter.cutoff");
    assert_eq!(first["unit"], "Hz");
    assert_eq!(first["value"], 1650.0);
    assert_eq!(first["macro"]["macro"], 1);
    // Closing and reopening gives the same assignments.
    ok(&mut s, "session_open", json!({"path": file}));
    let status = ok(&mut s, "session_status", json!({}));
    assert_eq!(assignments(&status["document"]["macros"]).len(), 1);
    // A project without macros has no block.
    let plain = ok(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    assert!(plain.get("macros").is_none());
}

#[test]
fn the_stateless_search_ranks_filters_and_is_bounded() {
    let mut s = server();
    let demo = "examples/projects/demo.atomik";
    let v = ok(
        &mut s,
        "param_search",
        json!({"path": demo, "query": "bass cutoff"}),
    );
    let rows = v["results"].as_array().unwrap();
    assert_eq!(rows[0]["target"]["device"], "bass");
    assert_eq!(rows[0]["target"]["param"], "filter.cutoff");
    assert_eq!(rows[0]["value"], 700.0);
    // Every word must match; the path form works.
    let v = ok(
        &mut s,
        "param_search",
        json!({"path": demo, "query": "bass > nosuchparam"}),
    );
    assert!(v["results"].as_array().unwrap().is_empty());
    let v = ok(
        &mut s,
        "param_search",
        json!({"path": demo, "query": "echo > feedback"}),
    );
    assert_eq!(v["results"][0]["target"]["param"], "feedback");
    let v = ok(
        &mut s,
        "param_search",
        json!({"path": demo, "query": "", "limit": 7}),
    );
    assert_eq!(v["results"].as_array().unwrap().len(), 7);
    let v = ok(
        &mut s,
        "param_search",
        json!({"path": demo, "query": "a", "limit": 100000}),
    );
    assert!(v["results"].as_array().unwrap().len() <= 500);
    let r = call(
        &mut s,
        "param_search",
        json!({"path": "does-not-exist.atomik"}),
    );
    assert!(r.is_error);
}

#[test]
fn the_session_search_lists_rows_and_acts_on_one() {
    let mut s = server();
    open_demo(&mut s);
    let v = ok(
        &mut s,
        "session_param_search",
        json!({"query": "lead cutoff", "limit": 5}),
    );
    let rows = v["results"].as_array().unwrap();
    assert_eq!(rows[0]["row"], 0);
    assert_eq!(rows[0]["target"]["device"], "lead");
    assert!(rows[0]["value"].as_str().unwrap().contains("Hz"));
    // Adjust moves the value one step up, one undo step.
    let before = rows[0]["value"].as_str().unwrap().to_owned();
    let v = ok(
        &mut s,
        "session_param_search",
        json!({"query": "lead cutoff", "action": "adjust", "steps": 3}),
    );
    assert_eq!(v["done"], true);
    assert_ne!(v["results"][0]["value"].as_str().unwrap(), before);
    // Assign takes the next free macro of a new bank, and the row says so.
    let v = ok(
        &mut s,
        "session_param_search",
        json!({"query": "lead cutoff", "action": "assign"}),
    );
    assert_eq!(v["done"], true);
    assert_eq!(v["results"][0]["macro"], "M1");
    // Learn on a driven row is refused with the reason; a free row arms.
    let v = ok(
        &mut s,
        "session_param_search",
        json!({"query": "lead cutoff", "action": "learn"}),
    );
    assert_eq!(v["done"], false);
    assert!(v["message"].as_str().unwrap().contains("macro"), "{v}");
    let v = ok(
        &mut s,
        "session_param_search",
        json!({"query": "lead resonance", "action": "learn"}),
    );
    assert_eq!(v["done"], true);
    let status = ok(&mut s, "session_status", json!({}));
    assert!(status["midi"]["armed"].is_string());
    // Reveal selects the device; a row that is not there does nothing.
    let v = ok(
        &mut s,
        "session_param_search",
        json!({"query": "bass cutoff", "action": "reveal"}),
    );
    assert_eq!(v["done"], true);
    let v = ok(
        &mut s,
        "session_param_search",
        json!({"query": "bass cutoff", "action": "reveal", "row": 400}),
    );
    assert_eq!(v["done"], false);
}

#[test]
fn following_the_selection_and_showing_a_lane_add_an_empty_lane() {
    let mut s = server();
    open_demo(&mut s);
    let v = macros(&mut s, json!({"action": "follow", "on": true}));
    assert_eq!(v["follow_selection"], true);
    // The lane of a param of a device with a track.
    let v = macros(
        &mut s,
        json!({"action": "lane", "device": "bass", "param": "filter.resonance"}),
    );
    assert_eq!(v["done"], true);
    let inspected = ok(&mut s, "session_status", json!({}));
    let bass = inspected["document"]["song"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["device"] == "bass")
        .unwrap()
        .clone();
    let lane = bass["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["kind"] == "automation:filter.resonance")
        .unwrap()
        .clone();
    assert_eq!(lane["clips"], 0, "no data");
    // A param of a delay has no lane: automate it through a macro.
    let v = macros(
        &mut s,
        json!({"action": "lane", "device": "echo", "param": "mix"}),
    );
    assert_eq!(v["done"], false);
    assert!(v["message"].as_str().unwrap().contains("macro"), "{v}");
    // A param a macro drives shows the macro's lane.
    macros(
        &mut s,
        json!({"action": "assign", "device": "echo", "param": "mix", "macro": 1}),
    );
    let v = macros(
        &mut s,
        json!({"action": "lane", "device": "echo", "param": "mix"}),
    );
    assert_eq!(v["done"], true);
    let v = macros(&mut s, json!({"action": "follow", "on": false}));
    assert_eq!(v["follow_selection"], false);
}
