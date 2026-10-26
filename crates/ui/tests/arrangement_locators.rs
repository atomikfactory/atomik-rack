//! The song position and the loop locators of the arrangement: dragging the playhead from the
//! ruler or its line, dragging each loop edge, and the Alt and Ctrl clicks that set them.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{RackDoc, SongCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::viewport::Tool;
use atomik_ui::{ArrangementView, CursorHint, Modifiers, TransportAction, UiAction, UiEvent};
use support::song::*;
use support::*;

/// Ctrl+Alt: the disable-snap role of the standard roles.
const NO_SNAP: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: true,
    meta: false,
};

fn locates(actions: &[UiAction]) -> Vec<i64> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Transport(TransportAction::Locate(t)) => Some(*t),
            _ => None,
        })
        .collect()
}

fn loops(actions: &[UiAction]) -> Vec<((i64, i64), bool)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetLoop { range, enabled } => Some((range, enabled)),
            _ => None,
        })
        .collect()
}

/// The song with the loop on bars 2 to 4, and a view on it.
fn setup(snap: Snap) -> (RackDoc, Registry, ArrangementView) {
    let (mut doc, reg) = (song_doc(), registry());
    doc.song.loop_range = (BAR, 4 * BAR);
    let mut v = arrangement(&doc, &reg);
    v.set_snap(snap);
    (doc, reg, v)
}

fn send(
    v: &mut ArrangementView,
    doc: &RackDoc,
    reg: &Registry,
    events: &[UiEvent],
) -> Vec<UiAction> {
    run(|e| v.handle(e, doc, reg), events)
}

#[test]
fn ruler_drag_locates_each_new_snapped_tick_once_and_clamps_at_zero() {
    let (doc, reg, mut v) = setup(Snap::Beat);
    let y = v.ruler_label_y();
    let at = |t: i64| Point::new(v.tick_to_x(t), y);
    let events = [
        down(at(2 * BAR)),
        mv(at(2 * BAR + PPQ / 10)),
        mv(at(3 * BAR)),
        mv(at(3 * BAR + PPQ / 10)),
        mv(Point::new(v.tick_to_x(0) - 40.0, y)),
        up(at(0)),
    ];
    let a = send(&mut v, &doc, &reg, &events);
    assert_eq!(locates(&a), vec![2 * BAR, 3 * BAR, 0]);
}

#[test]
fn ruler_drag_with_the_disable_snap_role_locates_the_free_tick() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let y = v.ruler_label_y();
    let p = Point::new(v.tick_to_x(2 * BAR), y);
    let to = Point::new(v.tick_to_x(3 * BAR) + 5.0, y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv_with(to, NO_SNAP), up(to)]);
    let l = locates(&a);
    assert_eq!(l[0], 2 * BAR);
    assert_ne!(l[1] % BAR, 0, "{l:?}");
}

#[test]
fn playhead_line_in_the_grid_drags_without_jumping_to_the_pointer() {
    let (doc, reg, mut v) = setup(Snap::Beat);
    v.set_playhead(5 * BAR + PPQ);
    let y = v.lane_rect(&key("lead"), 0).unwrap().center().y;
    let x = v.tick_to_x(5 * BAR + PPQ);
    let step = PPQ as f64 * v.px_per_tick();
    let a = send(&mut v, &doc, &reg, &[down(Point::new(x + 2.0, y))]);
    assert!(
        locates(&a).is_empty(),
        "a press on the line does not locate"
    );
    assert_eq!(v.cursor(), CursorHint::ResizeHorizontal, "while dragging");
    let to = Point::new(x + 2.0 + 2.0 * step, y);
    let a = send(&mut v, &doc, &reg, &[mv(to), up(to)]);
    assert_eq!(locates(&a), vec![5 * BAR + 3 * PPQ]);
    assert!(song_commands(&a).is_empty(), "no clip is drawn");
}

#[test]
fn playhead_flag_in_the_ruler_drags_and_hints_the_cursor() {
    let (doc, reg, mut v) = setup(Snap::Beat);
    v.set_playhead(2 * BAR);
    let y = v.loop_band_y() + 4.0;
    let p = Point::new(v.tick_to_x(2 * BAR), y);
    assert_eq!(v.cursor(), CursorHint::Default);
    let _ = send(&mut v, &doc, &reg, &[mv(p)]);
    assert_eq!(v.cursor(), CursorHint::ResizeHorizontal);
    let to = Point::new(v.tick_to_x(2 * BAR + PPQ), y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(locates(&a), vec![2 * BAR + PPQ]);
    assert!(loops(&a).is_empty());
}

#[test]
fn pressing_beside_the_playhead_line_still_draws_in_the_lane() {
    let (doc, reg, mut v) = setup(Snap::Beat);
    v.set_playhead(5 * BAR + PPQ);
    let y = v.lane_rect(&key("lead"), 0).unwrap().center().y;
    let x = v.tick_to_x(5 * BAR + PPQ) + 12.0;
    let to = Point::new(x + 120.0, y);
    let a = send(
        &mut v,
        &doc,
        &reg,
        &[down(Point::new(x, y)), mv(to), up(to)],
    );
    assert!(locates(&a).is_empty());
    assert_eq!(song_commands(&a).len(), 1, "the drag draws a clip");
}

#[test]
fn dragging_the_right_locator_sets_the_loop_on_release() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let y = v.loop_band_y();
    let p = Point::new(v.tick_to_x(4 * BAR), y);
    let to = Point::new(v.tick_to_x(6 * BAR) + 3.0, y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to)]);
    assert!(a.is_empty(), "committed on release");
    let a = send(&mut v, &doc, &reg, &[up(to)]);
    assert_eq!(loops(&a), vec![((BAR, 6 * BAR), true)]);
}

#[test]
fn dragging_the_left_locator_sets_the_loop_and_keeps_the_enabled_flag() {
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    doc.song.loop_enabled = false;
    let _ = send(&mut v, &doc, &reg, &[]);
    let y = v.loop_band_y();
    let p = Point::new(v.tick_to_x(BAR), y);
    let to = Point::new(v.tick_to_x(2 * BAR) - 2.0, y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(loops(&a), vec![((2 * BAR, 4 * BAR), false)]);
}

#[test]
fn a_locator_dragged_past_the_other_leaves_one_snap_step() {
    let (doc, reg, mut v) = setup(Snap::Beat);
    let y = v.loop_band_y();
    let p = Point::new(v.tick_to_x(4 * BAR), y);
    let to = Point::new(v.tick_to_x(0), y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(loops(&a), vec![((0, PPQ), true)]);
    let (doc, reg, mut v) = setup(Snap::Beat);
    let p = Point::new(v.tick_to_x(BAR), y);
    let to = Point::new(v.tick_to_x(5 * BAR), y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(loops(&a), vec![((5 * BAR, 5 * BAR + PPQ), true)]);
}

#[test]
fn clicking_a_locator_without_moving_changes_nothing() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = Point::new(v.tick_to_x(4 * BAR), v.loop_band_y());
    let a = send(&mut v, &doc, &reg, &[down(p), up(p)]);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn alt_click_in_the_ruler_sets_the_left_locator_snapped() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    for y in [v.ruler_label_y(), v.loop_band_y()] {
        let p = Point::new(v.tick_to_x(2 * BAR) + 4.0, y);
        let a = send(&mut v, &doc, &reg, &[down_mods(p, alt(), 1), up(p)]);
        assert_eq!(loops(&a), vec![((2 * BAR, 4 * BAR), true)]);
        assert!(locates(&a).is_empty());
    }
    let p = Point::new(v.tick_to_x(6 * BAR), v.ruler_label_y());
    let a = send(&mut v, &doc, &reg, &[down_mods(p, alt(), 1), up(p)]);
    assert_eq!(
        loops(&a),
        vec![((6 * BAR, 7 * BAR), true)],
        "the right one gives way"
    );
}

#[test]
fn ctrl_and_cmd_click_in_the_ruler_set_the_right_locator() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let cmd = Modifiers {
        meta: true,
        ..Modifiers::NONE
    };
    for m in [Modifiers::CTRL, cmd] {
        let p = Point::new(v.tick_to_x(6 * BAR) - 3.0, v.ruler_label_y());
        let a = send(&mut v, &doc, &reg, &[down_mods(p, m, 1), up(p)]);
        assert_eq!(loops(&a), vec![((BAR, 6 * BAR), true)]);
    }
    let p = Point::new(v.tick_to_x(0), v.ruler_label_y());
    let a = send(
        &mut v,
        &doc,
        &reg,
        &[down_mods(p, Modifiers::CTRL, 1), up(p)],
    );
    assert_eq!(loops(&a), vec![((0, BAR), true)]);
}

#[test]
fn the_locator_clicks_beat_the_razor_and_ctrl_alt_still_locates() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    v.set_tool(Tool::Razor);
    let p = Point::new(v.tick_to_x(2 * BAR), v.ruler_label_y());
    let a = send(
        &mut v,
        &doc,
        &reg,
        &[down_mods(p, Modifiers::CTRL, 1), up(p)],
    );
    assert_eq!(loops(&a), vec![((BAR, 2 * BAR), true)]);
    assert_eq!(song_commands(&a).len(), 1, "the razor does not split");
    v.set_tool(Tool::Select);
    let a = send(&mut v, &doc, &reg, &[down_mods(p, NO_SNAP, 1), up(p)]);
    assert_eq!(locates(&a), vec![2 * BAR]);
    assert!(loops(&a).is_empty());
}

#[test]
fn the_playhead_does_not_take_a_clip_edge_resize_under_it() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let r = v.clip_rect(atomik_model::ClipId(1)).unwrap();
    let edge = Point::new(r.x1 - 2.0, r.center().y);
    v.set_playhead(v.x_to_tick(edge.x));
    let to = Point::new(edge.x + (v.tick_to_x(BAR) - v.tick_to_x(0)) * 1.2, edge.y);
    let a = send(&mut v, &doc, &reg, &[down(edge), mv(to), up(to)]);
    assert!(locates(&a).is_empty(), "{a:?}");
    assert!(!song_commands(&a).is_empty(), "the clip resizes");
}
