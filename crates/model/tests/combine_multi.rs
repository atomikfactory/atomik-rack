//! `Combine` on several selected devices (model side): the selections a rack produces, which
//! combine, which are refused and what each refusal says, and the devices a container cannot hold,
//! which stay in the rack.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{Command, ContainerCommand, Document, ModelError, RackDoc, empty_doc};
use common::audio::{add_audio_track, audio_registry};
use common::container_ops::assert_invariants;
use common::song::{add_clip, add_track, note, notes_clip};
use common::*;

const CONTAINER_KEY: &str = "container";

fn combine(keys: &[&str]) -> Command {
    Command::Container(ContainerCommand::Combine {
        keys: keys.iter().map(|k| key(k)).collect(),
        name: String::new(),
    })
}

fn uncombine() -> Command {
    Command::Container(ContainerCommand::Uncombine {
        container: key(CONTAINER_KEY),
    })
}

/// A track on `k` with one note clip.
fn with_notes(k: &str) -> Vec<Command> {
    vec![
        add_track(k),
        add_clip(
            k,
            0,
            notes_clip(
                0,
                1920,
                vec![note(0, 480, 60, 0.8), note(480, 480, 64, 0.7)],
            ),
        ),
    ]
}

/// Two instruments, each with its own mixer channel and song track.
fn two_instruments(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [add(0, SYNTH, Some("one")), add(1, SYNTH, Some("two"))],
    );
    run(&mut d, r, with_notes("one"));
    run(&mut d, r, with_notes("two"));
    d
}

/// Applies `c`, which must be refused; the document is the same afterwards.
fn refused(d: &mut Document, r: &Registry, c: Command) -> ModelError {
    let before = d.doc().clone();
    let e = d.apply(c, r).expect_err("the command must be refused");
    assert_eq!(d.doc(), &before, "a refused command changes nothing");
    e
}

/// The notes a combine of `keys` would report.
fn notes_of(d: &Document, r: &Registry, keys: &[&str]) -> Vec<String> {
    let Command::Container(c) = combine(keys) else {
        unreachable!()
    };
    d.doc().container_move_notes(r, &c).expect("a plan")
}

fn inner_keys(d: &Document) -> Vec<String> {
    d.doc()
        .inner(&key(CONTAINER_KEY))
        .expect("a container")
        .devices
        .iter()
        .map(|x| x.key.0.clone())
        .collect()
}

/// One undo step gives back `before`, redo gives back the combined document, and both are valid.
fn check_undo_redo(d: &mut Document, r: &Registry, before: &RackDoc) {
    let after = d.doc().clone();
    d.doc().validate().expect("the combined document is valid");
    assert_invariants(d.doc(), r);
    d.undo(r).expect("one undo step");
    assert_eq!(d.doc(), before, "undo restores the document exactly");
    d.redo(r).expect("redo");
    assert_eq!(d.doc(), &after, "redo restores the combined document");
}

// ------------------------------------------------------------------------- several owners

#[test]
fn two_instruments_that_each_have_a_track_and_a_channel_are_refused_naming_both() {
    let r = console_registry();
    let mut d = two_instruments(&r);
    let e = refused(&mut d, &r, combine(&["one", "two"]));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    let text = e.to_string();
    assert!(text.contains("Combine keeps one track"), "{text}");
    assert!(
        text.contains("\"Poly Synth\" and \"Poly Synth 2\""),
        "{text}"
    );
    assert!(text.contains("combine them separately"), "{text}");
}

#[test]
fn an_instrument_an_effect_and_another_instrument_are_refused_naming_the_instruments() {
    let r = console_registry();
    let mut d = two_instruments(&r);
    run(&mut d, &r, [add(2, DELAY, Some("fx"))]);
    let e = refused(&mut d, &r, combine(&["one", "fx", "two"]));
    let text = e.to_string();
    assert!(
        text.contains("\"Poly Synth\" and \"Poly Synth 2\""),
        "{text}"
    );
    assert!(!text.contains("Stereo Delay"), "{text}");
}

#[test]
fn each_instrument_with_its_own_effects_combines_separately() {
    let r = console_registry();
    let mut d = two_instruments(&r);
    run(&mut d, &r, [add(2, DELAY, Some("fx"))]);
    let before = d.doc().clone();
    d.apply(combine(&["one", "fx"]), &r).unwrap();
    assert_eq!(inner_keys(&d), ["one", "fx"]);
    check_undo_redo(&mut d, &r, &before);
}

#[test]
fn a_device_driven_by_a_macro_refuses_the_combine_by_name() {
    let r = console_registry();
    let mut d = two_instruments(&r);
    d.apply(
        Command::Macro(atomik_model::MacroCommand::Assign {
            bank: None,
            index: 0,
            device: key("two"),
            param: "filter.cutoff".into(),
            min: None,
            max: None,
            invert: false,
        }),
        &r,
    )
    .unwrap();
    let e = refused(&mut d, &r, combine(&["one", "two"]));
    assert!(matches!(e, ModelError::BadMacro(_)), "{e:?}");
    assert!(e.to_string().contains("\"Poly Synth 2\""), "{e}");
}

// ------------------------------------------------------------------------- what stays behind

#[test]
fn an_audio_track_and_its_effects_combine_the_effects_and_leave_the_track() {
    let r = audio_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add_audio_track(), add(1, DELAY, Some("fx"))]);
    let track_key = d.doc().song.tracks[0].device.0.clone();
    let before = d.doc().clone();
    let notes = notes_of(&d, &r, &[track_key.as_str(), "fx"]);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("stays in the rack: a container cannot hold it"),
        "{notes:?}"
    );
    d.apply(combine(&[track_key.as_str(), "fx"]), &r).unwrap();
    assert_eq!(inner_keys(&d), ["fx"]);
    assert!(d.doc().device(&key(&track_key)).is_some());
    assert_eq!(d.doc().song.tracks, before.song.tracks);
    check_undo_redo(&mut d, &r, &before);
}

#[test]
fn a_selection_with_the_mixer_strips_leaves_them_in_the_rack() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("one")), add(1, DELAY, Some("fx"))],
    );
    run(&mut d, &r, with_notes("one"));
    let all: Vec<String> = keys(&d);
    let names: Vec<&str> = all.iter().map(String::as_str).collect();
    let before = d.doc().clone();
    let notes = notes_of(&d, &r, &names);
    // The channel and the master stay; the instrument and the effect combine.
    let stays = notes
        .iter()
        .filter(|n| n.contains("stays in the rack"))
        .count();
    assert_eq!(stays, 2, "{notes:?}");
    d.apply(combine(&names), &r).unwrap();
    assert_eq!(inner_keys(&d), ["one", "fx"]);
    assert!(d.doc().console.master.is_some());
    check_undo_redo(&mut d, &r, &before);
}

#[test]
fn a_whole_rack_with_two_instruments_is_refused_naming_them() {
    let r = console_registry();
    let mut d = two_instruments(&r);
    let all: Vec<String> = keys(&d);
    let names: Vec<&str> = all.iter().map(String::as_str).collect();
    let e = refused(&mut d, &r, combine(&names));
    assert!(
        e.to_string()
            .contains("\"Poly Synth\" and \"Poly Synth 2\""),
        "{e}"
    );
}

#[test]
fn a_selection_of_devices_a_container_cannot_hold_is_refused_naming_one() {
    let r = console_registry();
    let mut d = two_instruments(&r);
    let channel = d.doc().console.channels[0].channel.0.clone();
    let master = d.doc().console.master.clone().unwrap().0;
    let e = refused(&mut d, &r, combine(&[channel.as_str(), master.as_str()]));
    assert!(matches!(e, ModelError::NotAllowedInContainer(_)), "{e:?}");
    assert!(
        e.to_string()
            .contains("cannot be placed inside a container"),
        "{e}"
    );
}

#[test]
fn a_drum_machine_with_its_generator_combines_and_uncombines_exactly() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, "atomik.drum-machine", Some("m")),
            add(1, "atomik.drum-generator", Some("g")),
            connect("g:notes_out", "m:notes_in"),
        ],
    );
    run(&mut d, &r, with_notes("m"));
    let before = d.doc().clone();
    d.apply(combine(&["m", "g"]), &r).unwrap();
    assert_eq!(inner_keys(&d), ["m", "g"]);
    d.apply(uncombine(), &r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_combine_that_left_nothing_out_reports_no_notes() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("one")), add(1, DELAY, Some("fx"))],
    );
    run(&mut d, &r, with_notes("one"));
    assert!(notes_of(&d, &r, &["one", "fx"]).is_empty());
}
