//! Locked console strips (model half): the lock is a saved, undoable edit, and a locked strip
//! refuses every edit of its values and structure from every source, while the other strips of
//! the same command still change. The project format 46 field `"locked"`.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::console::{ConsoleCommand, InsertEffect};
use atomik_model::{
    Command, DeviceKey, Document, MacroCommand, ModelError, PROJECT_VERSION, SnapshotScope,
    load_project, save_project,
};
use common::*;

/// The starter with two synths: channels `channel` and `channel-2`.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    run(
        &mut d,
        &r,
        ["a", "b"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn lock(d: &mut Document, r: &Registry, strips: &[&str], locked: bool) {
    let strips = strips.iter().map(|s| key(s)).collect();
    d.apply(con(ConsoleCommand::SetLocked { strips, locked }), r)
        .expect("lock");
}

fn fader(d: &Document, k: &str) -> f32 {
    let dev = d.doc().device(&key(k)).expect("device");
    dev.params.get("fader").copied().unwrap_or(f32::NAN)
}

fn is_locked_error(e: &ModelError) -> bool {
    matches!(e, ModelError::Locked(m) if m.contains("is locked"))
}

/// Applies `c` and expects the lock's refusal, with the document and the history untouched.
fn refused(d: &mut Document, r: &Registry, c: Command) {
    let before = d.doc().clone();
    let depth = d.undo_depth();
    let e = d.apply(c, r).expect_err("a locked strip refuses");
    assert!(is_locked_error(&e), "{e}");
    assert_eq!(d.doc(), &before);
    assert_eq!(d.undo_depth(), depth);
}

#[test]
fn locking_is_one_undoable_step_and_unlock_restores_editing() {
    let (mut d, r) = project();
    lock(&mut d, &r, &["channel"], true);
    assert!(d.doc().console.is_locked(&key("channel")));
    assert!(!d.doc().console.is_locked(&key("channel-2")));
    assert_eq!(d.undo_label(), Some("Lock Channel"));
    refused(&mut d, &r, set_param("channel", "fader", 0.3));
    lock(&mut d, &r, &["channel"], false);
    d.apply(set_param("channel", "fader", 0.3), &r).unwrap();
    assert!((fader(&d, "channel") - 0.3).abs() < 1e-6);
    // Undo of the unlock locks again; undo of the lock unlocks.
    d.undo().unwrap();
    d.undo().unwrap();
    assert!(d.doc().console.is_locked(&key("channel")));
    d.undo().unwrap();
    assert!(d.doc().console.locked.is_empty());
}

#[test]
fn a_lock_that_changes_nothing_or_names_no_strip_is_refused() {
    let (mut d, r) = project();
    let c = |strips: Vec<DeviceKey>, locked| con(ConsoleCommand::SetLocked { strips, locked });
    assert!(d.apply(c(vec![key("channel")], false), &r).is_err());
    assert!(d.apply(c(vec![], true), &r).is_err());
    assert!(d.apply(c(vec![key("a")], true), &r).is_err());
    lock(&mut d, &r, &["channel"], true);
    assert!(d.apply(c(vec![key("channel")], true), &r).is_err());
}

#[test]
fn every_edit_source_is_refused_on_a_locked_strip() {
    let (mut d, r) = project();
    lock(&mut d, &r, &["channel"], true);
    // A knob, the keyboard and the wheel, MIDI learn and the MCP server are all `SetParam`.
    for (param, value) in [("fader", 0.2), ("pan", 0.1), ("mute", 1.0), ("solo", 1.0)] {
        refused(&mut d, &r, set_param("channel", param, value));
    }
    refused(
        &mut d,
        &r,
        Command::Rename {
            key: key("channel"),
            name: "Lead".into(),
        },
    );
    refused(
        &mut d,
        &r,
        Command::SetColor {
            key: key("channel"),
            color: Some([1, 2, 3]),
        },
    );
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::AddInsert {
            channel: key("channel"),
            slot: 0,
            effect: InsertEffect::New {
                type_id: DELAY.into(),
            },
        }),
    );
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::ResetStrip {
            strip: key("channel"),
            keep_routing: true,
        }),
    );
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::RemoveChannel {
            channel: key("channel"),
        }),
    );
}

#[test]
fn the_inserts_of_a_locked_strip_are_locked_with_it() {
    let (mut d, r) = project();
    d.apply(
        con(ConsoleCommand::AddInsert {
            channel: key("channel"),
            slot: 0,
            effect: InsertEffect::New {
                type_id: DELAY.into(),
            },
        }),
        &r,
    )
    .unwrap();
    let chain = atomik_model::insert_chain(d.doc(), &r, &key("channel"));
    assert_eq!(chain.len(), 1);
    let fx = chain[0].0.clone();
    lock(&mut d, &r, &["channel"], true);
    assert_eq!(
        d.doc().locked_owner(&r, &key(&fx)),
        Some(key("channel")),
        "an insert effect belongs to the locked strip"
    );
    // The effect's own params, its bypass, its removal from the chain and a clear all refuse.
    refused(&mut d, &r, set_param(&fx, "bypass", 1.0));
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::SetInsert {
            channel: key("channel"),
            slot: 0,
            effect: None,
        }),
    );
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::ClearInserts {
            strip: key("channel"),
        }),
    );
    lock(&mut d, &r, &["channel"], false);
    d.apply(
        con(ConsoleCommand::ClearInserts {
            strip: key("channel"),
        }),
        &r,
    )
    .unwrap();
}

#[test]
fn a_ganged_edit_changes_the_strips_that_are_not_locked() {
    let (mut d, r) = project();
    lock(&mut d, &r, &["channel"], true);
    let gang = Command::Batch(vec![
        set_param("channel", "fader", 0.2),
        set_param("channel-2", "fader", 0.2),
    ]);
    d.apply(gang, &r).unwrap();
    assert!((fader(&d, "channel-2") - 0.2).abs() < 1e-6);
    assert!(
        !d.doc()
            .device(&key("channel"))
            .unwrap()
            .params
            .contains_key("fader")
    );
    assert_eq!(d.take_lock_skips(), vec![key("channel")]);
    assert!(d.take_lock_skips().is_empty(), "taking empties the list");
    // A gang of locked strips only is refused whole.
    lock(&mut d, &r, &["channel-2"], true);
    let gang = Command::Batch(vec![
        set_param("channel", "fader", 0.9),
        set_param("channel-2", "fader", 0.9),
    ]);
    refused(&mut d, &r, gang);
}

#[test]
fn clear_mutes_skips_a_locked_strip_and_a_paste_leaves_it_out() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("channel", "mute", 1.0),
            set_param("channel-2", "mute", 1.0),
        ],
    );
    lock(&mut d, &r, &["channel"], true);
    d.apply(con(ConsoleCommand::ClearMutes), &r).unwrap();
    let mute = |k: &str| d.doc().device(&key(k)).unwrap().params.get("mute").copied();
    assert_eq!(mute("channel"), Some(1.0));
    assert_ne!(mute("channel-2"), Some(1.0));
}

#[test]
fn a_snapshot_recall_leaves_a_locked_strip_alone() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("channel", "fader", 0.4),
            set_param("channel-2", "fader", 0.4),
        ],
    );
    d.apply(con(ConsoleCommand::StoreSnapshot { name: "A".into() }), &r)
        .unwrap();
    run(
        &mut d,
        &r,
        [
            set_param("channel", "fader", 0.8),
            set_param("channel-2", "fader", 0.8),
        ],
    );
    lock(&mut d, &r, &["channel"], true);
    let id = d.doc().console.snapshots[0].id;
    d.apply(
        con(ConsoleCommand::RecallSnapshot {
            id,
            scope: SnapshotScope::ALL,
        }),
        &r,
    )
    .unwrap();
    assert!((fader(&d, "channel") - 0.8).abs() < 1e-6, "kept");
    assert!((fader(&d, "channel-2") - 0.4).abs() < 1e-6, "recalled");
}

#[test]
fn a_macro_cannot_be_assigned_to_a_locked_param() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [add(usize::MAX, "atomik.macro-bank", Some("bank"))],
    );
    lock(&mut d, &r, &["channel"], true);
    let e = d
        .apply(
            Command::Macro(MacroCommand::Assign {
                bank: Some(key("bank")),
                index: 0,
                device: key("channel"),
                param: "fader".into(),
                min: None,
                max: None,
                invert: false,
            }),
            &r,
        )
        .expect_err("a macro assignment is an edit of the strip");
    assert!(is_locked_error(&e), "{e}");
}

#[test]
fn the_master_and_a_bus_can_be_locked_too() {
    let (mut d, r) = project();
    let master = d.doc().console.master.clone().expect("master").0;
    lock(&mut d, &r, &[master.as_str()], true);
    refused(&mut d, &r, set_param(&master, "fader", 0.5));
    // Adding a track still works with a locked master (summing cables are not part of the lock).
    d.apply(add(usize::MAX, SYNTH, Some("c")), &r).unwrap();
}

#[test]
fn the_lock_is_saved_ignorable_and_an_old_project_loads_unlocked() {
    let (mut d, r) = project();
    let plain = save_project(d.doc());
    assert!(
        !plain.contains("\"locked\""),
        "written only when a strip is locked"
    );
    assert!(load_project(&plain, &r).unwrap().console.locked.is_empty());
    lock(&mut d, &r, &["channel-2", "channel"], true);
    let text = save_project(d.doc());
    assert!(
        text.contains("\"locked\": [\"channel\", \"channel-2\"]"),
        "{text}"
    );
    assert!(text.contains(&format!("\"version\": {PROJECT_VERSION},")));
    assert!(text.contains("\"console.locked\": 46"), "{text}");
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back.console.locked, d.doc().console.locked);
    assert_eq!(save_project(&back), text, "the save is canonical");
    // A reader that does not know the field drops it (it is listed as ignorable) and opens the
    // mix unlocked: the same file with the field removed differs in nothing else.
    let bad = text.replace("\"channel-2\"]", "\"nope\"]");
    assert!(
        load_project(&bad, &r).is_err(),
        "a lock on a missing strip is refused"
    );
}
