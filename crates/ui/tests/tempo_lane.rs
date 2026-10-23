//! The tempo lane and the meter changes of the arrangement and the transport bar: the lane's
//! gestures (add, drag, delete, ramp), the meter tags of the ruler, the commands behind the
//! context menus, the snap grid over a changing meter and the readouts at the playhead.

mod support;

use atomik_model::{Command, MeterEvent, RackDoc, SongCommand, TEMPO_MAX, TempoEvent};
use atomik_ui::action::ContextTarget;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::tempo_lane::{self as lane, TEMPO_LANE_HEIGHT};
use atomik_ui::timeline::Snap;
use atomik_ui::viewport::Tool;
use atomik_ui::{
    ArrangementView, DisplayList, Key, Modifiers, PointerButton, TransportState, UiAction, UiEvent,
};
use support::song::*;
use support::*;

fn ramp(tick: i64, bpm: f64) -> TempoEvent {
    TempoEvent {
        tick,
        bpm,
        ramp: true,
    }
}

fn mev(bar: u32, num: u8, den: u8) -> MeterEvent {
    MeterEvent { bar, num, den }
}

/// An arrangement with the tempo lane shown over `doc`.
fn with_lane(doc: &RackDoc) -> ArrangementView {
    let mut v = arrangement(doc, &registry());
    v.set_tempo_lane(true);
    v.set_snap(Snap::Beat);
    v
}

fn handle(v: &mut ArrangementView, doc: &RackDoc, events: &[UiEvent]) -> Vec<UiAction> {
    let reg = registry();
    run(|e| v.handle(e, doc, &reg), events)
}

fn command(v: &mut ArrangementView, doc: &RackDoc, id: &str, arg: Option<&str>) -> CommandOutcome {
    v.command(id, arg, doc, &registry())
}

/// The song commands among the actions, as the tempo maps they set (drag steps included).
fn tempo_maps(actions: &[UiAction]) -> Vec<Vec<TempoEvent>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetTempoMap(m) | SongCommand::DragTempoMap { map: m, .. } => Some(m),
            _ => None,
        })
        .collect()
}

fn meter_maps(actions: &[UiAction]) -> Vec<Vec<MeterEvent>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetMeterMap(m) | SongCommand::DragMeterMap { map: m, .. } => Some(m),
            _ => None,
        })
        .collect()
}

fn three_points() -> RackDoc {
    let mut doc = song_doc();
    doc.song.tempo = vec![
        TempoEvent::step(0, 100.0),
        TempoEvent::step(2 * BAR, 140.0),
        TempoEvent::step(4 * BAR, 90.0),
    ];
    doc
}

// ---------------------------------------------------------------- layout

#[test]
fn the_lane_is_hidden_by_default_and_pushes_the_lanes_down_when_shown() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    assert!(!v.tempo_lane_visible());
    assert_eq!(v.tempo_lane_rect().height(), 0.0);
    let before = v.lane_rect(&key("bass"), 0).unwrap();
    v.set_tempo_lane(true);
    assert!(v.tempo_lane_visible());
    assert_eq!(v.tempo_lane_rect().height(), TEMPO_LANE_HEIGHT);
    let after = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((after.y0 - before.y0 - TEMPO_LANE_HEIGHT).abs() < 1e-9);
    // The ruler stays where it was and the lane sits right under it.
    assert!((v.tempo_lane_rect().y0 - 34.0).abs() < 1e-9);
}

#[test]
fn points_sit_at_their_tick_and_bpm() {
    let doc = three_points();
    let v = with_lane(&doc);
    let (p0, p1, p2) = (
        v.tempo_point_center(0).unwrap(),
        v.tempo_point_center(1).unwrap(),
        v.tempo_point_center(2).unwrap(),
    );
    assert!((p1.x - v.tick_to_x(2 * BAR)).abs() < 1e-9);
    assert!((p2.x - v.tick_to_x(4 * BAR)).abs() < 1e-9);
    assert!(
        p1.y < p0.y && p0.y < p2.y,
        "faster is higher: {p0:?} {p1:?} {p2:?}"
    );
    let lane_rect = v.tempo_lane_rect();
    assert!(p1.y > lane_rect.y0 && p2.y < lane_rect.y1);
    assert!(v.tempo_point_center(3).is_none());
}

#[test]
fn the_lane_paints_its_scale_and_the_ruler_the_meter_tags() {
    let mut doc = three_points();
    doc.song.meter = vec![mev(1, 4, 4), mev(3, 3, 4), mev(5, 7, 8)];
    let mut v = with_lane(&doc);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    let texts = dl.texts();
    for t in ["TEMPO", "100.00 BPM", "100", "3/4", "7/8"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
    assert!(
        !texts.contains(&"4/4"),
        "the first 4/4 has no tag: {texts:?}"
    );
    // Bars are numbered through the changes: 1, 2 | 3, 4 | 5, ...
    for bar in ["1", "2", "3", "4", "5"] {
        assert!(texts.contains(&bar), "bar {bar} in {texts:?}");
    }
}

// ---------------------------------------------------------------- tempo gestures

#[test]
fn the_pencil_adds_a_point_at_the_snapped_tick_and_the_pointer_bpm() {
    let doc = song_doc();
    let mut v = with_lane(&doc);
    v.set_tool(Tool::Pencil);
    let rect = v.tempo_lane_rect();
    let x = v.tick_to_x(3 * BAR + PPQ / 3);
    let y = rect.y0 + 10.0;
    let a = handle(
        &mut v,
        &doc,
        &[down(Point::new(x, y)), up(Point::new(x, y))],
    );
    let maps = tempo_maps(&a);
    assert_eq!(maps.len(), 1, "{a:?}");
    let map = &maps[0];
    assert_eq!(map.len(), 2);
    assert_eq!(map[1].tick, 3 * BAR, "snapped to the beat before: {map:?}");
    let want = lane::round_bpm(
        lane::y_to_bpm(y, rect, lane::bpm_range(&doc.song.tempo)),
        false,
    );
    assert_eq!(map[1].bpm, want);
    assert!(
        map[1].bpm > 140.0,
        "near the top of the lane: {}",
        map[1].bpm
    );
    assert_eq!(v.selected_tempo_point(), Some(1));
}

#[test]
fn a_double_click_adds_a_point_with_the_selection_tool() {
    let doc = song_doc();
    let mut v = with_lane(&doc);
    let rect = v.tempo_lane_rect();
    let p = Point::new(v.tick_to_x(BAR), rect.center().y);
    let a = handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    assert_eq!(tempo_maps(&a).len(), 1);
    // A single click with the selection tool adds nothing.
    let mut v = with_lane(&doc);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert!(tempo_maps(&a).is_empty());
}

#[test]
fn no_point_is_added_on_tick_zero_or_on_an_existing_tick() {
    let doc = three_points();
    let mut v = with_lane(&doc);
    v.set_tool(Tool::Pencil);
    let rect = v.tempo_lane_rect();
    // The pencil at the left edge (tick 0) and under the point at 2 bars (but far from it
    // vertically, so the click is on the lane and not on the point) adds nothing.
    let bottom = rect.y1 - 4.0;
    for x in [v.tick_to_x(0), v.tick_to_x(2 * BAR)] {
        let p = Point::new(x, bottom);
        let a = handle(&mut v, &doc, &[down(p), up(p)]);
        assert!(tempo_maps(&a).is_empty(), "x {x}: {a:?}");
    }
}

#[test]
fn dragging_a_point_moves_it_in_ticks_and_bpm_as_one_merged_step() {
    let doc = three_points();
    let mut v = with_lane(&doc);
    let rect = v.tempo_lane_rect();
    let range = lane::bpm_range(&doc.song.tempo);
    let p = v.tempo_point_center(1).unwrap();
    let to = Point::new(v.tick_to_x(3 * BAR), lane::bpm_to_y(150.0, rect, range));
    let mid = Point::new(to.x - 10.0, to.y + 4.0);
    let a = handle(&mut v, &doc, &[down(p), mv(mid), mv(to), up(to)]);
    let steps: Vec<(Vec<TempoEvent>, u64)> = song_commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::DragTempoMap { map, merge } => Some((map, merge)),
            _ => None,
        })
        .collect();
    assert_eq!(steps.len(), 2, "{a:?}");
    assert_eq!(steps[0].1, steps[1].1, "one merge id for the gesture");
    let last = &steps[1].0;
    assert_eq!(last[1], TempoEvent::step(3 * BAR, 150.0));
    assert_eq!((last[0], last[2]), (doc.song.tempo[0], doc.song.tempo[2]));
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    assert_eq!(v.selected_tempo_point(), Some(1));
}

#[test]
fn a_dragged_point_stays_between_its_neighbours_and_shift_gives_tenths() {
    let doc = three_points();
    let mut v = with_lane(&doc);
    let rect = v.tempo_lane_rect();
    let p = v.tempo_point_center(1).unwrap();
    // Far past the last point: one tick before it.
    let far = Point::new(v.tick_to_x(9 * BAR), p.y);
    let a = handle(&mut v, &doc, &[down(p), mv(far), up(far)]);
    assert_eq!(tempo_maps(&a)[0][1].tick, 4 * BAR - 1);
    // Fine: tenths of a BPM.
    let mut v = with_lane(&doc);
    let range = lane::bpm_range(&doc.song.tempo);
    let p = v.tempo_point_center(1).unwrap();
    let to = Point::new(p.x, lane::bpm_to_y(150.37, rect, range));
    let fine = Modifiers::SHIFT;
    let a = handle(&mut v, &doc, &[down(p), mv_with(to, fine), up(to)]);
    let bpm = tempo_maps(&a)[0][1].bpm;
    assert!((bpm - 150.4).abs() < 1e-9, "{bpm}");
    // Whole BPMs without Shift.
    let mut v = with_lane(&doc);
    let a = handle(&mut v, &doc, &[down(p), mv(to), up(to)]);
    assert_eq!(tempo_maps(&a)[0][1].bpm, 150.0);
}

#[test]
fn the_first_point_only_moves_vertically_and_escape_restores_the_map() {
    let doc = three_points();
    let mut v = with_lane(&doc);
    let rect = v.tempo_lane_rect();
    let p = v.tempo_point_center(0).unwrap();
    let to = Point::new(p.x + 60.0, rect.y0 + 12.0);
    let a = handle(&mut v, &doc, &[down(p), mv(to)]);
    let first = tempo_maps(&a)[0][0];
    assert_eq!(first.tick, 0);
    assert!(first.bpm > 100.0);
    // Escape: the original map goes back as part of the same merged gesture.
    let reg = registry();
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(tempo_maps(&a), vec![doc.song.tempo.clone()]);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    assert!(!v.is_dragging());
}

#[test]
fn a_press_without_a_move_sends_nothing() {
    let doc = three_points();
    let mut v = with_lane(&doc);
    let p = v.tempo_point_center(1).unwrap();
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert!(a.is_empty(), "{a:?}");
    assert_eq!(v.selected_tempo_point(), Some(1));
}

#[test]
fn the_eraser_and_the_delete_command_remove_a_point_but_not_the_first() {
    let doc = three_points();
    let mut v = with_lane(&doc);
    v.set_tool(Tool::Eraser);
    let p1 = v.tempo_point_center(1).unwrap();
    let a = handle(&mut v, &doc, &[down(p1), up(p1)]);
    assert_eq!(
        tempo_maps(&a),
        vec![vec![doc.song.tempo[0], doc.song.tempo[2]]]
    );
    let p0 = v.tempo_point_center(0).unwrap();
    let a = handle(&mut v, &doc, &[down(p0), up(p0)]);
    assert!(tempo_maps(&a).is_empty());
    // Delete on a selected point.
    let mut v = with_lane(&doc);
    let p2 = v.tempo_point_center(2).unwrap();
    handle(&mut v, &doc, &[down(p2), up(p2)]);
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "edit.delete", None) else {
        panic!("edit.delete takes the selected tempo point");
    };
    assert_eq!(tempo_maps(&a), vec![doc.song.tempo[..2].to_vec()]);
    assert_eq!(v.selected_tempo_point(), None);
}

#[test]
fn the_context_menu_commands_ramp_set_and_delete() {
    let doc = three_points();
    let mut v = with_lane(&doc);
    let p1 = v.tempo_point_center(1).unwrap();
    let a = handle(&mut v, &doc, &[down_with(p1, PointerButton::Secondary, 1)]);
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::TempoPoint,
        at: p1
    }));
    let menu = v.tempo_menu();
    assert!(menu.can_ramp && menu.can_delete && !menu.ramped);
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.tempo_toggle_ramp", None)
    else {
        panic!("ramp");
    };
    let map = &tempo_maps(&a)[0];
    assert!(map[1].ramp && !map[0].ramp && !map[2].ramp);
    // Set tempo: the app starts the transport's entry for that event.
    handle(&mut v, &doc, &[down_with(p1, PointerButton::Secondary, 1)]);
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.tempo_set_value", None)
    else {
        panic!("set");
    };
    assert_eq!(a, vec![UiAction::EditTempoPoint(1)]);
    // Delete from the menu.
    handle(&mut v, &doc, &[down_with(p1, PointerButton::Secondary, 1)]);
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.tempo_delete_point", None)
    else {
        panic!("delete");
    };
    assert_eq!(tempo_maps(&a)[0].len(), 2);
    // The last point cannot ramp; the first cannot be deleted.
    let p2 = v.tempo_point_center(2).unwrap();
    handle(&mut v, &doc, &[down_with(p2, PointerButton::Secondary, 1)]);
    assert!(!v.tempo_menu().can_ramp);
    assert!(matches!(
        command(&mut v, &doc, "sequencer.tempo_toggle_ramp", None),
        CommandOutcome::NotApplicable(_)
    ));
    let p0 = v.tempo_point_center(0).unwrap();
    handle(&mut v, &doc, &[down_with(p0, PointerButton::Secondary, 1)]);
    assert!(!v.tempo_menu().can_delete);
    assert!(matches!(
        command(&mut v, &doc, "sequencer.tempo_delete_point", None),
        CommandOutcome::NotApplicable(_)
    ));
}

#[test]
fn the_lane_menu_adds_a_point_on_the_curve() {
    let mut doc = song_doc();
    doc.song.tempo = vec![ramp(0, 60.0), TempoEvent::step(2 * BAR, 180.0)];
    let mut v = with_lane(&doc);
    let rect = v.tempo_lane_rect();
    let p = Point::new(v.tick_to_x(BAR), rect.y0 + 5.0);
    let a = handle(&mut v, &doc, &[down_with(p, PointerButton::Secondary, 1)]);
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::TempoLane,
        at: p
    }));
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.tempo_add_point", None)
    else {
        panic!("add");
    };
    let map = &tempo_maps(&a)[0];
    assert_eq!(map.len(), 3);
    assert_eq!(map[1].tick, BAR);
    // Halfway up the ramp: 120 BPM, and the ramp goes on through the new point.
    assert!((map[1].bpm - 120.0).abs() < 1e-9, "{}", map[1].bpm);
    assert!(map[0].ramp && map[1].ramp && !map[2].ramp);
}

// ---------------------------------------------------------------- meter changes

#[test]
fn a_ruler_menu_inserts_a_meter_change_at_the_bar_under_the_pointer() {
    let doc = song_doc();
    let mut v = with_lane(&doc);
    let p = Point::new(v.tick_to_x(2 * BAR + PPQ), v.ruler_label_y());
    let a = handle(&mut v, &doc, &[down_with(p, PointerButton::Secondary, 1)]);
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::RulerMeter,
        at: p
    }));
    assert_eq!(v.context_bar(), 3);
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.meter_insert", Some("3/4"))
    else {
        panic!("insert");
    };
    assert_eq!(meter_maps(&a), vec![vec![mev(1, 4, 4), mev(3, 3, 4)]]);
    for bad in ["3/5", "x", "0/4", "40/4"] {
        assert!(
            !matches!(
                command(&mut v, &doc, "sequencer.meter_insert", Some(bad)),
                CommandOutcome::Handled(_)
            ),
            "{bad}"
        );
    }
}

fn with_changes() -> RackDoc {
    let mut doc = song_doc();
    doc.song.meter = vec![mev(1, 4, 4), mev(3, 3, 4), mev(6, 6, 8)];
    doc
}

#[test]
fn a_meter_tag_drags_to_another_bar_as_one_merged_step() {
    let doc = with_changes();
    let mut v = with_lane(&doc);
    let tag = v.meter_tag_center(1).unwrap();
    // Bars of the map without the change: 4/4 all the way. Bar 5 starts at 4 * BAR.
    let to = Point::new(v.tick_to_x(4 * BAR) + 2.0, tag.y);
    let mid = Point::new(v.tick_to_x(3 * BAR) + 2.0, tag.y);
    let a = handle(&mut v, &doc, &[down(tag), mv(mid), mv(to), up(to)]);
    let steps: Vec<(Vec<MeterEvent>, u64)> = song_commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::DragMeterMap { map, merge } => Some((map, merge)),
            _ => None,
        })
        .collect();
    assert_eq!(steps.len(), 2, "{a:?}");
    assert_eq!(steps[0].1, steps[1].1);
    assert_eq!(steps[1].0, vec![mev(1, 4, 4), mev(5, 3, 4), mev(6, 6, 8)]);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn the_first_meter_has_no_drag_and_the_commands_set_and_delete() {
    let doc = with_changes();
    let mut v = with_lane(&doc);
    // The first change is 4/4 (no tag); the second can be set to another signature.
    let tag = v.meter_tag_center(1).unwrap();
    handle(&mut v, &doc, &[down_with(tag, PointerButton::Secondary, 1)]);
    assert!(v.meter_change_deletable());
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.meter_set", Some("5/4"))
    else {
        panic!("set");
    };
    assert_eq!(
        meter_maps(&a),
        vec![vec![mev(1, 4, 4), mev(3, 5, 4), mev(6, 6, 8)]]
    );
    handle(&mut v, &doc, &[down_with(tag, PointerButton::Secondary, 1)]);
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.meter_delete", None) else {
        panic!("delete");
    };
    assert_eq!(meter_maps(&a), vec![vec![mev(1, 4, 4), mev(6, 6, 8)]]);
    // Eraser on a tag.
    let mut v = with_lane(&doc);
    v.set_tool(Tool::Eraser);
    let tag = v.meter_tag_center(2).unwrap();
    let a = handle(&mut v, &doc, &[down(tag), up(tag)]);
    assert_eq!(meter_maps(&a), vec![vec![mev(1, 4, 4), mev(3, 3, 4)]]);
}

#[test]
fn bar_one_can_be_set_through_the_ruler_menu() {
    let mut doc = song_doc();
    doc.song.meter = vec![mev(1, 7, 8)];
    let mut v = with_lane(&doc);
    let tag = v.meter_tag_center(0).unwrap();
    handle(&mut v, &doc, &[down_with(tag, PointerButton::Secondary, 1)]);
    assert!(!v.meter_change_deletable(), "the first change stays");
    let CommandOutcome::Handled(a) = command(&mut v, &doc, "sequencer.meter_set", Some("4/4"))
    else {
        panic!("set");
    };
    assert_eq!(meter_maps(&a), vec![vec![mev(1, 4, 4)]]);
}

// ---------------------------------------------------------------- the grid follows the meter

#[test]
fn the_snap_grid_and_new_clip_length_follow_the_meter_map() {
    let mut doc = song_doc();
    doc.song.meter = vec![mev(1, 4, 4), mev(2, 3, 4)];
    let reg = registry();
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    // From bar 2 the bars are 3/4: bar lines at 4 PPQ + k * 3 PPQ. A double-click in the free
    // stretch of the lead lane floors to a bar line and the new clip is one 3/4 bar long.
    let lead = v.lane_rect(&key("lead"), 0).unwrap();
    let tick = 8 * BAR + PPQ; // 33 PPQ: between the bar lines at 31 and 34 PPQ
    let p = Point::new(v.tick_to_x(tick), lead.center().y);
    let a = handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    let cmds = commands(&a);
    let Some(Command::Song(SongCommand::AddClip { clip, .. })) = cmds.first() else {
        panic!("a new clip: {a:?}");
    };
    assert_eq!(clip.start, 31 * PPQ);
    assert_eq!(clip.length, 3 * PPQ);
}

#[test]
fn the_ruler_click_snaps_the_locate_to_the_meter_map() {
    let mut doc = song_doc();
    doc.song.meter = vec![mev(1, 4, 4), mev(2, 3, 4)];
    let reg = registry();
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    // Just before bar 4 (BAR + 2 * 3 PPQ): the nearest bar line is bar 4.
    let tick = BAR + 6 * PPQ - 100;
    let p = Point::new(v.tick_to_x(tick), v.ruler_label_y());
    let a = v.handle(&down(p), &doc, &reg);
    assert!(
        a.contains(&UiAction::Transport(atomik_ui::TransportAction::Locate(
            BAR + 6 * PPQ
        ))),
        "{a:?}"
    );
}

// ---------------------------------------------------------------- the transport follows the playhead

#[test]
fn the_transport_shows_and_edits_the_tempo_in_effect_at_the_playhead() {
    let mut doc = three_points();
    doc.song.meter = vec![mev(1, 4, 4), mev(3, 3, 4)];
    let reg = registry();
    let mut bar = transport_bar(&doc, &reg);
    let at = |bar: &mut atomik_ui::TransportBar, position: i64| {
        bar.set_state(TransportState {
            position,
            ..TransportState::default()
        });
    };
    let shown = |bar: &mut atomik_ui::TransportBar| -> Vec<String> {
        let mut dl = DisplayList::new();
        bar.paint(&mut dl, &doc, &reg);
        dl.texts().into_iter().map(str::to_owned).collect()
    };
    at(&mut bar, 0);
    let t = shown(&mut bar);
    assert!(
        t.contains(&"100.00".to_owned()) && t.contains(&"4/4".to_owned()),
        "{t:?}"
    );
    at(&mut bar, 2 * BAR + 100);
    let t = shown(&mut bar);
    assert!(
        t.contains(&"140.00".to_owned()) && t.contains(&"3/4".to_owned()),
        "{t:?}"
    );
    // Double-click and type: the event in effect (the second) changes, the others stay.
    let c = bar.tempo_center();
    let mut ev = vec![down_mods(c, Modifiers::NONE, 2), up(c)];
    ev.extend(
        "150"
            .chars()
            .map(|ch| press(Key::Char(ch), Modifiers::NONE)),
    );
    ev.push(press(Key::Enter, Modifiers::NONE));
    let a = run(|e| bar.handle(e, &doc, &reg), &ev);
    let map = &tempo_maps(&a)[0];
    assert_eq!(map[0].bpm, 100.0);
    assert_eq!(map[1].bpm, 150.0);
    assert_eq!(map[2].bpm, 90.0);
}

#[test]
fn begin_tempo_edit_types_into_a_chosen_event_and_a_ramp_shows_the_tempo_inside_it() {
    let mut doc = song_doc();
    doc.song.tempo = vec![ramp(0, 60.0), TempoEvent::step(2 * BAR, 180.0)];
    let reg = registry();
    let mut bar = transport_bar(&doc, &reg);
    bar.set_state(TransportState {
        position: BAR,
        ..TransportState::default()
    });
    let mut dl = DisplayList::new();
    bar.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"120.00"), "{:?}", dl.texts());
    bar.begin_tempo_edit(Some(1));
    assert!(bar.is_editing_tempo());
    assert_eq!(bar.tempo_text(), Some("180"));
    let ev: Vec<UiEvent> = "200"
        .chars()
        .map(|ch| press(Key::Char(ch), Modifiers::NONE))
        .chain([press(Key::Enter, Modifiers::NONE)])
        .collect();
    let a = run(|e| bar.handle(e, &doc, &reg), &ev);
    let map = &tempo_maps(&a)[0];
    assert!(map[0].ramp, "the ramp stays");
    assert_eq!(map[1].bpm, 200.0);
    // The limit clamps.
    bar.begin_tempo_edit(None);
    let ev: Vec<UiEvent> = "9999"
        .chars()
        .map(|ch| press(Key::Char(ch), Modifiers::NONE))
        .chain([press(Key::Enter, Modifiers::NONE)])
        .collect();
    let a = run(|e| bar.handle(e, &doc, &reg), &ev);
    assert_eq!(tempo_maps(&a)[0][0].bpm, TEMPO_MAX);
}
