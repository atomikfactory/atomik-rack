//! Folders in the arrangement: M and S are the folder's own flags, a folder is dragged by its
//! header into and out of other folders, the rubber band over the headers selects tracks, and the
//! colour menu's Custom item asks for the colour picker.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::song::effective_tracks;
use atomik_model::{Command, Document, SongCommand, TrackPlace};
use atomik_ui::arrangement::{ADD_ROW_HEIGHT, FolderButton};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, ColorTarget, Key, Modifiers, PointerButton, UiAction, UiEvent};
use support::song::*;
use support::*;

/// Tracks in the order bass, lead, keys, pad.
fn setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let mut d = Document::new(song_doc());
    d.apply(
        Command::Song(SongCommand::AddTrack { device: key("pad") }),
        &reg,
    )
    .unwrap();
    // The reference song mutes lead and solos and arms keys: start from plain tracks.
    for k in ["lead", "keys"] {
        d.apply(
            Command::Song(SongCommand::SetTrackFlags {
                device: key(k),
                muted: false,
                solo: false,
                armed: false,
            }),
            &reg,
        )
        .unwrap();
    }
    let mut v = arrangement(d.doc(), &reg);
    // Tall enough to leave empty room under the rows of the four tracks and the add row.
    v.set_bounds(Rect::new(0.0, 0.0, 1200.0, 900.0));
    v.sync(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    (d, reg, v)
}

fn down_with(at: Point, modifiers: Modifiers) -> UiEvent {
    UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers,
        clicks: 1,
    }
}

fn run(d: &Document, reg: &Registry, v: &mut ArrangementView, events: &[UiEvent]) -> Vec<UiAction> {
    events
        .iter()
        .flat_map(|e| v.handle(e, d.doc(), reg))
        .collect()
}

fn apply(d: &mut Document, reg: &Registry, v: &mut ArrangementView, actions: &[UiAction]) {
    for a in actions {
        if let UiAction::Command(c) = a {
            d.apply(c.clone(), reg)
                .unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
    }
    v.sync(d.doc(), reg);
}

fn group(d: &mut Document, reg: &Registry, v: &mut ArrangementView, devices: &[&str]) {
    let name = format!("Folder {}", d.doc().song.org.groups.len() + 1);
    let cmd = Command::Song(SongCommand::GroupTracks {
        devices: devices.iter().map(|k| key(k)).collect(),
        name,
    });
    d.apply(cmd, reg).unwrap();
    v.sync(d.doc(), reg);
}

fn order(d: &Document) -> Vec<String> {
    d.doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

fn playing(d: &Document) -> Vec<String> {
    effective_tracks(&d.doc().song)
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

fn click(at: Point) -> [UiEvent; 2] {
    [down_with(at, Modifiers::NONE), up(at)]
}

// ------------------------------------------------------------------ mute and solo

#[test]
fn m_and_s_on_a_folder_set_the_folders_own_flags() {
    let (mut d, reg, mut v) = setup();
    group(&mut d, &reg, &mut v, &["bass", "lead"]);
    let m = v.folder_button_center(1, FolderButton::Mute).unwrap();
    let actions = run(&d, &reg, &mut v, &click(m));
    let flags = [SongCommand::SetGroupFlags {
        group: 1,
        muted: true,
        solo: false,
    }];
    assert_eq!(song_commands(&actions), flags);
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(playing(&d), ["keys", "pad"]);
    assert!(
        d.doc().song.tracks.iter().all(|t| !t.muted),
        "the tracks keep their own flags"
    );
    // A second press unmutes the folder.
    let actions = run(&d, &reg, &mut v, &click(m));
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(playing(&d).len(), 4);
    // Solo plays the folder's tracks alone, and one undo step takes it back.
    let s = v.folder_button_center(1, FolderButton::Solo).unwrap();
    let actions = run(&d, &reg, &mut v, &click(s));
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(playing(&d), ["bass", "lead"]);
    d.undo().unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(playing(&d).len(), 4);
}

#[test]
fn the_arm_button_of_a_folder_still_arms_every_track_in_it() {
    let (mut d, reg, mut v) = setup();
    group(&mut d, &reg, &mut v, &["bass", "lead"]);
    let r = v.folder_button_center(1, FolderButton::Arm).unwrap();
    let actions = run(&d, &reg, &mut v, &click(r));
    assert!(
        song_commands(&actions)
            .iter()
            .all(|c| matches!(c, SongCommand::SetTrackFlags { armed: true, .. }))
    );
    apply(&mut d, &reg, &mut v, &actions);
    let armed: Vec<_> = d
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| t.armed)
        .map(|t| t.device.0.clone())
        .collect();
    assert_eq!(armed, ["bass", "lead"]);
}

// ------------------------------------------------------------------ dragging a folder

/// Presses the header of folder `group` and moves to `to` in two steps.
fn drag_folder(d: &Document, reg: &Registry, v: &mut ArrangementView, group: u32, to: Point) {
    let start = v.folder_header_center(group).unwrap();
    let events = [
        down_with(start, Modifiers::NONE),
        mv_with(Point::new(start.x, (start.y + to.y) / 2.0), Modifiers::NONE),
        mv_with(to, Modifiers::NONE),
    ];
    let sent = run(d, reg, v, &events);
    assert!(sent.is_empty(), "nothing is sent while dragging: {sent:?}");
}

#[test]
fn dragging_a_folder_to_the_end_of_the_list_moves_it_with_its_tracks() {
    let (mut d, reg, mut v) = setup();
    group(&mut d, &reg, &mut v, &["bass", "lead"]);
    let pad = v.lane_rect(&key("pad"), 0).unwrap();
    let to = Point::new(100.0, pad.y1 - 3.0);
    drag_folder(&d, &reg, &mut v, 1, to);
    let drop = v
        .track_drop()
        .expect("the end of the list takes the folder");
    assert_eq!(drop.place.into, None);
    let actions = run(&d, &reg, &mut v, &[up(to)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::MoveGroupTo {
            group: 1,
            place: TrackPlace {
                to_index: 2,
                into: None
            }
        }]
    );
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(order(&d), ["keys", "pad", "bass", "lead"]);
    assert_eq!(d.doc().song.track_group(&key("bass")), Some(1));
    assert_eq!(d.doc().song.track_group(&key("lead")), Some(1));
    d.undo().unwrap();
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"]);
}

#[test]
fn a_folder_is_dragged_into_another_folder() {
    let (mut d, reg, mut v) = setup();
    group(&mut d, &reg, &mut v, &["bass", "lead"]);
    group(&mut d, &reg, &mut v, &["keys", "pad"]);
    // The lower half of folder 2's row is the start of that folder.
    let row = v.folder_header_center(2).unwrap();
    let to = Point::new(100.0, row.y + 8.0);
    drag_folder(&d, &reg, &mut v, 1, to);
    let actions = run(&d, &reg, &mut v, &[up(to)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::MoveGroupTo {
            group: 1,
            place: TrackPlace {
                to_index: 0,
                into: Some(2)
            }
        }]
    );
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(d.doc().song.org.group(1).unwrap().parent, Some(2));
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"]);
}

#[test]
fn a_folder_cannot_be_dropped_into_itself_and_escape_cancels() {
    let (mut d, reg, mut v) = setup();
    group(&mut d, &reg, &mut v, &["bass", "lead"]);
    let lead = v.lane_rect(&key("lead"), 0).unwrap();
    let to = Point::new(100.0, lead.y1 - 3.0);
    drag_folder(&d, &reg, &mut v, 1, to);
    assert!(v.track_drop().is_none(), "into itself shows no line");
    let actions = run(&d, &reg, &mut v, &[up(to)]);
    assert!(actions.is_empty());
    // Escape ends a drag that had a place.
    let pad = v.lane_rect(&key("pad"), 0).unwrap();
    drag_folder(&d, &reg, &mut v, 1, Point::new(100.0, pad.y1 - 3.0));
    assert!(v.track_drop().is_some());
    run(
        &d,
        &reg,
        &mut v,
        &[UiEvent::Key {
            key: Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        }],
    );
    assert!(v.track_drop().is_none());
    let actions = run(&d, &reg, &mut v, &[up(Point::new(100.0, pad.y1 - 3.0))]);
    assert!(actions.is_empty(), "the release after Escape does nothing");
    apply(&mut d, &reg, &mut v, &[]);
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"]);
}

#[test]
fn pressing_a_folder_header_without_moving_only_selects_the_folder() {
    let (mut d, reg, mut v) = setup();
    group(&mut d, &reg, &mut v, &["bass", "lead"]);
    let at = v.folder_header_center(1).unwrap();
    let actions = run(&d, &reg, &mut v, &click(at));
    assert!(song_commands(&actions).is_empty());
    assert_eq!(v.selected_tracks(), [key("bass"), key("lead")]);
}

// ------------------------------------------------------------------ the rubber band

#[test]
fn a_band_dragged_up_the_empty_header_column_selects_the_tracks_it_touches() {
    let (d, reg, mut v) = setup();
    let pad = v.lane_rect(&key("pad"), 0).unwrap();
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let lead = v.track_header_center(&key("lead")).unwrap();
    let x = v.track_header_center(&key("pad")).unwrap().x;
    let start = Point::new(x, pad.y1 + ADD_ROW_HEIGHT + 20.0);
    let sent = run(
        &d,
        &reg,
        &mut v,
        &[
            down_with(start, Modifiers::NONE),
            mv(Point::new(x, keys.center().y)),
            mv(Point::new(x, lead.y)),
        ],
    );
    assert!(sent.is_empty(), "{sent:?}");
    assert!(v.header_band().is_some(), "the band is drawn");
    assert_eq!(v.selected_tracks(), [key("lead"), key("keys"), key("pad")]);
    // Back down: the selection follows the band.
    run(&d, &reg, &mut v, &[mv(Point::new(x, keys.center().y))]);
    assert_eq!(v.selected_tracks(), [key("keys"), key("pad")]);
    run(&d, &reg, &mut v, &[up(Point::new(x, keys.center().y))]);
    assert!(v.header_band().is_none());
    assert_eq!(v.selected_tracks(), [key("keys"), key("pad")]);
}

#[test]
fn alt_on_a_header_starts_a_band_and_shift_keeps_the_selection() {
    let (d, reg, mut v) = setup();
    let alt = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    let from = v.track_header_center(&key("bass")).unwrap();
    let to = v.track_header_center(&key("lead")).unwrap();
    run(
        &d,
        &reg,
        &mut v,
        &[
            down_with(from, alt),
            mv_with(Point::new(from.x, (from.y + to.y) / 2.0), alt),
            mv_with(to, alt),
            up(to),
        ],
    );
    assert_eq!(v.selected_tracks(), [key("bass"), key("lead")]);
    // A second band with Shift adds to them.
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let pad = v.lane_rect(&key("pad"), 0).unwrap();
    let x = from.x;
    let start = Point::new(x, pad.y1 + ADD_ROW_HEIGHT + 20.0);
    run(
        &d,
        &reg,
        &mut v,
        &[
            down_with(start, shift),
            mv_with(Point::new(x, pad.center().y), shift),
            up(Point::new(x, pad.center().y)),
        ],
    );
    assert_eq!(v.selected_tracks(), [key("bass"), key("lead"), key("pad")]);
    // A press on nothing clears the selection; Escape during a band puts it back.
    run(&d, &reg, &mut v, &click(start));
    assert!(v.selected_tracks().is_empty());
    v.set_selected_track(Some(key("keys")));
    run(
        &d,
        &reg,
        &mut v,
        &[down_with(start, Modifiers::NONE), mv(Point::new(x, from.y))],
    );
    assert_ne!(v.selected_tracks(), [key("keys")]);
    run(
        &d,
        &reg,
        &mut v,
        &[UiEvent::Key {
            key: Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        }],
    );
    assert_eq!(v.selected_tracks(), [key("keys")]);
}

// ------------------------------------------------------------------ the colour picker

#[test]
fn the_custom_colour_item_asks_for_the_picker_for_the_tracks_or_the_folder() {
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("lead"));
    let CommandOutcome::Handled(actions) =
        v.command("sequencer.track_color", Some("custom"), d.doc(), &reg)
    else {
        panic!("the custom item must be handled");
    };
    assert_eq!(
        actions,
        [UiAction::PickColor(ColorTarget::Tracks(vec![
            key("bass"),
            key("lead")
        ]))]
    );
    group(&mut d, &reg, &mut v, &["bass", "lead"]);
    v.set_selected_group(Some(1));
    let CommandOutcome::Handled(actions) =
        v.command("sequencer.track_color", Some("custom"), d.doc(), &reg)
    else {
        panic!("the custom item must be handled");
    };
    assert_eq!(actions, [UiAction::PickColor(ColorTarget::Folder(1))]);
    // With nothing selected there is nothing to colour.
    v.set_selected_group(None);
    v.set_selected_track(None);
    assert!(matches!(
        v.command("sequencer.track_color", Some("custom"), d.doc(), &reg),
        CommandOutcome::Declined
    ));
}
