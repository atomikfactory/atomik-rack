//! Master keyboard modes, Manual Rec and the corner's clear buttons: what the app does with
//! what the arrangement asks for.

use super::*;
use crate::master_keyboard::MasterKeyboard;
use crate::sink::RecordingSink;
use atomik_model::{
    CreateOptions, CreateSource, DeviceKey, SongCommand, WorkflowCommand, empty_doc,
};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{Modifiers, PointerButton};

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

fn instrument(c: &mut AppCore<RecordingSink>) -> DeviceKey {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.view().selected().unwrap().clone()
}

fn armed(c: &AppCore<RecordingSink>) -> Vec<String> {
    let mut v: Vec<String> = c
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.armed)
        .map(|t| t.device.0.clone())
        .collect();
    v.sort();
    v
}

fn click_at(c: &mut AppCore<RecordingSink>, p: Point) {
    for event in [
        UiEvent::PointerMove {
            pos: p,
            modifiers: Modifiers::NONE,
        },
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
    ] {
        c.handle_pane_event(Pane::Arrangement, &event);
    }
}

fn show(c: &mut AppCore<RecordingSink>) {
    let doc = c.doc().clone();
    c.arrangement_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1100.0, 600.0));
    c.arrangement_mut().sync(&doc, &default_registry());
}

fn click_header(c: &mut AppCore<RecordingSink>, track: &DeviceKey) {
    show(c);
    let p = c.arrangement().track_header_center(track).expect("header");
    click_at(c, p);
}

/// The header arrow of `track`: on the first line, right of the name.
fn arrow_of(c: &AppCore<RecordingSink>, track: &DeviceKey) -> Point {
    let p = c.arrangement().track_header_center(track).expect("header");
    Point::new(180.0, p.y + 2.0)
}

fn target_key(c: &AppCore<RecordingSink>) -> Option<DeviceKey> {
    c.notes_target_device().map(|d| d.key.clone())
}

// ------------------------------------------------------------------ Standard

#[test]
fn standard_mode_follows_the_selected_track() {
    let mut c = core();
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    assert_eq!(c.master_keyboard_mode(), MasterKeyboard::Standard);
    click_header(&mut c, &first);
    assert_eq!(target_key(&c), Some(first.clone()));
    assert_eq!(armed(&c), vec![first.0.clone()]);
    click_header(&mut c, &second);
    assert_eq!(target_key(&c), Some(second));
    // The arrow cannot choose a track in this mode.
    show(&mut c);
    let arrow = arrow_of(&c, &first);
    click_at(&mut c, arrow);
    assert_eq!(c.master_keyboard_track(), None);
}

// ------------------------------------------------------------------ Separated

#[test]
fn separated_mode_plays_the_chosen_track_whatever_is_selected() {
    let mut c = core();
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    c.set_master_keyboard_mode(MasterKeyboard::Separated);
    // Nothing chosen yet: it behaves like Standard.
    click_header(&mut c, &first);
    assert_eq!(target_key(&c), Some(first.clone()));
    // The arrow of the second track sends the keyboard there.
    show(&mut c);
    let arrow = arrow_of(&c, &second);
    click_at(&mut c, arrow);
    assert_eq!(c.master_keyboard_track(), Some(&second));
    assert_eq!(target_key(&c), Some(second.clone()));
    // Selecting the first track to edit it leaves the keyboard on the second.
    click_header(&mut c, &first);
    assert_eq!(c.view().selected(), Some(&first));
    assert_eq!(target_key(&c), Some(second.clone()));
    // A second click on the lit arrow hands the keyboard back to the selection.
    show(&mut c);
    let arrow = arrow_of(&c, &second);
    click_at(&mut c, arrow);
    assert_eq!(c.master_keyboard_track(), None);
    assert_eq!(target_key(&c), Some(first));
}

#[test]
fn separated_mode_refuses_a_missing_track_and_new_documents_forget_the_choice() {
    let mut c = core();
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    c.set_master_keyboard_mode(MasterKeyboard::Separated);
    c.set_master_keyboard_track(Some(first.clone()));
    assert_eq!(target_key(&c), Some(first.clone()));
    // A key that is not a track is refused.
    c.set_master_keyboard_track(Some(DeviceKey("nothing".into())));
    assert_eq!(c.master_keyboard_track(), Some(&first));
    // Back to Standard: the choice is kept but ignored.
    c.set_master_keyboard_mode(MasterKeyboard::Standard);
    click_header(&mut c, &second);
    assert_eq!(target_key(&c), Some(second));
    c.new_document();
    assert_eq!(c.master_keyboard_track(), None);
}

// ------------------------------------------------------------------ Manual Rec

#[test]
fn manual_rec_leaves_the_armed_flags_untouched_when_a_track_is_selected() {
    let mut c = core();
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    assert_eq!(armed(&c), vec![second.0.clone()]);
    c.set_manual_rec(true);
    assert!(c.manual_rec());
    click_header(&mut c, &first);
    assert_eq!(c.view().selected(), Some(&first), "still selected");
    assert_eq!(armed(&c), vec![second.0.clone()], "arming is the user's");
    // Off again: selecting arms as before.
    c.set_manual_rec(false);
    click_header(&mut c, &second);
    click_header(&mut c, &first);
    assert!(
        armed(&c).contains(&first.0),
        "selecting arms again: {:?}",
        armed(&c)
    );
}

#[test]
fn a_track_created_under_manual_rec_is_not_armed_and_one_undo_removes_it() {
    let mut c = core();
    let first = instrument(&mut c);
    c.set_manual_rec(true);
    let tracks = c.doc().song.tracks.len();
    let second = instrument(&mut c);
    assert_eq!(c.doc().song.tracks.len(), tracks + 1);
    assert!(armed(&c).contains(&first.0), "the earlier arming is kept");
    assert!(!armed(&c).contains(&second.0), "{second:?} is not armed");
    assert!(c.undo());
    assert_eq!(c.doc().song.tracks.len(), tracks, "creation was one step");
}

#[test]
fn the_corner_button_toggles_manual_rec() {
    let mut c = core();
    instrument(&mut c);
    show(&mut c);
    // The third button of the row under the snap selector.
    click_at(&mut c, Point::new(94.0, 32.5));
    assert!(c.manual_rec());
    click_at(&mut c, Point::new(94.0, 32.5));
    assert!(!c.manual_rec());
}

// ------------------------------------------------------------------ clear mutes and solos

fn flag(c: &mut AppCore<RecordingSink>, device: &DeviceKey, muted: bool, solo: bool) {
    let armed = c.doc().song.track(device).unwrap().armed;
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: device.clone(),
        muted,
        solo,
        armed,
    }))
    .unwrap();
}

#[test]
fn the_master_buttons_clear_all_mutes_and_all_solos_each_in_one_undo_step() {
    let mut c = core();
    let a = instrument(&mut c);
    let b = instrument(&mut c);
    let d = instrument(&mut c);
    flag(&mut c, &a, true, false);
    flag(&mut c, &b, true, true);
    flag(&mut c, &d, false, true);
    show(&mut c);
    let muted = |c: &AppCore<RecordingSink>| c.doc().song.tracks.iter().filter(|t| t.muted).count();
    let soloed = |c: &AppCore<RecordingSink>| c.doc().song.tracks.iter().filter(|t| t.solo).count();
    assert_eq!((muted(&c), soloed(&c)), (2, 2));
    click_at(&mut c, Point::new(22.0, 32.5));
    assert_eq!((muted(&c), soloed(&c)), (0, 2), "only the mutes went");
    click_at(&mut c, Point::new(46.0, 32.5));
    assert_eq!((muted(&c), soloed(&c)), (0, 0));
    // One undo step each.
    assert!(c.undo());
    assert_eq!((muted(&c), soloed(&c)), (0, 2));
    assert!(c.undo());
    assert_eq!((muted(&c), soloed(&c)), (2, 2));
}
