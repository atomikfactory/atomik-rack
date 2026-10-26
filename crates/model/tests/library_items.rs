//! Library device items (`.atomikpatch`, `.atomikcontainer`): saving a snapshot, loading it back
//! as a snapshot to paste, the item kind and the one line summaries of the browser.

mod common;

use atomik_model::{
    AssetMeta, ClipFile, Command, DeviceSnapshot, Document, ItemKind, PROJECT_FORMAT,
    PROJECT_VERSION, WorkflowCommand, ZoneDoc, empty_doc, load_device_item, load_project,
    register_stand_ins, save_clip_file, save_device_item, save_project, summarize_item,
};
use common::plugin::*;
use common::song::{add_track, note, notes_clip};
use common::workflow::{one_step, wf};
use common::*;
use serde_json::Value;

const ASSET: &str = "4242424242424242424242424242424242424242424242424242424242424242";
const BAR: i64 = 61_440;

fn snapshot(d: &Document, keys: &[&str]) -> DeviceSnapshot {
    let keys: Vec<_> = keys.iter().map(|k| key(k)).collect();
    DeviceSnapshot::capture(d.doc(), &keys, false).unwrap()
}

/// A synth into a delay, plus a lone mixer.
fn rack(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("lead")),
            add(1, DELAY, Some("echo")),
            add(2, MIXER, Some("mix")),
            connect("lead:out_l", "echo:in_l"),
            connect("lead:out_r", "echo:in_r"),
        ],
    );
    d
}

fn sampler_rack(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            Command::RegisterAsset {
                hash: ASSET.into(),
                meta: AssetMeta {
                    channels: 2,
                    sample_rate: 48_000,
                    frames: 48_000,
                    original_name: "kick.wav".into(),
                },
            },
            add(0, SAMPLER, Some("smp")),
            Command::SetZones {
                device: key("smp"),
                zones: vec![ZoneDoc::new(ASSET, 48_000)],
                merge: None,
            },
        ],
    );
    d
}

fn container_rack(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, CONTAINER, Some("layer")),
            add_inner("layer", 0, SYNTH, "lo"),
            add_inner("layer", 1, DELAY, "fx"),
            connect("layer/lo:out_l", "layer/fx:in_l"),
            connect("layer/fx:out_l", "layer/boundary:out_l"),
            set_macro("layer", 0, target("lo", "filter.cutoff", 200.0, 8000.0)),
            set_macro("layer", 0, target("fx", "mix", 0.0, 1.0)),
        ],
    );
    d
}

fn paste(d: &mut Document, r: &atomik_device_sdk::Registry, s: DeviceSnapshot) {
    one_step(
        d,
        r,
        wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: None,
            at: 0,
        }),
    );
}

fn clip_text(meter: (u8, u8), length: i64, notes: i64) -> String {
    let notes = (0..notes)
        .map(|i| note(i * 3_840, 3_840, 60, 0.8))
        .collect();
    save_clip_file(&ClipFile {
        tempo_bpm: 120.0,
        meter,
        clip: notes_clip(0, length, notes),
    })
    .unwrap()
}

#[test]
fn save_device_item_patch_is_a_current_project_without_song_or_console() {
    let r = registry();
    let d = rack(&r);
    let text = save_device_item(&snapshot(&d, &["lead", "echo"]), d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["format"], PROJECT_FORMAT);
    assert_eq!(v["version"], PROJECT_VERSION);
    assert_eq!(v["devices"].as_array().unwrap().len(), 2);
    assert_eq!(v["cables"].as_array().unwrap().len(), 2);
    assert!(v["song"]["tracks"].as_array().unwrap().is_empty());
    assert!(v["console"]["channels"].as_array().unwrap().is_empty());
    // Whatever reads a project reads an item.
    assert_eq!(load_project(&text, &r).unwrap().devices.len(), 2);
}

#[test]
fn save_device_item_twice_is_byte_identical() {
    let r = registry();
    let d = rack(&r);
    let s = snapshot(&d, &["lead", "echo"]);
    assert_eq!(save_device_item(&s, d.doc()), save_device_item(&s, d.doc()));
}

#[test]
fn save_device_item_without_the_cable_partner_has_no_cables() {
    let r = registry();
    let d = rack(&r);
    let text = save_device_item(&snapshot(&d, &["lead", "mix"]), d.doc());
    let s = load_device_item(&text, &r).unwrap();
    assert!(s.cables.is_empty());
    assert_eq!(s.devices.len(), 2);
}

#[test]
fn load_device_item_patch_gives_back_the_devices_and_cables() {
    let r = registry();
    let d = rack(&r);
    let before = snapshot(&d, &["lead", "echo"]);
    let after = load_device_item(&save_device_item(&before, d.doc()), &r).unwrap();
    assert_eq!(after, before);
}

#[test]
fn load_device_item_pasted_is_one_undo_step() {
    let r = registry();
    let d = rack(&r);
    let item = save_device_item(&snapshot(&d, &["lead", "echo"]), d.doc());
    let mut target = Document::new(empty_doc());
    paste(&mut target, &r, load_device_item(&item, &r).unwrap());
    assert_eq!(keys(&target), ["poly-synth", "stereo-delay"]);
    assert_eq!(target.doc().cables.len(), 2);
}

#[test]
fn load_device_item_sampler_keeps_zones_and_assets() {
    let r = registry();
    let d = sampler_rack(&r);
    let s = snapshot(&d, &["smp"]);
    let back = load_device_item(&save_device_item(&s, d.doc()), &r).unwrap();
    assert_eq!(back.devices[0].zones, s.devices[0].zones);
    assert_eq!(back.assets, s.assets);
    assert_eq!(back.assets[ASSET].frames, 48_000);
    // The paste into a document that registers the pool entry first, as the app does.
    let mut target = Document::new(empty_doc());
    for (hash, meta) in &back.assets {
        let register = Command::RegisterAsset {
            hash: hash.clone(),
            meta: meta.clone(),
        };
        run(&mut target, &r, [register]);
    }
    paste(&mut target, &r, back);
    assert_eq!(target.doc().devices[0].zones.len(), 1);
}

#[test]
fn load_device_item_container_keeps_inner_devices_cables_and_macros() {
    let r = registry();
    let d = container_rack(&r);
    let s = snapshot(&d, &["layer"]);
    assert_eq!(ItemKind::of(&s), ItemKind::Container);
    let back = load_device_item(&save_device_item(&s, d.doc()), &r).unwrap();
    assert_eq!(back, s);
    let inner = back.devices[0].inner.as_ref().unwrap();
    assert_eq!(inner.devices.len(), 2);
    assert_eq!(inner.macros[0].targets.len(), 2);
}

#[test]
fn load_device_item_plugin_device_loads_as_a_stand_in() {
    let installed = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &installed,
        [add_plugin(0, &synth_ref(), &synth_layout(), Some("lead"))],
    );
    let s = snapshot(&d, &["lead"]);
    let text = save_device_item(&s, d.doc());
    // The plugin is not installed here.
    let bare = registry();
    let back = load_device_item(&text, &bare).expect("a missing plugin loads as a stand-in");
    let (got, want) = (&back.devices[0], &s.devices[0]);
    assert_eq!(
        got.plugin.as_ref().unwrap().id,
        want.plugin.as_ref().unwrap().id
    );
    // The paste works once the stand-ins are registered.
    let mut reg = registry();
    let mut stand = empty_doc();
    stand.devices = back.devices.clone();
    register_stand_ins(&stand, &mut reg).unwrap();
    let mut target = Document::new(empty_doc());
    paste(&mut target, &reg, back);
    assert_eq!(target.doc().devices.len(), 1);
}

#[test]
fn load_device_item_not_a_project_is_an_error_naming_the_field() {
    let r = registry();
    let e = load_device_item("{\"format\": \"nope\"}", &r).unwrap_err();
    assert!(e.to_string().contains("format"), "{e}");
    assert!(load_device_item("not json", &r).is_err());
}

#[test]
fn load_device_item_of_a_whole_project_takes_only_devices_and_cables() {
    let r = registry();
    let mut d = rack(&r);
    run(&mut d, &r, [add_track("lead")]);
    let s = load_device_item(&save_project(d.doc()), &r).unwrap();
    assert_eq!(s.devices.len(), 3);
    assert_eq!(s.cables.len(), 2);
    assert!(s.tracks.is_empty());
}

#[test]
fn item_kind_of_a_single_container_is_container_and_anything_else_a_patch() {
    let r = registry();
    let d = container_rack(&r);
    assert_eq!(ItemKind::of(&snapshot(&d, &["layer"])), ItemKind::Container);
    let d = rack(&r);
    assert_eq!(ItemKind::of(&snapshot(&d, &["lead"])), ItemKind::Patch);
    let two = snapshot(&d, &["lead", "echo"]);
    assert_eq!(ItemKind::of(&two), ItemKind::Patch);
}

#[test]
fn summarize_item_patch_lists_the_device_names() {
    let r = registry();
    let d = rack(&r);
    let two = save_device_item(&snapshot(&d, &["lead", "echo"]), d.doc());
    assert_eq!(
        summarize_item(ItemKind::Patch, &two).unwrap().detail,
        "2 devices: Poly Synth, Stereo Delay"
    );
    let one = save_device_item(&snapshot(&d, &["lead"]), d.doc());
    assert_eq!(
        summarize_item(ItemKind::Patch, &one).unwrap().detail,
        "1 device: Poly Synth"
    );
    let three = save_device_item(&snapshot(&d, &["lead", "echo", "mix"]), d.doc());
    let detail = summarize_item(ItemKind::Patch, &three).unwrap().detail;
    assert!(
        detail.starts_with("3 devices: Poly Synth, Stereo Delay, "),
        "{detail}"
    );
    assert!(!detail.ends_with("..."), "{detail}");
}

#[test]
fn summarize_item_patch_past_three_devices_ends_with_an_ellipsis() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    for i in 0..4 {
        run(&mut d, &r, [add(i, DELAY, None)]);
    }
    let all = keys(&d);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let text = save_device_item(&snapshot(&d, &refs), d.doc());
    let detail = summarize_item(ItemKind::Patch, &text).unwrap().detail;
    assert!(detail.starts_with("4 devices: "), "{detail}");
    assert!(detail.ends_with(", ..."), "{detail}");
}

#[test]
fn summarize_item_container_counts_the_inside() {
    let r = registry();
    let d = container_rack(&r);
    let text = save_device_item(&snapshot(&d, &["layer"]), d.doc());
    assert_eq!(
        summarize_item(ItemKind::Container, &text).unwrap().detail,
        "Container, 2 inside"
    );
}

#[test]
fn summarize_item_clip_counts_bars_and_notes() {
    let four = clip_text((4, 4), 4 * BAR, 32);
    assert_eq!(
        summarize_item(ItemKind::Clip, &four).unwrap().detail,
        "4 bars, 32 notes"
    );
    let one = clip_text((4, 4), BAR, 1);
    assert_eq!(
        summarize_item(ItemKind::Clip, &one).unwrap().detail,
        "1 bar, 1 note"
    );
    // Half a bar of a 3/4 clip still takes the bar it starts.
    let part = clip_text((3, 4), BAR / 2, 0);
    assert_eq!(
        summarize_item(ItemKind::Clip, &part).unwrap().detail,
        "1 bar, 0 notes"
    );
}

#[test]
fn summarize_item_project_gives_tempo_and_track_count() {
    let r = registry();
    let mut d = rack(&r);
    run(
        &mut d,
        &r,
        [
            add(3, SYNTH, Some("pad")),
            add_track("lead"),
            add_track("pad"),
        ],
    );
    let text = save_project(d.doc());
    assert_eq!(
        summarize_item(ItemKind::Project, &text).unwrap().detail,
        "120 BPM, 2 tracks"
    );
}

#[test]
fn summarize_item_unreadable_text_gives_none() {
    assert!(summarize_item(ItemKind::Patch, "").is_none());
    assert!(summarize_item(ItemKind::Container, "{}").is_none());
    assert!(summarize_item(ItemKind::Clip, "{}").is_none());
    assert!(summarize_item(ItemKind::Project, "[1]").is_none());
}
