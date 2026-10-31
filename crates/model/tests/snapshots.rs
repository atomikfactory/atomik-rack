//! Mixer snapshots (model half): capture and recall, the A/B compare, one undo step per
//! operation, the rules the commands refuse, a recall after the console changed, the scopes, and
//! the project format 27 (round trip, refusals with their JSON path, and the version 26 file that
//! loads unchanged).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::console::{MAX_SNAPSHOT_NAME_CHARS, MAX_SNAPSHOTS};
use atomik_model::{
    Command, ConsoleCommand, DeviceKey, Document, InsertEffect, PROJECT_VERSION, SnapshotScope,
    SyncOp, load_project, save_project,
};
use common::console_ops::assert_console_invariants;
use common::*;
use serde_json::Value;

/// The starter with four synths: channels `channel`, `channel-2`, `channel-3`, `channel-4`, an
/// effect `comp` on the insert of `channel`.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut doc = legacy_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    run(
        &mut d,
        &r,
        ["a", "b", "c", "d"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, COMP, Some("comp")),
            con(ConsoleCommand::SetInsert {
                channel: key("channel"),
                slot: 0,
                effect: Some(InsertEffect::Existing(key("comp"))),
            }),
        ],
    );
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn store(name: &str) -> Command {
    con(ConsoleCommand::StoreSnapshot { name: name.into() })
}

fn recall(id: u32) -> Command {
    con(ConsoleCommand::RecallSnapshot {
        id,
        scope: SnapshotScope::ALL,
    })
}

fn recall_scoped(id: u32, scope: SnapshotScope) -> Command {
    con(ConsoleCommand::RecallSnapshot { id, scope })
}

fn compare(id: u32) -> Command {
    con(ConsoleCommand::Compare { id })
}

fn value(d: &Document, r: &Registry, dev: &str, param: &str) -> f32 {
    d.doc().param_value(&key(dev), param, r).unwrap()
}

/// A mix with something different on every kind of thing a snapshot holds.
fn mix_one(d: &mut Document, r: &Registry) {
    run(
        d,
        r,
        [
            set_param("channel", "fader", 0.4),
            set_param("channel", "pan", -0.5),
            set_param("channel", "input.gain", 6.0),
            set_param("channel", "send2.level", 0.3),
            set_param("channel", "send2.on", 1.0),
            set_param("channel-2", "mute", 1.0),
            set_param("channel-3", "eq.hf.gain", 9.0),
            set_param("master", "fader", 0.6),
            set_param("comp", "bypass", 1.0),
            con(ConsoleCommand::CreateVca {
                name: "Drums".into(),
                members: vec![key("channel"), key("channel-2")],
            }),
            con(ConsoleCommand::SetVcaLevel {
                vca: 1,
                db: -6.0,
                merge: None,
            }),
            con(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
        ],
    );
}

/// A different mix.
fn mix_two(d: &mut Document, r: &Registry) {
    run(
        d,
        r,
        [
            set_param("channel", "fader", 0.9),
            set_param("channel", "pan", 0.25),
            set_param("channel", "send2.level", 0.0),
            set_param("channel", "send2.on", 0.0),
            set_param("channel-2", "mute", 0.0),
            set_param("channel-4", "solo", 1.0),
            set_param("master", "fader", 0.2),
            set_param("comp", "bypass", 0.0),
            con(ConsoleCommand::SetVcaLevel {
                vca: 1,
                db: 3.0,
                merge: None,
            }),
            con(ConsoleCommand::SetVcaMute { vca: 1, on: false }),
            con(ConsoleCommand::AssignToVca {
                vca: None,
                strips: vec![key("channel-2")],
            }),
        ],
    );
}

/// Applies `c` and expects a refusal that leaves the document and the history alone.
fn refused(d: &mut Document, r: &Registry, c: Command) -> String {
    let before = d.doc().clone();
    let can_undo = d.can_undo();
    let e = d.apply(c.clone(), r).expect_err(&format!("{c:?} accepted"));
    assert_eq!(d.doc(), &before, "{c:?} changed the document");
    assert_eq!(d.can_undo(), can_undo, "{c:?} recorded a step");
    e.to_string()
}

/// Applies `c` and checks it is exactly one undo step.
fn one_step(d: &mut Document, r: &Registry, c: Command) {
    let before = d.doc().clone();
    d.apply(c.clone(), r)
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    let after = d.doc().clone();
    assert_ne!(after, before, "{c:?} changed nothing");
    assert_console_invariants(&after);
    d.undo(r).expect("undo");
    assert_eq!(d.doc(), &before, "{c:?} undone in one step");
    d.redo(r).expect("redo");
    assert_eq!(d.doc(), &after, "{c:?} redone");
}

fn names(d: &Document) -> Vec<String> {
    d.doc()
        .console
        .snapshots
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

// ---- Store, recall, round trip -------------------------------------------------------------

#[test]
fn a_stored_mix_comes_back_exactly() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    let stored = d.doc().capture_mixer(&r);
    d.apply(store("A"), &r).unwrap();
    assert_eq!(d.doc().console.snapshot(1).unwrap().state, stored);
    mix_two(&mut d, &r);
    assert_ne!(d.doc().capture_mixer(&r), stored);
    assert!(!d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    d.apply(recall(1), &r).unwrap();
    assert_eq!(d.doc().capture_mixer(&r), stored);
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    assert_eq!(value(&d, &r, "channel", "fader"), 0.4);
    assert_eq!(value(&d, &r, "channel", "send2.level"), 0.3);
    assert_eq!(value(&d, &r, "channel-2", "mute"), 1.0);
    assert_eq!(value(&d, &r, "comp", "bypass"), 1.0);
    assert_eq!(value(&d, &r, "channel-4", "solo"), 0.0);
    let g = d.doc().console.vca(1).unwrap();
    assert_eq!((g.level_db, g.mute), (-6.0, true));
    assert_eq!(g.members, [key("channel"), key("channel-2")]);
    assert_console_invariants(d.doc());
}

#[test]
fn a_snapshot_holds_every_strip_param_but_not_the_internal_ones_or_the_device_params() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("channel-3", "eq.hf.gain", 4.0),
            set_param("comp", "threshold", -30.0),
        ],
    );
    d.apply(store("A"), &r).unwrap();
    let state = &d.doc().console.snapshot(1).unwrap().state;
    for strip in ["channel", "channel-2", "channel-3", "channel-4", "master"] {
        assert!(state.devices.contains_key(&key(strip)), "{strip}");
    }
    assert!(
        state.devices[&key("channel-3")]
            .keys()
            .all(|k| !k.starts_with("internal."))
    );
    // The effect keeps its bypass only.
    assert!(state.devices[&key("comp")].keys().all(|k| k == "bypass"));
    // The instrument is not part of the mixer.
    assert!(!state.devices.contains_key(&key("a")));
}

#[test]
fn recall_sends_ordinary_param_changes_for_the_engine() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    let ops = d.apply(recall(1), &r).unwrap();
    assert!(ops.iter().any(|o| matches!(
        o,
        SyncOp::ParamChanged { key: k, param, .. } if k.0 == "channel" && param == "fader"
    )));
    assert!(ops.contains(&SyncOp::ConsoleChanged));
    // What did not change is not sent.
    assert!(!ops.iter().any(|o| matches!(
        o,
        SyncOp::ParamChanged { key: k, param, .. } if k.0 == "channel-3" && param == "eq.hf.gain"
    )));
}

#[test]
fn the_snapshots_do_not_touch_the_arrangement_or_the_rack() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    let before = d.doc().clone();
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    d.apply(recall(1), &r).unwrap();
    let after = d.doc();
    assert_eq!(after.song, before.song);
    assert_eq!(after.cables, before.cables);
    assert_eq!(after.tempo_bpm, before.tempo_bpm);
    let keys = |doc: &atomik_model::RackDoc| -> Vec<String> {
        doc.devices.iter().map(|x| x.key.0.clone()).collect()
    };
    assert_eq!(keys(after), keys(&before));
}

// ---- One undo step each ----------------------------------------------------------------------

#[test]
fn every_operation_is_one_undo_step() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    one_step(&mut d, &r, store("A"));
    mix_two(&mut d, &r);
    one_step(&mut d, &r, store("B"));
    one_step(&mut d, &r, recall(1));
    one_step(&mut d, &r, recall_scoped(2, SnapshotScope::LEVELS));
    run(&mut d, &r, [set_param("channel", "fader", 0.1)]);
    one_step(&mut d, &r, con(ConsoleCommand::UpdateSnapshot { id: 1 }));
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RenameSnapshot {
            id: 1,
            name: "Verse".into(),
        }),
    );
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::MoveSnapshot { id: 2, to: 0 }),
    );
    one_step(&mut d, &r, compare(1));
    one_step(&mut d, &r, compare(1));
    one_step(&mut d, &r, con(ConsoleCommand::EndCompare));
    one_step(&mut d, &r, con(ConsoleCommand::DeleteSnapshot { id: 1 }));
}

#[test]
fn undoing_a_recall_puts_the_whole_mix_back() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    let before = d.doc().capture_mixer(&r);
    d.apply(recall(1), &r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc().capture_mixer(&r), before);
    assert_eq!(d.doc().console.snapshots.len(), 1);
}

// ---- The rules --------------------------------------------------------------------------------

#[test]
fn names_are_trimmed_unique_and_bounded() {
    let (mut d, r) = project();
    d.apply(store("  Verse "), &r).unwrap();
    assert_eq!(names(&d), ["Verse"]);
    assert!(refused(&mut d, &r, store("verse")).contains("unique"));
    assert!(refused(&mut d, &r, store("   ")).contains("empty"));
    let long = "x".repeat(MAX_SNAPSHOT_NAME_CHARS + 1);
    assert!(refused(&mut d, &r, store(&long)).contains("at most"));
    d.apply(store("Chorus"), &r).unwrap();
    let rename = |id, name: &str| {
        con(ConsoleCommand::RenameSnapshot {
            id,
            name: name.into(),
        })
    };
    assert!(refused(&mut d, &r, rename(2, "VERSE")).contains("unique"));
    // The same snapshot may keep its name in another case.
    d.apply(rename(1, "VERSE"), &r).unwrap();
    assert_eq!(names(&d), ["VERSE", "Chorus"]);
    assert!(refused(&mut d, &r, rename(9, "x")).contains("no snapshot 9"));
}

#[test]
fn there_are_at_most_sixteen_and_ids_are_not_reused() {
    let (mut d, r) = project();
    for n in 0..MAX_SNAPSHOTS {
        d.apply(store(&format!("S{n}")), &r).unwrap();
    }
    assert!(refused(&mut d, &r, store("one more")).contains("at most 16"));
    d.apply(con(ConsoleCommand::DeleteSnapshot { id: 3 }), &r)
        .unwrap();
    d.apply(store("new"), &r).unwrap();
    assert_eq!(d.doc().console.snapshots.last().unwrap().id, 17);
}

#[test]
fn commands_that_change_nothing_are_refused() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    assert!(refused(&mut d, &r, recall(1)).contains("already matches"));
    assert!(refused(&mut d, &r, con(ConsoleCommand::UpdateSnapshot { id: 1 })).contains("already"));
    assert!(
        refused(
            &mut d,
            &r,
            con(ConsoleCommand::MoveSnapshot { id: 1, to: 0 })
        )
        .contains("already")
    );
    assert!(refused(&mut d, &r, con(ConsoleCommand::EndCompare)).contains("no snapshot"));
    assert!(refused(&mut d, &r, recall(7)).contains("no snapshot 7"));
    assert!(refused(&mut d, &r, compare(7)).contains("no snapshot 7"));
}

#[test]
fn reordering_moves_the_chip_only() {
    let (mut d, r) = project();
    for n in ["A", "B", "C"] {
        d.apply(store(n), &r).unwrap();
    }
    d.apply(con(ConsoleCommand::MoveSnapshot { id: 3, to: 0 }), &r)
        .unwrap();
    assert_eq!(names(&d), ["C", "A", "B"]);
    assert_eq!(d.doc().console.snapshots[0].id, 3);
}

// ---- The scopes -------------------------------------------------------------------------------

#[test]
fn a_scoped_recall_applies_only_its_categories() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    run(&mut d, &r, [set_param("channel-3", "eq.hf.gain", 0.1)]);

    d.apply(recall_scoped(1, SnapshotScope::MUTES), &r).unwrap();
    assert_eq!(value(&d, &r, "channel-2", "mute"), 1.0);
    assert_eq!(value(&d, &r, "channel-4", "solo"), 0.0);
    assert!(d.doc().console.vca(1).unwrap().mute);
    assert_eq!(value(&d, &r, "channel", "fader"), 0.9, "levels stay");
    assert_eq!(value(&d, &r, "channel", "send2.level"), 0.0, "sends stay");
    assert_eq!(value(&d, &r, "channel", "send2.on"), 0.0);
    assert_eq!(value(&d, &r, "comp", "bypass"), 0.0, "inserts stay");

    d.apply(recall_scoped(1, SnapshotScope::LEVELS), &r)
        .unwrap();
    assert_eq!(value(&d, &r, "channel", "fader"), 0.4);
    assert_eq!(value(&d, &r, "master", "fader"), 0.6);
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, -6.0);
    assert_eq!(value(&d, &r, "channel", "send2.level"), 0.0);

    d.apply(recall_scoped(1, SnapshotScope::SENDS), &r).unwrap();
    assert_eq!(value(&d, &r, "channel", "send2.level"), 0.3);
    assert_eq!(value(&d, &r, "channel", "send2.on"), 1.0);
    assert_eq!(value(&d, &r, "comp", "bypass"), 0.0);

    d.apply(recall_scoped(1, SnapshotScope::INSERTS), &r)
        .unwrap();
    assert_eq!(value(&d, &r, "comp", "bypass"), 1.0);
    assert_eq!(
        value(&d, &r, "channel-3", "eq.hf.gain"),
        0.1,
        "processing stays"
    );
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::INSERTS));
    assert!(!d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
}

#[test]
fn the_scope_names_are_known() {
    for (n, s) in [
        ("all", SnapshotScope::ALL),
        ("levels", SnapshotScope::LEVELS),
        ("mutes", SnapshotScope::MUTES),
        ("sends", SnapshotScope::SENDS),
        ("inserts", SnapshotScope::INSERTS),
    ] {
        assert_eq!(SnapshotScope::from_name(n), Some(s));
    }
}

// ---- Structural drift -------------------------------------------------------------------------

#[test]
fn a_recall_skips_what_was_deleted_and_leaves_what_was_added() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    // A channel goes, another comes, a group is deleted.
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("e")),
            con(ConsoleCommand::RemoveChannel {
                channel: key("channel-3"),
            }),
            set_param("channel-5", "fader", 0.33),
            con(ConsoleCommand::DeleteVca { vca: 1 }),
        ],
    );
    let notes = d.doc().snapshot_notes(1);
    assert!(notes.iter().any(|n| n.contains("channel-3")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("Drums")), "{notes:?}");
    d.apply(recall(1), &r).unwrap();
    assert_eq!(value(&d, &r, "channel", "fader"), 0.4);
    assert_eq!(value(&d, &r, "channel-2", "mute"), 1.0);
    assert_eq!(
        value(&d, &r, "channel-5", "fader"),
        0.33,
        "a new channel stays"
    );
    assert!(
        d.doc().console.vcas.is_empty(),
        "a deleted group stays deleted"
    );
    assert_console_invariants(d.doc());
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
}

#[test]
fn a_deleted_effect_is_skipped() {
    let (mut d, r) = project();
    run(&mut d, &r, [set_param("comp", "bypass", 1.0)]);
    d.apply(store("A"), &r).unwrap();
    run(&mut d, &r, [Command::RemoveDevice { key: key("comp") }]);
    let notes = d.doc().snapshot_notes(1);
    assert!(notes.iter().any(|n| n.contains("comp")), "{notes:?}");
    run(&mut d, &r, [set_param("channel", "fader", 0.2)]);
    d.apply(recall(1), &r).unwrap();
    assert!(d.doc().device(&key("comp")).is_none());
}

#[test]
fn group_membership_recalls_for_the_strips_the_snapshot_knew() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [con(ConsoleCommand::CreateVca {
            name: "Drums".into(),
            members: vec![key("channel"), key("channel-2")],
        })],
    );
    d.apply(store("A"), &r).unwrap();
    // channel-2 leaves, channel-3 joins, channel-5 is new and joins too.
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::AssignToVca {
                vca: None,
                strips: vec![key("channel-2")],
            }),
            con(ConsoleCommand::AssignToVca {
                vca: Some(1),
                strips: vec![key("channel-3")],
            }),
            add(usize::MAX, SYNTH, Some("e")),
            con(ConsoleCommand::AssignToVca {
                vca: Some(1),
                strips: vec![key("channel-5")],
            }),
        ],
    );
    d.apply(recall(1), &r).unwrap();
    let members: Vec<String> = d
        .doc()
        .console
        .vca(1)
        .unwrap()
        .members
        .iter()
        .map(|m| m.0.clone())
        .collect();
    assert_eq!(members, ["channel", "channel-2", "channel-5"]);
}

// ---- Compare ----------------------------------------------------------------------------------

#[test]
fn compare_flips_between_the_working_mix_and_the_snapshot_without_losing_either() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    let working = d.doc().capture_mixer(&r);
    let snapshot = d.doc().console.snapshot(1).unwrap().state.clone();

    d.apply(compare(1), &r).unwrap();
    let c = d.doc().console.compare.clone().unwrap();
    assert!(c.on_snapshot);
    assert_eq!(c.snapshot, 1);
    assert_eq!(c.other, working);
    assert_eq!(d.doc().capture_mixer(&r), snapshot);

    for _ in 0..3 {
        d.apply(compare(1), &r).unwrap();
        assert!(!d.doc().console.compare.as_ref().unwrap().on_snapshot);
        assert_eq!(d.doc().capture_mixer(&r), working);
        d.apply(compare(1), &r).unwrap();
        assert!(d.doc().console.compare.as_ref().unwrap().on_snapshot);
        assert_eq!(d.doc().capture_mixer(&r), snapshot);
    }
    // The stored snapshot never changed.
    assert_eq!(d.doc().console.snapshot(1).unwrap().state, snapshot);
}

#[test]
fn an_edit_made_while_comparing_belongs_to_the_side_it_was_made_on() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    let stored = d.doc().console.snapshot(1).unwrap().state.clone();
    d.apply(compare(1), &r).unwrap();
    // On the snapshot's side: nudge a fader.
    run(&mut d, &r, [set_param("channel", "fader", 0.55)]);
    d.apply(compare(1), &r).unwrap();
    assert_eq!(
        value(&d, &r, "channel", "fader"),
        0.9,
        "working side is as it was"
    );
    // On the working side: nudge another.
    run(&mut d, &r, [set_param("channel", "pan", -0.9)]);
    d.apply(compare(1), &r).unwrap();
    assert_eq!(
        value(&d, &r, "channel", "fader"),
        0.55,
        "the edit came back"
    );
    assert_eq!(value(&d, &r, "channel", "pan"), -0.5);
    d.apply(compare(1), &r).unwrap();
    assert_eq!(value(&d, &r, "channel", "pan"), -0.9);
    assert_eq!(
        d.doc().console.snapshot(1).unwrap().state,
        stored,
        "the stored snapshot is only changed by Update"
    );
}

#[test]
fn ending_the_compare_keeps_the_live_side_and_recall_ends_it() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    let working = d.doc().capture_mixer(&r);
    let snapshot = d.doc().console.snapshot(1).unwrap().state.clone();

    // Ending on the snapshot's side keeps the snapshot's mix.
    d.apply(compare(1), &r).unwrap();
    d.apply(con(ConsoleCommand::EndCompare), &r).unwrap();
    assert!(d.doc().console.compare.is_none());
    assert_eq!(d.doc().capture_mixer(&r), snapshot);

    // Ending on the working side keeps the working mix.
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc().capture_mixer(&r), working);
    d.apply(compare(1), &r).unwrap();
    d.apply(compare(1), &r).unwrap();
    d.apply(con(ConsoleCommand::EndCompare), &r).unwrap();
    assert!(d.doc().console.compare.is_none());
    assert_eq!(d.doc().capture_mixer(&r), working);

    // A recall ends a compare and keeps what it recalled.
    d.apply(compare(1), &r).unwrap();
    d.apply(compare(1), &r).unwrap();
    d.apply(recall(1), &r).unwrap();
    assert!(d.doc().console.compare.is_none());
    assert_eq!(d.doc().capture_mixer(&r), snapshot);
}

#[test]
fn deleting_the_compared_snapshot_ends_the_compare_and_another_one_takes_its_place() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    d.apply(store("B"), &r).unwrap();
    let working = d.doc().capture_mixer(&r);
    d.apply(compare(1), &r).unwrap();
    // B takes the place of A on the snapshot side; the working mix stays in the slot.
    d.apply(compare(2), &r).unwrap();
    assert_eq!(d.doc().console.compare.as_ref().unwrap().snapshot, 2);
    assert_eq!(
        d.doc().capture_mixer(&r),
        d.doc().console.snapshot(2).unwrap().state
    );
    assert_eq!(d.doc().console.compare.as_ref().unwrap().other, working);
    // From the working side only the slot changes.
    d.apply(compare(2), &r).unwrap();
    assert_eq!(d.doc().capture_mixer(&r), working);
    d.apply(compare(1), &r).unwrap();
    assert_eq!(d.doc().capture_mixer(&r), working);
    assert_eq!(
        d.doc().console.compare.as_ref().unwrap().other,
        d.doc().console.snapshot(1).unwrap().state
    );
    d.apply(con(ConsoleCommand::DeleteSnapshot { id: 1 }), &r)
        .unwrap();
    assert!(d.doc().console.compare.is_none());
}

#[test]
fn undoing_a_compare_restores_the_mix_and_the_slot() {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    let start = d.doc().clone();
    d.apply(compare(1), &r).unwrap();
    let on_b = d.doc().clone();
    d.apply(compare(1), &r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &on_b);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &start);
    assert!(d.doc().console.compare.is_none());
}

// ---- The file ---------------------------------------------------------------------------------

fn saved() -> (Document, Registry) {
    let (mut d, r) = project();
    mix_one(&mut d, &r);
    d.apply(store("A"), &r).unwrap();
    mix_two(&mut d, &r);
    d.apply(store("Chorus 2"), &r).unwrap();
    (d, r)
}

#[test]
fn snapshots_round_trip_in_format_27_in_a_stable_text() {
    let (d, r) = saved();
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 37);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 37);
    let snaps = &v["console"]["snapshots"];
    assert_eq!(snaps[0]["name"], "A");
    assert_eq!(snaps[1]["name"], "Chorus 2");
    let fader = snaps[0]["devices"]["channel"]["fader"].as_f64().unwrap();
    assert!((fader - 0.4).abs() < 1e-6, "{fader}");
    assert_eq!(snaps[0]["devices"]["comp"]["bypass"], 1.0);
    assert_eq!(snaps[0]["vcas"][0]["level_db"], -6.0);
    assert_eq!(snaps[0]["vcas"][0]["mute"], true);
    // The default is not written.
    assert!(snaps[0]["devices"]["channel"].get("mute").is_none());
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.console, d.doc().console);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn the_compare_slot_is_not_saved() {
    let (mut d, r) = saved();
    d.apply(compare(1), &r).unwrap();
    let text = save_project(d.doc());
    assert!(!text.contains("compare"));
    let loaded = load_project(&text, &r).unwrap();
    assert!(loaded.console.compare.is_none());
    assert_eq!(loaded.console.snapshots, d.doc().console.snapshots);
}

#[test]
fn a_project_without_snapshots_differs_from_its_version_26_file_in_the_version_line() {
    let (d, r) = project();
    let text = save_project(d.doc());
    assert!(!text.contains("snapshots"));
    let v26 = text.replace("\"version\": 37", "\"version\": 26");
    let loaded = load_project(&v26, &r).unwrap();
    assert!(loaded.console.snapshots.is_empty());
    assert_eq!(save_project(&loaded), text);
    // A file before the format has no field for them.
    let with = save_project(saved().0.doc()).replace("\"version\": 37", "\"version\": 26");
    let e = load_project(&with, &r).unwrap_err().to_string();
    assert!(e.contains("snapshots"), "{e}");
}

#[test]
fn a_snapshot_may_name_what_does_not_exist() {
    let (d, r) = saved();
    let text = save_project(d.doc()).replacen(
        "\"channel-3\": {",
        "\"gone\": {\"fader\": 0.5, \"nope\": 2.0, ",
        1,
    );
    // `channel-3` has other params, so the edit leaves a valid object either way.
    let loaded = load_project(&text, &r).unwrap();
    assert!(
        loaded
            .console
            .snapshot(1)
            .unwrap()
            .state
            .devices
            .contains_key(&DeviceKey("gone".into()))
    );
    let mut d2 = Document::new(loaded);
    d2.apply(recall(1), &r).ok();
    assert_console_invariants(d2.doc());
}

fn edited(from: &str, to: &str) -> String {
    let (d, _) = saved();
    let text = save_project(d.doc());
    assert!(text.contains(from), "{from} not in {text}");
    text.replacen(from, to, 1)
}

#[test]
fn a_bad_snapshot_is_refused_with_its_path() {
    let r = console_registry();
    let cases: [(&str, &str, &str); 8] = [
        (
            "\"id\": 2, \"name\": \"Chorus 2\"",
            "\"id\": 1, \"name\": \"Chorus 2\"",
            "used twice",
        ),
        (
            "\"id\": 1, \"name\": \"A\"",
            "\"id\": 0, \"name\": \"A\"",
            "from 1",
        ),
        ("\"name\": \"Chorus 2\"", "\"name\": \"a\"", "used twice"),
        (
            "\"name\": \"Chorus 2\"",
            "\"name\": \" Chorus\"",
            "valid snapshot name",
        ),
        ("\"fader\": 0.4", "\"fader\": \"loud\"", "number"),
        ("\"comp\": {\"bypass\": 1}", "\"comp\": 3", "object"),
        ("\"level_db\": -6", "\"level_db\": -61", "outside"),
        (
            "\"members\": [\"channel\", \"channel-2\"]}]}",
            "\"members\": [\"channel\", \"channel-2\"], \"x\": 1}]}",
            "unknown field",
        ),
    ];
    for (from, to, want) in cases {
        let e = load_project(&edited(from, to), &r).unwrap_err().to_string();
        assert!(e.contains(want) && e.contains("snapshots"), "{from} -> {e}");
    }
}

#[test]
fn more_than_sixteen_in_a_file_are_refused() {
    let (d, r) = project();
    let mut text = save_project(d.doc());
    let one = "{\"id\": 1, \"name\": \"A\", \"devices\": {}}";
    let many: Vec<String> = (1..=17)
        .map(|n| {
            one.replace("\"id\": 1", &format!("\"id\": {n}"))
                .replace("\"A\"", &format!("\"S{n}\""))
        })
        .collect();
    text = text.replacen(
        "\"master\":",
        &format!("\"snapshots\": [{}],\n    \"master\":", many.join(", ")),
        1,
    );
    let e = load_project(&text, &r).unwrap_err().to_string();
    assert!(e.contains("at most 16"), "{e}");
}
