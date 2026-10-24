//! Selecting a strip in the console arms its song track like creating an instrument does: the
//! tracks an earlier auto-arm armed are disarmed, the tracks the user armed stay, and strips
//! without a track (buses) change nothing.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{
    ConsoleCommand, CreateOptions, CreateSource, DeviceKey, SongCommand, WorkflowCommand,
    console::OutputDest, empty_doc,
};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{Modifiers, PointerButton};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

fn instrument(c: &mut AppCore<RecordingSink>) -> DeviceKey {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.view().selected().unwrap().clone()
}

fn hand_armed(c: &mut AppCore<RecordingSink>, k: &str) {
    c.apply_command(Command::AddDevice {
        index: 0,
        type_id: SYNTH.into(),
        key: Some(key(k)),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key(k) }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key(k),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
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

/// Clicks a strip's name plate in the console, as the shell forwards it.
fn click_strip(c: &mut AppCore<RecordingSink>, strip: &DeviceKey, modifiers: Modifiers) {
    let doc = c.doc().clone();
    c.console_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1100.0, 760.0));
    c.console_mut().sync(&doc, &default_registry());
    let p: Point = c.console().name_center(strip).expect("strip shown");
    c.handle_pane_event(
        Pane::Console,
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
            clicks: 1,
        },
    );
    c.handle_pane_event(
        Pane::Console,
        &UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
        },
    );
}

fn channel_of(c: &AppCore<RecordingSink>, source: &DeviceKey) -> DeviceKey {
    c.doc().console.channel_of(source).unwrap().channel.clone()
}

#[test]
fn selecting_a_channel_arms_its_track_and_disarms_the_auto_armed_one() {
    let mut c = core();
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    assert_eq!(armed(&c), vec![second.0.clone()]);
    let ch = channel_of(&c, &first);
    click_strip(&mut c, &ch, Modifiers::NONE);
    assert_eq!(armed(&c), vec![first.0.clone()]);
    assert_eq!(c.view().selected(), Some(&first));
    assert_eq!(
        c.notes_target_device().map(|d| d.key.clone()),
        Some(first.clone()),
        "the live keyboard plays the selected channel"
    );
    // Back to the other one: the auto-armed first is disarmed again.
    let ch = channel_of(&c, &second);
    click_strip(&mut c, &ch, Modifiers::NONE);
    assert_eq!(armed(&c), vec![second.0.clone()]);
}

#[test]
fn a_track_the_user_armed_stays_armed_and_the_selection_still_plays() {
    let mut c = core();
    hand_armed(&mut c, "hand");
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    let ch = channel_of(&c, &first);
    click_strip(&mut c, &ch, Modifiers::NONE);
    let mut want = vec!["hand".to_string(), first.0.clone()];
    want.sort();
    assert_eq!(
        armed(&c),
        want,
        "{second:?} was auto-armed, so it is disarmed"
    );
    assert_eq!(c.notes_target_device().map(|d| d.key.clone()), Some(first));
}

#[test]
fn selecting_a_bus_arms_and_disarms_nothing() {
    let mut c = core();
    hand_armed(&mut c, "hand");
    let synth = instrument(&mut c);
    c.apply_command(Command::Console(ConsoleCommand::CreateBus {
        name: "Drums".into(),
        dest: OutputDest::Master,
    }))
    .unwrap();
    let before = armed(&c);
    let bus = c.doc().console.buses[0].bus.clone();
    click_strip(&mut c, &bus, Modifiers::NONE);
    assert_eq!(armed(&c), before);
    assert!(armed(&c).contains(&synth.0), "the auto-armed track stays");
}

#[test]
fn arming_by_selection_is_one_undo_step() {
    let mut c = core();
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    let ch = channel_of(&c, &first);
    click_strip(&mut c, &ch, Modifiers::NONE);
    assert_eq!(armed(&c), vec![first.0.clone()]);
    assert!(c.undo());
    assert_eq!(armed(&c), vec![second.0.clone()]);
}

/// Clicks a track header in the arrangement, as the shell forwards it.
fn click_header(c: &mut AppCore<RecordingSink>, track: &DeviceKey) {
    let doc = c.doc().clone();
    c.arrangement_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1100.0, 600.0));
    c.arrangement_mut().sync(&doc, &default_registry());
    let p = c
        .arrangement()
        .track_header_center(track)
        .expect("header shown");
    for event in [
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

#[test]
fn clicking_a_track_header_acts_like_clicking_its_channel_strip() {
    let mut c = core();
    let first = instrument(&mut c);
    let second = instrument(&mut c);
    assert_eq!(armed(&c), vec![second.0.clone()]);
    click_header(&mut c, &first);
    assert_eq!(armed(&c), vec![first.0.clone()]);
    assert_eq!(c.view().selected(), Some(&first));
    assert_eq!(c.console().selected_strips(), vec![channel_of(&c, &first)]);
    assert_eq!(c.notes_target_device().map(|d| d.key.clone()), Some(first));
    assert!(c.take_rack_focus_request(), "the keys go to the rack");
}
