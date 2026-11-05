//! Mixer snapshots, the second round of follow-ups: a morph that starts from the mix as it is,
//! the marker a snapshot recalls at, and the A/B comparison of one strip.

mod common;

use atomik_device_sdk::Registry;
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

fn value(d: &Document, r: &Registry, dev: &str, param: &str) -> f32 {
    d.doc().param_value(&key(dev), param, r).unwrap()
}

fn refused(d: &mut Document, r: &Registry, c: Command) -> String {
    let before = d.doc().clone();
    let e = d.apply(c.clone(), r).expect_err(&format!("{c:?} accepted"));
    assert_eq!(d.doc(), &before, "{c:?} changed the document");
    e.to_string()
}

/// Two snapshots: A at fader 0.2, B at fader 0.8 with the channel muted; the mix is left on B.
fn two_snapshots() -> (Document, Registry) {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("channel", "fader", 0.2),
            set_param("channel", "pan", -1.0),
            store("A"),
            set_param("channel", "fader", 0.8),
            set_param("channel", "pan", 1.0),
            set_param("channel", "mute", 1.0),
            store("B"),
        ],
    );
    (d, r)
}

fn morph_from_mix(d: &Document, r: &Registry, to: u32, at: f32) -> Command {
    con(ConsoleCommand::MorphFromMix {
        start: Box::new(d.doc().capture_mixer_with(r, false)),
        to,
        at,
    })
}

// ---- Morph from the mix --------------------------------------------------------------------

#[test]
fn a_morph_can_start_from_a_mix_that_is_no_snapshot() {
    let (mut d, r) = two_snapshots();
    // A mix that is neither snapshot.
    run(&mut d, &r, [set_param("channel", "fader", 0.5)]);
    let start = d.doc().capture_mixer_with(&r, false);
    for (at, fader) in [(0.0f32, 0.5f32), (0.5, 0.65), (1.0, 0.8)] {
        let c = con(ConsoleCommand::MorphFromMix {
            start: Box::new(start.clone()),
            to: 2,
            at,
        });
        if at == 0.0 {
            assert!(refused(&mut d, &r, c).contains("already there"));
            continue;
        }
        d.apply(c, &r).unwrap();
        let f = value(&d, &r, "channel", "fader");
        assert!((f - fader).abs() < 1e-5, "at {at}: {f}");
        assert_console_invariants(d.doc());
    }
    assert!(d.doc().snapshot_matches(2, &r, SnapshotScope::ALL));
}

#[test]
fn a_morph_from_the_mix_folds_into_one_undo_step_and_matches_a_recall_at_its_end() {
    let (mut d, r) = two_snapshots();
    run(&mut d, &r, [set_param("channel", "fader", 0.5)]);
    let before = d.doc().clone();
    let start = d.doc().capture_mixer_with(&r, false);
    let step = |at: f32| {
        con(ConsoleCommand::MorphFromMix {
            start: Box::new(start.clone()),
            to: 1,
            at,
        })
    };
    d.apply(step(0.25), &r).unwrap();
    d.apply_joined(step(0.5), &r).unwrap();
    assert!(
        d.doc().morph_from_matches(&r, &start, 1, 0.5),
        "the step that was applied is a no-op to repeat"
    );
    d.apply_joined(step(1.0), &r).unwrap();
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_morph_from_the_mix_is_refused_for_bad_input() {
    let (mut d, r) = two_snapshots();
    run(&mut d, &r, [set_param("channel", "fader", 0.5)]);
    let c = morph_from_mix(&d, &r, 9, 0.5);
    assert!(refused(&mut d, &r, c).contains("no snapshot 9"));
    let c = morph_from_mix(&d, &r, 1, 1.5);
    assert!(refused(&mut d, &r, c).contains("between 0 and 1"));
    let c = morph_from_mix(&d, &r, 1, f32::NAN);
    refused(&mut d, &r, c);
    run(&mut d, &r, [con(ConsoleCommand::Compare { id: 1 })]);
    let c = morph_from_mix(&d, &r, 2, 0.5);
    assert!(refused(&mut d, &r, c).contains("compare"));
}

// ---- Marker --------------------------------------------------------------------------------

#[test]
fn a_snapshot_recalls_at_a_marker_that_is_saved_only_when_set() {
    let (mut d, r) = two_snapshots();
    let v: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    assert!(v["console"]["snapshots"][0].get("marker").is_none());

    d.apply(
        con(ConsoleCommand::SetSnapshotMarker {
            id: 2,
            marker: Some("  Chorus ".into()),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(
        d.doc().console.snapshot(2).unwrap().marker.as_deref(),
        Some("Chorus")
    );
    assert_eq!(d.doc().console.snapshot_for_marker("chorus"), Some(2));
    assert_eq!(d.doc().console.snapshot_for_marker("Verse"), None);

    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["console"]["snapshots"][1]["marker"], "Chorus");
    assert!(v["console"]["snapshots"][0].get("marker").is_none());
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.console, d.doc().console);
    assert_eq!(save_project(&loaded), text);

    let same = con(ConsoleCommand::SetSnapshotMarker {
        id: 2,
        marker: Some(" Chorus".into()),
    });
    d.apply(same, &r).expect_err("the same marker is a no-op");
    let empty = con(ConsoleCommand::SetSnapshotMarker {
        id: 1,
        marker: Some("  ".into()),
    });
    refused(&mut d, &r, empty);
    d.apply(
        con(ConsoleCommand::SetSnapshotMarker {
            id: 2,
            marker: None,
        }),
        &r,
    )
    .unwrap();
    assert!(d.doc().console.snapshot(2).unwrap().marker.is_none());
    d.undo().unwrap();
    assert!(d.doc().console.snapshot(2).unwrap().marker.is_some());
}

#[test]
fn a_marker_name_in_a_file_must_not_be_empty() {
    let (mut d, r) = two_snapshots();
    run(
        &mut d,
        &r,
        [con(ConsoleCommand::SetSnapshotMarker {
            id: 1,
            marker: Some("X".into()),
        })],
    );
    let text = save_project(d.doc()).replace("\"marker\": \"X\"", "\"marker\": \"\"");
    let e = load_project(&text, &r).expect_err("an empty marker name");
    assert!(e.to_string().contains("marker"), "{e}");
}

// ---- A/B of one strip ----------------------------------------------------------------------

/// The key of the second channel strip.
fn second_channel(d: &Document) -> String {
    d.doc().console.channels[1].channel.0.clone()
}

#[test]
fn a_strip_compare_flips_that_strip_and_leaves_the_rest_of_the_mix() {
    let (mut d, r) = project();
    let other = second_channel(&d);
    run(
        &mut d,
        &r,
        [
            set_param("channel", "fader", 0.2),
            set_param(&other, "fader", 0.3),
            store("A"),
            set_param("channel", "fader", 0.9),
            set_param(&other, "fader", 0.6),
        ],
    );
    let strip = || key("channel");
    d.apply(
        con(ConsoleCommand::CompareStrip {
            id: 1,
            strip: strip(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(
        value(&d, &r, "channel", "fader"),
        0.2,
        "the strip is the snapshot's"
    );
    assert_eq!(value(&d, &r, &other, "fader"), 0.6, "the rest stays");
    let c = d.doc().console.compare.clone().expect("comparing");
    assert_eq!(c.strip, Some(strip()));
    assert!(c.on_snapshot);
    assert!(!c.other.devices.contains_key(&key(&other)));

    // The flip: the strip goes back, the other strip is still untouched; an edit made on the
    // other strip while comparing survives both flips.
    run(&mut d, &r, [set_param(&other, "fader", 0.7)]);
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &r).unwrap();
    assert_eq!(value(&d, &r, "channel", "fader"), 0.9);
    assert_eq!(value(&d, &r, &other, "fader"), 0.7);
    d.apply(
        con(ConsoleCommand::CompareStrip {
            id: 1,
            strip: strip(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(value(&d, &r, "channel", "fader"), 0.2);
    assert_eq!(value(&d, &r, &other, "fader"), 0.7);
    assert_console_invariants(d.doc());

    d.apply(con(ConsoleCommand::EndCompare), &r).unwrap();
    assert!(d.doc().console.compare.is_none());
    assert_eq!(
        value(&d, &r, "channel", "fader"),
        0.2,
        "the live side stays"
    );
}

#[test]
fn a_strip_compare_covers_the_insert_chain_of_the_strip() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("comp", "bypass", 0.0),
            store("A"),
            set_param("comp", "bypass", 1.0),
        ],
    );
    d.apply(
        con(ConsoleCommand::CompareStrip {
            id: 1,
            strip: key("channel"),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(value(&d, &r, "comp", "bypass"), 0.0);
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &r).unwrap();
    assert_eq!(value(&d, &r, "comp", "bypass"), 1.0);
}

#[test]
fn a_strip_compare_is_one_undo_step_per_move_and_is_refused_when_it_means_nothing() {
    let (mut d, r) = project();
    let other = second_channel(&d);
    run(
        &mut d,
        &r,
        [
            set_param("channel", "fader", 0.2),
            store("A"),
            set_param("channel", "fader", 0.9),
        ],
    );
    let c = con(ConsoleCommand::CompareStrip {
        id: 7,
        strip: key("channel"),
    });
    assert!(refused(&mut d, &r, c).contains("no snapshot 7"));
    let c = con(ConsoleCommand::CompareStrip {
        id: 1,
        strip: key("comp"),
    });
    assert!(refused(&mut d, &r, c).contains("channel"));

    let before = d.doc().clone();
    d.apply(
        con(ConsoleCommand::CompareStrip {
            id: 1,
            strip: key("channel"),
        }),
        &r,
    )
    .unwrap();
    let compared = d.doc().clone();
    // Another strip, and a whole-mix compare with another snapshot, are refused meanwhile.
    let c = con(ConsoleCommand::CompareStrip {
        id: 1,
        strip: key(&other),
    });
    assert!(refused(&mut d, &r, c).contains("end the compare"));
    run(&mut d, &r, [store("B")]);
    let c = con(ConsoleCommand::Compare { id: 2 });
    assert!(refused(&mut d, &r, c).contains("strip compare"));
    d.undo().unwrap();
    assert_eq!(
        d.doc(),
        &compared,
        "a snapshot stored meanwhile is one step"
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "beginning the compare is one step");
}
