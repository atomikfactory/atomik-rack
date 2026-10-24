//! Track organisation in the arrangement: folder rows (collapse, buttons, rename, summary),
//! selecting several tracks and grouping them, track colours and comments, folded automation
//! lanes, take rows that stay open with the project, moving, and the track height presets, run
//! against a real `Document` so the undo steps count. `cargo test -p atomik-ui --test organise --
//! --ignored` writes a preview PNG to `target/preview`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, DeviceKey, Document, SongCommand};
use atomik_ui::arrangement::{
    FOLDED_LANE_HEIGHT, FOLDER_ROW_HEIGHT, FolderButton, HEIGHT_LARGE, HEIGHT_SMALL, HeightPreset,
};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, ContextTarget, DisplayList, Key, LaneButton, Modifiers, PointerButton,
    TextEngine, UiAction, UiEvent,
};
use std::path::PathBuf;
use support::cpu::CpuCanvas;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const CUTOFF: &str = "filter.cutoff";

/// The song of the other arrangement tests (`bass` with a cutoff lane, `lead`, `keys`) and a
/// fourth track, `pad`.
fn setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let mut d = Document::new(song_doc());
    d.apply(
        Command::Song(SongCommand::AddTrack { device: key("pad") }),
        &reg,
    )
    .unwrap();
    let mut v = arrangement(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    (d, reg, v)
}

fn down(at: Point, modifiers: Modifiers, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers,
        clicks,
    }
}

fn up(at: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn run(d: &Document, reg: &Registry, v: &mut ArrangementView, events: &[UiEvent]) -> Vec<UiAction> {
    events
        .iter()
        .flat_map(|e| v.handle(e, d.doc(), reg))
        .collect()
}

fn click_with(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    at: Point,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    run(d, reg, v, &[down(at, modifiers, 1), up(at)])
}

fn click(d: &Document, reg: &Registry, v: &mut ArrangementView, at: Point) -> Vec<UiAction> {
    click_with(d, reg, v, at, Modifiers::NONE)
}

/// Applies the commands among `actions` and syncs the view.
fn apply(d: &mut Document, reg: &Registry, v: &mut ArrangementView, actions: &[UiAction]) {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                d.apply(c.clone(), reg)
                    .unwrap_or_else(|e| panic!("{c:?}: {e}"));
            }
            UiAction::EndMerge => d.end_merge(),
            _ => {}
        }
    }
    v.sync(d.doc(), reg);
}

fn command(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    id: &str,
    arg: Option<&str>,
) -> CommandOutcome {
    v.command(id, arg, d.doc(), reg)
}

/// Runs a command that must be handled and applies what it asks for.
fn do_command(
    d: &mut Document,
    reg: &Registry,
    v: &mut ArrangementView,
    id: &str,
    arg: Option<&str>,
) -> Vec<UiAction> {
    let CommandOutcome::Handled(actions) = command(d, reg, v, id, arg) else {
        panic!("{id} must be handled");
    };
    apply(d, reg, v, &actions);
    actions
}

fn order(d: &Document) -> Vec<String> {
    d.doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

/// Selects `bass` and `lead` (neighbours) and groups them: folder 1, "Folder 1".
fn grouped() -> (Document, Registry, ArrangementView) {
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("lead"));
    do_command(&mut d, &reg, &mut v, "sequencer.group_tracks", None);
    (d, reg, v)
}

fn type_text(d: &Document, reg: &Registry, v: &mut ArrangementView, text: &str) -> Vec<UiAction> {
    let mut out = Vec::new();
    for ch in text.chars() {
        let key = if ch == ' ' { Key::Space } else { Key::Char(ch) };
        out.extend(v.handle(&press(key, Modifiers::NONE), d.doc(), reg));
    }
    out
}

fn song_cmds(actions: &[UiAction]) -> Vec<SongCommand> {
    song_commands(actions)
}

// ------------------------------------------------------------------ grouping

#[test]
fn shift_clicking_headers_selects_several_tracks_and_group_tracks_makes_a_folder() {
    let (mut d, reg, mut v) = setup();
    let bass = v.track_header_center(&key("bass")).unwrap();
    let keys = v.track_header_center(&key("keys")).unwrap();
    click(&d, &reg, &mut v, bass);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    click_with(&d, &reg, &mut v, keys, shift);
    assert_eq!(v.selected_tracks(), [key("bass"), key("keys")]);
    // Shift-click again takes it out, a plain click goes back to one track.
    click_with(&d, &reg, &mut v, keys, shift);
    assert_eq!(v.selected_tracks(), [key("bass")]);
    click_with(&d, &reg, &mut v, keys, shift);
    click(&d, &reg, &mut v, keys);
    assert_eq!(v.selected_tracks(), [key("keys")]);
    click(&d, &reg, &mut v, bass);
    click_with(&d, &reg, &mut v, keys, shift);
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.group_tracks", None);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::GroupTracks {
            devices: vec![key("bass"), key("keys")],
            name: "Folder 1".into()
        }]
    );
    assert_eq!(order(&d), ["bass", "keys", "lead", "pad"]);
    assert!(
        v.folder_header_center(1).is_some(),
        "the folder row is there"
    );
    // One undo step.
    d.undo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert!(v.folder_header_center(1).is_none());
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"]);
}

#[test]
fn group_tracks_takes_the_tracks_of_the_selected_clips_and_needs_a_selection() {
    let (mut d, reg, mut v) = setup();
    assert!(matches!(
        command(&d, &reg, &mut v, "sequencer.group_tracks", None),
        CommandOutcome::NotApplicable(_)
    ));
    // Clip 3 is on `keys`, clip 2 on `lead`.
    v.set_selected_clips(&[atomik_model::ClipId(3), atomik_model::ClipId(2)]);
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.group_tracks", None);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::GroupTracks {
            devices: vec![key("lead"), key("keys")],
            name: "Folder 1".into()
        }]
    );
    // The next folder gets the next number.
    v.set_selected_track(Some(key("pad")));
    let actions = command(&d, &reg, &mut v, "sequencer.group_tracks", None);
    assert_eq!(
        song_cmds(actions.actions()),
        [SongCommand::GroupTracks {
            devices: vec![key("pad")],
            name: "Folder 2".into()
        }]
    );
}

#[test]
fn the_folder_row_sits_above_its_first_track_and_the_tracks_keep_their_rows() {
    let (d, _, v) = grouped();
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"]);
    let folder = v.folder_header_center(1).unwrap();
    let bass = v.lane_rect(&key("bass"), 0).unwrap();
    assert!(folder.y < bass.y0, "the header is above the first track");
    assert!(
        (bass.y0 - folder.y - FOLDER_ROW_HEIGHT / 2.0).abs() < 1.0,
        "and as tall as a folder row"
    );
    assert_eq!(v.track_depth(&key("bass")), 1);
    assert_eq!(v.track_depth(&key("keys")), 0);
    assert!(v.folder_point(1, 0).is_some());
}

// ------------------------------------------------------------------ collapsing

#[test]
fn the_chevron_collapses_and_expands_and_the_tracks_leave_the_layout() {
    let (mut d, reg, mut v) = grouped();
    let keys_before = v.lane_rect(&key("keys"), 0).unwrap();
    let toggle = v.folder_toggle_center(1).unwrap();
    let actions = click(&d, &reg, &mut v, toggle);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::SetGroupCollapsed {
            group: 1,
            collapsed: true
        }]
    );
    apply(&mut d, &reg, &mut v, &actions);
    assert!(v.lane_rect(&key("bass"), 0).is_none(), "hidden");
    assert!(v.lane_rect(&key("bass"), 1).is_none(), "with its lanes");
    assert!(v.lane_rect(&key("lead"), 0).is_none());
    assert!(v.track_hidden(&key("bass")));
    let keys_after = v.lane_rect(&key("keys"), 0).unwrap();
    assert!(keys_after.y0 < keys_before.y0, "the rows below move up");
    assert!(v.folder_header_center(1).is_some(), "the folder row stays");
    // Expanding puts everything back where it was.
    let toggle = v.folder_toggle_center(1).unwrap();
    let actions = click(&d, &reg, &mut v, toggle);
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(v.lane_rect(&key("keys"), 0).unwrap(), keys_before);
    assert!(v.lane_rect(&key("bass"), 0).is_some());
    // Undo and redo follow the document.
    d.undo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert!(v.lane_rect(&key("bass"), 0).is_none());
}

#[test]
fn the_track_selection_keys_skip_tracks_in_a_collapsed_folder() {
    let (mut d, reg, mut v) = grouped();
    do_command(&mut d, &reg, &mut v, "sequencer.toggle_folder", None);
    v.set_selected_group(Some(1));
    do_command(&mut d, &reg, &mut v, "sequencer.toggle_folder", None);
    // `toggle_folder` ran twice: expanded again. Collapse once and walk down.
    do_command(&mut d, &reg, &mut v, "sequencer.toggle_folder", None);
    assert!(v.track_hidden(&key("bass")));
    v.set_selected_track(None);
    do_command(&mut d, &reg, &mut v, "sequencer.select_next_track", None);
    assert_eq!(
        v.selected_track(),
        Some(&key("keys")),
        "bass and lead are hidden"
    );
    do_command(&mut d, &reg, &mut v, "sequencer.select_prev_track", None);
    assert_eq!(
        v.selected_track(),
        Some(&key("keys")),
        "nothing visible above"
    );
}

#[test]
fn double_clicking_the_timeline_part_of_a_folder_row_collapses_it() {
    let (d, reg, mut v) = grouped();
    let at = v.folder_point(1, 3 * BAR).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(at, Modifiers::NONE, 2), up(at)]);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::SetGroupCollapsed {
            group: 1,
            collapsed: true
        }]
    );
}

// ------------------------------------------------------------------ mute, solo, arm

#[test]
fn folder_buttons_set_the_flag_on_every_track_in_one_batch() {
    let (mut d, reg, mut v) = grouped();
    let m = v.folder_button_center(1, FolderButton::Mute).unwrap();
    let actions = click(&d, &reg, &mut v, m);
    let Some(Command::Batch(batch)) = commands(&actions).into_iter().next() else {
        panic!("one batch: {actions:?}");
    };
    assert_eq!(batch.len(), 2, "bass and lead");
    apply(&mut d, &reg, &mut v, &actions);
    let song = &d.doc().song;
    assert!(song.track(&key("bass")).unwrap().muted);
    assert!(song.track(&key("lead")).unwrap().muted);
    assert!(
        !song.track(&key("keys")).unwrap().muted,
        "outside the folder"
    );
    // Lead was muted already: a press sets all (bass was the odd one), the next clears all.
    let actions = click(&d, &reg, &mut v, m);
    apply(&mut d, &reg, &mut v, &actions);
    assert!(!d.doc().song.track(&key("bass")).unwrap().muted);
    assert!(!d.doc().song.track(&key("lead")).unwrap().muted);
    // One undo step each.
    d.undo(&reg).unwrap();
    assert!(d.doc().song.track(&key("bass")).unwrap().muted);
    // Solo and arm keep the other flags of each track.
    let s = v.folder_button_center(1, FolderButton::Solo).unwrap();
    let a = v.folder_button_center(1, FolderButton::Arm).unwrap();
    for at in [s, a] {
        let actions = click(&d, &reg, &mut v, at);
        apply(&mut d, &reg, &mut v, &actions);
    }
    let bass = d.doc().song.track(&key("bass")).unwrap();
    assert!(bass.muted && bass.solo && bass.armed);
    let lead = d.doc().song.track(&key("lead")).unwrap();
    assert!(lead.muted && lead.solo && lead.armed);
}

// ------------------------------------------------------------------ names and comments

#[test]
fn double_clicking_a_folder_name_renames_it_and_enter_commits() {
    let (mut d, reg, mut v) = grouped();
    assert!(!v.is_text_owner());
    let at = v.folder_header_center(1).unwrap();
    run(&d, &reg, &mut v, &[down(at, Modifiers::NONE, 2), up(at)]);
    assert!(v.is_text_owner(), "the name is typed");
    assert_eq!(v.org_edit_text(), Some("Folder 1"));
    for _ in 0..8 {
        v.handle(&press(Key::Backspace, Modifiers::NONE), d.doc(), &reg);
    }
    type_text(&d, &reg, &mut v, "Drum Bus");
    assert_eq!(v.org_edit_text(), Some("Drum Bus"));
    let actions = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::RenameGroup {
            group: 1,
            name: "Drum Bus".into()
        }]
    );
    assert!(!v.is_text_owner());
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(d.doc().song.org.group(1).unwrap().name, "Drum Bus");
}

#[test]
fn escape_and_an_empty_or_unchanged_name_rename_nothing() {
    let (d, reg, mut v) = grouped();
    assert!(v.begin_folder_rename(1));
    type_text(&d, &reg, &mut v, "x");
    let actions = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    assert!(actions.is_empty() && !v.is_text_owner());
    assert!(v.begin_folder_rename(1));
    let actions = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    assert!(actions.is_empty(), "unchanged");
    assert!(v.begin_folder_rename(1));
    for _ in 0..20 {
        v.handle(&press(Key::Backspace, Modifiers::NONE), d.doc(), &reg);
    }
    let actions = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    assert!(actions.is_empty(), "an empty name cancels");
    assert!(!v.begin_folder_rename(9), "no such folder");
}

#[test]
fn a_press_outside_the_field_commits_the_typed_name() {
    let (d, reg, mut v) = grouped();
    v.begin_folder_rename(1);
    type_text(&d, &reg, &mut v, "!");
    let away = v.track_header_center(&key("pad")).unwrap();
    let actions = click(&d, &reg, &mut v, away);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::RenameGroup {
            group: 1,
            name: "Folder 1!".into()
        }]
    );
    assert!(!v.is_text_owner());
}

#[test]
fn a_comment_is_typed_in_the_header_and_shows_a_mark_and_a_tooltip() {
    let (mut d, reg, mut v) = setup();
    assert!(v.comment_mark_center(&key("keys")).is_none());
    // No track selected: nothing to comment.
    assert_eq!(
        command(&d, &reg, &mut v, "sequencer.track_comment", None),
        CommandOutcome::Declined
    );
    v.set_selected_track(Some(key("keys")));
    do_command(&mut d, &reg, &mut v, "sequencer.track_comment", None);
    assert!(v.is_text_owner());
    type_text(&d, &reg, &mut v, "Play it softly");
    let actions = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::SetTrackComment {
            device: key("keys"),
            comment: "Play it softly".into()
        }]
    );
    apply(&mut d, &reg, &mut v, &actions);
    let mark = v.comment_mark_center(&key("keys")).expect("the mark");
    assert!(v.comment_tooltip().is_none(), "no hover yet");
    v.handle(
        &UiEvent::PointerMove {
            pos: mark,
            modifiers: Modifiers::NONE,
        },
        d.doc(),
        &reg,
    );
    assert_eq!(v.comment_tooltip(), Some("Play it softly"));
    // The tooltip is painted: the display list has its text.
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    assert!(dl.to_json().contains("Play it softly"));
    // Clicking the mark edits the comment again.
    click(&d, &reg, &mut v, mark);
    assert_eq!(v.org_edit_text(), Some("Play it softly"));
    v.cancel_org_edit();
    // Clearing it removes the mark.
    v.begin_comment_edit(&key("keys"));
    for _ in 0..20 {
        v.handle(&press(Key::Backspace, Modifiers::NONE), d.doc(), &reg);
    }
    let actions = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    apply(&mut d, &reg, &mut v, &actions);
    assert!(v.comment_mark_center(&key("keys")).is_none());
}

#[test]
fn a_comment_of_several_lines_is_left_alone_when_the_edit_changes_nothing() {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::SetTrackComment {
            device: key("keys"),
            comment: "one\ntwo".into(),
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    v.begin_comment_edit(&key("keys"));
    assert_eq!(v.org_edit_text(), Some("one / two"));
    let actions = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    assert!(actions.is_empty(), "Enter without a change keeps the lines");
}

// ------------------------------------------------------------------ colours

#[test]
fn track_colour_applies_to_the_selected_tracks_in_one_step_and_to_a_folder() {
    let (mut d, reg, mut v) = setup();
    assert_eq!(
        command(&d, &reg, &mut v, "sequencer.track_color", Some("red")),
        CommandOutcome::Declined,
        "nothing selected"
    );
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("keys"));
    do_command(&mut d, &reg, &mut v, "sequencer.track_color", Some("blue"));
    let song = &d.doc().song;
    assert_eq!(song.track_color(&key("bass")), Some([0x3f, 0x8f, 0xd2]));
    assert_eq!(song.track_color(&key("keys")), Some([0x3f, 0x8f, 0xd2]));
    assert_eq!(song.track_color(&key("lead")), None);
    d.undo(&reg).unwrap();
    assert!(d.doc().song.org.colors.is_empty(), "one undo step");
    // None clears it again.
    do_command(&mut d, &reg, &mut v, "sequencer.track_color", Some("green"));
    do_command(&mut d, &reg, &mut v, "sequencer.track_color", Some("none"));
    assert!(d.doc().song.org.colors.is_empty());
    // With a folder selected the colour is the folder's.
    let (mut d, reg, mut v) = grouped();
    v.set_selected_group(Some(1));
    do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_color",
        Some("purple"),
    );
    assert_eq!(
        d.doc().song.org.group(1).unwrap().color,
        Some([0x9a, 0x6a, 0xd0])
    );
    assert!(d.doc().song.org.colors.is_empty(), "the tracks keep theirs");
}

// ------------------------------------------------------------------ folded lanes

#[test]
fn the_lane_chevron_folds_a_lane_to_a_strip_and_unfolds_it() {
    let (mut d, reg, mut v) = setup();
    let open = v.lane_rect(&key("bass"), 1).unwrap().height();
    assert_eq!(open, 52.0);
    let chevron = v
        .lane_button_center(&key("bass"), CUTOFF, LaneButton::Fold)
        .unwrap();
    let actions = click(&d, &reg, &mut v, chevron);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::SetLaneFolded {
            device: key("bass"),
            param: CUTOFF.into(),
            folded: true
        }]
    );
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(
        v.lane_rect(&key("bass"), 1).unwrap().height(),
        FOLDED_LANE_HEIGHT
    );
    // Nothing in a folded lane is editable: a double-click does not draw a clip.
    let inside = v.lane_rect(&key("bass"), 1).unwrap().center();
    let actions = run(
        &d,
        &reg,
        &mut v,
        &[down(
            Point::new(inside.x + 500.0, inside.y),
            Modifiers::NONE,
            2,
        )],
    );
    assert!(song_cmds(&actions).is_empty(), "{actions:?}");
    let chevron = v
        .lane_button_center(&key("bass"), CUTOFF, LaneButton::Fold)
        .unwrap();
    let actions = click(&d, &reg, &mut v, chevron);
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(v.lane_rect(&key("bass"), 1).unwrap().height(), open);
}

#[test]
fn fold_lanes_folds_the_selected_tracks_or_every_track() {
    let (mut d, reg, mut v) = setup();
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.fold_lanes", Some("fold"));
    assert_eq!(commands(&actions).len(), 1, "one batch");
    assert!(d.doc().song.lane_folded(&key("bass"), CUTOFF));
    // Nothing left to fold: no command.
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.fold_lanes", Some("fold"));
    assert!(actions.is_empty());
    v.set_selected_track(Some(key("lead")));
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.fold_lanes", Some("unfold"));
    assert!(
        actions.is_empty(),
        "lead has no lanes; bass is not selected"
    );
    v.set_selected_track(Some(key("bass")));
    do_command(&mut d, &reg, &mut v, "sequencer.fold_lanes", Some("unfold"));
    assert!(!d.doc().song.lane_folded(&key("bass"), CUTOFF));
    assert_eq!(
        command(&d, &reg, &mut v, "sequencer.fold_lanes", Some("sideways")),
        CommandOutcome::Declined
    );
}

// ------------------------------------------------------------------ take rows

#[test]
fn opened_take_rows_are_kept_in_the_song_and_follow_undo() {
    let (mut d, reg, mut v) = setup();
    let take = |key_no: u8| notes_clip(50, 0, 2 * BAR, vec![n(0, PPQ, key_no, 0.8)]);
    d.apply(
        Command::Song(SongCommand::NewTakeGroup {
            device: key("keys"),
            lane: 0,
            takes: vec![take(60), take(62)],
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert!(!v.takes_open(1));
    let toggle = v.take_group_toggle_center(1).unwrap();
    let actions = click(&d, &reg, &mut v, toggle);
    assert!(v.takes_open(1));
    apply(&mut d, &reg, &mut v, &actions);
    assert!(d.doc().song.org.open_takes.contains(&1));
    // Undo closes them, redo opens them: the view follows the document.
    d.undo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert!(!v.takes_open(1));
    d.redo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert!(v.takes_open(1));
}

// ------------------------------------------------------------------ moving, ungrouping, deleting

#[test]
fn move_keys_move_the_selected_track_or_folder_and_stop_quietly_at_the_ends() {
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("keys")));
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.move_track_up", None);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::MoveTrack {
            device: key("keys"),
            down: false
        }]
    );
    assert_eq!(order(&d), ["bass", "keys", "lead", "pad"]);
    v.set_selected_track(Some(key("bass")));
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.move_track_up", None);
    assert!(actions.is_empty(), "the first track stays");
    v.set_selected_track(Some(key("pad")));
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.move_track_down", None);
    assert!(actions.is_empty(), "the last one too");
    // A selected folder moves with its tracks.
    let (mut d, reg, mut v) = grouped();
    v.set_selected_group(Some(1));
    let actions = do_command(&mut d, &reg, &mut v, "sequencer.move_track_down", None);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::MoveGroup {
            group: 1,
            down: true
        }]
    );
    assert_eq!(order(&d), ["keys", "bass", "lead", "pad"]);
    // With nothing selected the command is not ours.
    v.set_selected_group(None);
    v.set_selected_track(None);
    assert_eq!(
        command(&d, &reg, &mut v, "sequencer.move_track_up", None),
        CommandOutcome::Declined
    );
}

#[test]
fn ungroup_dissolves_the_selected_folder_or_the_one_of_the_selected_track() {
    let (mut d, reg, mut v) = grouped();
    v.set_selected_track(Some(key("lead")));
    assert!(v.org_menu().in_folder);
    do_command(&mut d, &reg, &mut v, "sequencer.ungroup_tracks", None);
    assert!(d.doc().song.org.groups.is_empty());
    assert_eq!(order(&d).len(), 4, "no track is deleted");
    assert_eq!(
        command(&d, &reg, &mut v, "sequencer.ungroup_tracks", None),
        CommandOutcome::Declined,
        "the track is in no folder now"
    );
}

#[test]
fn delete_on_a_selected_folder_ungroups_it_and_keeps_the_tracks() {
    let (mut d, reg, mut v) = grouped();
    v.set_selected_group(Some(1));
    let actions = do_command(&mut d, &reg, &mut v, "edit.delete", None);
    assert_eq!(song_cmds(&actions), [SongCommand::Ungroup { group: 1 }]);
    assert_eq!(order(&d).len(), 4);
    assert!(v.selected_group().is_none());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.org.groups.len(), 1);
}

#[test]
fn right_clicking_a_folder_row_selects_it_and_asks_for_its_menu() {
    let (d, reg, mut v) = grouped();
    let at = v.folder_header_center(1).unwrap();
    let actions = run(&d, &reg, &mut v, &[right_down(at), right_up(at)]);
    assert!(
        actions.iter().any(|a| matches!(
            a,
            UiAction::ContextMenu {
                target: ContextTarget::Folder,
                ..
            }
        )),
        "{actions:?}"
    );
    assert_eq!(v.selected_group(), Some(1));
    let menu = v.org_menu();
    assert!(menu.folder_selected && menu.in_folder && !menu.collapsed);
}

#[test]
fn the_menu_state_follows_the_selection() {
    let (mut d, reg, mut v) = grouped();
    v.set_selected_track(None);
    let none = v.org_menu();
    assert!(!none.can_group && !none.in_folder && !none.can_move_up);
    v.set_selected_track(Some(key("keys")));
    let menu = v.org_menu();
    assert!(menu.can_group && menu.track_selected && !menu.in_folder);
    assert!(menu.can_move_up && menu.can_move_down);
    v.set_selected_track(Some(key("pad")));
    assert!(!v.org_menu().can_move_down, "the last track");
    v.set_selected_group(Some(1));
    do_command(&mut d, &reg, &mut v, "sequencer.toggle_folder", None);
    assert!(v.org_menu().collapsed);
}

// ------------------------------------------------------------------ height presets

fn base_height(d: &Document, k: &str) -> Option<u16> {
    d.doc().song.track_heights.get(&key(k)).copied()
}

#[test]
fn height_presets_set_the_selected_tracks_or_all_of_them() {
    let (mut d, reg, mut v) = setup();
    do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_height",
        Some("large"),
    );
    for k in ["bass", "lead", "keys", "pad"] {
        assert_eq!(base_height(&d, k), Some(HEIGHT_LARGE), "{k}");
    }
    assert_eq!(v.track_height(&key("bass")), Some(f64::from(HEIGHT_LARGE)));
    // One undo step for all of them.
    d.undo(&reg).unwrap();
    assert!(d.doc().song.track_heights.is_empty());
    // Selected tracks only.
    v.set_selected_track(Some(key("lead")));
    v.toggle_selected_track(&key("pad"));
    do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_height",
        Some("small"),
    );
    assert_eq!(base_height(&d, "lead"), Some(HEIGHT_SMALL));
    assert_eq!(base_height(&d, "pad"), Some(HEIGHT_SMALL));
    assert_eq!(base_height(&d, "bass"), None);
    // Medium is the default: the entries go.
    do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_height",
        Some("medium"),
    );
    assert!(d.doc().song.track_heights.is_empty());
    // Nothing changes, nothing is sent.
    let actions = do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_height",
        Some("medium"),
    );
    assert!(actions.is_empty());
    assert_eq!(
        command(&d, &reg, &mut v, "sequencer.track_height", Some("huge")),
        CommandOutcome::Declined
    );
    assert_eq!(HeightPreset::from_arg("fit"), Some(HeightPreset::Fit));
    assert_eq!(HeightPreset::Large.arg(), "large");
}

#[test]
fn a_selected_folder_sizes_its_tracks() {
    let (mut d, reg, mut v) = grouped();
    v.set_selected_group(Some(1));
    do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_height",
        Some("small"),
    );
    assert_eq!(base_height(&d, "bass"), Some(HEIGHT_SMALL));
    assert_eq!(base_height(&d, "lead"), Some(HEIGHT_SMALL));
    assert_eq!(base_height(&d, "keys"), None);
}

#[test]
fn fit_shares_the_view_among_the_tracks_at_any_zoom() {
    for zoom in [1.0, 0.5, 1.5] {
        let (mut d, reg, mut v) = setup();
        v.set_track_zoom(zoom);
        do_command(&mut d, &reg, &mut v, "sequencer.track_height", Some("fit"));
        // The rows of the four tracks and the lane of `bass` fill the view down to the
        // scrollbar and the add rows.
        let first = v.lane_rect(&key("bass"), 0).unwrap();
        let last = v.lane_rect(&key("pad"), 0).unwrap();
        let heights: Vec<f64> = ["bass", "lead", "keys", "pad"]
            .iter()
            .map(|k| v.track_height(&key(k)).unwrap())
            .collect();
        assert!(
            heights.iter().all(|h| (h - heights[0]).abs() < 1.0),
            "{heights:?}"
        );
        assert!(last.y0 > first.y0);
        let tall = heights[0];
        assert!(tall > 24.0 - f64::EPSILON && tall <= 400.0, "{tall}");
        // Everything is shown at once: no scrolling is needed.
        assert_eq!(v.scroll_y(), 0.0);
    }
    // A single selected track gets what the others leave.
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("pad")));
    do_command(&mut d, &reg, &mut v, "sequencer.track_height", Some("fit"));
    let fitted = v.track_height(&key("pad")).unwrap();
    assert!(fitted > 60.0, "{fitted}: more than the default");
    assert!(
        v.lane_rect(&key("pad"), 0).unwrap().y1 <= ARR_BOUNDS.y1,
        "it ends inside the view"
    );
}

// ------------------------------------------------------------------ pictures

fn record() -> DisplayList {
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("lead"));
    do_command(&mut d, &reg, &mut v, "sequencer.group_tracks", None);
    for c in [
        SongCommand::RenameGroup {
            group: 1,
            name: "Rhythm".into(),
        },
        SongCommand::SetGroupColor {
            group: 1,
            color: Some([0xe8, 0x9a, 0x3c]),
        },
        SongCommand::SetTrackColor {
            device: key("keys"),
            color: Some([0x5a, 0xb8, 0x6a]),
        },
        SongCommand::SetTrackComment {
            device: key("keys"),
            comment: "Play it softly".into(),
        },
        SongCommand::SetLaneFolded {
            device: key("bass"),
            param: CUTOFF.into(),
            folded: true,
        },
        SongCommand::GroupTracks {
            devices: vec![key("lead")],
            name: "Inner".into(),
        },
    ] {
        d.apply(Command::Song(c), &reg).unwrap();
    }
    v.sync(d.doc(), &reg);
    v.set_selected_track(Some(key("keys")));
    v.set_playhead(BAR + PPQ);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    dl
}

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

#[test]
fn the_organised_arrangement_matches_its_golden() {
    check("arrangement_folders_1x.json", &record().to_json());
}

#[test]
#[ignore = "writes a preview image; run explicitly"]
fn preview_png() {
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("lead"));
    do_command(&mut d, &reg, &mut v, "sequencer.group_tracks", None);
    for c in [
        SongCommand::SetGroupColor {
            group: 1,
            color: Some([0xe8, 0x9a, 0x3c]),
        },
        SongCommand::SetTrackColor {
            device: key("keys"),
            color: Some([0x5a, 0xb8, 0x6a]),
        },
        SongCommand::SetTrackComment {
            device: key("keys"),
            comment: "Play it softly".into(),
        },
        SongCommand::SetLaneFolded {
            device: key("bass"),
            param: CUTOFF.into(),
            folded: true,
        },
    ] {
        d.apply(Command::Song(c), &reg).unwrap();
    }
    let mut text = TextEngine::new();
    v.set_scale(1.5);
    v.set_focused(true);
    v.set_selected_track(Some(key("keys")));
    let (w, h) = (
        (ARR_BOUNDS.width() * 1.5) as u16,
        (ARR_BOUNDS.height() * 1.5) as u16,
    );
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, d.doc(), &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/arrangement_folders.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

/// Keeps `DeviceKey` in the imports for the helpers above.
#[test]
fn device_keys_compare_by_name() {
    assert_eq!(key("a"), DeviceKey("a".into()));
}
