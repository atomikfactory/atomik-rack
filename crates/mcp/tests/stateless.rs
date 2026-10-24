//! The stateless tools against the example patches and projects, and the path policy.

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

/// A path under `target/tmp` (inside the allowed root), made fresh.
fn tmp(name: &str) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-stateless");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

#[test]
fn list_devices_lists_and_describes() {
    let mut s = server();
    let all = call(&mut s, "list_devices", json!({}));
    assert!(!all.is_error);
    let v = all.parsed();
    assert!(v["count"].as_u64().unwrap() > 10);
    let one = call(
        &mut s,
        "list_devices",
        json!({"type_id": "atomik.poly-synth"}),
    )
    .parsed();
    assert_eq!(one["kind"], "instrument");
    let cutoff = one["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == "filter.cutoff")
        .expect("a cutoff parameter");
    assert!(cutoff["min"].as_f64().unwrap() < cutoff["max"].as_f64().unwrap());
    assert!(
        one["sockets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["direction"] == "output")
    );
    let fx = call(&mut s, "list_devices", json!({"kind": "effect"})).parsed();
    assert!(
        fx["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["kind"] == "effect")
    );
    let q = call(&mut s, "list_devices", json!({"query": "POLY"})).parsed();
    assert!(q["count"].as_u64().unwrap() >= 1);
    let unknown = call(&mut s, "list_devices", json!({"type_id": "nope"}));
    assert!(unknown.is_error);
}

#[test]
fn project_inspect_describes_a_project_and_a_patch() {
    let mut s = server();
    let p = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    assert!(!p.is_error, "{}", p.first_text());
    let v = p.parsed();
    assert_eq!(v["kind"], "project");
    assert_eq!(v["song"]["tracks"].as_array().unwrap().len(), 2);
    assert!(
        v["song"]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["chase"] == true),
        "every track reports its chase flag, on for a project that never set it"
    );
    let lanes: Vec<&serde_json::Value> = v["song"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["lanes"].as_array().unwrap())
        .filter(|l| l["kind"].as_str().unwrap().starts_with("automation:"))
        .collect();
    assert!(!lanes.is_empty());
    assert!(
        lanes.iter().all(|l| l["write"] == "touch"),
        "an automation lane reports its write mode (touch unless the project says otherwise)"
    );
    assert!(
        v["song"]["tracks"][0]["lanes"][0].get("write").is_none(),
        "a note lane has no write mode"
    );
    assert!(
        v["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["key"] == "bass")
    );
    assert!(
        v["cables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "bass:out_l -> channel:in_l")
    );
    assert_eq!(v["song"]["ticks_per_beat"], 15360);
    assert!(v["devices"][0].get("params").is_none());
    let with = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/demo.atomik", "include_params": true}),
    )
    .parsed();
    assert!(with["devices"][0]["params"].is_object());

    let patch = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/patches/minimal.json"}),
    );
    assert!(!patch.is_error, "{}", patch.first_text());
    assert_eq!(patch.parsed()["kind"], "patch");

    // A bundle folder resolves to its project file.
    let folder = call(
        &mut s,
        "project_inspect",
        json!({"path": "examples/projects/audio-demo"}),
    );
    assert!(!folder.is_error, "{}", folder.first_text());
}

#[test]
fn validate_reports_data_not_failures() {
    let mut s = server();
    let ok = call(
        &mut s,
        "validate",
        json!({"path": "examples/patches/minimal.json"}),
    );
    assert!(!ok.is_error);
    assert_eq!(ok.parsed()["valid"], true);

    let bad = tmp("bad.json");
    std::fs::write(
        &bad,
        r#"{"format":"atomik-patch","version":1,"sample_rate":48000,"tempo_bpm":120,
            "length_beats":1,"output_channels":2,
            "devices":[{"id":"a","type":"atomik.no-such-device"}],"cables":[]}"#,
    )
    .unwrap();
    let r = call(&mut s, "validate", json!({"path": bad}));
    assert!(!r.is_error, "an invalid file is a result");
    let v = r.parsed();
    assert_eq!(v["valid"], false);
    assert!(v["error"].as_str().unwrap().contains("devices[0]"), "{v}");

    let proj = call(
        &mut s,
        "validate",
        json!({"path": "examples/projects/demo.atomik"}),
    );
    assert_eq!(proj.parsed()["valid"], true);
}

#[test]
fn a_bounce_is_deterministic_across_blocks_and_workers() {
    let mut s = server();
    let a = tmp("demo-a.wav");
    let b = tmp("demo-b.wav");
    let ra = call(
        &mut s,
        "bounce",
        json!({"path": "examples/projects/demo.atomik", "output": a, "block": 512}),
    );
    assert!(!ra.is_error, "{}", ra.first_text());
    let rb = call(
        &mut s,
        "bounce",
        json!({"path": "examples/projects/demo.atomik", "output": b, "block": 333, "workers": 2}),
    );
    assert!(!rb.is_error, "{}", rb.first_text());
    let audio = ra.parsed()["audio"].clone();
    assert_eq!(audio["silent"], false);
    assert_eq!(audio["channels"], 2);
    assert_eq!(audio["non_finite_samples"], 0);
    assert!(audio["duration_s"].as_f64().unwrap() > 10.0);
    assert!(audio["window_rms_dbfs"].as_array().unwrap().len() >= 10);
    assert_eq!(audio["spectrum"]["bands"].as_array().unwrap().len(), 8);

    let cmp = call(&mut s, "compare_audio", json!({"a": a, "b": b})).parsed();
    assert_eq!(cmp["bit_identical"], true, "{cmp}");
    assert_eq!(cmp["matches"], true);

    // Analysing the written file agrees with the report of the bounce.
    let an = call(&mut s, "analyze_audio", json!({"path": a})).parsed();
    assert_eq!(an["frames"], audio["frames"]);
    assert_eq!(an["peak_dbfs"], audio["peak_dbfs"]);
}

#[test]
fn a_bounce_can_compare_itself_with_a_reference() {
    let mut s = server();
    let reference = tmp("ref.wav");
    let first = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": reference}),
    );
    assert!(!first.is_error, "{}", first.first_text());
    let second = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": tmp("again.wav"),
               "reference": reference, "format": "f32"}),
    );
    let c = second.parsed()["comparison"].clone();
    assert_eq!(c["matches"], true, "{c}");
    assert_eq!(c["bit_identical"], true);
}

#[test]
fn a_changed_patch_is_caught_against_its_reference() {
    let mut s = server();
    let reference = tmp("before.wav");
    call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": reference}),
    );
    // The same patch with the filter nearly closed: darker, so quieter in the loudness windows.
    let text = std::fs::read_to_string(repo().join("examples/patches/minimal.json")).unwrap();
    let edited = tmp("edited.json");
    std::fs::write(
        &edited,
        text.replace("\"filter.cutoff\": 1500", "\"filter.cutoff\": 120"),
    )
    .unwrap();
    let after = tmp("after.wav");
    let r = call(
        &mut s,
        "bounce",
        json!({"path": edited, "output": after, "reference": reference}),
    );
    assert!(!r.is_error, "{}", r.first_text());
    let c = r.parsed()["comparison"].clone();
    assert_eq!(c["bit_identical"], false, "{c}");
    assert_eq!(c["same_shape"], true);
    assert!(c["max_abs_diff"].as_f64().unwrap() > 0.0);
    // A tolerance wide enough accepts it; the default (the goldens' 0.5 dB) does not.
    let loose = call(
        &mut s,
        "compare_audio",
        json!({"a": reference, "b": after, "tolerance_db": 60.0}),
    )
    .parsed();
    assert_eq!(loose["matches"], true, "{loose}");
    let strict = call(&mut s, "compare_audio", json!({"a": reference, "b": after})).parsed();
    assert_eq!(strict["matches"], false, "{strict}");
}

#[test]
fn bounce_refuses_what_it_cannot_do_with_a_clear_message() {
    let mut s = server();
    let plugins = call(
        &mut s,
        "bounce",
        json!({"path": "examples/projects/plugin-demo"}),
    );
    assert!(plugins.is_error);
    assert!(
        plugins.first_text().contains("--allow-plugins"),
        "{}",
        plugins.first_text()
    );
    let patch_with_loop = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "loop_count": 2}),
    );
    assert!(patch_with_loop.is_error);
    let bad_block = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "block": 0}),
    );
    assert!(bad_block.is_error);
    let bad_format = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "format": "mp3"}),
    );
    assert!(bad_format.is_error);
}

#[test]
fn compare_audio_flags_a_different_shape() {
    let mut s = server();
    let a = tmp("short.wav");
    let b = tmp("long.wav");
    call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": a}),
    );
    call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/slice-demo.json", "output": b}),
    );
    let c = call(&mut s, "compare_audio", json!({"a": a, "b": b})).parsed();
    assert_eq!(c["same_shape"], false);
    assert_eq!(c["matches"], false);
}

#[test]
fn list_plugins_reads_a_cache_and_survives_a_missing_one() {
    let mut s = server();
    let missing = tmp("no-cache.json");
    std::fs::write(&missing, "not a cache").unwrap();
    let r = call(&mut s, "list_plugins", json!({"cache": missing}));
    assert!(!r.is_error, "{}", r.first_text());
    let v = r.parsed();
    assert_eq!(v["count"], 0);
    assert!(v["cache"].as_str().unwrap().starts_with("Rebuilt"), "{v}");
}

#[test]
fn paths_outside_the_allowed_folders_are_refused() {
    let mut s = server();
    let outside = std::env::temp_dir().join("atomik-mcp-outside.json");
    std::fs::write(&outside, "{}").unwrap();
    let read = call(
        &mut s,
        "validate",
        json!({"path": outside.to_string_lossy()}),
    );
    assert!(read.is_error);
    assert!(
        read.first_text().contains("outside the allowed folders"),
        "{}",
        read.first_text()
    );
    let escape = call(
        &mut s,
        "validate",
        json!({"path": "../../../../../../../../../../../../windows/win.ini"}),
    );
    assert!(escape.is_error);
    let write = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json",
               "output": std::env::temp_dir().join("atomik-mcp-out.wav").to_string_lossy()}),
    );
    assert!(write.is_error);
    assert!(write.first_text().contains("outside the allowed folders"));
    // `..` through a folder that does not exist yet must not slip out either.
    let dotdot = call(
        &mut s,
        "bounce",
        json!({"path": "examples/patches/minimal.json", "output": "newdir/../../../escape.wav"}),
    );
    assert!(dotdot.is_error, "{}", dotdot.first_text());
    assert!(!repo().join("newdir").exists());
    let _ = std::fs::remove_file(outside);
}

#[test]
fn an_extra_root_allows_its_folder() {
    let extra = std::env::temp_dir().join("atomik-mcp-extra-root");
    std::fs::create_dir_all(&extra).unwrap();
    let file = extra.join("p.json");
    std::fs::copy(repo().join("examples/patches/minimal.json"), &file).unwrap();
    let mut s = Server::new(Paths::new(repo(), std::slice::from_ref(&extra)));
    let r = call(&mut s, "validate", json!({"path": file.to_string_lossy()}));
    assert!(!r.is_error, "{}", r.first_text());
    assert_eq!(r.parsed()["valid"], true);
    let _ = std::fs::remove_dir_all(extra);
}

#[test]
fn argument_types_are_checked() {
    let mut s = server();
    let r = call(&mut s, "bounce", json!({"path": 3}));
    assert!(r.is_error);
    assert!(r.first_text().contains("must be a string"));
    let r = call(
        &mut s,
        "analyze_audio",
        json!({"path": "x.wav", "window_s": "long"}),
    );
    assert!(r.is_error);
}
