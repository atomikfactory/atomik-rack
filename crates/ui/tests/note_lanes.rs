//! Note lanes in the arrangement: the extra rows of a track with several note lanes, their
//! headers (label, mute, the chip on lane 0), the selected lane, clips dragged between lanes,
//! creating a clip on a lane, renaming a lane and the lane commands, run against a real
//! `Document` where the undo steps count.

mod support;

use atomik_model::{
    ClipContent, ClipId, Command, Document, Lane, LaneKind, RackDoc, SongCommand, WorkflowCommand,
};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, DisplayList, Key, LaneButton, LaneMenu, Modifiers, PointerButton, TrackButton,
    UiAction, UiEvent,
};
use support::song::*;
use support::*;

/// The song of the other arrangement tests with `keys` holding three note lanes (clips 3, 11 and
/// 12) and `bass` holding two note lanes (clips 1 and 5, and 13) before its cutoff lane.
fn doc3() -> RackDoc {
    let mut doc = song_doc();
    let keys = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == key("keys"))
        .unwrap();
    keys.lanes.push(Lane::notes().with_clips(vec![notes_clip(
        11,
        BAR,
        BAR,
        vec![n(0, PPQ, 64, 0.7)],
    )]));
    keys.lanes.push(Lane::notes().with_clips(vec![notes_clip(
        12,
        2 * BAR,
        BAR,
        vec![n(0, PPQ, 67, 0.7)],
    )]));
    let bass = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == key("bass"))
        .unwrap();
    bass.lanes.insert(
        1,
        Lane::notes().with_clips(vec![notes_clip(13, 0, BAR, vec![n(0, PPQ, 40, 0.7)])]),
    );
    doc.song_mut().next_clip_id = 14;
    doc
}

type Reg = atomik_device_sdk::Registry;

fn setup() -> (RackDoc, Reg, ArrangementView) {
    let (doc, reg) = (doc3(), registry());
    let mut v = arrangement(&doc, &reg);
    // Tall enough to show every row of the fixture.
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1200.0, 800.0));
    v.set_snap(Snap::Bar);
    (doc, reg, v)
}

fn run(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg, events: &[UiEvent]) -> Vec<UiAction> {
    events.iter().flat_map(|e| v.handle(e, doc, reg)).collect()
}

fn click(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg, at: Point) -> Vec<UiAction> {
    run(v, doc, reg, &[down(at), up(at)])
}

fn double_click(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg, at: Point) -> Vec<UiAction> {
    run(
        v,
        doc,
        reg,
        &[down_with(at, PointerButton::Primary, 2), up(at)],
    )
}

fn grab(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.x0 + 30.0, r.center().y + 6.0)
}

fn lane_mid(v: &ArrangementView, device: &str, lane: usize, x: f64) -> Point {
    Point::new(x, v.lane_rect(&key(device), lane).unwrap().center().y + 6.0)
}

fn painted(v: &mut ArrangementView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn typed(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg, text: &str) {
    for c in text.chars() {
        run(v, doc, reg, &[press(Key::Char(c), Modifiers::NONE)]);
    }
}

fn erase(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg, count: usize) {
    for _ in 0..count {
        run(v, doc, reg, &[press(Key::Backspace, Modifiers::NONE)]);
    }
}

// ------------------------------------------------------------------ rows

#[test]
fn rows_note_lanes_follow_lane_zero_and_come_before_the_automation_lanes() {
    let (_, _, v) = setup();
    let l0 = v.lane_rect(&key("bass"), 0).unwrap();
    let l1 = v.lane_rect(&key("bass"), 1).unwrap();
    let cutoff = v.lane_rect(&key("bass"), 2).unwrap();
    assert!(l0.y1 <= l1.y0 + 1e-9 && l1.y1 <= cutoff.y0 + 1e-9);
    assert_eq!(
        l1.height(),
        l0.height(),
        "extra note lanes use the track height"
    );
    assert!(cutoff.height() < l1.height());
}

#[test]
fn rows_extra_note_lanes_stay_when_the_track_collapses_its_automation_lanes() {
    let (doc, reg, mut v) = setup();
    let p = v
        .track_button_center(&key("bass"), TrackButton::Expand)
        .unwrap();
    click(&mut v, &doc, &reg, p);
    assert!(
        v.lane_rect(&key("bass"), 1).is_some(),
        "the note lane stays"
    );
    assert!(
        v.lane_rect(&key("bass"), 2).is_none(),
        "the automation lane folds away"
    );
}

#[test]
fn rows_a_track_height_change_applies_to_every_note_lane() {
    let (mut doc, reg, mut v) = setup();
    doc.song_mut().track_heights.insert(key("keys"), 90);
    v.sync(&doc, &reg);
    let h = |lane| v.lane_rect(&key("keys"), lane).unwrap().height();
    assert_eq!((h(0), h(1), h(2)), (90.0, 90.0, 90.0));
}

// ------------------------------------------------------------------ selected lane

#[test]
fn record_lane_is_zero_until_a_lane_is_picked() {
    let (_, _, v) = setup();
    assert_eq!(v.record_lane(&key("keys")), 0);
    assert_eq!(v.selected_lane(), None);
}

#[test]
fn record_lane_follows_a_click_on_a_lane_header() {
    let (doc, reg, mut v) = setup();
    let at = v.note_lane_header_center(&key("keys"), 2).unwrap();
    click(&mut v, &doc, &reg, at);
    assert_eq!(v.record_lane(&key("keys")), 2);
    assert_eq!(v.selected_lane(), Some((&key("keys"), 2)));
    assert_eq!(v.selected_track(), Some(&key("keys")));
}

#[test]
fn record_lane_follows_a_click_on_an_empty_note_lane() {
    let (doc, reg, mut v) = setup();
    let at = lane_mid(&v, "keys", 1, v.tick_to_x(3 * BAR));
    click(&mut v, &doc, &reg, at);
    assert_eq!(v.record_lane(&key("keys")), 1);
}

#[test]
fn record_lane_follows_a_click_on_a_clip() {
    let (doc, reg, mut v) = setup();
    click_clip(&mut v, &doc, &reg, 12);
    assert_eq!(v.record_lane(&key("keys")), 2);
    click_clip(&mut v, &doc, &reg, 3);
    assert_eq!(v.record_lane(&key("keys")), 0);
}

#[test]
fn record_lane_is_kept_per_track() {
    let (doc, reg, mut v) = setup();
    click_clip(&mut v, &doc, &reg, 12);
    click_clip(&mut v, &doc, &reg, 13);
    assert_eq!(v.record_lane(&key("keys")), 2);
    assert_eq!(v.record_lane(&key("bass")), 1);
}

#[test]
fn record_lane_is_clamped_when_the_lane_goes_away() {
    let (doc, reg, mut v) = setup();
    click_clip(&mut v, &doc, &reg, 12);
    let mut d = Document::new(doc);
    d.apply(
        Command::Song(SongCommand::RemoveNoteLane {
            device: key("keys"),
            lane: 2,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.record_lane(&key("keys")), 1);
    assert!(v.lane_rect(&key("keys"), 2).is_none());
}

#[test]
fn record_lane_of_an_unknown_track_is_zero() {
    let (_, _, v) = setup();
    assert_eq!(v.record_lane(&key("nobody")), 0);
}

// ------------------------------------------------------------------ headers and mute

#[test]
fn lane_mute_button_on_a_lane_header_mutes_that_lane() {
    let (doc, reg, mut v) = setup();
    let at = v
        .note_lane_button_center(&key("keys"), 1, LaneButton::Mute)
        .unwrap();
    let acts = click(&mut v, &doc, &reg, at);
    assert_eq!(
        song_commands(&acts),
        vec![SongCommand::SetLaneMuted {
            device: key("keys"),
            lane: 1,
            muted: true
        }]
    );
}

#[test]
fn lane_mute_button_on_a_muted_lane_unmutes_it() {
    let (mut doc, reg, mut v) = setup();
    doc.song_mut().tracks[2].lanes[1].muted = true;
    v.sync(&doc, &reg);
    let at = v
        .note_lane_button_center(&key("keys"), 1, LaneButton::Mute)
        .unwrap();
    let acts = click(&mut v, &doc, &reg, at);
    assert_eq!(
        song_commands(&acts),
        vec![SongCommand::SetLaneMuted {
            device: key("keys"),
            lane: 1,
            muted: false
        }]
    );
}

#[test]
fn lane_chip_on_lane_zero_shows_with_several_lanes_and_mutes_lane_zero() {
    let (doc, reg, mut v) = setup();
    let at = v
        .note_lane_button_center(&key("keys"), 0, LaneButton::Mute)
        .expect("a chip");
    let acts = click(&mut v, &doc, &reg, at);
    assert_eq!(
        song_commands(&acts),
        vec![SongCommand::SetLaneMuted {
            device: key("keys"),
            lane: 0,
            muted: true
        }]
    );
}

#[test]
fn lane_chip_is_absent_with_one_note_lane() {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    assert!(
        v.note_lane_button_center(&key("keys"), 0, LaneButton::Mute)
            .is_none()
    );
}

#[test]
fn lane_chip_is_hidden_in_a_compact_header() {
    let mut doc = doc3();
    doc.song_mut().track_heights.insert(key("keys"), 40);
    let reg = registry();
    let v = arrangement(&doc, &reg);
    assert!(
        v.note_lane_button_center(&key("keys"), 0, LaneButton::Mute)
            .is_none()
    );
    assert!(
        v.note_lane_button_center(&key("keys"), 1, LaneButton::Mute)
            .is_some(),
        "lane headers keep their button"
    );
}

#[test]
fn lane_mute_paints_the_clips_of_a_muted_lane_dimmed() {
    let (mut doc, reg, mut v) = setup();
    let before = painted(&mut v, &doc);
    doc.song_mut().tracks[2].lanes[1].muted = true;
    v.sync(&doc, &reg);
    let after = painted(&mut v, &doc);
    assert_ne!(before, after);
}

#[test]
fn lane_selection_is_marked_in_the_painting() {
    let (doc, reg, mut v) = setup();
    let before = painted(&mut v, &doc);
    let at = v.note_lane_header_center(&key("keys"), 1).unwrap();
    click(&mut v, &doc, &reg, at);
    v.set_selected_track(None);
    v.set_selected_lane(&key("keys"), 1);
    let after = painted(&mut v, &doc);
    assert_ne!(before, after);
}

// ------------------------------------------------------------------ clips on lanes

#[test]
fn creating_a_clip_by_double_click_on_a_note_lane_makes_a_notes_clip() {
    let (doc, reg, mut v) = setup();
    let at = lane_mid(&v, "keys", 2, v.tick_to_x(5 * BAR));
    let acts = double_click(&mut v, &doc, &reg, at);
    let cmds = song_commands(&acts);
    assert_eq!(cmds.len(), 1, "{acts:?}");
    let SongCommand::AddClip { device, lane, clip } = &cmds[0] else {
        panic!("{cmds:?}");
    };
    assert_eq!((device, *lane), (&key("keys"), 2));
    assert!(matches!(clip.content, ClipContent::Notes(_)));
}

#[test]
fn drawing_a_clip_on_a_note_lane_makes_a_notes_clip() {
    let (doc, reg, mut v) = setup();
    let y = v.lane_rect(&key("keys"), 1).unwrap().center().y;
    let (a, b) = (v.tick_to_x(5 * BAR), v.tick_to_x(7 * BAR));
    let acts = run(
        &mut v,
        &doc,
        &reg,
        &[
            down(Point::new(a, y)),
            mv(Point::new((a + b) / 2.0, y)),
            mv(Point::new(b, y)),
            up(Point::new(b, y)),
        ],
    );
    let cmds = song_commands(&acts);
    let Some(SongCommand::AddClip { lane, clip, .. }) = cmds.first() else {
        panic!("{cmds:?}");
    };
    assert_eq!(*lane, 1);
    assert!(matches!(clip.content, ClipContent::Notes(_)));
}

#[test]
fn double_clicking_a_clip_on_the_third_lane_opens_the_piano_roll() {
    let (doc, reg, mut v) = setup();
    let at = grab(&v, 12);
    let acts = double_click(&mut v, &doc, &reg, at);
    assert!(
        acts.contains(&UiAction::OpenPianoRoll(ClipId(12))),
        "{acts:?}"
    );
}

/// `(clip, track, lane, start)` of every move among `actions`.
fn moves(actions: &[UiAction]) -> Vec<(u64, String, Option<usize>, i64)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Workflow(WorkflowCommand::MoveClips { moves, .. }) => Some(moves),
            _ => None,
        })
        .flatten()
        .map(|m| (m.id.0, m.track.0, m.lane, m.start))
        .collect()
}

fn drag(
    v: &mut ArrangementView,
    doc: &RackDoc,
    reg: &Reg,
    from: Point,
    to: Point,
) -> Vec<UiAction> {
    let mid = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    run(v, doc, reg, &[down(from), mv(mid), mv(to), up(to)])
}

#[test]
fn drag_a_note_clip_to_another_lane_of_its_track_moves_it_there() {
    let (doc, reg, mut v) = setup();
    let from = grab(&v, 3);
    let to = lane_mid(&v, "keys", 2, from.x);
    let acts = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(moves(&acts), vec![(3, "keys".to_owned(), Some(2), 0)]);
    let mut d = Document::new(doc);
    for a in &acts {
        match a {
            UiAction::Command(c) => {
                d.apply(c.clone(), &reg).unwrap();
            }
            UiAction::EndMerge => d.end_merge(),
            _ => {}
        }
    }
    let keys = d.doc().song.track(&key("keys")).unwrap();
    assert!(keys.lanes[2].clips.iter().any(|c| c.id == ClipId(3)));
    assert!(keys.lanes[0].clips.is_empty());
}

#[test]
fn drag_a_note_clip_to_another_track_shifts_its_lane_by_the_rows() {
    let (doc, reg, mut v) = setup();
    // Lane 1 of bass to lane 2 of keys: one lane down from the grab.
    let from = grab(&v, 13);
    let to = lane_mid(&v, "keys", 2, from.x);
    let acts = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(moves(&acts), vec![(13, "keys".to_owned(), Some(2), 0)]);
}

#[test]
fn drag_a_note_clip_to_a_track_with_fewer_lanes_clamps_the_lane() {
    let (doc, reg, mut v) = setup();
    let from = grab(&v, 12);
    let to = lane_mid(&v, "lead", 0, from.x);
    let acts = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        moves(&acts),
        vec![(12, "lead".to_owned(), Some(0), 2 * BAR)]
    );
}

#[test]
fn drag_a_note_clip_over_an_automation_row_of_its_own_track_moves_nothing() {
    let (doc, reg, mut v) = setup();
    let from = grab(&v, 1);
    let to = lane_mid(&v, "bass", 2, from.x);
    let acts = drag(&mut v, &doc, &reg, from, to);
    assert!(moves(&acts).is_empty(), "{acts:?}");
}

#[test]
fn drag_drop_placeholder_highlights_the_target_lane() {
    let (doc, reg, mut v) = setup();
    let from = grab(&v, 3);
    let plain = painted(&mut v, &doc);
    let to = lane_mid(&v, "keys", 2, from.x);
    run(
        &mut v,
        &doc,
        &reg,
        &[
            down(from),
            mv(Point::new(from.x + 5.0, from.y + 5.0)),
            mv(to),
        ],
    );
    assert_ne!(plain, painted(&mut v, &doc));
}

#[test]
fn drag_an_automation_clip_keeps_its_kind_rule() {
    let (doc, reg, mut v) = setup();
    // The cutoff clip (4) onto the keys track: it has no cutoff lane there, so nothing moves.
    let from = grab(&v, 4);
    let to = lane_mid(&v, "keys", 1, from.x);
    let acts = drag(&mut v, &doc, &reg, from, to);
    assert!(moves(&acts).is_empty(), "{acts:?}");
}

// ------------------------------------------------------------------ rename

#[test]
fn rename_double_click_on_a_lane_name_starts_the_inline_editor_and_enter_renames() {
    let (doc, reg, mut v) = setup();
    let at = v.note_lane_header_center(&key("keys"), 1).unwrap();
    double_click(&mut v, &doc, &reg, at);
    assert_eq!(v.org_edit_text(), Some("Notes 2"));
    erase(&mut v, &doc, &reg, "Notes 2".len());
    typed(&mut v, &doc, &reg, "Strings");
    let acts = run(&mut v, &doc, &reg, &[press(Key::Enter, Modifiers::NONE)]);
    assert_eq!(
        song_commands(&acts),
        vec![SongCommand::RenameLane {
            device: key("keys"),
            lane: 1,
            name: Some("Strings".to_owned())
        }]
    );
    assert_eq!(v.org_edit_text(), None);
}

#[test]
fn rename_unchanged_or_cancelled_emits_nothing() {
    let (doc, reg, mut v) = setup();
    assert!(v.begin_lane_rename(&key("keys"), 2));
    let acts = run(&mut v, &doc, &reg, &[press(Key::Enter, Modifiers::NONE)]);
    assert!(song_commands(&acts).is_empty());
    assert!(v.begin_lane_rename(&key("keys"), 2));
    typed(&mut v, &doc, &reg, "x");
    let acts = run(&mut v, &doc, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert!(song_commands(&acts).is_empty());
}

#[test]
fn rename_a_blank_name_resets_a_named_lane() {
    let (mut doc, reg, mut v) = setup();
    doc.song_mut().tracks[2].lanes[1].name = Some("Strings".into());
    v.sync(&doc, &reg);
    assert!(v.begin_lane_rename(&key("keys"), 1));
    assert_eq!(v.org_edit_text(), Some("Strings"));
    erase(&mut v, &doc, &reg, "Strings".len());
    let acts = run(&mut v, &doc, &reg, &[press(Key::Enter, Modifiers::NONE)]);
    assert_eq!(
        song_commands(&acts),
        vec![SongCommand::RenameLane {
            device: key("keys"),
            lane: 1,
            name: None
        }]
    );
}

#[test]
fn rename_refuses_an_automation_lane_or_a_missing_one() {
    let (_, _, mut v) = setup();
    assert!(!v.begin_lane_rename(&key("bass"), 2));
    assert!(!v.begin_lane_rename(&key("keys"), 9));
    assert!(!v.begin_lane_rename(&key("nobody"), 0));
}

#[test]
fn rename_editor_closes_when_its_lane_goes_away() {
    let (doc, reg, mut v) = setup();
    assert!(v.begin_lane_rename(&key("keys"), 2));
    let mut d = Document::new(doc);
    d.apply(
        Command::Song(SongCommand::RemoveNoteLane {
            device: key("keys"),
            lane: 2,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.org_edit_text(), None);
}

// ------------------------------------------------------------------ lane commands

fn select_lane(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg, lane: usize) {
    let at = v.note_lane_header_center(&key("keys"), lane).unwrap();
    click(v, doc, reg, at);
}

/// The song commands of a handled command, `None` when it declined.
fn command(
    v: &mut ArrangementView,
    doc: &RackDoc,
    reg: &Reg,
    id: &str,
) -> Option<Vec<SongCommand>> {
    match v.command(id, None, doc, reg) {
        CommandOutcome::Handled(a) => Some(song_commands(&a)),
        _ => None,
    }
}

#[test]
fn commands_add_note_lane_targets_the_selected_track() {
    let (doc, reg, mut v) = setup();
    assert_eq!(command(&mut v, &doc, &reg, "sequencer.add_note_lane"), None);
    select_lane(&mut v, &doc, &reg, 1);
    assert_eq!(
        command(&mut v, &doc, &reg, "sequencer.add_note_lane"),
        Some(vec![SongCommand::AddNoteLane {
            device: key("keys"),
            name: None
        }])
    );
}

#[test]
fn commands_delete_note_lane_removes_the_selected_lane_and_needs_two() {
    let (doc, reg, mut v) = setup();
    select_lane(&mut v, &doc, &reg, 1);
    assert_eq!(
        command(&mut v, &doc, &reg, "sequencer.delete_note_lane"),
        Some(vec![SongCommand::RemoveNoteLane {
            device: key("keys"),
            lane: 1
        }])
    );
    let plain = song_doc();
    let mut one = arrangement(&plain, &reg);
    one.set_selected_track(Some(key("keys")));
    assert_eq!(
        command(&mut one, &plain, &reg, "sequencer.delete_note_lane"),
        None
    );
}

#[test]
fn commands_toggle_lane_mute_flips_the_selected_lane() {
    let (doc, reg, mut v) = setup();
    select_lane(&mut v, &doc, &reg, 2);
    assert_eq!(
        command(&mut v, &doc, &reg, "sequencer.toggle_lane_mute"),
        Some(vec![SongCommand::SetLaneMuted {
            device: key("keys"),
            lane: 2,
            muted: true
        }])
    );
}

#[test]
fn commands_rename_note_lane_opens_the_editor_on_the_selected_lane() {
    let (doc, reg, mut v) = setup();
    select_lane(&mut v, &doc, &reg, 2);
    assert_eq!(
        command(&mut v, &doc, &reg, "sequencer.rename_note_lane"),
        Some(Vec::new())
    );
    assert_eq!(v.org_edit_text(), Some("Notes 3"));
}

#[test]
fn commands_add_note_lane_selects_the_new_lane_once_it_exists() {
    let (doc, reg, mut v) = setup();
    select_lane(&mut v, &doc, &reg, 0);
    let _ = command(&mut v, &doc, &reg, "sequencer.add_note_lane");
    let mut d = Document::new(doc);
    d.apply(
        Command::Song(SongCommand::AddNoteLane {
            device: key("keys"),
            name: None,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.record_lane(&key("keys")), 3);
}

#[test]
fn lane_menu_reports_what_the_selected_lane_allows() {
    let (doc, reg, mut v) = setup();
    assert_eq!(v.lane_menu(), LaneMenu::default());
    select_lane(&mut v, &doc, &reg, 1);
    let m = v.lane_menu();
    assert!(m.can_add && m.can_rename && m.can_delete && !m.muted);
    let plain = song_doc();
    let mut one = arrangement(&plain, &reg);
    one.set_selected_track(Some(key("keys")));
    let m = one.lane_menu();
    assert!(m.can_add && m.can_rename && !m.can_delete);
}

#[test]
fn fixture_has_contiguous_note_lanes_before_automation() {
    let doc = doc3();
    let bass = doc.song.track(&key("bass")).unwrap();
    assert_eq!(bass.note_lane_count(), 2);
    assert!(matches!(bass.lanes[2].kind, LaneKind::Automation(_)));
}

fn click_clip(v: &mut ArrangementView, doc: &RackDoc, reg: &Reg, id: u64) {
    let at = grab(v, id);
    click(v, doc, reg, at);
}

#[test]
fn rows_a_take_group_on_a_second_lane_opens_its_rows_under_that_lane() {
    let (doc, reg, mut v) = setup();
    let mut d = Document::new(doc);
    let take = |key: u8| notes_clip(50, 4 * BAR, BAR, vec![n(0, PPQ, key, 0.8)]);
    d.apply(
        Command::Song(SongCommand::NewTakeGroup {
            device: key("keys"),
            lane: 1,
            takes: vec![take(70), take(72)],
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    let group = d.doc().song.take_groups().next().unwrap().2.id;
    let strip = v.take_group_toggle_center(group).expect("a strip");
    let lane1 = v.lane_rect(&key("keys"), 1).unwrap();
    let lane2 = v.lane_rect(&key("keys"), 2).unwrap();
    assert!(
        strip.y > lane1.y1 && strip.y < lane2.y0,
        "the strip sits between lanes 1 and 2"
    );
    // Lane 0 keeps its row directly above lane 1.
    assert!(v.lane_rect(&key("keys"), 0).unwrap().y1 <= lane1.y0 + 1e-9);
}
