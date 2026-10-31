//! The arrangement's part: track selection with
//! the Up and Down keys, join, duplicate and delete of clips, tracks and automation points as
//! workflow commands, and the context-menu requests of clips, lanes and track headers.

mod support;

use atomik_model::{ClipId, Command, SongCommand, WorkflowCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::kurbo::Point;
use atomik_ui::{ArrangementView, ContextTarget, Modifiers, PointerButton, UiAction, UiEvent};
use support::song::*;
use support::*;

fn setup() -> (
    ArrangementView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    (v, doc, reg)
}

fn select(v: &mut ArrangementView, ids: &[u64]) {
    let ids: Vec<ClipId> = ids.iter().map(|i| ClipId(*i)).collect();
    v.set_selected_clips(&ids);
}

fn press(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    at: Point,
    button: PointerButton,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: at,
            button,
            modifiers,
            clicks: 1,
        },
        doc,
        reg,
    );
    out.extend(v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button,
            modifiers,
        },
        doc,
        reg,
    ));
    out
}

fn dev(v: &ArrangementView) -> Option<String> {
    v.selected_track().map(|k| k.0.clone())
}

// ------------------------------------------------------------------ track selection

#[test]
fn a_header_click_selects_the_track_and_clears_the_clip_selection() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1, 3]);
    let at = v.track_header_center(&key("lead")).unwrap();
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert_eq!(dev(&v).as_deref(), Some("lead"));
    assert!(v.selected_clips().is_empty());
    assert!(commands(&a).is_empty(), "view state only: {a:?}");
}

#[test]
fn clicking_a_clip_drops_the_track_selection() {
    let (mut v, doc, reg) = setup();
    let h = v.track_header_center(&key("lead")).unwrap();
    press(
        &mut v,
        &doc,
        &reg,
        h,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    let c = v.clip_rect(ClipId(3)).unwrap().center();
    press(
        &mut v,
        &doc,
        &reg,
        c,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert_eq!(dev(&v), None);
    assert_eq!(v.selected_clips(), vec![ClipId(3)]);
}

#[test]
fn up_and_down_move_the_track_selection() {
    let (mut v, doc, reg) = setup();
    // Nothing selected: Down starts at the first track, Up at the last.
    assert!(
        v.command("sequencer.select_next_track", None, &doc, &reg)
            .is_taken()
    );
    assert_eq!(dev(&v).as_deref(), Some("bass"));
    v.command("sequencer.select_next_track", None, &doc, &reg);
    assert_eq!(dev(&v).as_deref(), Some("lead"));
    v.command("sequencer.select_next_track", None, &doc, &reg);
    v.command("sequencer.select_next_track", None, &doc, &reg);
    assert_eq!(dev(&v).as_deref(), Some("keys"), "stops at the end");
    v.command("sequencer.select_prev_track", None, &doc, &reg);
    assert_eq!(dev(&v).as_deref(), Some("lead"));
    v.set_selected_track(None);
    v.command("sequencer.select_prev_track", None, &doc, &reg);
    assert_eq!(dev(&v).as_deref(), Some("keys"));
}

#[test]
fn the_track_keys_start_from_the_track_of_the_selected_clip() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[2]);
    v.command("sequencer.select_next_track", None, &doc, &reg);
    assert_eq!(dev(&v).as_deref(), Some("keys"));
    assert!(
        v.selected_clips().is_empty(),
        "the clip selection gives way"
    );
}

// ------------------------------------------------------------------ join, duplicate, delete

#[test]
fn join_emits_one_command_for_the_selected_clips_in_time_order() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[5, 1]);
    assert_eq!(
        v.command("sequencer.join", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::JoinClips {
                ids: vec![ClipId(1), ClipId(5)]
            }
        ))])
    );
    select(&mut v, &[1]);
    assert!(matches!(
        v.command("sequencer.join", None, &doc, &reg),
        CommandOutcome::NotApplicable(_)
    ));
    select(&mut v, &[]);
    assert!(matches!(
        v.command("sequencer.join", None, &doc, &reg),
        CommandOutcome::NotApplicable(_)
    ));
}

#[test]
fn duplicate_copies_the_whole_selection_in_one_command() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[3, 1]);
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateClips {
                ids: vec![ClipId(1), ClipId(3)]
            }
        ))])
    );
}

#[test]
fn duplicate_with_a_selected_track_duplicates_the_track() {
    let (mut v, doc, reg) = setup();
    v.set_selected_track(Some(key("lead")));
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateTracks {
                devices: vec![key("lead")],
                place: None,
            }
        ))])
    );
}

#[test]
fn duplicate_with_several_selected_tracks_copies_them_in_one_command() {
    let (mut v, doc, reg) = setup();
    v.set_selected_track(Some(key("lead")));
    v.toggle_selected_track(&key("bass"));
    let selected = v.selected_tracks();
    assert_eq!(selected.len(), 2);
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateTracks {
                devices: selected,
                place: None,
            }
        ))])
    );
}

#[test]
fn delete_removes_several_clips_in_one_undo_step_and_a_track_by_itself() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1]);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Song(
            SongCommand::RemoveClip { id: ClipId(1) }
        ))])
    );
    select(&mut v, &[1, 3]);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::RemoveClips {
                ids: vec![ClipId(1), ClipId(3)]
            }
        ))])
    );
    select(&mut v, &[]);
    v.set_selected_track(Some(key("keys")));
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Song(
            SongCommand::RemoveTrack {
                device: key("keys")
            }
        ))])
    );
}

// ------------------------------------------------------------------ automation points

fn point_at(v: &ArrangementView, id: u64, index: usize) -> Point {
    v.automation_point_pos(ClipId(id), index).unwrap()
}

#[test]
fn a_point_click_selects_the_point_and_ctrl_click_adds_one() {
    let (mut v, doc, reg) = setup();
    let p0 = point_at(&v, 4, 1);
    press(
        &mut v,
        &doc,
        &reg,
        p0,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert_eq!(v.selected_points(), Some((ClipId(4), vec![1])));
    let p1 = point_at(&v, 4, 2);
    press(
        &mut v,
        &doc,
        &reg,
        p1,
        PointerButton::Primary,
        Modifiers::CTRL,
    );
    assert_eq!(v.selected_points(), Some((ClipId(4), vec![1, 2])));
    // Ctrl-click on a selected point takes it out again.
    press(
        &mut v,
        &doc,
        &reg,
        p1,
        PointerButton::Primary,
        Modifiers::CTRL,
    );
    assert_eq!(v.selected_points(), Some((ClipId(4), vec![1])));
}

#[test]
fn duplicate_and_delete_work_on_the_selected_points() {
    let (mut v, doc, reg) = setup();
    v.set_selected_points(ClipId(4), &[1, 2]);
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicatePoints {
                clip: ClipId(4),
                indices: vec![1, 2]
            }
        ))])
    );
    let out = v.command("edit.delete", None, &doc, &reg);
    let [UiAction::Command(Command::Song(SongCommand::SetClipContent { id, content, .. }))] =
        out.actions()
    else {
        panic!("{out:?}")
    };
    assert_eq!(*id, ClipId(4));
    let atomik_model::ClipContent::Automation(points) = content else {
        panic!()
    };
    assert_eq!(points.len(), 2, "two of the four points are gone");
    assert!(v.selected_points().is_none());
}

#[test]
fn clicking_a_clip_or_the_empty_lane_clears_the_point_selection() {
    let (mut v, doc, reg) = setup();
    v.set_selected_points(ClipId(4), &[1]);
    let c = v.clip_rect(ClipId(3)).unwrap().center();
    press(
        &mut v,
        &doc,
        &reg,
        c,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert!(v.selected_points().is_none());
}

// ------------------------------------------------------------------ context menus

#[test]
fn right_click_on_a_clip_selects_it_and_asks_for_the_clip_menu() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1]);
    let at = v.clip_rect(ClipId(3)).unwrap().center();
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert_eq!(v.selected_clips(), vec![ClipId(3)]);
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::Clip,
        at
    }));
    assert!(commands(&a).is_empty());
}

#[test]
fn right_click_on_a_selected_clip_keeps_the_selection() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1, 3]);
    let at = v.clip_rect(ClipId(3)).unwrap().center();
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert_eq!(v.selected_clips(), vec![ClipId(1), ClipId(3)]);
    assert!(a.iter().any(|x| matches!(
        x,
        UiAction::ContextMenu {
            target: ContextTarget::Clip,
            ..
        }
    )));
}

#[test]
fn right_click_on_an_empty_lane_asks_for_the_lane_menu() {
    let (mut v, doc, reg) = setup();
    let lane = v.lane_rect(&key("lead"), 0).unwrap();
    // Past the end of the lead clip (bars 1 to 4).
    let at = Point::new(v.tick_to_x(5 * BAR), lane.center().y);
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::Lane,
        at
    }));
}

#[test]
fn right_click_on_a_track_header_selects_the_track_and_asks_for_its_menu() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1]);
    let at = v.track_header_center(&key("keys")).unwrap();
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert_eq!(dev(&v).as_deref(), Some("keys"));
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::TrackHeader,
        at
    }));
}

#[test]
fn right_click_on_an_automation_point_still_deletes_it() {
    let (mut v, doc, reg) = setup();
    let p = point_at(&v, 4, 1);
    let a = press(
        &mut v,
        &doc,
        &reg,
        p,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert!(
        matches!(
            song_commands(&a).as_slice(),
            [SongCommand::SetClipContent { .. }]
        ),
        "{a:?}"
    );
    assert!(!a.iter().any(|x| matches!(x, UiAction::ContextMenu { .. })));
}

// ------------------------------------------------------------------ additions

fn escape(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> Vec<UiAction> {
    v.handle(
        &UiEvent::Key {
            key: atomik_ui::Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        doc,
        reg,
    )
}

#[test]
fn escape_clears_the_clip_track_and_point_selection() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1, 3]);
    escape(&mut v, &doc, &reg);
    assert!(v.selected_clips().is_empty());
    let at = v.track_header_center(&key("lead")).unwrap();
    press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert!(v.selected_track().is_some());
    escape(&mut v, &doc, &reg);
    assert!(v.selected_track().is_none());
    let p0 = point_at(&v, 4, 1);
    press(
        &mut v,
        &doc,
        &reg,
        p0,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert!(v.selected_points().is_some());
    escape(&mut v, &doc, &reg);
    assert!(v.selected_points().is_none());
}

#[test]
fn a_selection_command_with_nothing_selected_declines_and_emits_nothing() {
    let (mut v, doc, reg) = setup();
    for id in ["edit.delete", "edit.duplicate", "sequencer.join"] {
        let o = v.command(id, None, &doc, &reg);
        assert!(
            !matches!(o, CommandOutcome::Handled(ref a) if !a.is_empty()),
            "{id} must not emit with nothing selected"
        );
    }
}

// ------------------------------------------------------------------ restructuring commands

fn workflow(c: WorkflowCommand) -> CommandOutcome {
    CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(c))])
}

#[test]
fn insert_and_remove_bars_act_on_the_loop_range() {
    let (mut v, doc, reg) = setup();
    let (a, z) = doc.song.loop_range;
    assert_eq!(
        v.command("sequencer.insert_bars", None, &doc, &reg),
        workflow(WorkflowCommand::InsertTime {
            at: a,
            length: z - a
        })
    );
    assert_eq!(
        v.command("sequencer.remove_bars", None, &doc, &reg),
        workflow(WorkflowCommand::RemoveTime {
            at: a,
            length: z - a
        })
    );
}

#[test]
fn reverse_scale_and_chop_work_on_the_selected_clips_in_time_order() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[5, 1]);
    let ids = vec![ClipId(1), ClipId(5)];
    assert_eq!(
        v.command("sequencer.reverse_clips", Some("bars"), &doc, &reg),
        workflow(WorkflowCommand::ReverseClips {
            ids: ids.clone(),
            graphical: true
        })
    );
    assert_eq!(
        v.command("sequencer.reverse_clips", Some("starts"), &doc, &reg),
        workflow(WorkflowCommand::ReverseClips {
            ids: ids.clone(),
            graphical: false
        })
    );
    assert_eq!(
        v.command("sequencer.scale_tempo", Some("double"), &doc, &reg),
        workflow(WorkflowCommand::ScaleTempo {
            ids: ids.clone(),
            percent: 200.0
        })
    );
    assert_eq!(
        v.command("sequencer.scale_tempo", Some("halve"), &doc, &reg),
        workflow(WorkflowCommand::ScaleTempo {
            ids: ids.clone(),
            percent: 50.0
        })
    );
    let first = doc.song.clip(ClipId(1)).unwrap().2.start;
    let bar = doc.song.bar_ticks_at(first);
    assert_eq!(
        v.command("sequencer.chop_clips", Some("bar"), &doc, &reg),
        workflow(WorkflowCommand::ChopClips {
            ids: ids.clone(),
            length: bar
        })
    );
    assert_eq!(
        v.command("sequencer.chop_clips", Some("half_bar"), &doc, &reg),
        workflow(WorkflowCommand::ChopClips {
            ids,
            length: bar / 2
        })
    );
    // Nothing selected: a message, not a command; an unknown argument is declined.
    select(&mut v, &[]);
    for (id, arg) in [
        ("sequencer.reverse_clips", "bars"),
        ("sequencer.scale_tempo", "double"),
        ("sequencer.chop_clips", "bar"),
    ] {
        assert!(matches!(
            v.command(id, Some(arg), &doc, &reg),
            CommandOutcome::NotApplicable(_)
        ));
    }
    select(&mut v, &[1]);
    assert_eq!(
        v.command("sequencer.scale_tempo", Some("nonsense"), &doc, &reg),
        CommandOutcome::Declined
    );
}
