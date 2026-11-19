//! Selecting tracks in the arrangement: an audio track behaves like a note track (its meter is
//! part of its header, an empty click on its lane gives up the clip selection), Shift selects a
//! range in the order shown, Ctrl toggles one track, and the header menu keeps a selection of
//! several tracks so the colour reaches all of them in one undo step.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipId, Command, Document, RackDoc, SongCommand};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{ArrangementView, ContextTarget, Modifiers, PointerButton, UiAction, UiEvent};
use support::audio::*;
use support::song::*;
use support::*;

const CTRL: Modifiers = Modifiers {
    ctrl: true,
    ..Modifiers::NONE
};
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::NONE
};
const SHIFT_CTRL: Modifiers = Modifiers {
    shift: true,
    ctrl: true,
    ..Modifiers::NONE
};

fn press(
    v: &mut ArrangementView,
    doc: &RackDoc,
    at: Point,
    button: PointerButton,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: at,
            button,
            modifiers,
            clicks: 1,
        },
        doc,
        &reg,
    );
    out.extend(v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button,
            modifiers,
        },
        doc,
        &reg,
    ));
    out
}

fn click(v: &mut ArrangementView, doc: &RackDoc, at: Point, modifiers: Modifiers) {
    press(v, doc, at, PointerButton::Primary, modifiers);
}

fn names(v: &ArrangementView) -> Vec<String> {
    v.selected_tracks().into_iter().map(|d| d.0).collect()
}

/// A point on the header of `device`: the name (`meter` false) or the level meter of an audio
/// track.
fn header(v: &ArrangementView, device: &str, meter: bool) -> Point {
    let c = v.track_header_center(&key(device)).unwrap();
    Point::new(if meter { c.x + 30.0 } else { c.x }, c.y)
}

fn click_on(v: &mut ArrangementView, doc: &RackDoc, device: &str, meter: bool, m: Modifiers) {
    let at = header(v, device, meter);
    click(v, doc, at, m);
}

fn right_click_on(v: &mut ArrangementView, doc: &RackDoc, device: &str) -> Vec<UiAction> {
    let at = header(v, device, false);
    press(v, doc, at, PointerButton::Secondary, Modifiers::NONE)
}

/// A plain click on the empty lane area of the track.
fn click_lane(v: &mut ArrangementView, doc: &RackDoc, device: &str) {
    let at = Point::new(700.0, header(v, device, false).y);
    click(v, doc, at, Modifiers::NONE);
}

fn audio_setup() -> (RackDoc, ArrangementView) {
    let (doc, reg) = (audio_doc(), registry());
    let v = audio_arrangement(&doc, &reg);
    (doc, v)
}

/// Bass, lead, keys, pad.
fn song_setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let mut d = Document::new(song_doc());
    d.apply(
        Command::Song(SongCommand::AddTrack { device: key("pad") }),
        &reg,
    )
    .unwrap();
    let mut v = arrangement(d.doc(), &reg);
    v.set_bounds(Rect::new(0.0, 0.0, 1200.0, 900.0));
    v.sync(d.doc(), &reg);
    (d, reg, v)
}

// ------------------------------------------------------------------ audio tracks

#[test]
fn an_audio_track_starts_a_multi_selection_like_a_note_track() {
    let (doc, mut v) = audio_setup();
    // From the name of an audio track, then Ctrl on a note track and on the other audio track.
    click_on(&mut v, &doc, "audio-track", false, Modifiers::NONE);
    click_on(&mut v, &doc, "bass", false, CTRL);
    assert_eq!(names(&v), ["bass", "audio-track"]);
    click_on(&mut v, &doc, "audio-track-2", false, CTRL);
    assert_eq!(names(&v), ["bass", "audio-track", "audio-track-2"]);
}

#[test]
fn shift_and_ctrl_on_the_meter_of_an_audio_track_pick_the_track() {
    let (doc, mut v) = audio_setup();
    click_on(&mut v, &doc, "audio-track", true, Modifiers::NONE);
    assert_eq!(names(&v), ["audio-track"], "a plain click picks it too");
    click_on(&mut v, &doc, "audio-track-2", true, CTRL);
    assert_eq!(names(&v), ["audio-track", "audio-track-2"]);
    click_on(&mut v, &doc, "bass", false, SHIFT);
    assert_eq!(names(&v), ["bass", "audio-track", "audio-track-2"]);
}

#[test]
fn an_empty_click_deselects_an_audio_track_and_its_clips() {
    let (doc, mut v) = audio_setup();
    // The selected audio track gives way.
    click_on(&mut v, &doc, "audio-track", false, Modifiers::NONE);
    click_lane(&mut v, &doc, "audio-track-2");
    assert_eq!(v.selected_track(), None);
    // So does a selected audio clip, as the selected note clip does on an empty note lane.
    v.set_selected_clips(&[ClipId(10)]);
    click_lane(&mut v, &doc, "audio-track-2");
    assert!(v.selected_clips().is_empty());
    v.set_selected_clips(&[ClipId(1)]);
    click_lane(&mut v, &doc, "bass");
    assert!(v.selected_clips().is_empty(), "the note track did already");
}

// ------------------------------------------------------------------ ranges

#[test]
fn shift_selects_the_range_from_the_anchor_and_ctrl_moves_the_anchor() {
    let (d, _reg, mut v) = song_setup();
    let doc = d.doc();
    click_on(&mut v, doc, "lead", false, Modifiers::NONE);
    click_on(&mut v, doc, "pad", false, SHIFT);
    assert_eq!(names(&v), ["lead", "keys", "pad"]);
    // Another Shift+click replaces the range, from the same anchor.
    click_on(&mut v, doc, "bass", false, SHIFT);
    assert_eq!(names(&v), ["bass", "lead"]);
    click_on(&mut v, doc, "keys", false, SHIFT);
    assert_eq!(names(&v), ["lead", "keys"]);
    // Ctrl picks one track and makes it the anchor.
    click_on(&mut v, doc, "bass", false, CTRL);
    assert_eq!(names(&v), ["bass", "lead", "keys"]);
    click_on(&mut v, doc, "keys", false, CTRL);
    assert_eq!(names(&v), ["bass", "lead"], "Ctrl takes one track out");
    click_on(&mut v, doc, "pad", false, CTRL);
    click_on(&mut v, doc, "keys", false, SHIFT);
    assert_eq!(
        names(&v),
        ["keys", "pad"],
        "from the track Ctrl picked last"
    );
    // Shift+Ctrl adds the range to what is selected.
    click_on(&mut v, doc, "lead", false, Modifiers::NONE);
    click_on(&mut v, doc, "pad", false, CTRL);
    click_on(&mut v, doc, "keys", false, SHIFT_CTRL);
    assert_eq!(names(&v), ["lead", "keys", "pad"]);
}

#[test]
fn shift_without_an_anchor_selects_the_track_alone() {
    let (d, _reg, mut v) = song_setup();
    click_on(&mut v, d.doc(), "keys", false, SHIFT);
    assert_eq!(names(&v), ["keys"]);
    click_on(&mut v, d.doc(), "pad", false, SHIFT);
    assert_eq!(names(&v), ["keys", "pad"]);
}

#[test]
fn a_range_skips_the_tracks_of_a_collapsed_folder() {
    let (mut d, reg, mut v) = song_setup();
    d.apply(
        Command::Song(SongCommand::GroupTracks {
            devices: vec![key("lead"), key("keys")],
            name: "Folder".into(),
        }),
        &reg,
    )
    .unwrap();
    let group = d.doc().song.org.groups[0].id;
    d.apply(
        Command::Song(SongCommand::SetGroupCollapsed {
            group,
            collapsed: true,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    click_on(&mut v, d.doc(), "bass", false, Modifiers::NONE);
    click_on(&mut v, d.doc(), "pad", false, SHIFT);
    assert_eq!(names(&v), ["bass", "pad"]);
}

// ------------------------------------------------------------------ the header menu

#[test]
fn a_right_click_on_a_selected_track_keeps_the_selection() {
    let (d, _reg, mut v) = song_setup();
    let doc = d.doc();
    click_on(&mut v, doc, "bass", false, Modifiers::NONE);
    click_on(&mut v, doc, "keys", false, SHIFT);
    assert_eq!(names(&v), ["bass", "lead", "keys"]);
    let a = right_click_on(&mut v, doc, "lead");
    assert!(
        a.iter().any(|a| matches!(
            a,
            UiAction::ContextMenu {
                target: ContextTarget::TrackHeader,
                ..
            }
        )),
        "{a:?}"
    );
    assert_eq!(names(&v), ["bass", "lead", "keys"]);
    assert_eq!(
        v.selected_track().map(|k| k.0.as_str()),
        Some("lead"),
        "the items that take one track act on the one clicked"
    );
    // A right click on an unselected track selects it alone.
    right_click_on(&mut v, doc, "pad");
    assert_eq!(names(&v), ["pad"]);
}

#[test]
fn the_track_colour_reaches_every_selected_track_in_one_undo_step() {
    let (d, reg, mut v) = song_setup();
    let doc = d.doc();
    click_on(&mut v, doc, "bass", false, Modifiers::NONE);
    click_on(&mut v, doc, "keys", false, CTRL);
    right_click_on(&mut v, doc, "keys");
    let out = v.command("sequencer.track_color", Some("green"), doc, &reg);
    let atomik_ui::keymap::CommandOutcome::Handled(actions) = out else {
        panic!("{out:?}");
    };
    let [UiAction::Command(Command::Batch(cmds))] = actions.as_slice() else {
        panic!("one batch: {actions:?}");
    };
    let coloured: Vec<String> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Song(SongCommand::SetTrackColor { device, .. }) => Some(device.0.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(coloured, ["bass", "keys"]);
}

#[test]
fn a_right_click_on_the_lane_label_of_a_selected_track_keeps_the_selection() {
    let (mut d, reg, mut v) = song_setup();
    d.apply(
        Command::Song(SongCommand::AddNoteLane {
            device: key("keys"),
            name: None,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    let doc = d.doc();
    click_on(&mut v, doc, "bass", false, Modifiers::NONE);
    click_on(&mut v, doc, "keys", false, CTRL);
    let at = v
        .note_lane_header_center(&key("keys"), 0)
        .expect("a lane label");
    // Ctrl+click there toggles the track like on the rest of the header.
    click(&mut v, doc, at, CTRL);
    assert_eq!(names(&v), ["bass"]);
    click(&mut v, doc, at, CTRL);
    let a = press(&mut v, doc, at, PointerButton::Secondary, Modifiers::NONE);
    assert!(
        a.iter().any(|a| matches!(a, UiAction::ContextMenu { .. })),
        "{a:?}"
    );
    assert_eq!(names(&v), ["bass", "keys"]);
}

#[test]
fn dragging_from_empty_space_on_an_audio_lane_selects_clips_across_tracks() {
    let (doc, mut v) = audio_setup();
    let reg = registry();
    let clip = v.clip_rect(ClipId(10)).unwrap().center();
    let empty = Point::new(clip.x, header(&v, "audio-track-2", false).y);
    let events = [
        down(empty),
        UiEvent::PointerMove {
            pos: Point::new(clip.x, (clip.y + empty.y) / 2.0),
            modifiers: Modifiers::NONE,
        },
        UiEvent::PointerMove {
            pos: Point::new(clip.x + 4.0, clip.y),
            modifiers: Modifiers::NONE,
        },
        up(Point::new(clip.x + 4.0, clip.y)),
    ];
    run(|e| v.handle(e, &doc, &reg), &events);
    assert_eq!(v.selected_clips(), vec![ClipId(10)]);
    // Over a note clip as well: the band crosses tracks.
    let bass = v.clip_rect(ClipId(1)).unwrap().center();
    let far = Point::new(bass.x + 2.0, bass.y);
    let events = [
        down(empty),
        UiEvent::PointerMove {
            pos: far,
            modifiers: Modifiers::NONE,
        },
        up(far),
    ];
    run(|e| v.handle(e, &doc, &reg), &events);
    assert!(v.selected_clips().contains(&ClipId(1)));
    // A click without a drag gives up the selection.
    run(|e| v.handle(e, &doc, &reg), &[down(empty), up(empty)]);
    assert!(v.selected_clips().is_empty());
}
