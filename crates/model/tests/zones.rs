//! Sampler zones in the document: the
//! `SetZones` command, undo/redo and drag merging, asset references, and project format v4.

mod common;

use atomik_device_sdk::{LoopMode, Registry, SampleMap, TriggerMode};
use atomik_model::{
    Command, Document, ModelError, RackDoc, SyncOp, ZoneDoc, empty_doc, load_project, save_project,
};
use common::audio::{hash, register};
use common::*;
use proptest::prelude::*;
use serde_json::Value;
use std::collections::BTreeSet;

const SAMPLER: &str = "atomik.sampler";

fn reg() -> Registry {
    registry()
}

/// A sampler `s` and assets 1 (1000 frames), 2 (500 frames) and 3 (80 frames) in the pool.
fn doc_with_sampler(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SAMPLER, Some("s")), reg).unwrap();
    for (n, frames) in [(1, 1000), (2, 500), (3, 80)] {
        d.apply(register(n, frames), reg).unwrap();
    }
    d
}

fn zone(n: u8, frames: u64) -> ZoneDoc {
    ZoneDoc::new(hash(n), frames)
}

fn set(zones: Vec<ZoneDoc>, merge: Option<u64>) -> Command {
    Command::SetZones {
        device: key("s"),
        zones,
        merge,
    }
}

fn zones_of(d: &Document) -> Vec<ZoneDoc> {
    d.doc().device(&key("s")).unwrap().zones.clone()
}

fn looped() -> ZoneDoc {
    let mut z = zone(1, 1000);
    z.loop_mode = LoopMode::Forward;
    z.loop_start = 100;
    z.loop_end = 900;
    z.crossfade = 100;
    z
}

#[test]
fn a_new_sampler_has_no_zones_and_other_devices_never_do() {
    let r = reg();
    let d = three_devices(&r);
    assert!(d.doc().devices.iter().all(|x| x.zones.is_empty()));
    let d = doc_with_sampler(&r);
    assert!(zones_of(&d).is_empty());
}

#[test]
fn set_zones_replaces_the_list_in_one_undoable_step() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    let a = vec![zone(1, 1000), zone(2, 500)];
    let ops = d.apply(set(a.clone(), None), &r).unwrap();
    assert_eq!(ops, [SyncOp::ZonesChanged(key("s"))]);
    assert_eq!(zones_of(&d), a);
    let b = vec![looped()];
    d.apply(set(b.clone(), None), &r).unwrap();
    assert_eq!(zones_of(&d), b);
    assert_eq!(d.undo(&r).unwrap(), [SyncOp::ZonesChanged(key("s"))]);
    assert_eq!(zones_of(&d), a);
    assert_eq!(d.undo(&r).unwrap(), [SyncOp::ZonesChanged(key("s"))]);
    assert!(zones_of(&d).is_empty());
    assert_eq!(d.redo(&r).unwrap(), [SyncOp::ZonesChanged(key("s"))]);
    assert_eq!(d.redo(&r).unwrap(), [SyncOp::ZonesChanged(key("s"))]);
    assert_eq!(zones_of(&d), b);
    assert!(d.redo(&r).is_none());
}

#[test]
fn consecutive_drags_with_the_same_merge_id_are_one_step() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    d.apply(set(vec![zone(1, 1000)], None), &r).unwrap();
    for hi in [100, 90, 80] {
        let mut z = zone(1, 1000);
        z.key_hi = hi;
        d.apply(set(vec![z], Some(7)), &r).unwrap();
    }
    assert_eq!(zones_of(&d)[0].key_hi, 80);
    d.undo(&r).unwrap();
    assert_eq!(zones_of(&d)[0].key_hi, 127, "one step for the whole drag");
    d.undo(&r).unwrap();
    assert!(zones_of(&d).is_empty());
}

#[test]
fn a_new_merge_id_a_gap_or_end_merge_starts_a_new_step() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    let with_hi = |hi| {
        let mut z = zone(1, 1000);
        z.key_hi = hi;
        vec![z]
    };
    d.apply(set(with_hi(10), Some(1)), &r).unwrap();
    d.apply(set(with_hi(20), Some(2)), &r).unwrap();
    d.apply(set(with_hi(30), Some(2)), &r).unwrap();
    d.end_merge();
    d.apply(set(with_hi(40), Some(2)), &r).unwrap();
    d.apply(set(with_hi(50), None), &r).unwrap();
    d.apply(set(with_hi(60), Some(2)), &r).unwrap();
    let mut his = Vec::new();
    for _ in 0..5 {
        d.undo(&r).unwrap();
        his.push(zones_of(&d).first().map(|z| z.key_hi));
    }
    assert_eq!(
        his,
        [Some(50), Some(40), Some(30), Some(10), None],
        "steps: 10 | 20+30 | 40 | 50 | 60"
    );
}

#[test]
fn merging_does_not_cross_devices() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    d.apply(add(1, SAMPLER, Some("t")), &r).unwrap();
    d.apply(set(vec![zone(1, 1000)], Some(1)), &r).unwrap();
    d.apply(
        Command::SetZones {
            device: key("t"),
            zones: vec![zone(2, 500)],
            merge: Some(1),
        },
        &r,
    )
    .unwrap();
    d.undo(&r).unwrap();
    assert_eq!(zones_of(&d).len(), 1, "device s keeps its zones");
}

#[test]
fn rejected_lists_change_nothing() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    d.apply(set(vec![zone(1, 1000)], None), &r).unwrap();
    let before = d.doc().clone();
    let good = zone(1, 1000);
    let mut cases: Vec<(&str, Command)> = Vec::new();
    cases.push((
        "unknown device",
        Command::SetZones {
            device: key("nope"),
            zones: vec![],
            merge: None,
        },
    ));
    let mut synth = d.clone();
    synth.apply(add(1, SYNTH, Some("y")), &r).unwrap();
    let synth_cmd = Command::SetZones {
        device: key("y"),
        zones: vec![],
        merge: None,
    };
    let bad_zone = |f: fn(&mut ZoneDoc)| {
        let mut z = good.clone();
        f(&mut z);
        set(vec![good.clone(), z], None)
    };
    cases.extend([
        ("missing asset", set(vec![zone(9, 10)], None)),
        ("not a hash", set(vec![ZoneDoc::new("xyz", 10)], None)),
        (
            "uppercase hash",
            set(vec![ZoneDoc::new(hash(0xab).to_uppercase(), 10)], None),
        ),
        ("key order", bad_zone(|z| (z.key_lo, z.key_hi) = (70, 60))),
        ("key range", bad_zone(|z| z.key_hi = 128)),
        ("vel zero", bad_zone(|z| z.vel_lo = 0)),
        ("vel order", bad_zone(|z| (z.vel_lo, z.vel_hi) = (90, 80))),
        ("root", bad_zone(|z| z.root_key = 128)),
        ("transpose", bad_zone(|z| z.transpose = -49)),
        ("tune", bad_zone(|z| z.tune_cents = 100.5)),
        ("tune nan", bad_zone(|z| z.tune_cents = f32::NAN)),
        ("gain", bad_zone(|z| z.gain_db = 12.5)),
        ("pan", bad_zone(|z| z.pan = 1.01)),
        ("end beyond asset", bad_zone(|z| z.end = 1001)),
        ("empty sample", bad_zone(|z| z.start = z.end)),
        (
            "loop outside",
            bad_zone(|z| {
                z.loop_mode = LoopMode::PingPong;
                z.loop_start = 10;
                z.loop_end = 1001;
            }),
        ),
        (
            "crossfade too long",
            bad_zone(|z| {
                z.loop_mode = LoopMode::Forward;
                z.loop_start = 10;
                z.loop_end = 500;
                z.crossfade = 11;
            }),
        ),
    ]);
    for (what, c) in cases {
        let e = d.apply(c, &r).expect_err(what);
        assert!(
            matches!(e, ModelError::BadZones(_) | ModelError::UnknownDevice(_)),
            "{what}: {e:?}"
        );
        assert_eq!(d.doc(), &before, "{what} changed the document");
    }
    let e = synth.apply(synth_cmd, &r).unwrap_err();
    assert!(matches!(e, ModelError::BadZones(m) if m.contains("does not take samples")));
    // After all those rejections the history still has exactly the earlier steps.
    assert!(d.undo(&r).is_some());
    assert!(zones_of(&d).is_empty());
}

#[test]
fn frames_are_checked_against_the_assets_own_length() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    assert!(d.apply(set(vec![zone(3, 80)], None), &r).is_ok());
    // Asset 3 has 80 frames; the same zone over asset 1 is fine, over asset 3 with 81 is not.
    let mut z = zone(3, 80);
    z.end = 81;
    assert!(d.apply(set(vec![z], None), &r).is_err());
    let mut z = zone(1, 1000);
    z.end = 81;
    assert!(d.apply(set(vec![z], None), &r).is_ok());
}

#[test]
fn looping_fields_are_ignored_while_the_loop_is_off() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    let mut z = zone(1, 1000);
    z.loop_start = 5_000;
    z.loop_end = 1;
    z.crossfade = 99_999;
    assert!(d.apply(set(vec![z], None), &r).is_ok());
}

#[test]
fn removing_a_sampler_and_undoing_restores_its_zones_exactly() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    let mut z = looped();
    z.trigger = TriggerMode::OneShot;
    z.tune_cents = -12.5;
    d.apply(set(vec![z, zone(2, 500)], None), &r).unwrap();
    let before = d.doc().clone();
    d.apply(Command::RemoveDevice { key: key("s") }, &r)
        .unwrap();
    assert!(d.doc().device(&key("s")).is_none());
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

fn assets(d: &Document) -> BTreeSet<String> {
    d.doc().assets.keys().cloned().collect()
}

#[test]
fn zone_assets_count_as_referenced_and_survive_prune() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    d.apply(set(vec![zone(1, 1000), zone(2, 500)], None), &r)
        .unwrap();
    let want: BTreeSet<String> = [hash(1), hash(2)].into();
    assert_eq!(d.doc().referenced_assets(), want);
    assert_eq!(d.referenced_assets(), want);
    d.prune_unreferenced_assets();
    assert_eq!(assets(&d), want, "asset 3 (unused) goes, zone assets stay");
    let mut doc = d.doc().clone();
    doc.prune_unreferenced_assets();
    assert_eq!(doc.assets.keys().cloned().collect::<BTreeSet<_>>(), want);
}

#[test]
fn an_asset_only_the_undo_history_holds_survives_until_the_history_is_gone() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    d.apply(set(vec![zone(1, 1000)], None), &r).unwrap();
    d.apply(set(vec![zone(2, 500)], None), &r).unwrap();
    // Asset 1 is no longer in the document, but undo needs it.
    assert!(!d.doc().referenced_assets().contains(&hash(1)));
    assert!(d.referenced_assets().contains(&hash(1)));
    d.prune_unreferenced_assets();
    assert!(d.doc().assets.contains_key(&hash(1)));
    d.undo(&r).unwrap();
    assert_eq!(zones_of(&d)[0].asset, hash(1));
    // Redo history counts too.
    d.redo(&r).unwrap();
    d.undo(&r).unwrap();
    d.prune_unreferenced_assets();
    assert!(d.doc().assets.contains_key(&hash(2)));
    // A fresh document over the same content has no history: the old asset is pruned.
    let mut fresh = Document::new(d.doc().clone());
    fresh.apply(set(vec![zone(2, 500)], None), &r).unwrap();
    let mut fresh2 = Document::new(fresh.doc().clone());
    fresh2.prune_unreferenced_assets();
    assert_eq!(assets(&fresh2), [hash(2)].into());
}

#[test]
fn a_removed_samplers_assets_stay_while_undo_can_bring_it_back() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    d.apply(set(vec![zone(2, 500)], None), &r).unwrap();
    d.apply(Command::RemoveDevice { key: key("s") }, &r)
        .unwrap();
    assert!(d.doc().referenced_assets().is_empty());
    d.prune_unreferenced_assets();
    assert_eq!(assets(&d), [hash(2)].into());
}

// ---- project format v4 -------------------------------------------------------------------

fn project_doc(reg: &Registry) -> RackDoc {
    let mut d = doc_with_sampler(reg);
    let mut a = looped();
    a.trigger = TriggerMode::OneShot;
    a.tune_cents = -0.1;
    a.gain_db = 3.5;
    a.pan = -0.25;
    a.transpose = -12;
    a.key_lo = 36;
    a.key_hi = 72;
    a.vel_lo = 20;
    a.root_key = 48;
    let mut b = zone(2, 500);
    b.loop_mode = LoopMode::PingPong;
    b.loop_start = 50;
    b.loop_end = 450;
    b.start = 10;
    d.apply(set(vec![a, b], None), reg).unwrap();
    d.doc().clone()
}

#[test]
fn v4_saves_zones_in_the_device_object_only_when_non_empty() {
    let r = reg();
    let mut d = Document::new(project_doc(&r));
    d.apply(add(1, SYNTH, Some("y")), &r).unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 38);
    let devices = v["devices"].as_array().unwrap();
    assert_eq!(devices[0]["id"], "s");
    assert_eq!(
        devices[0]["zones"][0],
        serde_json::json!({
            "asset": hash(1), "key_lo": 36, "key_hi": 72, "vel_lo": 20, "vel_hi": 127,
            "root_key": 48, "transpose": -12, "tune_cents": -0.1, "gain_db": 3.5, "pan": -0.25,
            "start": 0, "end": 1000, "loop_mode": "forward", "loop_start": 100,
            "loop_end": 900, "crossfade": 100, "trigger": "one_shot", "reverse": false
        })
    );
    assert_eq!(devices[0]["zones"][1]["loop_mode"], "ping_pong");
    assert_eq!(devices[0]["zones"][1]["trigger"], "gate");
    assert!(devices[1].get("zones").is_none());
    // A sampler with no zones writes none either.
    let plain = save_project(Document::new(empty_doc()).doc());
    assert!(!plain.contains("zones"));
}

#[test]
fn v4_round_trips_bit_exactly_and_canonically() {
    let r = reg();
    let doc = project_doc(&r);
    let text = save_project(&doc);
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back, doc);
    assert_eq!(save_project(&back), text);
}

#[test]
fn v1_to_v3_projects_still_load_with_no_zones() {
    let r = reg();
    let mut doc = project_doc(&r);
    doc.devices[0].zones.clear();
    let text = save_project(&doc);
    for version in 1..=3 {
        let old = text.replace("\"version\": 38", &format!("\"version\": {version}"));
        let mut v: Value = serde_json::from_str(&old).unwrap();
        if version < 3 {
            // Before v3 there are no input channels and no pool; v1 has no console either.
            for k in ["assets", "input_channels"] {
                v.as_object_mut().unwrap().remove(k);
            }
        }
        if version == 1 {
            v.as_object_mut().unwrap().remove("console");
        }
        let back = load_project(&v.to_string(), &r).unwrap();
        assert!(
            back.devices.iter().all(|d| d.zones.is_empty()),
            "v{version}"
        );
    }
}

#[test]
fn zones_need_version_4() {
    let r = reg();
    let text = save_project(&project_doc(&r)).replace("\"version\": 38", "\"version\": 3");
    let e = load_project(&text, &r).unwrap_err();
    let ModelError::Load { path: at, message } = e else {
        panic!()
    };
    let m = format!("{at}: {message}");
    assert!(m.starts_with("devices[0].zones: unknown field"), "{m}");
}

fn z(v: &mut Value) -> &mut Value {
    &mut v["devices"][0]["zones"][1]
}

fn edited(text: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    edit(&mut v);
    v.to_string()
}

fn rejects(text: &str, path: &str, needle: &str) {
    let e = load_project(text, &reg()).expect_err("project must be rejected");
    let ModelError::Load { path: at, message } = &e else {
        panic!("expected a load error, got {e:?}")
    };
    let msg = &format!("{at}: {message}");
    assert!(
        msg.starts_with(&format!("{path}: ")),
        "{msg} (wanted {path})"
    );
    assert!(msg.contains(needle), "{msg} lacks {needle:?}");
}

#[test]
fn load_rejects_bad_zones_with_paths() {
    let r = reg();
    let text = save_project(&project_doc(&r));
    rejects(
        &edited(&text, |v| z(v)["extra"] = 1.into()),
        "devices[0].zones[1].extra",
        "unknown field",
    );
    rejects(
        &edited(&text, |v| {
            z(v).as_object_mut().unwrap().remove("pan");
        }),
        "devices[0].zones[1].pan",
        "missing field",
    );
    rejects(
        &edited(&text, |v| z(v)["asset"] = hash(9).into()),
        "devices[0].zones[1]",
        "not in the assets pool",
    );
    rejects(
        &edited(&text, |v| z(v)["asset"] = "abc".into()),
        "devices[0].zones[1]",
        "not a valid asset id",
    );
    rejects(
        &edited(&text, |v| z(v)["loop_mode"] = "backwards".into()),
        "devices[0].zones[1].loop_mode",
        "expected",
    );
    rejects(
        &edited(&text, |v| z(v)["trigger"] = "hold".into()),
        "devices[0].zones[1].trigger",
        "expected",
    );
    rejects(
        &edited(&text, |v| z(v)["key_hi"] = 300.into()),
        "devices[0].zones[1].key_hi",
        "outside",
    );
    rejects(
        &edited(&text, |v| z(v)["end"] = 501.into()),
        "devices[0].zones[1]",
        "`end` breaks the frame ordering",
    );
    rejects(
        &edited(&text, |v| z(v)["pan"] = 2.into()),
        "devices[0].zones[1]",
        "`pan` is out of range",
    );
    rejects(
        &edited(&text, |v| z(v)["transpose"] = 1000.into()),
        "devices[0].zones[1].transpose",
        "outside",
    );
    // Only samplers take zones.
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("y")), &r).unwrap();
    let plain = save_project(d.doc());
    rejects(
        &edited(&plain, |v| {
            v["devices"][0]["zones"] = edited_zone_list(&text);
        }),
        "devices[0].zones",
        "does not take samples",
    );
}

fn edited_zone_list(text: &str) -> Value {
    let v: Value = serde_json::from_str(text).unwrap();
    v["devices"][0]["zones"].clone()
}

#[test]
fn a_zone_asset_missing_from_the_pool_is_a_load_error_like_a_clip_asset() {
    let r = reg();
    let text = save_project(&project_doc(&r));
    rejects(
        &edited(&text, |v| {
            v["assets"].as_object_mut().unwrap().remove(&hash(1));
        }),
        "devices[0].zones[0]",
        "assets pool",
    );
}

fn zone_strategy() -> impl Strategy<Value = ZoneDoc> {
    (
        (
            1u8..=3,
            0u8..=127,
            0u8..=127,
            1u8..=127,
            1u8..=127,
            0u8..=127,
        ),
        (-48i8..=48, -100f32..=100.0, -60f32..=12.0, -1f32..=1.0),
        (0u64..=60, 0u64..=60, 0u64..=60, 0u64..=60, 0u64..=40),
        (0u8..3, any::<bool>(), any::<bool>()),
    )
        .prop_map(
            |(
                (n, k1, k2, v1, v2, root),
                (tr, tune, gain, pan),
                (s, e, ls, le, xf),
                (lm, os, rev),
            )| {
                let frames = [1000, 500, 80][usize::from(n) - 1];
                let mut z = ZoneDoc::new(hash(n), frames);
                (z.key_lo, z.key_hi) = (k1.min(k2), k1.max(k2));
                (z.vel_lo, z.vel_hi) = (v1.min(v2), v1.max(v2));
                (z.root_key, z.transpose, z.tune_cents, z.gain_db, z.pan) =
                    (root, tr, tune, gain, pan);
                z.start = s.min(e);
                z.end = (s.max(e) + 1).min(frames);
                z.loop_start = ls.min(le);
                z.loop_end = ls.max(le) + 1;
                z.crossfade = xf;
                z.loop_mode =
                    [LoopMode::Off, LoopMode::Forward, LoopMode::PingPong][usize::from(lm)];
                z.reverse = rev;
                z.trigger = if os {
                    TriggerMode::OneShot
                } else {
                    TriggerMode::Gate
                };
                z
            },
        )
}

/// The model's rules and the SDK's `SampleMap::new` agree on every zone, valid or not.
fn sdk_accepts(z: &ZoneDoc, frames: u64) -> bool {
    use atomik_device_sdk::SampleBuffer;
    use std::sync::Arc;
    let data: Arc<[Vec<f32>]> = vec![vec![0.0; frames as usize]].into();
    let buf = Arc::new(SampleBuffer::new(data).unwrap());
    SampleMap::new(vec![buf], vec![z.to_zone(0)]).is_ok()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn set_zones_accepts_exactly_what_the_sdk_accepts(z in zone_strategy()) {
        let r = reg();
        let mut d = doc_with_sampler(&r);
        let frames = d.doc().assets[&z.asset].frames;
        let ok = d.apply(set(vec![z.clone()], None), &r).is_ok();
        prop_assert_eq!(ok, sdk_accepts(&z, frames), "{:?}", z);
        // And a bit-flipped variant of the same zone.
        let mut w = z.clone();
        w.end = w.end.saturating_add(frames);
        let ok = d.apply(set(vec![w.clone()], None), &r).is_ok();
        prop_assert_eq!(ok, sdk_accepts(&w, frames));
    }

    #[test]
    fn zone_lists_round_trip_through_save_undo_and_load(zs in proptest::collection::vec(zone_strategy(), 0..6)) {
        let r = reg();
        let mut d = doc_with_sampler(&r);
        let before = d.doc().clone();
        let valid: Vec<ZoneDoc> = zs
            .into_iter()
            .filter(|z| sdk_accepts(z, d.doc().assets[&z.asset].frames))
            .collect();
        d.apply(set(valid.clone(), None), &r).unwrap();
        let text = save_project(d.doc());
        let back = load_project(&text, &r).unwrap();
        prop_assert_eq!(&back, d.doc());
        prop_assert_eq!(save_project(&back), text);
        d.undo(&r).unwrap();
        prop_assert_eq!(d.doc(), &before);
        d.redo(&r).unwrap();
        prop_assert_eq!(&zones_of(&d), &valid);
    }
}

/// The model cannot see the device crate's constants at build time (only its tests can), so
/// the two copies are checked against each other here.
#[test]
fn the_pad_constants_agree_with_the_drum_machine_device() {
    use atomik_devices::drum_machine;
    assert_eq!(atomik_model::PAD_KEY_BASE, drum_machine::PAD_KEY_BASE);
    assert_eq!(
        usize::from(atomik_model::PAD_COUNT),
        drum_machine::PAD_COUNT
    );
}

// ---- project format v6: reverse --------------------------------------------------

#[test]
fn v6_writes_reverse_last_in_every_zone_and_round_trips() {
    let r = reg();
    let mut doc = project_doc(&r);
    doc.devices[0].zones[1].reverse = true;
    let text = save_project(&doc);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 38);
    assert_eq!(v["devices"][0]["zones"][0]["reverse"], false);
    assert_eq!(v["devices"][0]["zones"][1]["reverse"], true);
    assert!(text.contains("\"trigger\": \"gate\", \"reverse\": true}"));
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back, doc);
    assert_eq!(save_project(&back), text);
}

#[test]
fn v4_and_v5_zones_load_forward_and_reject_reverse() {
    let r = reg();
    let doc = project_doc(&r);
    let text = save_project(&doc);
    for version in 4..=5 {
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["version"] = serde_json::json!(version);
        for zone in v["devices"][0]["zones"].as_array_mut().unwrap() {
            zone.as_object_mut().unwrap().remove("reverse");
        }
        let back = load_project(&v.to_string(), &r).unwrap();
        assert!(
            back.devices[0].zones.iter().all(|z| !z.reverse),
            "v{version}"
        );
        // Saved again, it is a v6 file with `reverse: false`.
        assert_eq!(save_project(&back), text, "v{version}");
        // A `reverse` key is unknown before v6.
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["version"] = serde_json::json!(version);
        let e = load_project(&v.to_string(), &r).unwrap_err().to_string();
        assert!(
            e.contains("reverse") && e.contains("unknown field"),
            "v{version}: {e}"
        );
    }
}

#[test]
fn v6_requires_reverse_and_a_boolean() {
    let r = reg();
    let text = save_project(&project_doc(&r));
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["devices"][0]["zones"][0]
        .as_object_mut()
        .unwrap()
        .remove("reverse");
    let e = load_project(&v.to_string(), &r).unwrap_err().to_string();
    assert!(e.contains("reverse"), "{e}");
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["devices"][0]["zones"][0]["reverse"] = serde_json::json!(1);
    let e = load_project(&v.to_string(), &r).unwrap_err().to_string();
    assert!(e.contains("reverse") && e.contains("boolean"), "{e}");
}

#[test]
fn set_zones_skips_loop_checks_for_a_reversed_zone() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    let mut z = looped();
    z.reverse = true;
    z.loop_start = 900; // breaks the loop ordering: ignored when reversed (as in the SDK)
    z.loop_end = 100;
    z.crossfade = 99_999;
    d.apply(set(vec![z.clone()], None), &r).unwrap();
    assert_eq!(zones_of(&d), [z.clone()]);
    z.reverse = false;
    assert!(d.apply(set(vec![z], None), &r).is_err());
}

#[test]
fn slice_constants_match_the_loop_slicer_device() {
    assert_eq!(
        atomik_model::SLICE_KEY_BASE,
        atomik_devices::loop_slicer::SLICE_KEY_BASE
    );
    assert_eq!(
        atomik_model::MAX_SLICES,
        atomik_devices::loop_slicer::SLICE_COUNT
    );
}

// ---- Batch with SetParam and SetZones ---------------------------------------------

fn set_param(value: f32) -> Command {
    Command::SetParam {
        key: key("s"),
        param: "tune.semi".into(),
        value,
        merge: Some(9),
    }
}

fn tune(d: &Document) -> Option<f32> {
    d.doc()
        .device(&key("s"))
        .unwrap()
        .params
        .get("tune.semi")
        .copied()
}

#[test]
fn a_batch_of_set_param_and_set_zones_is_one_step_reporting_each_kind() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    let before = d.doc().clone();
    let ops = d
        .apply(
            Command::Batch(vec![
                set_param(5.0),
                set(vec![zone(1, 1000)], Some(3)),
                set(vec![zone(2, 500)], Some(3)),
            ]),
            &r,
        )
        .unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::ParamChanged {
                key: key("s"),
                param: "tune.semi".into(),
                value: 5.0
            },
            SyncOp::ZonesChanged(key("s")),
        ],
        "one ZonesChanged however many SetZones"
    );
    assert_eq!(tune(&d), Some(5.0));
    assert_eq!(zones_of(&d), vec![zone(2, 500)]);
    d.undo(&r).unwrap();
    assert_eq!(*d.doc(), before, "one undo restores everything");
    d.redo(&r).unwrap();
    assert_eq!(tune(&d), Some(5.0));
    assert_eq!(zones_of(&d), vec![zone(2, 500)]);
}

#[test]
fn a_batch_ignores_merge_ids_and_never_joins_a_drag_group() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    d.apply(set(vec![zone(1, 1000)], Some(3)), &r).unwrap();
    d.apply(
        Command::Batch(vec![set_param(2.0), set(vec![zone(2, 500)], Some(3))]),
        &r,
    )
    .unwrap();
    d.apply(set(vec![zone(3, 80)], Some(3)), &r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(
        zones_of(&d),
        vec![zone(2, 500)],
        "the later drag is its own step"
    );
    d.undo(&r).unwrap();
    assert_eq!(zones_of(&d), vec![zone(1, 1000)]);
    assert_eq!(tune(&d), None);
}

#[test]
fn a_batch_with_set_param_and_set_zones_is_all_or_nothing() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    let before = d.doc().clone();
    let mut bad = zone(1, 1000);
    bad.key_lo = 90;
    bad.key_hi = 10;
    for batch in [
        vec![set_param(5.0), set(vec![bad.clone()], None)],
        vec![
            set(vec![zone(1, 1000)], None),
            Command::SetParam {
                key: key("s"),
                param: "nope".into(),
                value: 1.0,
                merge: None,
            },
        ],
        vec![set_param(5.0), Command::SetTempo(100.0)],
    ] {
        d.apply(Command::Batch(batch), &r).unwrap_err();
        assert_eq!(*d.doc(), before);
    }
}

#[test]
fn a_batch_validates_set_zones_against_the_evolving_document() {
    let r = reg();
    let mut d = doc_with_sampler(&r);
    // Registering is not batchable, so the asset must be in the pool already: asset 4 is not.
    d.apply(
        Command::Batch(vec![set_param(1.0), set(vec![zone(4, 100)], None)]),
        &r,
    )
    .unwrap_err();
    // Two SetParams on one param: the second sees the first (undo restores the original).
    d.apply(Command::Batch(vec![set_param(1.0), set_param(2.0)]), &r)
        .unwrap();
    assert_eq!(tune(&d), Some(2.0));
    d.undo(&r).unwrap();
    assert_eq!(tune(&d), None);
}
