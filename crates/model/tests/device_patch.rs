//! Device patches: `ApplyPatch`, `ResetDevice` and `SetPatchName` for every device family
//! (params, zones, step patterns, curves, drum patterns, plugin state, a container's inside),
//! the optional patch name of project format 36 and the one-device patch item.

mod common;

use atomik_device_sdk::{PluginStateBlob, Registry};
use atomik_model::{
    AssetMeta, Command, CurvePatternDoc, CurveStepDoc, DeviceSnapshot, Document, Frozen, ItemKind,
    ModelError, PROJECT_VERSION, RackDevice, SongCommand, SyncOp, WorkflowCommand, ZoneDoc,
    empty_doc, load_device_item, load_project, save_device_item, save_project, summarize_item,
};
use common::audio::*;
use common::plugin::*;
use common::song::*;
use common::workflow::{one_step, rejected, text, wf};
use common::*;
use serde_json::Value;

const ASSET: &str = "4242424242424242424242424242424242424242424242424242424242424242";
const CV: &str = "atomik.cv-sequencer";
const DRUMS: &str = "atomik.drum-generator";

fn register_asset() -> Command {
    Command::RegisterAsset {
        hash: ASSET.into(),
        meta: AssetMeta {
            channels: 2,
            sample_rate: 48_000,
            frames: 48_000,
            original_name: "kick.wav".into(),
        },
    }
}

/// A document with devices `list` (type, key) in order, the sample in the pool, and then
/// `setup` applied.
fn doc_with(r: &Registry, list: &[(&str, &str)], setup: Vec<Command>) -> Document {
    let mut d = Document::new(empty_doc());
    run(&mut d, r, [register_asset()]);
    for (i, (t, k)) in list.iter().enumerate() {
        run(&mut d, r, [add(i, t, Some(k))]);
    }
    run(&mut d, r, setup);
    d
}

fn device<'d>(d: &'d Document, k: &str) -> &'d RackDevice {
    d.doc().resolve(&key(k)).unwrap()
}

fn patch_of(d: &Document, k: &str) -> DeviceSnapshot {
    DeviceSnapshot::capture(d.doc(), &[key(k)], false).unwrap()
}

fn apply(target: &str, snapshot: &DeviceSnapshot) -> Command {
    wf(WorkflowCommand::ApplyPatch {
        device: key(target),
        snapshot: snapshot.clone(),
    })
}

fn reset(target: &str) -> Command {
    wf(WorkflowCommand::ResetDevice {
        device: key(target),
    })
}

/// The parts of two devices that make their sound; both bounce the same when they are equal.
fn assert_same_sound(a: &RackDevice, b: &RackDevice) {
    assert_eq!(a.type_id, b.type_id);
    assert_eq!(a.params, b.params, "params");
    assert_eq!(a.zones, b.zones, "zones");
    assert_eq!(a.steps, b.steps, "steps");
    assert_eq!(a.curves, b.curves, "curves");
    assert_eq!(a.drums, b.drums, "drums");
    assert_eq!(a.inner, b.inner, "inside");
    assert_eq!(
        a.plugin.as_ref().map(|p| p.state.clone()),
        b.plugin.as_ref().map(|p| p.state.clone()),
        "plugin state"
    );
}

/// A command that moves the first real param of `type_id` on device `k` to the far end of its
/// range.
fn tweak(r: &Registry, type_id: &str, k: &str) -> Option<Command> {
    let desc = r.get(type_id).unwrap().descriptor;
    let p = desc.params.iter().find(|p| p.min < p.max)?;
    let to = if p.default < p.max { p.max } else { p.min };
    Some(set_param(k, p.key, to))
}

/// The acceptance run for one family: `src` has the sound, `dst` is a new device of the same
/// type. Applying the patch is one undo step that restores the text byte for byte, the result
/// has the sound of a device pasted from the same patch, and a reset gives a new device back.
fn family(r: &Registry, type_id: &str, mut setup: Vec<Command>) -> Vec<SyncOp> {
    setup.extend(tweak(r, type_id, "src"));
    let mut d = doc_with(r, &[(type_id, "src"), (type_id, "dst")], setup);
    let fresh = device(&d, "dst").clone();
    let snapshot = patch_of(&d, "src");
    assert_ne!(
        device(&d, "src").params,
        fresh.params,
        "the source differs from a new device"
    );
    let ops = one_step(&mut d, r, apply("dst", &snapshot));
    assert_same_sound(device(&d, "dst"), device(&d, "src"));

    // A device created from the same patch has the same sound.
    let mut pasted = doc_with(r, &[], vec![]);
    run(
        &mut pasted,
        r,
        [wf(WorkflowCommand::PasteDevices {
            route: false,
            snapshot: snapshot.clone(),
            container: None,
            at: 0,
        })],
    );
    assert_same_sound(&pasted.doc().devices[0], device(&d, "dst"));

    // Reset leaves a device equal to a new one.
    one_step(&mut d, r, reset("dst"));
    let mut reset_device = device(&d, "dst").clone();
    reset_device.patch_name.clear();
    assert_eq!(reset_device, fresh);
    ops
}

#[test]
fn a_synth_takes_every_param_and_the_rest_go_back_to_default() {
    let r = registry();
    let mut d = doc_with(
        &r,
        &[(SYNTH, "src"), (SYNTH, "dst")],
        vec![
            set_param("src", "filter.cutoff", 1234.0),
            set_param("dst", "filter.cutoff", 400.0),
            set_param("dst", "filter.resonance", 0.8),
        ],
    );
    let snapshot = patch_of(&d, "src");
    let ops = one_step(&mut d, &r, apply("dst", &snapshot));
    assert!(ops.iter().any(|o| matches!(o, SyncOp::ParamChanged { .. })));
    assert_eq!(
        device(&d, "dst").params.get("filter.cutoff"),
        device(&d, "src").params.get("filter.cutoff")
    );
    // A param the patch does not store is back at its default, not at the old value.
    assert!(!device(&d, "dst").params.contains_key("filter.resonance"));
    assert_same_sound(device(&d, "dst"), device(&d, "src"));
}

#[test]
fn every_factory_device_takes_a_patch_of_its_own_type() {
    let r = registry();
    for type_id in r.type_ids().collect::<Vec<_>>() {
        if type_id == SPLIT || type_id == CONTAINER {
            continue;
        }
        let Some(c) = tweak(&r, type_id, "src") else {
            continue;
        };
        let mut d = doc_with(&r, &[(type_id, "src"), (type_id, "dst")], vec![c]);
        let snapshot = patch_of(&d, "src");
        one_step(&mut d, &r, apply("dst", &snapshot));
        assert_same_sound(device(&d, "dst"), device(&d, "src"));
        one_step(&mut d, &r, reset("dst"));
        let mut fresh = doc_with(&r, &[(type_id, "dst")], vec![]);
        let mut reset_device = device(&d, "dst").clone();
        reset_device.name.clone_from(&device(&fresh, "dst").name);
        assert_eq!(&reset_device, device(&fresh, "dst"), "{type_id}");
        fresh.end_merge();
    }
}

#[test]
fn a_sampler_takes_its_zones_and_they_are_one_undo_step() {
    let r = registry();
    let ops = family(
        &r,
        SAMPLER,
        vec![Command::SetZones {
            device: key("src"),
            zones: vec![ZoneDoc::new(ASSET, 48_000)],
            merge: None,
        }],
    );
    assert!(ops.iter().any(|o| matches!(o, SyncOp::ZonesChanged(_))));
}

#[test]
fn a_patch_whose_samples_are_not_in_the_pool_is_refused() {
    let r = registry();
    let src = doc_with(
        &r,
        &[(SAMPLER, "src")],
        vec![Command::SetZones {
            device: key("src"),
            zones: vec![ZoneDoc::new(ASSET, 48_000)],
            merge: None,
        }],
    );
    let mut snapshot = patch_of(&src, "src");
    snapshot.assets.clear();
    let mut dst = Document::new(empty_doc());
    run(&mut dst, &r, [add(0, SAMPLER, Some("dst"))]);
    let e = rejected(&mut dst, &r, apply("dst", &snapshot));
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
}

#[test]
fn a_step_sequencer_takes_its_patterns() {
    let r = registry();
    let ops = family(&r, SEQ, vec![Command::AddPattern { device: key("src") }]);
    assert!(
        ops.iter()
            .any(|o| matches!(o, SyncOp::PatternsChanged { .. }))
    );
}

#[test]
fn a_cv_sequencer_takes_its_curves() {
    let r = registry();
    let mut p = CurvePatternDoc::default();
    p.rows[0][3] = CurveStepDoc {
        value: 64,
        slide: true,
    };
    let ops = family(
        &r,
        CV,
        vec![Command::SetCurves {
            device: key("src"),
            index: 0,
            curves: p,
            merge: None,
        }],
    );
    assert!(
        ops.iter()
            .any(|o| matches!(o, SyncOp::PatternsChanged { .. }))
    );
}

#[test]
fn a_drum_generator_takes_its_patterns() {
    let r = registry();
    family(
        &r,
        DRUMS,
        vec![Command::SetDrumStep {
            device: key("src"),
            index: 0,
            lane: 0,
            step: 0,
            velocity: 90,
            prob: 100,
            merge: None,
        }],
    );
}

#[test]
fn a_plugin_takes_its_state_and_it_comes_back_on_undo() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("src")),
            add_plugin(1, &synth_ref(), &synth_layout(), Some("dst")),
            set_param("src", "p10", 0.9),
            sync_state("src", &[1, 2, 3, 4]),
            sync_state("dst", &[9, 9]),
        ],
    );
    let snapshot = patch_of(&d, "src");
    let ops = one_step(&mut d, &r, apply("dst", &snapshot));
    assert!(
        ops.iter()
            .any(|o| matches!(o, SyncOp::PluginStateChanged { device } if *device == key("dst")))
    );
    assert_eq!(
        plugin_of(d.doc(), "dst").state.as_deref(),
        Some(&PluginStateBlob(vec![1, 2, 3, 4]))
    );
    // Undo puts the old blob back, redo the patch's.
    d.undo(&r).unwrap();
    assert_eq!(
        plugin_of(d.doc(), "dst").state.as_deref(),
        Some(&PluginStateBlob(vec![9, 9]))
    );
    d.redo(&r).unwrap();
    assert_same_sound(device(&d, "dst"), device(&d, "src"));
    // A reset leaves the plugin without state, as a new one.
    one_step(&mut d, &r, reset("dst"));
    assert!(plugin_of(d.doc(), "dst").state.is_none());
}

#[test]
fn a_container_takes_its_inside_with_its_cables_kept() {
    let r = registry();
    let mut d = container_doc(&r);
    run(
        &mut d,
        &r,
        [
            add(1, CONTAINER, Some("other")),
            add(2, MIXER, Some("mix")),
            connect("other:out_l", "mix:ch1_in_l"),
            set_param("c/synth", "filter.cutoff", 777.0),
        ],
    );
    let snapshot = patch_of(&d, "c");
    let before = text(&d);
    let ops = one_step(&mut d, &r, apply("other", &snapshot));
    assert_eq!(device(&d, "other").inner, device(&d, "c").inner);
    // The cable out of the container is still there.
    assert!(
        d.doc()
            .cables
            .contains(&cable("other:out_l", "mix:ch1_in_l"))
    );
    assert!(ops.contains(&SyncOp::DeviceRemoved(key("other"))));
    assert!(ops.contains(&SyncOp::DeviceAdded(key("other"))));
    assert_ne!(text(&d), before);
    one_step(&mut d, &r, reset("other"));
    assert!(
        device(&d, "other")
            .inner
            .as_ref()
            .unwrap()
            .devices
            .is_empty()
    );
}

#[test]
fn a_device_inside_a_container_takes_a_patch() {
    let r = registry();
    let mut d = container_doc(&r);
    run(&mut d, &r, [set_param("c/synth", "filter.cutoff", 321.0)]);
    run(&mut d, &r, [add(1, SYNTH, Some("top"))]);
    let snapshot = patch_of(&d, "top");
    run(&mut d, &r, [set_param("top", "filter.cutoff", 9000.0)]);
    let from_top = patch_of(&d, "top");
    one_step(&mut d, &r, apply("c/synth", &from_top));
    assert_eq!(
        device(&d, "c/synth").params.get("filter.cutoff"),
        Some(&9000.0)
    );
    let _ = snapshot;
}

#[test]
fn a_type_mismatch_leaves_the_document_untouched() {
    let r = registry();
    let mut d = doc_with(&r, &[(SYNTH, "s"), (DELAY, "fx")], vec![]);
    let snapshot = patch_of(&d, "s");
    let e = rejected(&mut d, &r, apply("fx", &snapshot));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    // A patch of two devices is not a device patch.
    let two = DeviceSnapshot::capture(d.doc(), &[key("s"), key("fx")], false).unwrap();
    rejected(&mut d, &r, apply("s", &two));
    // Nor is an unknown device a target.
    let e = rejected(&mut d, &r, apply("nope", &snapshot));
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e:?}");
}

#[test]
fn a_patch_that_changes_nothing_is_rejected() {
    let r = registry();
    let mut d = doc_with(&r, &[(SYNTH, "s")], vec![]);
    let own = patch_of(&d, "s");
    rejected(&mut d, &r, apply("s", &own));
    rejected(&mut d, &r, reset("s"));
}

#[test]
fn a_param_a_macro_drives_is_left_alone() {
    let r = registry();
    let mut d = container_doc(&r);
    run(
        &mut d,
        &r,
        [
            set_macro("c", 0, target("synth", "filter.cutoff", 200.0, 8000.0)),
            add(1, CONTAINER, Some("src")),
            add_inner("src", 0, SYNTH, "synth"),
            set_param("src/synth", "filter.cutoff", 3000.0),
            set_param("src/synth", "filter.resonance", 0.7),
        ],
    );
    // A patch of the inner synth applied to the macro-driven one changes the other params only.
    let snapshot = patch_of(&d, "src/synth");
    let driven_before = device(&d, "c/synth").params.get("filter.cutoff").copied();
    run(&mut d, &r, [apply("c/synth", &snapshot)]);
    assert_eq!(
        device(&d, "c/synth").params.get("filter.cutoff").copied(),
        driven_before
    );
}

#[test]
fn a_console_device_has_no_patch() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("s"))]);
    let channel = d.doc().console.channels[0].channel.clone();
    let snapshot = patch_of(&d, "s");
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::ApplyPatch {
            device: channel,
            snapshot,
        }),
    );
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
}

#[test]
fn a_frozen_device_refuses_a_patch_and_a_reset() {
    let r = audio_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            register(1, 480_000),
            add(0, SYNTH, Some("synth")),
            add(1, SYNTH, Some("other")),
            add_track("synth"),
            add_clip(
                "synth",
                0,
                notes_clip(0, 2 * BAR, vec![note(0, 480, 60, 0.8)]),
            ),
            sc(SongCommand::FreezeTrack {
                device: key("synth"),
                frozen: Frozen {
                    start: 0,
                    length: 2 * BAR,
                    audio: audio(1),
                    effects: Vec::new(),
                },
            }),
            set_param("other", "filter.cutoff", 500.0),
        ],
    );
    let snapshot = patch_of(&d, "other");
    let e = rejected(&mut d, &r, apply("synth", &snapshot));
    assert!(matches!(e, ModelError::Frozen(_)), "{e:?}");
    let e = rejected(&mut d, &r, reset("synth"));
    assert!(matches!(e, ModelError::Frozen(_)), "{e:?}");
}

#[test]
fn the_defaults_patch_is_a_new_device_of_the_type() {
    let r = registry();
    let s = DeviceSnapshot::defaults(&r, SEQ).unwrap();
    let one = s.single_device().unwrap();
    assert!(one.params.is_empty() && one.steps.is_some() && one.patch_name.is_empty());
    assert!(DeviceSnapshot::defaults(&r, "atomik.nope").is_err());
}

// ------------------------------------------------------------------ the patch name

#[test]
fn applying_a_patch_sets_its_name_and_a_reset_clears_it() {
    let r = registry();
    let mut d = doc_with(
        &r,
        &[(SYNTH, "src"), (SYNTH, "dst")],
        vec![set_param("src", "filter.cutoff", 1000.0)],
    );
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SetPatchName {
            device: key("src"),
            name: "Warm Pad".into(),
        }),
    );
    let snapshot = patch_of(&d, "src");
    assert_eq!(snapshot.devices[0].patch_name, "Warm Pad");
    one_step(&mut d, &r, apply("dst", &snapshot));
    assert_eq!(device(&d, "dst").patch_name, "Warm Pad");
    one_step(&mut d, &r, reset("dst"));
    assert_eq!(device(&d, "dst").patch_name, "");
}

#[test]
fn a_patch_name_is_trimmed_short_and_not_a_control_text() {
    let r = registry();
    let mut d = doc_with(&r, &[(SYNTH, "s")], vec![]);
    for bad in [" Pad", "Pad ", "a\nb", &"x".repeat(65)] {
        rejected(
            &mut d,
            &r,
            wf(WorkflowCommand::SetPatchName {
                device: key("s"),
                name: bad.into(),
            }),
        );
    }
    // Clearing a name that is not there changes nothing.
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::SetPatchName {
            device: key("s"),
            name: String::new(),
        }),
    );
}

#[test]
fn the_patch_name_round_trips_and_is_left_out_when_empty() {
    let r = registry();
    let mut d = doc_with(&r, &[(SYNTH, "s"), (CONTAINER, "c")], vec![]);
    run(&mut d, &r, [add_inner("c", 0, DELAY, "fx")]);
    let plain = save_project(d.doc());
    assert!(!plain.contains("\"patch\""), "no name, no field");
    run(
        &mut d,
        &r,
        [
            wf(WorkflowCommand::SetPatchName {
                device: key("s"),
                name: "Warm Pad".into(),
            }),
            wf(WorkflowCommand::SetPatchName {
                device: key("c/fx"),
                name: "Wide".into(),
            }),
        ],
    );
    let named = save_project(d.doc());
    assert!(named.contains("\"patch\": \"Warm Pad\""));
    let back = load_project(&named, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), named);
    // Without the names the text is what it was.
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(save_project(d.doc()), plain);
    let v: Value = serde_json::from_str(&plain).unwrap();
    assert_eq!(v["version"], PROJECT_VERSION);
}

#[test]
fn a_version_35_file_refuses_the_patch_name_and_36_refuses_a_bad_one() {
    let r = registry();
    let mut d = doc_with(&r, &[(SYNTH, "s")], vec![]);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::SetPatchName {
            device: key("s"),
            name: "Pad".into(),
        })],
    );
    let named = save_project(d.doc());
    let old = named.replacen("\"version\": 39", "\"version\": 35", 1);
    let e = load_project(&old, &r).unwrap_err().to_string();
    assert!(e.contains("patch"), "{e}");
    for bad in ["\"\"", "\" Pad\"", "\"a\\nb\"", "7"] {
        let broken = named.replacen("\"Pad\"", bad, 1);
        assert!(load_project(&broken, &r).is_err(), "{bad}");
    }
}

// ------------------------------------------------------------------ the item

#[test]
fn a_one_device_item_says_its_type_and_carries_the_name() {
    let r = registry();
    let mut d = doc_with(
        &r,
        &[(SYNTH, "s"), (DELAY, "fx")],
        vec![set_param("s", "filter.cutoff", 900.0)],
    );
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::SetPatchName {
            device: key("s"),
            name: "Warm Pad".into(),
        })],
    );
    let one = save_device_item(&patch_of(&d, "s"), d.doc());
    let info = summarize_item(ItemKind::Patch, &one).unwrap();
    assert_eq!(info.device_type.as_deref(), Some(SYNTH));
    let loaded = load_device_item(&one, &r).unwrap();
    assert_eq!(loaded.single_device().unwrap().patch_name, "Warm Pad");

    let two = DeviceSnapshot::capture(d.doc(), &[key("s"), key("fx")], false).unwrap();
    let text = save_device_item(&two, d.doc());
    assert_eq!(
        summarize_item(ItemKind::Patch, &text).unwrap().device_type,
        None
    );
}

#[test]
fn a_loaded_item_applies_to_a_device_of_the_same_type() {
    let r = registry();
    let mut d = doc_with(
        &r,
        &[(SYNTH, "s"), (SYNTH, "t")],
        vec![set_param("s", "filter.cutoff", 900.0)],
    );
    let item = save_device_item(&patch_of(&d, "s"), d.doc());
    let snapshot = load_device_item(&item, &r).unwrap();
    one_step(&mut d, &r, apply("t", &snapshot));
    assert_same_sound(device(&d, "t"), device(&d, "s"));
}

// ---------------------------------------------------------------------- trials

/// A trial of patch applications records no undo step; committing it leaves one step that
/// goes back to the original, and cancelling it restores the original and the redo history.
#[test]
fn a_trial_is_one_undo_step_when_committed_and_nothing_when_cancelled() {
    let r = registry();
    let type_id = SYNTH;
    let mut setup = Vec::new();
    setup.extend(tweak(&r, type_id, "src"));
    let mut d = doc_with(&r, &[(type_id, "src"), (type_id, "dst")], setup);
    let snapshot = patch_of(&d, "src");
    let original = device(&d, "dst").clone();
    let depth = d.undo_depth();

    for _ in 0..50 {
        d.apply_trial(apply("dst", &snapshot), &r).unwrap();
        d.apply_trial(reset("dst"), &r).unwrap();
    }
    d.apply_trial(apply("dst", &snapshot), &r).unwrap();
    assert!(d.in_trial());
    assert_eq!(d.undo_depth(), depth, "a trial records no undo step");
    assert_same_sound(device(&d, "dst"), device(&d, "src"));
    let trialled = d.clone();

    // Cancel: the device is as it was, byte for byte.
    let mut cancelled = trialled.clone();
    let ops = cancelled.cancel_trial().unwrap();
    assert!(!ops.is_empty());
    assert!(!cancelled.in_trial());
    assert_eq!(device(&cancelled, "dst"), &original);
    assert_eq!(cancelled.undo_depth(), depth);

    // Commit: exactly one step, and undo restores the original.
    let mut committed = trialled;
    committed.commit_trial();
    assert!(!committed.in_trial());
    assert_eq!(committed.undo_depth(), depth + 1);
    assert_same_sound(device(&committed, "dst"), device(&committed, "src"));
    committed.undo(&r).unwrap();
    assert_eq!(device(&committed, "dst"), &original);
    assert_eq!(committed.undo_depth(), depth);
}

#[test]
fn undo_waits_for_the_trial_and_the_redo_history_survives_a_cancel() {
    let r = registry();
    let type_id = SYNTH;
    let mut d = doc_with(&r, &[(type_id, "src"), (type_id, "dst")], vec![]);
    run(&mut d, &r, tweak(&r, type_id, "src"));
    let snapshot = patch_of(&d, "src");
    d.undo(&r).unwrap();
    assert!(d.can_redo());
    d.apply_trial(apply("dst", &snapshot), &r).unwrap();
    assert!(d.undo(&r).is_none(), "undo is refused during a trial");
    d.cancel_trial().unwrap();
    assert!(d.can_redo(), "the redo history comes back");
    // A trial that changes nothing the history records leaves everything alone.
    let mut e = doc_with(&r, &[(type_id, "src")], vec![]);
    assert!(!e.in_trial());
    e.commit_trial();
    assert_eq!(e.undo_depth(), 1);
}
