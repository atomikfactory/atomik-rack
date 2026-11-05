//! Mixer snapshots, the follow-ups: the effect devices' params, a note and a colour per
//! snapshot, a morph between two snapshots, and the larger list.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::console::{MAX_SNAPSHOT_NOTE_CHARS, MAX_SNAPSHOTS};
use atomik_model::{
    Command, ConsoleCommand, Document, InsertEffect, SnapshotScope, load_project, save_project,
};
use common::console_ops::assert_console_invariants;
use common::*;
use serde_json::Value;

/// The starter with four synths and an effect `comp` on the insert of `channel`.
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

fn morph(from: u32, to: u32, at: f32) -> Command {
    con(ConsoleCommand::MorphSnapshots { from, to, at })
}

fn value(d: &Document, r: &Registry, dev: &str, param: &str) -> f32 {
    d.doc().param_value(&key(dev), param, r).unwrap()
}

fn refused(d: &mut Document, r: &Registry, c: Command) -> String {
    let before = d.doc().clone();
    let e = d.apply(c.clone(), r).expect_err(&format!("{c:?} accepted"));
    assert_eq!(d.doc(), &before, "{c:?} changed the document");
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
    d.undo().expect("undo");
    assert_eq!(d.doc(), &before, "{c:?} undone in one step");
    d.redo().expect("redo");
    assert_eq!(d.doc(), &after, "{c:?} redone");
}

// ---- Effect device params ------------------------------------------------------------------

#[test]
fn a_snapshot_holds_the_effect_params_once_asked_to() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("comp", "threshold", -30.0),
            set_param("comp", "bypass", 1.0),
            store("A"),
        ],
    );
    let plain = d.doc().console.snapshot(1).unwrap().state.clone();
    assert!(!plain.effects);
    assert_eq!(plain.devices[&key("comp")].len(), 1, "the bypass only");

    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotEffects {
            id: 1,
            effects: true,
        }),
    );
    d.apply(
        con(ConsoleCommand::SetSnapshotEffects {
            id: 1,
            effects: true,
        }),
        &r,
    )
    .expect_err("already holds them");
    let full = &d.doc().console.snapshot(1).unwrap().state;
    assert!(full.effects);
    assert_eq!(full.devices[&key("comp")]["threshold"], -30.0);
    assert_eq!(full.devices[&key("comp")]["bypass"], 1.0);

    run(
        &mut d,
        &r,
        [
            set_param("comp", "threshold", -6.0),
            set_param("comp", "bypass", 0.0),
        ],
    );
    assert!(!d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    one_step(&mut d, &r, recall(1));
    assert_eq!(value(&d, &r, "comp", "threshold"), -30.0);
    assert_eq!(value(&d, &r, "comp", "bypass"), 1.0);
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
}

#[test]
fn the_effect_params_cannot_change_under_a_compare() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A"), set_param("channel", "fader", 0.3)]);
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &r).unwrap();
    let e = refused(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotEffects {
            id: 1,
            effects: true,
        }),
    );
    assert!(e.contains("compare"), "{e}");
}

#[test]
fn a_snapshot_without_the_effect_params_leaves_them_alone() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [set_param("comp", "threshold", -30.0), store("A")],
    );
    run(&mut d, &r, [set_param("comp", "threshold", -6.0)]);
    run(&mut d, &r, [set_param("channel", "fader", 0.3)]);
    d.apply(recall(1), &r).unwrap();
    assert_eq!(value(&d, &r, "comp", "threshold"), -6.0);
}

#[test]
fn turning_the_effect_params_off_keeps_the_bypass() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("comp", "threshold", -30.0),
            set_param("comp", "bypass", 1.0),
            store("A"),
            con(ConsoleCommand::SetSnapshotEffects {
                id: 1,
                effects: true,
            }),
        ],
    );
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotEffects {
            id: 1,
            effects: false,
        }),
    );
    let state = &d.doc().console.snapshot(1).unwrap().state;
    assert!(!state.effects);
    assert_eq!(state.devices[&key("comp")].len(), 1);
    assert_eq!(state.devices[&key("comp")]["bypass"], 1.0);
}

#[test]
fn update_and_compare_keep_the_effect_params_of_their_snapshot() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("comp", "threshold", -30.0),
            store("A"),
            con(ConsoleCommand::SetSnapshotEffects {
                id: 1,
                effects: true,
            }),
            set_param("comp", "threshold", -12.0),
        ],
    );
    one_step(&mut d, &r, con(ConsoleCommand::UpdateSnapshot { id: 1 }));
    let held = &d.doc().console.snapshot(1).unwrap().state;
    assert!(held.effects);
    assert_eq!(held.devices[&key("comp")]["threshold"], -12.0);

    // The working side of a compare keeps its effect params too.
    run(&mut d, &r, [set_param("comp", "threshold", -3.0)]);
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &r).unwrap();
    assert_eq!(value(&d, &r, "comp", "threshold"), -12.0);
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &r).unwrap();
    assert_eq!(value(&d, &r, "comp", "threshold"), -3.0);
}

// ---- Note and colour -----------------------------------------------------------------------

#[test]
fn a_snapshot_has_a_note_and_a_colour() {
    let (mut d, r) = project();
    d.apply(store("A"), &r).unwrap();
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotNote {
            id: 1,
            note: "  verse, vocal up ".into(),
        }),
    );
    assert_eq!(d.doc().console.snapshot(1).unwrap().note, "verse, vocal up");
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotColor {
            id: 1,
            color: Some([200, 60, 40]),
        }),
    );
    assert_eq!(
        d.doc().console.snapshot(1).unwrap().color,
        Some([200, 60, 40])
    );
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotColor {
            id: 1,
            color: Some([200, 60, 40]),
        }),
    );
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotNote {
            id: 1,
            note: "x".repeat(MAX_SNAPSHOT_NOTE_CHARS + 1),
        }),
    );
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::SetSnapshotNote {
            id: 9,
            note: "x".into(),
        }),
    );
    // Clearing both.
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::SetSnapshotNote {
                id: 1,
                note: " ".into(),
            }),
            con(ConsoleCommand::SetSnapshotColor { id: 1, color: None }),
        ],
    );
    let s = d.doc().console.snapshot(1).unwrap();
    assert!(s.note.is_empty() && s.color.is_none());
}

#[test]
fn the_effect_flag_the_note_and_the_colour_round_trip_and_are_written_only_when_set() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("comp", "threshold", -30.0),
            store("A"),
            store("B"),
        ],
    );
    let plain = save_project(d.doc());
    let v: Value = serde_json::from_str(&plain).unwrap();
    for field in ["effects", "note", "color"] {
        assert!(v["console"]["snapshots"][0].get(field).is_none(), "{field}");
    }
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::SetSnapshotEffects {
                id: 1,
                effects: true,
            }),
            con(ConsoleCommand::SetSnapshotNote {
                id: 1,
                note: "quote \"x\"".into(),
            }),
            con(ConsoleCommand::SetSnapshotColor {
                id: 1,
                color: Some([1, 128, 255]),
            }),
        ],
    );
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let s = &v["console"]["snapshots"][0];
    assert_eq!(s["effects"], true);
    assert_eq!(s["note"], "quote \"x\"");
    assert_eq!(s["color"], "#0180ff");
    assert_eq!(s["devices"]["comp"]["threshold"], -30.0);
    assert!(v["console"]["snapshots"][1].get("note").is_none());
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.console, d.doc().console);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_note_over_the_limit_in_a_file_names_its_path() {
    let (mut d, r) = project();
    d.apply(store("A"), &r).unwrap();
    let text = save_project(d.doc()).replacen(
        "\"devices\"",
        &format!(
            "\"note\": \"{}\", \"devices\"",
            "x".repeat(MAX_SNAPSHOT_NOTE_CHARS + 1)
        ),
        1,
    );
    let e = load_project(&text, &r).unwrap_err().to_string();
    assert!(e.contains("note"), "{e}");
}

// ---- More snapshots ------------------------------------------------------------------------

#[test]
fn a_console_holds_sixty_four_snapshots() {
    assert_eq!(MAX_SNAPSHOTS, 64);
    let (mut d, r) = project();
    for i in 0..MAX_SNAPSHOTS {
        d.apply(store(&format!("S{i}")), &r).unwrap();
    }
    assert!(refused(&mut d, &r, store("one more")).contains("at most"));
    let text = save_project(d.doc());
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.console.snapshots.len(), MAX_SNAPSHOTS);
}

// ---- Morph ---------------------------------------------------------------------------------

/// Snapshots A (fader 0.2, pan -1, mute off, comp threshold -40) and B (fader 0.8, pan 1, mute
/// on, comp threshold -10, comp holds its params), the working mix left on B.
fn two_snapshots() -> (Document, Registry) {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("channel", "fader", 0.2),
            set_param("channel", "pan", -1.0),
            set_param("comp", "threshold", -40.0),
            store("A"),
            con(ConsoleCommand::SetSnapshotEffects {
                id: 1,
                effects: true,
            }),
            set_param("channel", "fader", 0.8),
            set_param("channel", "pan", 1.0),
            set_param("channel", "mute", 1.0),
            set_param("comp", "threshold", -10.0),
            store("B"),
            con(ConsoleCommand::SetSnapshotEffects {
                id: 2,
                effects: true,
            }),
        ],
    );
    (d, r)
}

#[test]
fn a_morph_moves_the_params_between_the_two_snapshots() {
    let (mut d, r) = two_snapshots();
    d.apply(morph(1, 2, 0.0), &r).unwrap();
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    assert_eq!(value(&d, &r, "channel", "fader"), 0.2);

    d.apply(morph(1, 2, 0.25), &r).unwrap();
    let f = value(&d, &r, "channel", "fader");
    assert!((f - 0.35).abs() < 1e-5, "{f}");
    assert!((value(&d, &r, "channel", "pan") + 0.5).abs() < 1e-5);
    let t = value(&d, &r, "comp", "threshold");
    assert!((t + 32.5).abs() < 1e-3, "{t}");
    assert_eq!(value(&d, &r, "channel", "mute"), 0.0, "a switch holds");

    d.apply(morph(1, 2, 0.5), &r).unwrap();
    assert_eq!(value(&d, &r, "channel", "mute"), 1.0, "and flips half way");
    let f = value(&d, &r, "channel", "fader");
    assert!((f - 0.5).abs() < 1e-5, "{f}");

    d.apply(morph(1, 2, 1.0), &r).unwrap();
    assert!(d.doc().snapshot_matches(2, &r, SnapshotScope::ALL));
    assert_console_invariants(d.doc());
}

#[test]
fn a_morph_runs_in_both_directions_and_is_a_recall_at_its_ends() {
    let (mut d, r) = two_snapshots();
    d.apply(morph(2, 1, 1.0), &r).unwrap();
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    d.apply(morph(2, 1, 0.75), &r).unwrap();
    let f = value(&d, &r, "channel", "fader");
    assert!((f - 0.35).abs() < 1e-5, "{f}");
    assert!(!d.doc().morph_matches(&r, 2, 1, 0.0));
    assert!(d.doc().morph_matches(&r, 2, 1, 0.75));
}

#[test]
fn a_morph_step_is_one_undo_step_and_successive_steps_fold_into_one() {
    let (mut d, r) = two_snapshots();
    d.apply(recall(1), &r).unwrap();
    one_step(&mut d, &r, morph(1, 2, 0.5));
    let before = d.doc().clone();
    d.undo().unwrap();
    d.redo().unwrap();
    assert_eq!(d.doc(), &before);

    d.apply(recall(1), &r).unwrap();
    let start = d.doc().clone();
    d.apply(morph(1, 2, 0.1), &r).unwrap();
    d.apply_joined(morph(1, 2, 0.4), &r).unwrap();
    d.apply_joined(morph(1, 2, 1.0), &r).unwrap();
    assert!(d.doc().snapshot_matches(2, &r, SnapshotScope::ALL));
    d.undo().unwrap();
    assert_eq!(d.doc(), &start, "the whole morph is one step");
}

#[test]
fn a_morph_over_a_group_moves_its_level_and_flips_its_mute_half_way() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::CreateVca {
                name: "Drums".into(),
                members: vec![key("channel"), key("channel-2")],
            }),
            con(ConsoleCommand::SetVcaLevel {
                vca: 1,
                db: -12.0,
                merge: None,
            }),
            store("A"),
            con(ConsoleCommand::SetVcaLevel {
                vca: 1,
                db: 0.0,
                merge: None,
            }),
            con(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
            store("B"),
        ],
    );
    d.apply(morph(1, 2, 0.25), &r).unwrap();
    let g = d.doc().console.vca(1).unwrap();
    assert!((g.level_db + 9.0).abs() < 1e-4, "{}", g.level_db);
    assert!(!g.mute);
    d.apply(morph(1, 2, 0.5), &r).unwrap();
    assert!(d.doc().console.vca(1).unwrap().mute);
}

#[test]
fn a_morph_is_refused_when_it_cannot_mean_anything() {
    let (mut d, r) = two_snapshots();
    d.apply(recall(1), &r).unwrap();
    assert!(refused(&mut d, &r, morph(1, 1, 0.5)).contains("two different"));
    assert!(refused(&mut d, &r, morph(1, 9, 0.5)).contains("existing"));
    assert!(refused(&mut d, &r, morph(1, 2, 1.5)).contains("between 0 and 1"));
    assert!(refused(&mut d, &r, morph(1, 2, f32::NAN)).contains("between 0 and 1"));
    assert!(refused(&mut d, &r, morph(1, 2, 0.0)).contains("already there"));
    d.apply(con(ConsoleCommand::Compare { id: 2 }), &r).unwrap();
    assert!(refused(&mut d, &r, morph(1, 2, 0.5)).contains("compare"));
}

#[test]
fn a_morph_skips_what_no_longer_exists() {
    let (mut d, r) = two_snapshots();
    run(
        &mut d,
        &r,
        [
            recall(1),
            con(ConsoleCommand::RemoveChannel {
                channel: key("channel-4"),
            }),
        ],
    );
    d.apply(morph(1, 2, 0.5), &r).unwrap();
    assert_console_invariants(d.doc());
}
