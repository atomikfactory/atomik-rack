//! Cut, copy, paste and duplicate of console strips, and the Ctrl-drag copy.

use super::*;
use crate::clipboard::ClipboardKind;
use crate::sink::RecordingSink;
use atomik_model::{
    ConsoleCommand, CreateOptions, CreateSource, DeviceKey, WorkflowCommand, empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";
const EQ: &str = "atomik.eq";

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

/// A new instrument with its track and channel; returns the channel strip.
fn strip(c: &mut AppCore<RecordingSink>) -> DeviceKey {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.doc().console.channels.last().unwrap().channel.clone()
}

fn channels(c: &AppCore<RecordingSink>) -> Vec<DeviceKey> {
    c.doc()
        .console
        .channels
        .iter()
        .map(|x| x.channel.clone())
        .collect()
}

fn set(c: &mut AppCore<RecordingSink>, strip: &DeviceKey, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: strip.clone(),
        param: param.into(),
        value,
        merge: None,
    })
    .unwrap();
}

fn param(c: &AppCore<RecordingSink>, strip: &DeviceKey, param: &str) -> f32 {
    c.doc().device(strip).unwrap().params[param]
}

fn select(c: &mut AppCore<RecordingSink>, strips: &[DeviceKey]) {
    c.after_view_change();
    c.console_mut().set_selected_strips(strips);
}

fn run(c: &mut AppCore<RecordingSink>, id: &str) {
    assert!(c.pane_command(Pane::Console, id, None).is_some(), "{id}");
}

/// Three instruments; the middle one has a fader, a pan and an equaliser insert.
fn three() -> (AppCore<RecordingSink>, Vec<DeviceKey>) {
    let mut c = core();
    let a = strip(&mut c);
    let b = strip(&mut c);
    let d = strip(&mut c);
    set(&mut c, &b, "fader", 0.5);
    set(&mut c, &b, "pan", 0.25);
    c.apply_command(Command::Console(ConsoleCommand::SetInsert {
        channel: b.clone(),
        slot: 0,
        effect: Some(atomik_model::InsertEffect::New { type_id: EQ.into() }),
    }))
    .unwrap();
    (c, vec![a, b, d])
}

fn tracks(c: &AppCore<RecordingSink>) -> usize {
    c.doc().song.tracks.len()
}

#[test]
fn copy_and_paste_make_a_strip_after_the_selected_one_with_its_settings() {
    let (mut c, s) = three();
    select(&mut c, &[s[1].clone()]);
    run(&mut c, "edit.copy");
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Strips));
    select(&mut c, &[s[0].clone()]);
    let depth = c.document().undo_depth();
    run(&mut c, "edit.paste");
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    let now = channels(&c);
    assert_eq!(now.len(), 4);
    assert_eq!(now[0], s[0]);
    assert_eq!(&now[2..], &[s[1].clone(), s[2].clone()]);
    let copy = &now[1];
    assert_eq!(param(&c, copy, "fader"), 0.5);
    assert_eq!(param(&c, copy, "pan"), 0.25);
    assert_eq!(
        atomik_model::insert_chain(c.doc(), &default_registry(), copy).len(),
        1,
        "the insert came along"
    );
    assert_eq!(tracks(&c), 4, "the track was copied");
    assert_eq!(c.console().selected_strips(), vec![copy.clone()]);
    // The original strip is untouched.
    assert_eq!(param(&c, &s[1], "fader"), 0.5);
    c.undo();
    assert_eq!(channels(&c), s);
    assert_eq!(tracks(&c), 3);
}

#[test]
fn paste_with_nothing_selected_goes_to_the_end() {
    let (mut c, s) = three();
    select(&mut c, &[s[1].clone()]);
    run(&mut c, "edit.copy");
    select(&mut c, &[]);
    run(&mut c, "edit.paste");
    let now = channels(&c);
    assert_eq!(now.len(), 4);
    assert_eq!(&now[..3], &s[..]);
    assert_eq!(param(&c, &now[3], "fader"), 0.5);
}

#[test]
fn duplicate_copies_the_selected_strips_after_the_last_and_keeps_the_clipboard() {
    let (mut c, s) = three();
    select(&mut c, &[s[0].clone(), s[1].clone()]);
    run(&mut c, "edit.duplicate");
    let now = channels(&c);
    assert_eq!(now.len(), 5);
    assert_eq!(now[..2], s[..2]);
    assert_eq!(now[4], s[2]);
    assert_eq!(
        param(&c, &now[3], "fader"),
        0.5,
        "the copy of the middle strip"
    );
    assert_eq!(tracks(&c), 5);
    assert!(
        c.clipboard().is_empty(),
        "duplicate does not use the clipboard"
    );
    c.undo();
    assert_eq!(channels(&c), s);
}

#[test]
fn cut_removes_the_strip_and_a_paste_brings_it_back_with_its_settings() {
    let (mut c, s) = three();
    select(&mut c, &[s[1].clone()]);
    run(&mut c, "edit.cut");
    assert_eq!(channels(&c).len(), 2);
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Strips));
    let first = channels(&c)[0].clone();
    select(&mut c, &[first]);
    run(&mut c, "edit.paste");
    let now = channels(&c);
    assert_eq!(now.len(), 3);
    assert_eq!(param(&c, &now[1], "pan"), 0.25);
}

#[test]
fn a_bus_or_a_mixer_channel_cannot_be_copied() {
    let (mut c, s) = three();
    c.apply_command(Command::Console(ConsoleCommand::CreateBus {
        name: "Drums".into(),
        dest: atomik_model::console::OutputDest::Master,
    }))
    .unwrap();
    let bus = c.doc().console.buses.last().unwrap().bus.clone();
    select(&mut c, &[bus]);
    let before = c.doc().clone();
    run(&mut c, "edit.copy");
    assert!(c.clipboard().is_empty());
    run(&mut c, "edit.duplicate");
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().is_some());
    select(&mut c, &[s[0].clone()]);
    run(&mut c, "edit.paste");
    assert_eq!(c.doc(), &before, "an empty clipboard pastes nothing");
}

#[test]
fn a_ctrl_drag_copies_the_strip_to_the_dropped_position() {
    let (mut c, s) = three();
    assert!(c.copy_strips_to(&[s[2].clone()], 0));
    let now = channels(&c);
    assert_eq!(now.len(), 4);
    assert_eq!(&now[1..], &s[..]);
    assert_eq!(tracks(&c), 4);
    // A strip dragged within a selection copies the selection.
    assert!(c.copy_strips_to(&[s[0].clone(), s[1].clone()], 4));
    assert_eq!(channels(&c).len(), 6);
    assert_eq!(&channels(&c)[..4], &now[..]);
}

#[test]
fn the_clipboard_menu_rules_follow_the_console_selection() {
    let (mut c, s) = three();
    assert!(!c.has_selection(Pane::Console));
    select(&mut c, &[s[0].clone()]);
    assert!(c.has_selection(Pane::Console));
    assert!(!c.can_paste(Pane::Console));
    run(&mut c, "edit.copy");
    assert!(c.can_paste(Pane::Console));
    assert!(
        c.can_paste(Pane::Rack),
        "the rack takes the devices of a copied strip"
    );
}
