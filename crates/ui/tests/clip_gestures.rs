//! The clip and note gestures of the arrangement and the piano roll: left-edge trim, multi-clip
//! resize, the razor drag, nudge, relative snap, constrain and copy-drag.

mod support;

use atomik_model::{ClipContent, ClipId, Command, Note, SongCommand, WorkflowCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, Modifiers, SnapMode, Tool, UiAction};
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

fn left_edge(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.x0 + 2.0, r.center().y + 6.0)
}

fn bar_px(v: &ArrangementView) -> f64 {
    v.tick_to_x(BAR) - v.tick_to_x(0)
}

fn lane_point(v: &ArrangementView, dev: &str, tick: i64) -> Point {
    let r = v.lane_rect(&key(dev), 0).unwrap();
    Point::new(v.tick_to_x(tick), r.center().y + 6.0)
}

fn trims(actions: &[UiAction]) -> Vec<(Vec<ClipId>, i64, Option<u64>)> {
    workflow_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            WorkflowCommand::TrimClips {
                ids,
                start_delta,
                merge,
            } => Some((ids, start_delta, merge)),
            _ => None,
        })
        .collect()
}

fn starts(actions: &[UiAction]) -> Vec<(ClipId, i64)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipGeometry { id, start, .. } => Some((id, start)),
            _ => None,
        })
        .collect()
}

fn lengths(actions: &[UiAction]) -> Vec<(ClipId, i64)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipGeometry { id, length, .. } => Some((id, length)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- trim

#[test]
fn dragging_the_left_edge_of_a_note_clip_trims_its_start() {
    let (mut v, doc, reg) = setup();
    let bp = bar_px(&v);
    v.set_snap(Snap::Bar);
    let p = left_edge(&v, 1);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(bp * 1.2, 0.0))],
    );
    let t = trims(&a);
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].0.clone(), t[0].1), (vec![ClipId(1)], BAR));
    assert!(t[0].2.is_some(), "the drag shares one merge group");
    // Back to the start: the next move undoes the difference, not the whole.
    let a = run(|e| v.handle(e, &doc, &reg), &[mv(p), up(p)]);
    assert_eq!(trims(&a)[0].1, -BAR);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn trimming_stops_at_tick_zero_and_leaves_the_minimum_length() {
    let (mut v, doc, reg) = setup();
    let bp = bar_px(&v);
    v.set_snap(Snap::Bar);
    let p = left_edge(&v, 2);
    // Clip 2 starts at bar 2: dragging left by five bars grows it to tick 0 at most.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p - Vec2::new(bp * 5.0, 0.0))],
    );
    assert_eq!(trims(&a)[0].1, -BAR);
    let _ = v.handle(&up(p), &doc, &reg);
    // Dragging right by far more than the clip leaves one step.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(bp * 9.0, 0.0))],
    );
    assert_eq!(trims(&a)[0].1, 2 * BAR);
}

#[test]
fn trimming_moves_every_selected_clip_that_is_not_audio_in_one_command() {
    let (mut v, doc, reg) = setup();
    let bp = bar_px(&v);
    v.set_snap(Snap::Bar);
    select(&mut v, &[1, 3]);
    let p = left_edge(&v, 3);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(bp * 1.1, 0.0))],
    );
    let t = trims(&a);
    assert_eq!(t.len(), 1, "one command for the group");
    assert_eq!(t[0].0, vec![ClipId(3), ClipId(1)]);
    assert_eq!(t[0].1, BAR);
}

// ---------------------------------------------------------------- multi-clip resize

#[test]
fn resizing_the_right_edge_resizes_every_selected_clip_by_the_same_amount() {
    let (mut v, doc, reg) = setup();
    let bp = bar_px(&v);
    v.set_snap(Snap::Bar);
    select(&mut v, &[1, 3]);
    let r = v.clip_rect(ClipId(1)).unwrap();
    let edge = Point::new(r.x1 - 2.0, r.center().y + 6.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(edge), mv(edge + Vec2::new(bp * 1.2, 0.0))],
    );
    let mut got = lengths(&a);
    got.sort();
    assert_eq!(got, vec![(ClipId(1), 5 * BAR), (ClipId(3), 5 * BAR)]);
    let ev = run(|e| v.handle(e, &doc, &reg), &[up(edge)]);
    assert_eq!(ev, vec![UiAction::EndMerge]);
    // Shrinking never takes a clip under the snap step.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(edge), mv(edge - Vec2::new(bp * 9.0, 0.0))],
    );
    let mut got = lengths(&a);
    got.sort();
    assert_eq!(got, vec![(ClipId(1), BAR), (ClipId(3), BAR)]);
}

// ---------------------------------------------------------------- constrain and snap mode

#[test]
fn the_constrain_role_keeps_the_start_while_a_clip_moves() {
    let (mut v, doc, reg) = setup();
    let bp = bar_px(&v);
    v.set_snap(Snap::Bar);
    let r = v.clip_rect(ClipId(2)).unwrap();
    let p = Point::new(r.x0 + 40.0, r.center().y + 6.0);
    // Shift is pressed after the mouse-down, as the role reads the modifiers on every move.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(p),
            mv_with(p + Vec2::new(bp * 1.2, 0.0), Modifiers::SHIFT),
        ],
    );
    assert!(starts(&a).is_empty(), "{a:?}");
    // Without Shift the same move changes the start.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[mv(p + Vec2::new(bp * 1.2, 0.0))],
    );
    assert_eq!(starts(&a), vec![(ClipId(2), 2 * BAR)]);
}

#[test]
fn relative_snap_moves_a_clip_by_whole_steps_and_keeps_its_distance_from_the_grid() {
    let (mut doc, reg) = (song_doc(), registry());
    // Clip 5 starts 100 ticks off the grid.
    doc.song.tracks[0].lanes[0].clips[1].start += 100;
    let mut v = arrangement(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_snap(Snap::Beat);
    let r = v.clip_rect(ClipId(5)).unwrap();
    let p = Point::new(r.x0 + 40.0, r.center().y + 6.0);
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let moved = |v: &mut ArrangementView| {
        run(
            |e| v.handle(e, &doc, &reg),
            &[
                down(p),
                mv(p + Vec2::new(beat_px * 1.3, 0.0)),
                up(p + Vec2::new(beat_px * 1.3, 0.0)),
            ],
        )
    };
    let a = moved(&mut v);
    assert_eq!(
        starts(&a),
        vec![(ClipId(5), 4 * BAR + PPQ)],
        "absolute: onto a beat"
    );
    let out = v.command("sequencer.set_snap_mode", Some("relative"), &doc, &reg);
    assert_eq!(out, CommandOutcome::Handled(Vec::new()));
    assert_eq!(v.snap_mode(), SnapMode::Relative);
    let a = moved(&mut v);
    assert_eq!(
        starts(&a),
        vec![(ClipId(5), 4 * BAR + 100 + PPQ)],
        "relative: one step"
    );
    assert_eq!(
        v.command("sequencer.set_snap_mode", Some("sideways"), &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn the_new_snap_values_are_set_by_name() {
    let (mut v, doc, reg) = setup();
    for (name, snap) in [
        ("half", Snap::Half),
        ("onetwentyeighth", Snap::OneTwentyEighth),
        ("half_triplet", Snap::HalfTriplet),
    ] {
        let _ = v.command("sequencer.set_snap", Some(name), &doc, &reg);
        assert_eq!(v.snap(), snap, "{name}");
    }
    assert_eq!(
        Snap::Half.step(atomik_ui::timeline::Meter::default()),
        Some(2 * PPQ)
    );
    assert_eq!(
        Snap::OneTwentyEighth.step(atomik_ui::timeline::Meter::default()),
        Some(PPQ / 32)
    );
    assert_eq!(
        Snap::HalfTriplet.step(atomik_ui::timeline::Meter::default()),
        Some(PPQ * 4 / 3)
    );
}

// ---------------------------------------------------------------- nudge, delete, creation

#[test]
fn nudging_clips_is_one_batch_that_stops_at_tick_zero() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[2, 5]);
    let out = v.command("sequencer.nudge_right", None, &doc, &reg);
    let CommandOutcome::Handled(actions) = out else {
        panic!("{out:?}")
    };
    let got = commands(&actions);
    assert_eq!(got.len(), 1);
    let Command::Batch(list) = &got[0] else {
        panic!("{got:?}")
    };
    assert_eq!(list.len(), 2, "one geometry command per clip");
    // By beat and by tick.
    let CommandOutcome::Handled(actions) = v.command("sequencer.nudge_left_beat", None, &doc, &reg)
    else {
        panic!()
    };
    let Command::Batch(list) = &commands(&actions)[0] else {
        panic!()
    };
    let mut starts: Vec<i64> = list
        .iter()
        .filter_map(|c| match c {
            Command::Song(SongCommand::SetClipGeometry { start, .. }) => Some(*start),
            _ => None,
        })
        .collect();
    starts.sort_unstable();
    assert_eq!(starts, vec![BAR - PPQ, 4 * BAR - PPQ]);
    // Clip 1 starts at 0: the group cannot go further back.
    select(&mut v, &[1, 2]);
    let CommandOutcome::Handled(actions) = v.command("sequencer.nudge_left_tick", None, &doc, &reg)
    else {
        panic!()
    };
    assert!(actions.is_empty());
    assert_eq!(
        v.command("sequencer.nudge_left", None, &doc, &reg),
        CommandOutcome::Handled(Vec::new())
    );
    select(&mut v, &[]);
    assert_eq!(
        v.command("sequencer.nudge_left", None, &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn delete_events_outside_clips_acts_on_the_selection_or_every_clip() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1]);
    let CommandOutcome::Handled(a) = v.command("sequencer.delete_outside_events", None, &doc, &reg)
    else {
        panic!()
    };
    assert_eq!(
        workflow_commands(&a),
        vec![WorkflowCommand::DeleteOutsideEvents {
            ids: vec![ClipId(1)]
        }]
    );
    select(&mut v, &[]);
    let CommandOutcome::Handled(a) = v.command("sequencer.delete_outside_events", None, &doc, &reg)
    else {
        panic!()
    };
    let WorkflowCommand::DeleteOutsideEvents { ids } = &workflow_commands(&a)[0] else {
        panic!()
    };
    assert_eq!(ids.len(), 5);
}

#[test]
fn a_new_clip_is_as_long_as_the_snap_step() {
    let (mut v, doc, reg) = setup();
    let created = |v: &mut ArrangementView| -> i64 {
        let p = lane_point(v, "lead", 8 * BAR);
        let a = run(
            |e| v.handle(e, &doc, &reg),
            &[down_mods(p, Modifiers::NONE, 2), up(p)],
        );
        song_commands(&a)
            .into_iter()
            .find_map(|c| match c {
                SongCommand::AddClip { clip, .. } => Some(clip.length),
                _ => None,
            })
            .expect("a clip is added")
    };
    v.set_snap(Snap::Beat);
    assert_eq!(created(&mut v), PPQ);
    v.set_snap(Snap::Sixteenth);
    assert_eq!(created(&mut v), PPQ / 4);
    v.set_snap(Snap::Bar);
    assert_eq!(created(&mut v), BAR);
}

// ---------------------------------------------------------------- razor

#[test]
fn a_razor_drag_cuts_the_range_out_and_with_the_copy_role_copies_it() {
    let (mut v, doc, reg) = setup();
    v.set_snap(Snap::Bar);
    v.set_tool(Tool::Razor);
    let (a, b) = (lane_point(&v, "keys", BAR), lane_point(&v, "keys", 2 * BAR));
    let out = run(|e| v.handle(e, &doc, &reg), &[down(a), mv(b), up(b)]);
    assert_eq!(
        workflow_commands(&out),
        vec![WorkflowCommand::CutOut {
            ids: vec![ClipId(3)],
            start: BAR,
            end: 2 * BAR,
            copy: false
        }]
    );
    let out = run(
        |e| v.handle(e, &doc, &reg),
        &[down(a), mv_with(b, Modifiers::CTRL), up(b)],
    );
    assert_eq!(
        workflow_commands(&out),
        vec![WorkflowCommand::CutOut {
            ids: vec![ClipId(3)],
            start: BAR,
            end: 2 * BAR,
            copy: true
        }]
    );
    // A press and release without a drag still splits the clip under the pointer.
    let out = run(|e| v.handle(e, &doc, &reg), &[down(a), up(a)]);
    assert_eq!(
        workflow_commands(&out),
        vec![WorkflowCommand::SplitClips {
            ids: vec![ClipId(3)],
            at: BAR
        }]
    );
}

// ---------------------------------------------------------------- the piano roll

fn contents(actions: &[UiAction]) -> Vec<Vec<Note>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipContent {
                id: ClipId(1),
                content: ClipContent::Notes(n),
                ..
            } => Some(n),
            _ => None,
        })
        .collect()
}

#[test]
fn a_ctrl_drag_of_a_note_adds_a_moved_copy_and_keeps_the_original() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_snap(Snap::Beat);
    let r = v.note_rect(0).unwrap();
    let p = r.center();
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let to = Point::new(p.x + beat_px * 20.0, p.y);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, Modifiers::CTRL, 1),
            mv_with(to, Modifiers::CTRL),
        ],
    );
    let lists = contents(&a);
    assert_eq!(lists.len(), 1);
    let before = bass_line();
    assert_eq!(lists[0].len(), before.len() + 1, "the copy joins the notes");
    assert!(lists[0].contains(&before[0]), "the original stays");
    let copy = Note {
        tick: 20 * PPQ,
        ..before[0]
    };
    assert!(lists[0].contains(&copy), "{:?}", lists[0]);
    let a = v.handle(&up(to), &doc, &reg);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn the_constrain_role_moves_a_note_along_one_axis() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_snap(Snap::Beat);
    let r = v.note_rect(0).unwrap();
    let p = r.center();
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    // Mostly sideways with a little height: Shift keeps the key.
    let to = Point::new(p.x + beat_px * 3.0, v.key_center_y(38));
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv_with(to, Modifiers::SHIFT)],
    );
    let lists = contents(&a);
    let moved = lists[0]
        .iter()
        .find(|n| n.tick == 3 * PPQ && n.key == 36)
        .copied();
    assert!(moved.is_some(), "{:?}", lists[0]);
}
