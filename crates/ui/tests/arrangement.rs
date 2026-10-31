//! ArrangementView behaviour through its public contract: layout,
//! track controls, clip drawing / moving / resizing / looping, selection shortcuts,
//! automation point editing, ruler, zoom and scroll.

mod support;

use atomik_model::{ClipContent, ClipId, Command, Curve, LaneKind, SongCommand, WorkflowCommand};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{Key, Modifiers, TrackButton, TransportAction, UiAction, UiEvent};
use support::song::*;
use support::*;

fn mid_left(r: atomik_ui::kurbo::Rect, dx: f64) -> Point {
    Point::new(r.x0 + dx, r.center().y + 6.0)
}

// ---------------------------------------------------------------- layout

#[test]
fn tracks_lanes_and_add_rows_follow_the_song() {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    let bass = v.lane_rect(&key("bass"), 0).unwrap();
    let cutoff = v.lane_rect(&key("bass"), 1).unwrap();
    let lead = v.lane_rect(&key("lead"), 0).unwrap();
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    assert!(bass.y1 <= cutoff.y0 + 1e-9 && cutoff.y1 <= lead.y0 + 1e-9);
    assert!(lead.y1 <= keys.y0 + 1e-9);
    assert!(
        cutoff.height() < bass.height(),
        "automation lanes are shorter"
    );
    assert!(bass.x0 > 100.0, "lanes start right of the track list");
    // Only the synth without a track gets a "+" row; the delay has no Notes input.
    let pad = v.add_track_center(&key("pad")).unwrap();
    assert!(pad.y > keys.y1);
    assert!(v.add_track_center(&key("delay")).is_none());
    assert!(v.add_track_center(&key("bass")).is_none());
}

#[test]
fn clips_are_placed_by_tick() {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    let r = v.clip_rect(ClipId(5)).unwrap();
    assert!((r.x0 - v.tick_to_x(4 * BAR)).abs() < 1e-6);
    assert!((r.x1 - v.tick_to_x(6 * BAR)).abs() < 1e-6);
    let lane = v.lane_rect(&key("bass"), 0).unwrap();
    assert!(r.y0 >= lane.y0 && r.y1 <= lane.y1);
    assert_eq!(v.x_to_tick(v.tick_to_x(3 * BAR)), 3 * BAR);
}

#[test]
fn collapsing_a_track_hides_its_automation_lanes() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let p = v
        .track_button_center(&key("bass"), TrackButton::Expand)
        .unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(commands(&a).is_empty(), "expanding is view state");
    assert!(v.lane_rect(&key("bass"), 1).is_none());
    assert!(v.clip_rect(ClipId(4)).is_none());
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(v.lane_rect(&key("bass"), 1).is_some());
}

// ---------------------------------------------------------------- track controls

#[test]
fn mute_solo_arm_toggle_track_flags() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let cases = [
        ("bass", TrackButton::Mute, (true, false, false)),
        ("lead", TrackButton::Mute, (false, false, false)),
        ("bass", TrackButton::Solo, (false, true, false)),
        ("keys", TrackButton::Arm, (false, true, false)),
    ];
    for (dev, button, (muted, solo, armed)) in cases {
        let p = v.track_button_center(&key(dev), button).unwrap();
        let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
        assert_eq!(
            song_commands(&a),
            vec![SongCommand::SetTrackFlags {
                device: key(dev),
                muted,
                solo,
                armed
            }],
            "{dev} {button:?}"
        );
    }
}

#[test]
fn add_track_row_emits_add_track() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let p = v.add_track_center(&key("pad")).unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::AddTrack { device: key("pad") }]
    );
}

#[test]
fn param_picker_adds_an_automation_lane() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let p = v
        .track_button_center(&key("bass"), TrackButton::AddLane)
        .unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(a.is_empty());
    let items = v.param_menu_items();
    assert!(!items.is_empty());
    let checked = v.param_menu_checked();
    let at = |key: &str| items.iter().position(|k| k == key).unwrap();
    assert!(
        checked[at("filter.cutoff")],
        "a param with a lane is checked"
    );
    assert!(!checked[at("filter.resonance")]);
    assert!(
        !items.iter().any(|k| k.starts_with("internal.")),
        "internal params have no lane"
    );
    // The first row without a lane adds one, the row of the lane that exists removes it.
    let i = checked.iter().position(|c| !c).unwrap();
    let row = v.param_menu_row_center(i).unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(row), up(row)]);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::AddAutomationLane {
            device: key("bass"),
            param: items[i].clone()
        }]
    );
    assert!(v.param_menu_items().is_empty(), "the menu closes");
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    let row = v.param_menu_row_center(at("filter.cutoff")).unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(row), up(row)]);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::RemoveAutomationLane {
            device: key("bass"),
            param: "filter.cutoff".into()
        }],
        "a checked row removes its lane"
    );
    // Escape closes an open menu without emitting.
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert!(a.is_empty() && v.param_menu_items().is_empty());
}

// ---------------------------------------------------------------- drawing clips

#[test]
fn dragging_on_an_empty_lane_draws_a_snapped_clip() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let lane = v.lane_rect(&key("lead"), 0).unwrap();
    let y = lane.center().y;
    let x0 = v.tick_to_x(5 * BAR + PPQ + PPQ / 3);
    let x1 = v.tick_to_x(6 * BAR + PPQ / 2);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(Point::new(x0, y)), mv(Point::new(x1, y))],
    );
    assert!(a.is_empty(), "nothing is emitted until release");
    let a = v.handle(&up(Point::new(x1, y)), &doc, &reg);
    let cmds = song_commands(&a);
    assert_eq!(cmds.len(), 1);
    let SongCommand::AddClip { device, lane, clip } = &cmds[0] else {
        panic!("{cmds:?}")
    };
    assert_eq!((device, *lane), (&key("lead"), 0));
    assert_eq!((clip.start, clip.length), (5 * BAR + PPQ, BAR));
    assert_eq!(clip.content, ClipContent::Notes(Vec::new().into()));
    assert_eq!(
        (clip.offset, clip.loop_length, clip.muted),
        (0, None, false)
    );
}

#[test]
fn drawing_on_an_automation_lane_makes_an_automation_clip() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    let lane = v.lane_rect(&key("bass"), 1).unwrap();
    let y = lane.center().y;
    let (x0, x1) = (v.tick_to_x(5 * BAR + 10), v.tick_to_x(6 * BAR + 10));
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(Point::new(x0, y)),
            mv(Point::new(x1, y)),
            up(Point::new(x1, y)),
        ],
    );
    let cmds = song_commands(&a);
    let SongCommand::AddClip { lane, clip, .. } = &cmds[0] else {
        panic!("{cmds:?}")
    };
    assert_eq!(*lane, 1);
    assert_eq!((clip.start, clip.length), (5 * BAR, 2 * BAR));
    assert_eq!(clip.content, ClipContent::Automation(Vec::new()));
}

#[test]
fn a_click_on_an_empty_lane_only_clears_the_selection() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let c = v.clip_rect(ClipId(2)).unwrap();
    let _ = run(
        |e| v.handle(e, &doc, &reg),
        &[down(c.center()), up(c.center())],
    );
    assert_eq!(v.selected_clips(), vec![ClipId(2)]);
    let lane = v.lane_rect(&key("lead"), 0).unwrap();
    let p = Point::new(v.tick_to_x(8 * BAR), lane.center().y);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(commands(&a).is_empty());
    assert!(v.selected_clips().is_empty());
}

// ---------------------------------------------------------------- moving and resizing

/// (id, start, length, loop length, merge) of each `SetClipGeometry`.
type Geometry = (ClipId, i64, i64, Option<i64>, Option<u64>);

fn geometry(cmds: &[SongCommand]) -> Vec<Geometry> {
    cmds.iter()
        .filter_map(|c| match c {
            SongCommand::SetClipGeometry {
                id,
                start,
                length,
                loop_length,
                merge,
                ..
            } => Some((*id, *start, *length, *loop_length, *merge)),
            _ => None,
        })
        .collect()
}

#[test]
fn dragging_a_clip_moves_it_on_the_grid_with_one_merge_group() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.clip_rect(ClipId(2)).unwrap();
    let p0 = mid_left(r, 30.0);
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(p0),
            mv(p0 + Vec2::new(1.0, 0.0)),
            mv(p0 + Vec2::new(beat_px * 1.4, 0.0)),
            mv(p0 + Vec2::new(beat_px * 2.6, 0.0)),
        ],
    );
    let g = geometry(&song_commands(&a));
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!((g[0].0, g[0].1, g[0].2), (ClipId(2), BAR + PPQ, 3 * BAR));
    assert_eq!(g[1].1, BAR + 3 * PPQ);
    assert_eq!(g[1].3, Some(2 * BAR), "loop length is kept");
    let merge = g[0].4.expect("drags carry a merge id");
    assert_eq!(g[1].4, Some(merge));
    let a = v.handle(&up(p0 + Vec2::new(beat_px * 2.6, 0.0)), &doc, &reg);
    assert_eq!(a, vec![UiAction::EndMerge]);
    // A second drag gets a new merge group; Ctrl+Alt (the disable-snap role) disables snapping.
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(p0),
            mv_with(p0 + Vec2::new(beat_px * 0.5, 0.0), ctrl_alt),
        ],
    );
    let g2 = geometry(&song_commands(&a));
    assert_eq!(g2[0].1, BAR + PPQ / 2);
    assert_ne!(g2[0].4, Some(merge));
}

#[test]
fn moving_a_group_keeps_its_shape_and_stops_at_zero() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    let a = v.clip_rect(ClipId(2)).unwrap();
    let b = v.clip_rect(ClipId(5)).unwrap();
    let _ = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(mid_left(a, 20.0)),
            up(mid_left(a, 20.0)),
            down_mods(mid_left(b, 20.0), Modifiers::SHIFT, 1),
            up(mid_left(b, 20.0)),
        ],
    );
    assert_eq!(v.selected_clips(), vec![ClipId(2), ClipId(5)]);
    let p = mid_left(b, 20.0);
    let acts = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p - Vec2::new(5000.0, 0.0))],
    );
    let mut g: Vec<(ClipId, i64)> = geometry(&song_commands(&acts))
        .iter()
        .map(|x| (x.0, x.1))
        .collect();
    g.sort();
    // Clip 2 starts at bar 2: the group can move back by one bar only.
    assert_eq!(g, vec![(ClipId(2), 0), (ClipId(5), 3 * BAR)]);
}

#[test]
fn right_edge_resizes_and_shift_edge_sets_the_loop_length() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    let r = v.clip_rect(ClipId(1)).unwrap();
    let edge = Point::new(r.x1 - 2.0, r.center().y);
    let bar_px = v.tick_to_x(BAR) - v.tick_to_x(0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(edge),
            mv(edge + Vec2::new(bar_px * 1.2, 0.0)),
            up(edge),
        ],
    );
    let g = geometry(&song_commands(&a));
    assert_eq!(
        (g[0].0, g[0].1, g[0].2, g[0].3),
        (ClipId(1), 0, 5 * BAR, None)
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // Shrinking never goes below one snap step.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(edge), mv(edge - Vec2::new(bar_px * 9.0, 0.0))],
    );
    assert_eq!(geometry(&song_commands(&a))[0].2, BAR);
    let _ = v.handle(&up(edge), &doc, &reg);
    // Shift: the loop length changes, the clip length stays.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(edge, Modifiers::SHIFT, 1),
            mv_with(edge - Vec2::new(bar_px * 2.0, 0.0), Modifiers::SHIFT),
        ],
    );
    let g = geometry(&song_commands(&a));
    assert_eq!((g[0].2, g[0].3), (4 * BAR, Some(2 * BAR)));
}

// ---------------------------------------------------------------- selection shortcuts

#[test]
fn delete_duplicate_and_split() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let a = v.clip_rect(ClipId(1)).unwrap();
    let b = v.clip_rect(ClipId(3)).unwrap();
    let _ = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(a.center()),
            up(a.center()),
            down_mods(b.center(), Modifiers::CTRL, 1),
            up(b.center()),
        ],
    );
    assert_eq!(v.selected_clips(), vec![ClipId(1), ClipId(3)]);
    let d = v.command("edit.duplicate", None, &doc, &reg);
    assert_eq!(
        d.actions(),
        [UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateClips {
                ids: vec![ClipId(1), ClipId(3)]
            }
        ))],
        "one command, one undo step: the copies go right after the selection"
    );
    v.set_playhead(2 * BAR + PPQ);
    let s = v.command("sequencer.split_at_playhead", None, &doc, &reg);
    assert_eq!(
        workflow_commands(s.actions()),
        vec![WorkflowCommand::SplitClips {
            ids: vec![ClipId(1), ClipId(3)],
            at: 2 * BAR + PPQ
        }],
        "one command, one undo step"
    );
    let del = v.command("edit.delete", None, &doc, &reg);
    assert_eq!(
        del.actions(),
        [UiAction::Command(Command::Workflow(
            WorkflowCommand::RemoveClips {
                ids: vec![ClipId(1), ClipId(3)]
            }
        ))],
        "several clips are one undo step"
    );
}

#[test]
fn split_without_selection_splits_every_clip_under_the_playhead() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_playhead(BAR + PPQ);
    let s = v.command("sequencer.split_at_playhead", None, &doc, &reg);
    assert_eq!(
        workflow_commands(s.actions()),
        vec![WorkflowCommand::SplitClips {
            ids: vec![ClipId(1), ClipId(4), ClipId(2), ClipId(3)],
            at: BAR + PPQ
        }]
    );
    // On a clip boundary nothing splits.
    v.set_playhead(BAR * 4);
    let s = v.command("sequencer.split_at_playhead", None, &doc, &reg);
    assert!(workflow_commands(s.actions()).iter().all(
        |c| !matches!(c, WorkflowCommand::SplitClips { ids, .. } if ids.contains(&ClipId(1)))
    ));
}

#[test]
fn double_click_opens_note_clips_in_the_piano_roll() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let r = v.clip_rect(ClipId(2)).unwrap();
    let p = mid_left(r, 40.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), up(p), down_mods(p, Modifiers::NONE, 2), up(p)],
    );
    assert!(a.contains(&UiAction::OpenPianoRoll(ClipId(2))));
    assert!(commands(&a).is_empty());
    let r = v.clip_rect(ClipId(4)).unwrap();
    let p = Point::new(r.x0 + 40.0, r.y0 + 4.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down_mods(p, Modifiers::NONE, 2), up(p)],
    );
    assert!(!a.iter().any(|x| matches!(x, UiAction::OpenPianoRoll(_))));
}

// ---------------------------------------------------------------- automation

fn points_of(cmds: &[SongCommand]) -> Vec<(Vec<atomik_model::AutomationPoint>, Option<u64>)> {
    cmds.iter()
        .filter_map(|c| match c {
            SongCommand::SetClipContent {
                id: ClipId(4),
                content: ClipContent::Automation(p),
                merge,
            } => Some((p.clone(), *merge)),
            _ => None,
        })
        .collect()
}

#[test]
fn clicking_in_an_automation_clip_adds_a_point_with_a_taper_value() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let (top, bottom) = v.automation_value_range(ClipId(4)).unwrap();
    let x = v.tick_to_x(BAR + PPQ + 100);
    let p = Point::new(x, (top + bottom) / 2.0);
    let a = v.handle(&down(p), &doc, &reg);
    let pts = points_of(&song_commands(&a));
    assert_eq!(pts.len(), 1);
    let (list, merge) = &pts[0];
    assert!(merge.is_some());
    assert_eq!(list.len(), 5);
    let added = list[1];
    assert_eq!(added.tick, BAR + PPQ);
    assert_eq!(added.curve, Curve::Linear);
    let reg_desc = reg.get("atomik.poly-synth").unwrap().descriptor;
    let cutoff = reg_desc.param_by_key("filter.cutoff").unwrap();
    let expected = cutoff.from_normalized(0.5);
    assert!(
        (added.value - expected).abs() <= expected * 0.02,
        "{} vs {expected}",
        added.value
    );
    // Dragging the new point far up clamps to the param maximum.
    let a = v.handle(&mv(Point::new(x, top - 300.0)), &doc, &reg);
    let pts = points_of(&song_commands(&a));
    assert_eq!(pts[0].1, *merge);
    assert!((pts[0].0[1].value - cutoff.max).abs() < 1e-3);
    assert_eq!(
        v.handle(&up(Point::new(x, top)), &doc, &reg),
        vec![UiAction::EndMerge]
    );
}

#[test]
fn points_drag_delete_and_toggle_step() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let p2 = v.automation_point_pos(ClipId(4), 2).unwrap();
    // Drag point 2 one beat later.
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p2), mv(p2 + Vec2::new(beat_px, 0.0))],
    );
    let pts = points_of(&song_commands(&a));
    let last = &pts.last().unwrap().0;
    assert_eq!(last.len(), 4);
    assert_eq!(last[2].tick, 3 * BAR + PPQ);
    assert!((last[2].value - 1200.0).abs() < 1200.0 * 0.03);
    let _ = v.handle(&up(p2), &doc, &reg);
    // Right-click deletes.
    let a = run(|e| v.handle(e, &doc, &reg), &[right_down(p2), right_up(p2)]);
    let pts = points_of(&song_commands(&a));
    assert_eq!(pts.len(), 1);
    assert_eq!(pts[0].0.len(), 3);
    assert!(pts[0].0.iter().all(|p| p.tick != 3 * BAR));
    // Shift-click toggles linear/step.
    let p1 = v.automation_point_pos(ClipId(4), 1).unwrap();
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down_mods(p1, Modifiers::SHIFT, 1), up(p1)],
    );
    let pts = points_of(&song_commands(&a));
    assert_eq!(pts.len(), 1);
    assert_eq!(pts[0].0[1].curve, Curve::Linear);
    assert_eq!(pts[0].0[0].curve, Curve::Linear);
}

#[test]
fn automation_clip_header_moves_the_clip() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    let r = v.clip_rect(ClipId(4)).unwrap();
    let p = Point::new(r.x0 + 30.0, r.y0 + 4.0);
    let bar_px = v.tick_to_x(BAR) - v.tick_to_x(0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(bar_px, 0.0))],
    );
    let g = geometry(&song_commands(&a));
    assert_eq!((g[0].0, g[0].1), (ClipId(4), BAR));
    assert!(points_of(&song_commands(&a)).is_empty());
}

// ---------------------------------------------------------------- ruler and loop

#[test]
fn ruler_click_locates_and_snap_button_cycles() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let p = Point::new(v.tick_to_x(2 * BAR + PPQ + PPQ / 3), v.ruler_label_y());
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(
        a,
        vec![UiAction::Transport(TransportAction::Locate(2 * BAR + PPQ))]
    );
    let s = v.snap_button_center();
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(s), up(s)]);
    assert_eq!(v.snap(), Snap::Eighth);
    let _ = run(
        |e| v.handle(e, &doc, &reg),
        &[right_down(s), right_up(s), right_down(s), right_up(s)],
    );
    assert_eq!(v.snap(), Snap::Half);
}

// ---------------------------------------------------------------- zoom, scroll, keys

#[test]
fn ctrl_wheel_zooms_around_the_pointer_and_wheel_scrolls() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    // A short view, so the lanes can scroll vertically.
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1200.0, 200.0));
    let x = v.tick_to_x(3 * BAR);
    let lane = v.lane_rect(&key("lead"), 0).unwrap();
    let pos = Point::new(x, lane.center().y);
    let before = v.px_per_tick();
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 2.0),
            pos,
            modifiers: Modifiers::CTRL,
        },
        &doc,
        &reg,
    );
    assert!(v.px_per_tick() > before);
    assert!((v.tick_to_x(3 * BAR) - x).abs() < 1.0);
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos,
            modifiers: Modifiers::SHIFT,
        },
        &doc,
        &reg,
    );
    assert!(
        v.tick_to_x(3 * BAR) < x,
        "shift-wheel scrolls later in time"
    );
    let y0 = v.lane_rect(&key("keys"), 0).unwrap().y0;
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    let y1 = v.lane_rect(&key("keys"), 0).unwrap().y0;
    assert!(y1 < y0, "wheel scrolls the lanes");
}

#[test]
fn keys_are_commands_now_the_view_keeps_only_escape_and_home() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let k = |v: &mut atomik_ui::ArrangementView, key: Key, m: Modifiers| {
        v.handle(&press(key, m), &doc, &reg)
    };
    // The transport, undo and selection keys are registry commands; the shell resolves them,
    // so a remap applies and the view does not act on them by itself.
    for (key, m) in [
        (Key::Space, Modifiers::NONE),
        (Key::Enter, Modifiers::NONE),
        (Key::Char('l'), Modifiers::NONE),
        (Key::Char('R'), Modifiers::NONE),
        (Key::Char('z'), Modifiers::CTRL),
        (Key::Char('a'), Modifiers::CTRL),
        (Key::Char('d'), Modifiers::CTRL),
        (Key::Delete, Modifiers::NONE),
    ] {
        assert!(k(&mut v, key.clone(), m).is_empty(), "{key:?}");
    }
    assert!(v.selected_clips().is_empty());
    assert!(v.command("edit.select_all", None, &doc, &reg).is_taken());
    assert_eq!(v.selected_clips().len(), 5);
    // Escape clears the selection.
    assert!(k(&mut v, Key::Escape, Modifiers::NONE).is_empty());
    assert!(v.selected_clips().is_empty());
}

#[test]
fn pointer_releases_outside_the_bounds_still_end_drags() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let r = v.clip_rect(ClipId(3)).unwrap();
    let p = mid_left(r, 20.0);
    let _ = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(Point::new(p.x + 50.0, 2000.0))],
    );
    assert!(v.is_dragging());
    let a = v.handle(&up(Point::new(p.x + 50.0, 2000.0)), &doc, &reg);
    assert_eq!(a, vec![UiAction::EndMerge]);
    assert!(!v.is_dragging());
}

#[test]
fn lane_kinds_match_the_doc() {
    // Guards the fixture: lane 1 of the bass track is the cutoff automation lane.
    let doc = song_doc();
    assert_eq!(
        doc.song.tracks[0].lanes[1].kind,
        LaneKind::Automation("filter.cutoff".into())
    );
}

// ---- horizontal scrollbar -------------------------------------------------------------------

/// A point on the scrollbar strip at the bottom of the timeline, `fx` (0..1) along it.
fn on_hscroll(v: &atomik_ui::ArrangementView, fx: f64) -> Point {
    let b = v.bounds();
    let x0 = v.tick_to_x(v.scroll_ticks().round() as i64);
    Point::new(x0 + (b.x1 - x0) * fx, b.y1 - 6.0)
}

#[test]
fn dragging_the_scrollbar_thumb_scrolls_the_timeline_without_edits() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_px_per_tick(v.px_per_tick() * 8.0); // zoomed in: the song is wider than the view
    assert_eq!(v.scroll_ticks(), 0.0);
    let grab = on_hscroll(&v, 0.01);
    let mut actions = run(
        |e| v.handle(e, &doc, &reg),
        &[down(grab), mv(grab + Vec2::new(200.0, 0.0))],
    );
    let scrolled = v.scroll_ticks();
    assert!(scrolled > 0.0, "the thumb drag scrolls right");
    // Back past the left end: clamped at the song start.
    actions.extend(run(
        |e| v.handle(e, &doc, &reg),
        &[
            mv(grab - Vec2::new(1000.0, 0.0)),
            up(grab - Vec2::new(1000.0, 0.0)),
        ],
    ));
    assert_eq!(v.scroll_ticks(), 0.0);
    assert!(
        actions.iter().all(|a| !matches!(a, UiAction::Command(_))),
        "scrolling is view-only: {actions:?}"
    );
}

#[test]
fn clicking_the_scrollbar_end_jumps_but_stays_within_the_content() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_px_per_tick(v.px_per_tick() * 8.0);
    let end = on_hscroll(&v, 0.999);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(end), up(end)]);
    let far = v.scroll_ticks();
    assert!(far > 0.0, "a click near the right end jumps there");
    // Never past the later of the last clip end and the song End, plus four bars.
    let last = doc
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter())
        .map(|c| c.start + c.length)
        .max()
        .unwrap()
        .max(doc.song.end_tick());
    assert!(far <= (last + 4 * BAR) as f64, "{far} > {}", last + 4 * BAR);
    // A second click at the same place does not move further.
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(end), up(end)]);
    assert!((v.scroll_ticks() - far).abs() < 1e-6);
}

// ---------------------------------------------------------------- snap selector drag and wheel

#[test]
fn dragging_the_snap_selector_steps_the_resolutions_and_a_click_still_cycles() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let s = v.snap_button_center();
    // Up 10 px: two places along the selector (Beat -> Eighth -> Sixteenth).
    let ev = [
        down(s),
        mv(Point::new(s.x, s.y - 10.0)),
        up(Point::new(s.x, s.y - 10.0)),
    ];
    let a = run(|e| v.handle(e, &doc, &reg), &ev);
    assert_eq!(v.snap(), Snap::Sixteenth);
    assert!(a.is_empty(), "the snap is view state: {a:?}");
    // Down past the first entry stops there; the release is not a click.
    let ev = [
        down(s),
        mv(Point::new(s.x, s.y + 100.0)),
        up(Point::new(s.x, s.y + 100.0)),
    ];
    let _ = run(|e| v.handle(e, &doc, &reg), &ev);
    assert_eq!(v.snap(), Snap::Bar);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(s), up(s)]);
    assert_eq!(
        v.snap(),
        Snap::Half,
        "a click without movement cycles forward"
    );
}

#[test]
fn the_wheel_over_the_snap_selector_steps_one_resolution_per_notch() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let s = v.snap_button_center();
    let wheel = |dy: f64| UiEvent::Wheel {
        delta: Vec2::new(0.0, dy),
        pos: s,
        modifiers: Modifiers::NONE,
    };
    let _ = v.handle(&wheel(1.0), &doc, &reg);
    assert_eq!(v.snap(), Snap::Eighth);
    let _ = v.handle(&wheel(-1.0), &doc, &reg);
    let _ = v.handle(&wheel(-1.0), &doc, &reg);
    assert_eq!(
        v.snap(),
        Snap::Half,
        "the half note lies between the bar and the beat"
    );
    let _ = v.handle(&wheel(-1.0), &doc, &reg);
    assert_eq!(v.snap(), Snap::Bar);
    let _ = v.handle(&wheel(-1.0), &doc, &reg);
    assert_eq!(v.snap(), Snap::Bar, "no wrap at the ends");
}
