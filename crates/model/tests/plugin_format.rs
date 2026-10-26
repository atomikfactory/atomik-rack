//! Project format v9 (plugin devices): the `plugin` object, canonical
//! text, the committed golden, rejection with JSON paths, stand-ins for missing plugins, dead
//! param keys, and the files of versions 1 to 8 still loading.

mod common;

use atomik_device_sdk::PluginStateBlob;
use atomik_device_sdk::Registry;
use atomik_model::{
    Command, Curve, Document, ModelError, PROJECT_VERSION, PluginCommand, RackDoc, empty_doc,
    load_project, register_stand_ins, save_project,
};
use common::plugin::*;
use common::song::{add_clip, auto_clip, point};
use common::*;
use serde_json::{Value, json};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
    .replace("\r\n", "\n")
}

/// The document of the golden: a built-in synth, a CLAP synth with state, params, pins and an
/// automation lane, a VST3 effect (powered off, pinned to the audio thread), and a container
/// with a CLAP effect that a macro drives.
fn project(r: &Registry) -> RackDoc {
    let mut d = Document::new(empty_doc());
    let mut lead = add_plugin(1, &synth_ref(), &synth_layout(), Some("lead"));
    if let Command::Plugin(PluginCommand::Add { path_hint, .. }) = &mut lead {
        *path_hint = Some("C:/Program Files/Common Files/CLAP/Test \"Synth\".clap".into());
    }
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("plain")),
            lead,
            add_plugin(2, &vst3_ref(), &vst3_layout(), Some("verb")),
            add(3, CONTAINER, Some("layer")),
            add_plugin_inner("layer", 0, &fx_ref(), &fx_layout(), Some("comp"), false),
            connect("layer/boundary:in_l", "layer/comp:in_l"),
            connect("layer/boundary:in_r", "layer/comp:in_r"),
            connect("layer/comp:out_l", "layer/boundary:out_l"),
            connect("layer/comp:out_r", "layer/boundary:out_r"),
            set_macro("layer", 0, target("comp", "p1", -60.0, 12.0)),
            sync_state("layer/comp", &[0xde, 0xad, 0xbe, 0xef, 0, 1, 2]),
            connect("lead:out_l", "verb:in_l"),
            connect("lead:out_r", "verb:in_r"),
            connect("verb:out_l", "layer:in_l"),
            connect("verb:out_r", "layer:in_r"),
            set_param("lead", "p10", 0.25),
            set_param("lead", "p42", 4.0),
            set_param("verb", "p5", 0.5),
            set_param("verb", "p7", 2.0),
            sync_state("lead", b"synth patch"),
            sync_state("verb", &[0, 1, 2]),
            set_pinned("lead", &[42, 10]),
            Command::Plugin(PluginCommand::SetPower {
                device: key("verb"),
                powered: false,
            }),
            Command::Plugin(PluginCommand::SetRunMode {
                device: key("verb"),
                run_on_audio_thread: true,
            }),
            common::song::add_track("lead"),
            Command::Song(atomik_model::SongCommand::AddAutomationLane {
                device: key("lead"),
                param: "p11".into(),
            }),
            add_clip(
                "lead",
                1,
                auto_clip(
                    0,
                    960,
                    vec![point(0, 0.2, Curve::Linear), point(480, 0.8, Curve::Step)],
                ),
            ),
        ],
    );
    d.doc().clone()
}

fn text(r: &Registry) -> String {
    save_project(&project(r))
}

fn rejects(text: &str, r: &Registry, path: &str, needle: &str) {
    let e = load_project(text, r).expect_err("must be rejected");
    let ModelError::Patch(msg) = e else {
        panic!("not a load error: {e}");
    };
    assert!(
        msg.starts_with(&format!("{path}: ")),
        "{msg} lacks path {path}"
    );
    assert!(msg.contains(needle), "{msg} lacks {needle:?}");
}

fn edited(text: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    edit(&mut v);
    v.to_string()
}

/// The raw text of every `"plugin": {...}` object in `text`, in file order.
pub fn plugin_texts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find("\"plugin\": {") {
        let start = from + at + "\"plugin\": ".len();
        let (mut depth, mut in_str, mut esc) = (0, false, false);
        let mut end = start;
        for (i, &b) in bytes[start..].iter().enumerate() {
            if in_str {
                match (esc, b) {
                    (true, _) => esc = false,
                    (false, b'\\') => esc = true,
                    (false, b'"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push(text[start..end].to_owned());
        from = end;
    }
    out
}

// ---- the version -----------------------------------------------------------------------------

#[test]
fn the_format_is_version_11() {
    assert_eq!(PROJECT_VERSION, 32);
    let r = plugin_registry();
    assert!(text(&r).contains("\"version\": 32"));
}

#[test]
fn a_project_without_plugins_differs_from_v8_only_in_the_version_number() {
    let r = registry();
    let t = fixture("container-v8.atomik");
    let doc = load_project(&t, &r).unwrap();
    assert_eq!(save_project(&doc), as_current(&t, 8));
    assert!(!save_project(&doc).contains("\"plugin\""));
}

// ---- the plugin object ----------------------------------------------------------------------------

#[test]
fn the_plugin_object_is_written_with_the_planned_fields_in_order() {
    let r = plugin_registry();
    let t = text(&r);
    let plugins = plugin_texts(&t);
    assert_eq!(plugins.len(), 3, "lead, verb, comp");
    let lead = &plugins[0];
    let order = [
        "\"format\"",
        "\"id\"",
        "\"name\"",
        "\"vendor\"",
        "\"version\"",
        "\"path_hint\"",
        "\"layout\"",
        "\"state\"",
        "\"pinned\"",
        "\"run_on_audio_thread\"",
        "\"powered\"",
    ];
    let at: Vec<usize> = order
        .iter()
        .map(|k| {
            lead.find(k)
                .unwrap_or_else(|| panic!("{k} missing in {lead}"))
        })
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "field order: {at:?}");
    let layout_order = [
        "\"main_in\"",
        "\"main_out\"",
        "\"sidechain\"",
        "\"notes_in\"",
        "\"notes_out\"",
        "\"kind\"",
        "\"params\"",
    ];
    let at: Vec<usize> = layout_order.iter().map(|k| lead.find(k).unwrap()).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "layout order: {at:?}");

    let v: Value = serde_json::from_str(&format!("{{\"plugin\": {lead}}}")).unwrap();
    let p = &v["plugin"];
    assert_eq!(p["format"], "clap");
    assert_eq!(p["id"], "test.synth");
    assert_eq!(p["name"], "Test Synth");
    assert_eq!(p["vendor"], "Atomik Test");
    assert_eq!(p["version"], "1.2.3");
    assert_eq!(
        p["path_hint"],
        "C:/Program Files/Common Files/CLAP/Test \"Synth\".clap"
    );
    assert_eq!(p["state"], "c3ludGggcGF0Y2g=");
    assert_eq!(p["pinned"], json!([42, 10]));
    assert_eq!(p["run_on_audio_thread"], false);
    assert_eq!(p["powered"], true);
    let layout = &p["layout"];
    assert_eq!(layout["main_in"], Value::Null);
    assert_eq!(layout["main_out"], "stereo");
    assert_eq!(layout["sidechain"], false);
    assert_eq!(layout["notes_in"], true);
    assert_eq!(layout["notes_out"], false);
    assert_eq!(layout["kind"], "instrument");
    // The rows are the params the project uses (a value, a lane, a pin), sorted by id.
    let ids: Vec<u64> = layout["params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [10, 11, 42]);
    assert_eq!(
        layout["params"][0],
        json!({"id": 10, "name": "Cutoff", "module": "Filter", "min": 0, "max": 1,
               "default": 0.5, "stepped": false, "steps": 0})
    );
    assert_eq!(
        layout["params"][2],
        json!({"id": 42, "name": "Voices", "min": 1, "max": 16, "default": 8,
               "stepped": true, "steps": 0}),
        "module is written only when it is not empty"
    );
}

#[test]
fn params_are_sparse_and_a_plugin_without_state_has_no_state_field() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &fx_ref(), &fx_layout(), Some("f")),
            set_param("f", "p2", 1.0),
        ],
    );
    let t = save_project(d.doc());
    let v: Value = serde_json::from_str(&t).unwrap();
    let dev = &v["devices"][0];
    assert_eq!(dev["params"], json!({"p2": 1}));
    assert!(dev["plugin"].get("state").is_none());
    assert!(dev["plugin"].get("path_hint").is_none());
    let rows = dev["plugin"]["layout"]["params"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the used param has a row");
    assert_eq!(rows[0]["id"], 2);
}

#[test]
fn a_plugin_with_nothing_used_writes_an_empty_param_table() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add_plugin(0, &fx_ref(), &fx_layout(), Some("f"))],
    );
    let v: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    assert_eq!(v["devices"][0]["plugin"]["layout"]["params"], json!([]));
    assert!(v["devices"][0].get("params").is_none());
}

#[test]
fn a_big_state_round_trips() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    let blob: Vec<u8> = (0..1_000_003u32).map(|i| (i * 31 % 251) as u8).collect();
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &fx_ref(), &fx_layout(), Some("f")),
            sync_state("f", &blob),
        ],
    );
    let t = save_project(d.doc());
    assert!(!t.contains('\r'));
    let back = load_project(&t, &r).unwrap();
    assert_eq!(
        plugin_of(&back, "f").state.as_deref(),
        Some(&PluginStateBlob(blob))
    );
    assert_eq!(save_project(&back), t);
}

#[test]
fn state_lengths_around_the_padding_round_trip() {
    let r = plugin_registry();
    for len in 0..8usize {
        let blob: Vec<u8> = (0..len).map(|i| 250 - i as u8).collect();
        let mut d = Document::new(empty_doc());
        run(
            &mut d,
            &r,
            [
                add_plugin(0, &fx_ref(), &fx_layout(), Some("f")),
                sync_state("f", &blob),
            ],
        );
        let t = save_project(d.doc());
        let back = load_project(&t, &r).unwrap();
        assert_eq!(
            plugin_of(&back, "f").state.as_deref(),
            Some(&PluginStateBlob(blob)),
            "length {len}"
        );
    }
}

// ---- canonical text, the golden -------------------------------------------------------------------

#[test]
fn save_load_save_is_canonical_and_the_reload_is_the_compacted_document() {
    let r = plugin_registry();
    let doc = project(&r);
    let t = save_project(&doc);
    let back = load_project(&t, &r).unwrap();
    assert_eq!(back, doc.compacted(), "a reload keeps the used rows only");
    assert_ne!(back, doc, "the in-session layouts hold the full tables");
    assert_eq!(save_project(&back), t);
    // Compacting is idempotent and does not change what is saved.
    assert_eq!(doc.compacted().compacted(), doc.compacted());
    assert_eq!(save_project(&doc.compacted()), t);
}

#[test]
fn the_committed_golden_is_the_canonical_save() {
    let r = plugin_registry();
    let t = text(&r);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugin-v9.atomik");
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, as_old(&t, 9)).unwrap();
    }
    // The golden stays a version 9 file (it also proves that v9 still loads); it differs from
    // the current save in the version number only.
    let file = fixture("plugin-v9.atomik");
    assert_eq!(
        as_current(&file, 9),
        t,
        "regenerate with ATOMIK_UPDATE_GOLDEN=1"
    );
}

#[test]
fn the_golden_loads_with_the_plugins_installed_and_as_stand_ins_with_the_same_result() {
    let installed = plugin_registry();
    let bare = registry();
    let file = fixture("plugin-v9.atomik");
    let current = as_current(&file, 9);
    let a = load_project(&file, &installed).unwrap();
    let b = load_project(&file, &bare).expect("a missing plugin loads as a stand-in");
    assert_eq!(a, b, "stand-in or installed: the same document");
    assert_eq!(save_project(&a), current);
    assert_eq!(
        save_project(&b),
        current,
        "an unedited stand-in saves unchanged"
    );
    assert_eq!(a, project(&installed).compacted());
    // The stand-in types are not in the caller's registry; the helper adds them.
    assert!(!bare.is_plugin(CLAP_SYNTH));
    let mut reg = registry();
    let added = register_stand_ins(&b, &mut reg).unwrap();
    assert_eq!(added, [CLAP_FX, CLAP_SYNTH, VST3_FX]);
    // The commands now work on the stand-in document.
    let mut d = Document::new(b);
    run(
        &mut d,
        &reg,
        [
            set_param("lead", "p10", 0.75),
            Command::Rename {
                key: key("lead"),
                name: "Lead".into(),
            },
        ],
    );
}

#[test]
fn a_stand_in_keeps_cables_state_lanes_macros_pins_and_params_across_load_and_save() {
    let file = fixture("plugin-v9.atomik");
    let before = plugin_texts(&file);
    assert_eq!(before.len(), 3);
    let bare = registry();
    let doc = load_project(&file, &bare).unwrap();
    // Everything the plugin devices touch is in the document.
    assert_eq!(doc.cables.len(), 4, "cables to and from stand-ins");
    assert_eq!(
        plugin_of(&doc, "lead").pinned,
        [42, 10],
        "pins in the user's order"
    );
    assert!(doc.song.track(&key("lead")).is_some());
    let inner = doc.inner(&key("layer")).unwrap();
    assert_eq!(inner.macros[0].targets[0].param, "p1");
    assert_eq!(
        doc.device(&key("verb")).unwrap().params["p7"],
        2.0,
        "params"
    );
    assert_eq!(plugin_texts(&save_project(&doc)), before, "byte for byte");
}

// ---- versions 1 to 8 ---------------------------------------------------------------------------------------

#[test]
fn versions_one_to_eight_still_load_and_resave_as_v11() {
    let r = registry();
    let cr = console_registry();
    for (name, reg, from) in [
        ("demo-v1.atomik", &cr, 1),
        ("demo-v6.atomik", &cr, 6),
        ("drum-demo-v6.atomik", &cr, 6),
        ("seq-v7.atomik", &r, 7),
        ("container-v8.atomik", &r, 8),
    ] {
        let t = fixture(name);
        let doc = load_project(&t, reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let again = save_project(&doc);
        assert!(again.contains("\"version\": 32"), "{name}");
        assert_eq!(load_project(&again, reg).unwrap(), doc, "{name}");
        assert_eq!(save_project(&load_project(&again, reg).unwrap()), again);
        if from >= 6 {
            assert_eq!(again, as_current(&t, from), "{name}");
        }
    }
}

// ---- rejection with JSON paths --------------------------------------------------------------------------------

#[test]
fn plugin_needs_version_9_and_a_plugin_type() {
    let r = plugin_registry();
    let good = fixture("plugin-v9.atomik");
    rejects(
        &edited(&good, |v| v["version"] = json!(8)),
        &r,
        "devices[1].plugin",
        "unknown field",
    );
    rejects(
        &edited(&good, |v| {
            v["devices"][0]["plugin"] = v["devices"][1]["plugin"].clone()
        }),
        &r,
        "devices[0].plugin",
        "not a plugin",
    );
    // A plugin type id without a plugin object.
    rejects(
        &edited(&good, |v| {
            v["devices"][1].as_object_mut().unwrap().remove("plugin");
        }),
        &r,
        "devices[1].plugin",
        "missing field",
    );
    // The same with the plugin not installed (a stand-in cannot be built without the object).
    rejects(
        &edited(&good, |v| {
            v["devices"][1].as_object_mut().unwrap().remove("plugin");
        }),
        &registry(),
        "devices[1].plugin",
        "missing field",
    );
}

#[test]
fn plugin_fields_are_checked_with_their_paths() {
    let r = plugin_registry();
    let good = fixture("plugin-v9.atomik");
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| v["devices"][1]["plugin"]["format"] = json!("au")),
            "devices[1].plugin.format",
            "clap",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["format"] = json!("vst3")),
            "devices[1].plugin.format",
            "type",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["id"] = json!("other.plugin")),
            "devices[1].plugin.id",
            "type",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["name"] = json!(5)),
            "devices[1].plugin.name",
            "string",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["extra"] = json!(5)),
            "devices[1].plugin.extra",
            "unknown field",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["plugin"]
                    .as_object_mut()
                    .unwrap()
                    .remove("layout");
            }),
            "devices[1].plugin.layout",
            "missing field",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["layout"]["main_in"] = json!("quad")),
            "devices[1].plugin.layout.main_in",
            "mono",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["layout"]["kind"] = json!("drum")),
            "devices[1].plugin.layout.kind",
            "instrument",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["layout"]["sidechain"] = json!("yes")),
            "devices[1].plugin.layout.sidechain",
            "boolean",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["layout"]["extra"] = json!(1)),
            "devices[1].plugin.layout.extra",
            "unknown field",
        ),
        (
            Box::new(|v| {
                let rows = v["devices"][1]["plugin"]["layout"]["params"]
                    .as_array_mut()
                    .unwrap();
                let first = rows[0].clone();
                rows[1] = first;
            }),
            "devices[1].plugin.layout.params[1]",
            "duplicate",
        ),
        (
            Box::new(|v| {
                let rows = v["devices"][1]["plugin"]["layout"]["params"]
                    .as_array_mut()
                    .unwrap();
                rows[0]["min"] = json!(2.0);
                rows[0]["max"] = json!(1.0);
            }),
            "devices[1].plugin.layout.params[0]",
            "range",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["plugin"]["layout"]["params"][2]["id"] = json!(-1);
            }),
            "devices[1].plugin.layout.params[2].id",
            "integer",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["plugin"]["layout"]["params"][0]["stepped"] = json!(1);
            }),
            "devices[1].plugin.layout.params[0].stepped",
            "boolean",
        ),
        (
            Box::new(|v| {
                let l = &mut v["devices"][1]["plugin"]["layout"];
                l["notes_in"] = json!(false);
                l["main_out"] = Value::Null;
            }),
            "devices[1].plugin.layout",
            "socket",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["state"] = json!("not base64!")),
            "devices[1].plugin.state",
            "base64",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["state"] = json!("c3ludGggcGF0Y2g")),
            "devices[1].plugin.state",
            "base64",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["state"] = json!("c3ludGggcGF0Y2h=")),
            "devices[1].plugin.state",
            "base64",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["state"] = json!(12)),
            "devices[1].plugin.state",
            "string",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["plugin"]["pinned"] =
                    json!((0..17).map(|i| 10 + i).collect::<Vec<u32>>());
            }),
            "devices[1].plugin.pinned",
            "16",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["pinned"] = json!([10, 10])),
            "devices[1].plugin.pinned[1]",
            "duplicate",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["pinned"] = json!([1.5])),
            "devices[1].plugin.pinned[0]",
            "integer",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["powered"] = json!(1)),
            "devices[1].plugin.powered",
            "boolean",
        ),
        (
            Box::new(|v| v["devices"][1]["plugin"]["run_on_audio_thread"] = json!("on")),
            "devices[1].plugin.run_on_audio_thread",
            "boolean",
        ),
        (
            Box::new(|v| v["devices"][1]["params"]["pzz"] = json!(1.0)),
            "devices[1].params.pzz",
            "no param",
        ),
        (
            Box::new(|v| v["devices"][1]["params"]["p010"] = json!(1.0)),
            "devices[1].params.p010",
            "no param",
        ),
        (
            Box::new(|v| v["devices"][1]["params"]["p99999999999"] = json!(1.0)),
            "devices[1].params.p99999999999",
            "no param",
        ),
        (
            // A cable to a socket the saved layout does not have.
            Box::new(|v| {
                v["cables"][0]["to"] = json!("verb:sc_in_l");
            }),
            "cables[0].to",
            "sc_in_l",
        ),
        (
            // Inside a container.
            Box::new(|v| {
                v["devices"][3]["inner"]["devices"][0]["plugin"]["state"] = json!("@@@");
            }),
            "devices[3].inner.devices[0].plugin.state",
            "base64",
        ),
    ];
    for (edit, path, needle) in cases {
        for reg in [&r, &registry()] {
            let t = edited(&good, |v| edit(v));
            rejects(&t, reg, path, needle);
        }
    }
}

#[test]
fn a_state_over_16_mib_is_rejected_at_its_path() {
    let r = plugin_registry();
    let good = fixture("plugin-v9.atomik");
    // 22 369 624 base64 characters decode to 16 777 218 bytes: two over the cap.
    let big = "A".repeat(22_369_624);
    let t = edited(&good, |v| v["devices"][1]["plugin"]["state"] = json!(big));
    rejects(&t, &r, "devices[1].plugin.state", "16 MiB");
    // 22 369 620 characters (one padding-free fewer group) decode to 16 777 215: fine.
    let ok = "A".repeat(22_369_620);
    let t = edited(&good, |v| v["devices"][1]["plugin"]["state"] = json!(ok));
    let doc = load_project(&t, &r).unwrap();
    assert_eq!(
        plugin_of(&doc, "lead").state.as_ref().unwrap().0.len(),
        16_777_215
    );
}

#[test]
fn the_installed_descriptor_wins_over_the_saved_layout_for_validation() {
    // The saved layout says p10 exists (row), the installed plugin (an update) dropped it:
    // the stored value is a dead key, kept; nothing is an error at load.
    let mut updated = plugin_registry();
    updated.unregister_plugin(CLAP_SYNTH).unwrap();
    let mut cut = synth_layout();
    cut.params.retain(|p| p.id != 10);
    register_plugin(&mut updated, &synth_ref(), &cut);
    let file = fixture("plugin-v9.atomik");
    let doc = load_project(&file, &updated).expect("an update that drops a param still loads");
    assert_eq!(doc.device(&key("lead")).unwrap().params["p10"], 0.25);
    assert_eq!(
        save_project(&doc),
        as_current(&file, 9),
        "and saves back unchanged (but for the version)"
    );
}

#[test]
fn dead_param_keys_are_kept_in_params_lanes_and_macro_targets() {
    let installed = plugin_registry();
    let bare = registry();
    let file = fixture("plugin-v9.atomik");
    let t = edited(&file, |v| {
        v["devices"][1]["params"]["p77"] = json!(0.5);
        let lanes = v["song"]["tracks"][0]["lanes"].as_array_mut().unwrap();
        lanes.push(json!({"kind": "automation", "param": "p77",
            "clips": [{"id": 5, "start": 0, "length": 480,
                       "points": [[0, 7.5, "linear"], [240, -3.25, "step"]]}]}));
        v["song"]["next_clip_id"] = json!(6);
        v["devices"][3]["inner"]["macros"][2]["targets"] =
            json!([{"device": "comp", "param": "p77", "min": 0.0, "max": 1.0}]);
    });
    for reg in [&installed, &bare] {
        let doc = load_project(&t, reg).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(doc.device(&key("lead")).unwrap().params["p77"], 0.5);
        let lane = &doc.song.track(&key("lead")).unwrap().lanes[2];
        assert!(matches!(
            &lane.kind,
            atomik_model::song::LaneKind::Automation(p) if p == "p77"
        ));
        let atomik_model::song::ClipContent::Automation(points) = &lane.clips[0].content else {
            panic!("automation clip");
        };
        assert_eq!(points[0].value, 7.5, "dead lanes are not clamped");
        assert_eq!(points[1].value, -3.25);
        assert_eq!(
            doc.inner(&key("layer")).unwrap().macros[2].targets[0].param,
            "p77"
        );
        let saved = save_project(&doc);
        assert!(saved.contains("\"p77\""));
        assert_eq!(save_project(&load_project(&saved, reg).unwrap()), saved);
        // The expansion ignores the dead target and the engine never sees the key.
        let mut full = registry();
        register_stand_ins(&doc, &mut full).unwrap();
        let flat = doc.expand(&full);
        let comp = flat
            .devices
            .iter()
            .find(|d| d.key == key("layer/comp"))
            .unwrap();
        assert!(!comp.params.contains_key("p77"));
    }
    // A name that is not a plugin key stays an error.
    let bad_lane = edited(&file, |v| {
        let lanes = v["song"]["tracks"][0]["lanes"].as_array_mut().unwrap();
        lanes.push(json!({"kind": "automation", "param": "zz", "clips": []}));
    });
    rejects(
        &bad_lane,
        &installed,
        "song.tracks[0].lanes[2].param",
        "no param",
    );
    let bad_macro = edited(&file, |v| {
        v["devices"][3]["inner"]["macros"][2]["targets"] =
            json!([{"device": "comp", "param": "zz", "min": 0.0, "max": 1.0}]);
    });
    rejects(
        &bad_macro,
        &installed,
        "devices[3].inner.macros[2].targets[0].param",
        "no param",
    );
}

#[test]
fn a_dead_key_is_not_settable_and_not_a_live_lane_through_commands() {
    let r = plugin_registry();
    let file = fixture("plugin-v9.atomik");
    let t = edited(&file, |v| v["devices"][1]["params"]["p77"] = json!(0.5));
    let mut d = Document::new(load_project(&t, &r).unwrap());
    assert!(matches!(
        d.apply(set_param("lead", "p77", 0.1), &r),
        Err(ModelError::UnknownKey(_))
    ));
    assert_eq!(d.doc().device(&key("lead")).unwrap().params["p77"], 0.5);
}

#[test]
fn plugin_devices_of_the_same_plugin_may_repeat_and_nest() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &fx_ref(), &fx_layout(), Some("a")),
            add_plugin(1, &fx_ref(), &fx_layout(), Some("b")),
            add(2, CONTAINER, Some("c")),
            add_plugin_inner("c", 0, &fx_ref(), &fx_layout(), Some("a"), false),
            add(3, CONTAINER, Some("d")),
            add_inner("c", 1, CONTAINER, "nested"),
            add_plugin_inner(
                "c/nested",
                0,
                &vst3_ref(),
                &vst3_layout(),
                Some("deep"),
                false,
            ),
            sync_state("c/nested/deep", b"nested state"),
        ],
    );
    let t = save_project(d.doc());
    let back = load_project(&t, &r).unwrap();
    assert_eq!(back, d.doc().compacted());
    assert_eq!(save_project(&back), t);
}

#[test]
fn a_console_channel_and_a_song_track_survive_a_stand_in_load_and_save() {
    let full = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &full,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("lead")),
            add_plugin(1, &fx_ref(), &fx_layout(), Some("verb")),
            set_param("lead", "p10", 0.4),
            sync_state("lead", b"patch"),
            common::song::add_track("lead"),
            Command::Console(atomik_model::ConsoleCommand::SetReturn {
                slot: 0,
                effect: Some(key("verb")),
            }),
        ],
    );
    assert_eq!(d.doc().console.channels.len(), 1);
    let text = save_project(d.doc());
    // Plugins missing: the console devices are there, the plugin types are not.
    let bare = console_registry();
    let doc = load_project(&text, &bare).expect("stand-ins load");
    assert_eq!(doc, d.doc().compacted());
    assert_eq!(doc.console.channels.len(), 1);
    assert_eq!(doc.console.channels[0].source, Some(key("lead")));
    assert_eq!(doc.console.returns[0], Some(key("verb")));
    assert!(doc.song.track(&key("lead")).is_some());
    assert_eq!(save_project(&doc), text, "byte for byte");
    // The document works with the stand-in types registered from the layouts.
    let mut reg = console_registry();
    register_stand_ins(&doc, &mut reg).unwrap();
    let mut d = Document::new(doc);
    run(&mut d, &reg, [set_param("lead", "p10", 0.6)]);
    d.apply(Command::RemoveDevice { key: key("lead") }, &reg)
        .unwrap();
    assert!(
        d.doc().console.channels.is_empty(),
        "the channel goes with its source"
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().console.channels.len(), 1);
}

#[test]
fn layout_rows_are_saved_sorted_by_id_whatever_the_plugins_order() {
    let mut layout = fx_layout();
    layout.params.reverse();
    let mut r = registry();
    register_plugin(&mut r, &fx_ref(), &layout);
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &fx_ref(), &layout, Some("f")),
            set_param("f", "p3", 0.5),
            set_param("f", "p1", -3.0),
            set_pinned("f", &[2]),
        ],
    );
    let t = save_project(d.doc());
    let v: Value = serde_json::from_str(&t).unwrap();
    let ids: Vec<u64> = v["devices"][0]["plugin"]["layout"]["params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3]);
    let back = load_project(&t, &r).unwrap();
    assert_eq!(back, d.doc().compacted());
    assert_eq!(save_project(&back), t);
}

/// Found with the plugin demo: two instances of one missing plugin that use different
/// params saved different layout rows, and the stand-in descriptor of the type took the first
/// instance's rows only, so the second failed to load (an unknown param).
#[test]
fn two_stand_ins_of_one_plugin_with_different_used_params_share_a_union_descriptor() {
    let full = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &full,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("a")),
            add_plugin(1, &synth_ref(), &synth_layout(), Some("b")),
            set_param("a", "p10", 0.4),
            set_param("b", "p11", 0.7),
        ],
    );
    let text = save_project(d.doc());
    let bare = registry();
    let doc = load_project(&text, &bare).expect("both stand-ins load");
    assert_eq!(doc, d.doc().compacted());
    assert_eq!(save_project(&doc), text, "byte for byte");
    let mut reg = registry();
    let added = register_stand_ins(&doc, &mut reg).unwrap();
    assert_eq!(added, vec![CLAP_SYNTH.to_string()]);
    let desc = reg.get(CLAP_SYNTH).unwrap().descriptor;
    assert!(desc.param_by_key("p10").is_some() && desc.param_by_key("p11").is_some());
    let mut d = Document::new(doc);
    run(
        &mut d,
        &reg,
        [set_param("a", "p11", 0.1), set_param("b", "p10", 0.2)],
    );
}
