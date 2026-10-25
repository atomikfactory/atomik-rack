//! Markers and range markers of the arrangement: the strip under the ruler (add, move,
//! rename, delete, ranges, loop from a range), the commands and menus behind them, the key
//! commands and the marker list.

mod support;

use atomik_model::{ClipId, Marker, MeterEvent, RackDoc, RangeMarker, SongCommand};
use atomik_ui::action::{ContextTarget, TransportAction};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::marker_edit::{self as edit, MarkSel, RangePart};
use atomik_ui::timeline::Snap;
use atomik_ui::viewport::Tool;
use atomik_ui::{
    ArrangementView, DisplayList, Key, MARKER_LIST_WIDTH, MARKER_STRIP_HEIGHT, Modifiers,
    PointerButton, UiAction, UiEvent,
};
use support::song::*;
use support::*;

fn marker(id: u32, tick: i64, name: &str) -> Marker {
    Marker {
        id,
        tick,
        name: name.into(),
        color: None,
    }
}

fn range(id: u32, start: i64, end: i64, name: &str) -> RangeMarker {
    RangeMarker {
        id,
        start,
        end,
        name: name.into(),
        color: None,
    }
}

/// The song with two markers (bars 2 and 5, ids 1 and 2) and two ranges (bars 1 to 3 and
/// 5 to 9, ids 1 and 2).
fn marked() -> RackDoc {
    let mut doc = song_doc();
    doc.song.markers = vec![marker(1, BAR, "Drop"), marker(2, 4 * BAR, "Break")];
    doc.song.ranges = vec![
        range(1, 0, 2 * BAR, "Intro"),
        range(2, 4 * BAR, 8 * BAR, "Verse"),
    ];
    doc
}

fn with_strip(doc: &RackDoc) -> ArrangementView {
    let mut v = arrangement(doc, &registry());
    v.set_marker_strip(true);
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

fn key_event(k: Key) -> UiEvent {
    press(k, Modifiers::NONE)
}

fn typed(text: &str) -> Vec<UiEvent> {
    text.chars().map(|c| key_event(Key::Char(c))).collect()
}

/// The marker lists the song commands among `actions` set.
fn marker_lists(actions: &[UiAction]) -> Vec<Vec<Marker>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetMarkers(m) | SongCommand::DragMarkers { markers: m, .. } => Some(m),
            _ => None,
        })
        .collect()
}

fn range_lists(actions: &[UiAction]) -> Vec<Vec<RangeMarker>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetRanges(r) | SongCommand::DragRanges { ranges: r, .. } => Some(r),
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

fn locates(actions: &[UiAction]) -> Vec<i64> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Transport(TransportAction::Locate(t)) => Some(*t),
            _ => None,
        })
        .collect()
}

fn marker_point(v: &ArrangementView, id: u32) -> Point {
    v.marker_rect(id).unwrap().center()
}

fn range_point(v: &ArrangementView, id: u32) -> Point {
    v.range_rect(id).unwrap().center()
}

fn strip_point(v: &ArrangementView, tick: i64, markers_row: bool) -> Point {
    let r = v.marker_strip_rect();
    let y = if markers_row { r.y1 - 4.0 } else { r.y0 + 4.0 };
    Point::new(v.tick_to_x(tick), y)
}

// ---------------------------------------------------------------- pure edits

#[test]
fn auto_names_count_up_past_the_highest_number_in_use() {
    assert_eq!(edit::auto_name("Marker", []), "Marker 1");
    assert_eq!(
        edit::auto_name("Marker", ["Marker 1", "Marker 2"]),
        "Marker 3"
    );
    assert_eq!(
        edit::auto_name("Marker", ["Marker 7", "Drop", "Marker x", "Marker"]),
        "Marker 8"
    );
    assert_eq!(edit::auto_name("Range", ["Marker 4"]), "Range 1");
}

#[test]
fn names_are_trimmed_and_cut_and_an_empty_one_is_refused() {
    assert_eq!(edit::clean_name("  Verse  ").as_deref(), Some("Verse"));
    assert_eq!(edit::clean_name("   "), None);
    let long = "x".repeat(200);
    assert_eq!(edit::clean_name(&long).unwrap().chars().count(), 64);
}

#[test]
fn the_edits_keep_the_lists_sorted_and_the_ids_stable() {
    let m = vec![marker(1, 100, "a"), marker(2, 300, "b")];
    let (added, id) = edit::add_marker(&m, 200, "c").unwrap();
    assert_eq!(id, 3);
    assert_eq!(
        added.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![1, 3, 2]
    );
    assert!(
        edit::add_marker(&m, 100, "dup").is_none(),
        "one marker per tick"
    );
    assert!(edit::add_marker(&m, -1, "x").is_none());
    let moved = edit::move_marker(&m, 1, 500);
    assert_eq!(moved.iter().map(|m| m.id).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(edit::move_marker(&m, 1, -9)[0].tick, 0);
    assert_eq!(edit::delete_marker(&m, 1).len(), 1);
    assert_eq!(edit::rename_marker(&m, 2, " z ").unwrap()[1].name, "z");
    assert!(edit::rename_marker(&m, 2, "  ").is_none());
    assert_eq!(
        edit::color_marker(&m, 1, Some([1, 2, 3]))[0].color,
        Some([1, 2, 3])
    );
}

#[test]
fn ranges_move_keeping_their_length_and_resize_by_an_edge() {
    let r = vec![range(1, 100, 400, "a"), range(2, 500, 900, "b")];
    let moved = edit::move_range(&r, 1, 600);
    assert_eq!(
        (moved[1].id, moved[1].start, moved[1].end),
        (1, 600, 900),
        "sorted by start: {moved:?}"
    );
    assert_eq!(edit::move_range(&r, 1, -50)[0].start, 0);
    let a = edit::resize_range(&r, 1, RangePart::Start, 350, 10);
    assert_eq!((a[0].start, a[0].end), (350, 400));
    let a = edit::resize_range(&r, 1, RangePart::Start, 999, 10);
    assert_eq!(a[0].start, 390, "stays a minimum below the end");
    let a = edit::resize_range(&r, 1, RangePart::End, 50, 10);
    assert_eq!(a[0].end, 110);
    assert!(edit::add_range(&r, 5, 5, "x").is_none());
    assert!(edit::add_range(&r, 5, 9, "x").is_some());
    assert_eq!(edit::delete_range(&r, 2).len(), 1);
}

#[test]
fn palette_names_map_to_colours() {
    assert_eq!(edit::palette_color("none"), Some(None));
    assert_eq!(edit::palette_color("red"), Some(Some(edit::PALETTE[0].1)));
    assert_eq!(edit::palette_color("plaid"), None);
}

// ---------------------------------------------------------------- layout

#[test]
fn the_strip_is_off_by_default_and_pushes_the_lanes_down_when_shown() {
    let (doc, reg) = (marked(), registry());
    let mut v = arrangement(&doc, &reg);
    assert!(!v.marker_strip_visible());
    assert_eq!(v.marker_strip_rect().height(), 0.0);
    let before = v.lane_rect(&key("bass"), 0).unwrap();
    v.set_marker_strip(true);
    assert_eq!(v.marker_strip_rect().height(), MARKER_STRIP_HEIGHT);
    let after = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((after.y0 - before.y0 - MARKER_STRIP_HEIGHT).abs() < 1e-9);
    // Right under the ruler, above the tempo lane when both are shown.
    assert!((v.marker_strip_rect().y0 - 34.0).abs() < 1e-9);
    v.set_tempo_lane(true);
    assert!((v.tempo_lane_rect().y0 - v.marker_strip_rect().y1).abs() < 1e-9);
}

#[test]
fn markers_and_ranges_sit_at_their_ticks_in_their_rows() {
    let doc = marked();
    let v = with_strip(&doc);
    let strip = v.marker_strip_rect();
    let m = v.marker_rect(2).unwrap();
    assert!((m.x0 - v.tick_to_x(4 * BAR)).abs() < 1e-9);
    assert!(m.y0 >= strip.y0 + MARKER_STRIP_HEIGHT / 2.0, "bottom row");
    let r = v.range_rect(2).unwrap();
    assert!((r.x0 - v.tick_to_x(4 * BAR)).abs() < 1e-9);
    assert!((r.x1 - v.tick_to_x(8 * BAR)).abs() < 1e-9);
    assert!(r.y1 <= strip.y0 + MARKER_STRIP_HEIGHT / 2.0, "top row");
    assert!(v.marker_rect(9).is_none() && v.range_rect(9).is_none());
}

#[test]
fn the_strip_paints_the_names() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    let texts = dl.texts();
    for t in ["Drop", "Break", "Intro", "Verse", "RANGES", "MARKERS"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
}

// ---------------------------------------------------------------- adding

#[test]
fn add_marker_puts_a_numbered_marker_at_the_playhead_and_selects_it() {
    let doc = song_doc();
    let mut v = with_strip(&doc);
    v.set_playhead(2 * BAR + 7);
    let a = command(&mut v, &doc, "transport.add_marker", None);
    let lists = marker_lists(a.actions());
    assert_eq!(lists.len(), 1, "{a:?}");
    assert_eq!(lists[0], vec![marker(1, 2 * BAR + 7, "Marker 1")]);
    assert_eq!(v.selected_marker(), Some(MarkSel::Marker(1)));
}

#[test]
fn a_second_marker_is_numbered_on_and_one_per_tick() {
    let doc = marked();
    let mut v = with_strip(&doc);
    v.set_playhead(2 * BAR);
    let a = command(&mut v, &doc, "transport.add_marker", None);
    assert_eq!(marker_lists(a.actions())[0][1].name, "Marker 1");
    let mut doc2 = doc.clone();
    doc2.song.markers = marker_lists(a.actions()).remove(0);
    v.sync(&doc2, &registry());
    v.set_playhead(3 * BAR);
    let b = command(&mut v, &doc2, "transport.add_marker", None);
    assert!(
        marker_lists(b.actions())[0]
            .iter()
            .any(|m| m.name == "Marker 2")
    );
    // A marker on the playhead already: nothing to add.
    v.set_playhead(BAR);
    let c = command(&mut v, &doc2, "transport.add_marker", None);
    assert!(matches!(c, CommandOutcome::NotApplicable(_)), "{c:?}");
}

#[test]
fn a_double_click_in_the_markers_row_adds_a_marker_at_the_snapped_tick() {
    let doc = song_doc();
    let mut v = with_strip(&doc);
    let p = strip_point(&v, 3 * BAR + PPQ / 3, true);
    let a = handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    let lists = marker_lists(&a);
    assert_eq!(lists.len(), 1, "{a:?}");
    assert_eq!(lists[0][0].tick, 3 * BAR, "snapped to the beat before");
    assert_eq!(lists[0][0].name, "Marker 1");
    // A single click adds nothing.
    let mut v = with_strip(&doc);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert!(marker_lists(&a).is_empty() && range_lists(&a).is_empty());
}

#[test]
fn the_pencil_adds_a_marker_with_one_click() {
    let doc = song_doc();
    let mut v = with_strip(&doc);
    v.set_tool(Tool::Pencil);
    let p = strip_point(&v, 2 * BAR, true);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(marker_lists(&a)[0][0].tick, 2 * BAR);
}

#[test]
fn a_double_click_in_the_ranges_row_adds_a_one_bar_range() {
    let doc = song_doc();
    let mut v = with_strip(&doc);
    let p = strip_point(&v, 3 * BAR, false);
    let a = handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    let lists = range_lists(&a);
    assert_eq!(lists.len(), 1, "{a:?}");
    assert_eq!((lists[0][0].start, lists[0][0].end), (3 * BAR, 4 * BAR));
    assert_eq!(lists[0][0].name, "Range 1");
}

#[test]
fn add_range_from_the_loop_and_from_the_selection() {
    let mut doc = song_doc();
    doc.song.loop_range = (BAR, 3 * BAR);
    let mut v = with_strip(&doc);
    let a = command(&mut v, &doc, "transport.add_range_from_loop", None);
    let lists = range_lists(a.actions());
    assert_eq!(
        (
            lists[0][0].start,
            lists[0][0].end,
            lists[0][0].name.as_str()
        ),
        (BAR, 3 * BAR, "Range 1")
    );
    assert_eq!(v.selected_marker(), Some(MarkSel::Range(1)));
    // From the selected clips (clip 2 is the lead's, bars 2 to 5).
    let mut v = with_strip(&doc);
    v.set_selected_clips(&[ClipId(2)]);
    let a = command(&mut v, &doc, "sequencer.marker_range_from_selection", None);
    let lists = range_lists(a.actions());
    assert_eq!((lists[0][0].start, lists[0][0].end), (BAR, 4 * BAR));
    // Nothing selected: nothing to do.
    let mut v = with_strip(&doc);
    let a = command(&mut v, &doc, "sequencer.marker_range_from_selection", None);
    assert!(matches!(a, CommandOutcome::NotApplicable(_)));
}

// ---------------------------------------------------------------- clicking and dragging

#[test]
fn a_click_on_a_marker_selects_it_and_locates_the_playhead() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = marker_point(&v, 2);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(locates(&a), vec![4 * BAR], "{a:?}");
    assert_eq!(v.selected_marker(), Some(MarkSel::Marker(2)));
    assert!(marker_lists(&a).is_empty());
}

#[test]
fn dragging_a_marker_moves_it_snapped_as_one_merged_step() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = marker_point(&v, 2);
    let to = Point::new(p.x + 2.0 * PPQ as f64 * v.px_per_tick() + 1.0, p.y);
    let mid = Point::new(p.x + 20.0, p.y);
    let a = handle(&mut v, &doc, &[down(p), mv(mid), mv(to), up(to)]);
    let steps: Vec<(Vec<Marker>, u64)> = song_commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::DragMarkers { markers, merge } => Some((markers, merge)),
            _ => None,
        })
        .collect();
    assert!(steps.len() >= 2, "{a:?}");
    assert!(steps.iter().all(|s| s.1 == steps[0].1), "one merge id");
    let last = &steps.last().unwrap().0;
    let moved = last.iter().find(|m| m.id == 2).unwrap();
    assert_eq!(moved.tick, 4 * BAR + 2 * PPQ, "snapped to the beat");
    assert_eq!(last.iter().find(|m| m.id == 1).unwrap().tick, BAR);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    assert!(locates(&a).is_empty(), "a drag does not locate");
}

#[test]
fn the_disable_snap_role_drags_a_marker_free_and_it_never_goes_below_zero() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = marker_point(&v, 1);
    let to = Point::new(p.x + 7.0, p.y);
    // Ctrl+Alt in the standard roles (Alt alone in the legacy ones).
    let free = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let a = handle(&mut v, &doc, &[down(p), mv_with(to, free), up(to)]);
    let tick = marker_lists(&a).last().unwrap()[0].tick;
    assert_ne!(tick % (PPQ / 4), 0, "no snap: {tick}");
    assert!(tick > BAR);
    // Far to the left: tick 0.
    let mut v = with_strip(&doc);
    let far = Point::new(v.tick_to_x(0) - 100.0, p.y);
    let a = handle(&mut v, &doc, &[down(p), mv(far), up(far)]);
    assert_eq!(marker_lists(&a).last().unwrap()[0].tick, 0);
}

#[test]
fn escape_during_a_marker_drag_puts_the_list_back() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = marker_point(&v, 1);
    let to = Point::new(p.x + 80.0, p.y);
    let a = handle(
        &mut v,
        &doc,
        &[down(p), mv(to), key_event(Key::Escape), up(to)],
    );
    let lists = marker_lists(&a);
    assert!(lists.len() >= 2, "{a:?}");
    assert_eq!(lists.last().unwrap(), &doc.song.markers);
    assert!(a.contains(&UiAction::EndMerge));
}

#[test]
fn a_click_on_a_range_sets_the_loop_to_it() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = range_point(&v, 2);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(loops(&a), vec![((4 * BAR, 8 * BAR), true)], "{a:?}");
    assert_eq!(v.selected_marker(), Some(MarkSel::Range(2)));
    assert!(range_lists(&a).is_empty());
}

#[test]
fn dragging_a_range_body_moves_it_and_keeps_its_length() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = range_point(&v, 2);
    let to = Point::new(p.x + BAR as f64 * v.px_per_tick(), p.y);
    let a = handle(&mut v, &doc, &[down(p), mv(to), up(to)]);
    let r = range_lists(&a).last().unwrap().clone();
    let r2 = r.iter().find(|r| r.id == 2).unwrap();
    assert_eq!((r2.start, r2.end), (5 * BAR, 9 * BAR), "{r:?}");
    assert!(loops(&a).is_empty(), "a drag does not set the loop");
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn dragging_the_edges_of_a_range_resizes_it() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let g = v.range_rect(2).unwrap();
    // The right edge two beats to the right.
    let p = Point::new(g.x1 - 2.0, g.center().y);
    let to = Point::new(p.x + 2.0 * PPQ as f64 * v.px_per_tick(), p.y);
    let a = handle(&mut v, &doc, &[down(p), mv(to), up(to)]);
    let r2 = range_lists(&a)
        .last()
        .unwrap()
        .iter()
        .find(|r| r.id == 2)
        .cloned()
        .unwrap();
    assert_eq!((r2.start, r2.end), (4 * BAR, 8 * BAR + 2 * PPQ));
    // The left edge a bar to the right.
    let mut v = with_strip(&doc);
    let p = Point::new(g.x0 + 2.0, g.center().y);
    let to = Point::new(p.x + BAR as f64 * v.px_per_tick(), p.y);
    let a = handle(&mut v, &doc, &[down(p), mv(to), up(to)]);
    let r2 = range_lists(&a)
        .last()
        .unwrap()
        .iter()
        .find(|r| r.id == 2)
        .cloned()
        .unwrap();
    assert_eq!((r2.start, r2.end), (5 * BAR, 8 * BAR));
    // Past the other edge: a minimum length is kept.
    let mut v = with_strip(&doc);
    let to = Point::new(v.tick_to_x(9 * BAR), p.y);
    let a = handle(&mut v, &doc, &[down(p), mv(to), up(to)]);
    let r2 = range_lists(&a)
        .last()
        .unwrap()
        .iter()
        .find(|r| r.id == 2)
        .cloned()
        .unwrap();
    assert!(r2.start < r2.end && r2.end == 8 * BAR, "{r2:?}");
}

// ---------------------------------------------------------------- rename

#[test]
fn a_double_click_on_a_label_types_a_new_name_in_place() {
    let doc = marked();
    let mut v = with_strip(&doc);
    assert!(!v.is_text_owner());
    let p = marker_point(&v, 1);
    handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    assert!(v.is_text_owner());
    assert_eq!(v.marker_rename_text(), Some("Drop"));
    let mut events = vec![key_event(Key::Backspace); 4];
    events.extend(typed("Chorus 2"));
    handle(&mut v, &doc, &events);
    assert_eq!(v.marker_rename_text(), Some("Chorus 2"));
    let a = handle(&mut v, &doc, &[key_event(Key::Enter)]);
    assert!(!v.is_text_owner());
    let list = &marker_lists(&a)[0];
    assert_eq!(list[0].name, "Chorus 2");
    assert_eq!(list[1].name, "Break");
}

#[test]
fn escape_an_empty_name_or_the_same_name_rename_nothing() {
    let doc = marked();
    let mut v = with_strip(&doc);
    assert!(v.begin_marker_rename(MarkSel::Marker(1)));
    handle(&mut v, &doc, &typed("zzz"));
    let a = handle(&mut v, &doc, &[key_event(Key::Escape)]);
    assert!(marker_lists(&a).is_empty() && !v.is_text_owner());
    assert!(v.begin_marker_rename(MarkSel::Marker(1)));
    handle(&mut v, &doc, &vec![key_event(Key::Backspace); 10]);
    let a = handle(&mut v, &doc, &[key_event(Key::Enter)]);
    assert!(marker_lists(&a).is_empty(), "an empty name cancels");
    assert!(v.begin_marker_rename(MarkSel::Marker(1)));
    let a = handle(&mut v, &doc, &[key_event(Key::Enter)]);
    assert!(marker_lists(&a).is_empty(), "an unchanged name is no edit");
    assert!(!v.begin_marker_rename(MarkSel::Marker(77)));
}

#[test]
fn the_name_stops_at_the_model_limit_and_modified_keys_are_not_typed() {
    let doc = marked();
    let mut v = with_strip(&doc);
    v.begin_marker_rename(MarkSel::Range(1));
    handle(&mut v, &doc, &typed(&"y".repeat(100)));
    assert_eq!(v.marker_rename_text().unwrap().chars().count(), 64);
    let before = v.marker_rename_text().unwrap().to_owned();
    handle(&mut v, &doc, &[press(Key::Char('q'), Modifiers::CTRL)]);
    assert_eq!(v.marker_rename_text().unwrap(), before);
}

#[test]
fn a_press_elsewhere_commits_the_name_and_the_field_keeps_clicks_on_itself() {
    let doc = marked();
    let mut v = with_strip(&doc);
    v.begin_marker_rename(MarkSel::Marker(2));
    handle(&mut v, &doc, &typed("!"));
    // A click on the field keeps typing.
    let on_field = marker_point(&v, 2);
    let a = handle(&mut v, &doc, &[down(on_field), up(on_field)]);
    assert!(v.is_text_owner() && marker_lists(&a).is_empty());
    // A press on empty strip commits.
    let away = strip_point(&v, 9 * BAR, true);
    let a = handle(&mut v, &doc, &[down(away), up(away)]);
    assert!(!v.is_text_owner());
    assert_eq!(marker_lists(&a)[0][1].name, "Break!");
}

#[test]
fn losing_focus_drops_the_edit() {
    let doc = marked();
    let mut v = with_strip(&doc);
    v.set_focused(true);
    v.begin_marker_rename(MarkSel::Marker(2));
    v.set_focused(false);
    assert!(!v.is_text_owner());
}

#[test]
fn a_double_click_on_a_range_renames_the_range() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = range_point(&v, 1);
    handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    assert_eq!(v.marker_rename_text(), Some("Intro"));
    handle(&mut v, &doc, &typed("s"));
    let a = handle(&mut v, &doc, &[key_event(Key::Enter)]);
    assert_eq!(range_lists(&a)[0][0].name, "Intros");
    assert!(loops(&a).is_empty());
}

// ---------------------------------------------------------------- delete, menus, colours

#[test]
fn the_eraser_deletes_a_marker_or_a_range() {
    let doc = marked();
    let mut v = with_strip(&doc);
    v.set_tool(Tool::Eraser);
    let p = marker_point(&v, 1);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(marker_lists(&a)[0], vec![doc.song.markers[1].clone()]);
    let q = range_point(&v, 2);
    let a = handle(&mut v, &doc, &[down(q), up(q)]);
    assert_eq!(range_lists(&a)[0], vec![doc.song.ranges[0].clone()]);
}

#[test]
fn delete_removes_the_selected_marker_and_only_that() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = marker_point(&v, 2);
    handle(&mut v, &doc, &[down(p), up(p)]);
    let a = command(&mut v, &doc, "edit.delete", None);
    assert_eq!(
        marker_lists(a.actions())[0],
        vec![doc.song.markers[0].clone()]
    );
    assert_eq!(v.selected_marker(), None);
    // Nothing selected here: the command is not this view's.
    let a = command(&mut v, &doc, "edit.delete", None);
    assert!(!matches!(a, CommandOutcome::Handled(ref x) if !marker_lists(x).is_empty()));
}

#[test]
fn right_clicks_ask_for_the_menu_of_what_is_under_them() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let cases = [
        (marker_point(&v, 1), ContextTarget::Marker),
        (range_point(&v, 1), ContextTarget::MarkerRange),
        (strip_point(&v, 9 * BAR, true), ContextTarget::MarkerStrip),
    ];
    for (p, target) in cases {
        let a = handle(&mut v, &doc, &[right_down(p), right_up(p)]);
        assert!(
            a.iter()
                .any(|x| matches!(x, UiAction::ContextMenu { target: t, .. } if *t == target)),
            "{target:?}: {a:?}"
        );
    }
}

#[test]
fn the_menu_commands_act_on_the_marker_the_menu_opened_on() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = marker_point(&v, 2);
    handle(&mut v, &doc, &[right_down(p), right_up(p)]);
    let a = command(&mut v, &doc, "sequencer.marker_color", Some("green"));
    let list = &marker_lists(a.actions())[0];
    assert_eq!(list[1].color, Some(edit::PALETTE[3].1));
    assert_eq!(list[0].color, None);
    // The menu's target is used up by one command; the selection (the same marker) is next.
    let a = command(&mut v, &doc, "sequencer.marker_delete", None);
    assert_eq!(
        marker_lists(a.actions())[0],
        vec![doc.song.markers[0].clone()]
    );
    // Rename from the menu opens the field.
    let mut v = with_strip(&doc);
    handle(&mut v, &doc, &[right_down(p), right_up(p)]);
    command(&mut v, &doc, "sequencer.marker_rename", None);
    assert_eq!(v.marker_rename_text(), Some("Break"));
}

#[test]
fn colours_can_be_cleared_and_unknown_names_are_declined() {
    let mut doc = marked();
    doc.song.markers[0].color = Some([9, 9, 9]);
    let mut v = with_strip(&doc);
    v.set_selected_marker(Some(MarkSel::Marker(1)));
    let a = command(&mut v, &doc, "sequencer.marker_color", Some("none"));
    assert_eq!(marker_lists(a.actions())[0][0].color, None);
    let a = command(&mut v, &doc, "sequencer.marker_color", Some("plaid"));
    assert!(matches!(a, CommandOutcome::Declined));
    let a = command(&mut v, &doc, "sequencer.marker_color", None);
    assert!(matches!(a, CommandOutcome::Declined));
}

#[test]
fn loop_this_range_and_select_clips_in_range() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let p = range_point(&v, 1);
    handle(&mut v, &doc, &[right_down(p), right_up(p)]);
    let a = command(&mut v, &doc, "sequencer.marker_loop_range", None);
    assert_eq!(loops(a.actions()), vec![((0, 2 * BAR), true)]);
    // Bars 1-2 hold the bass clip (0..4 bars), the keys clip (0..4) and the lead clip from bar 2.
    handle(&mut v, &doc, &[right_down(p), right_up(p)]);
    let a = command(&mut v, &doc, "sequencer.marker_select_clips", None);
    assert!(a.is_taken());
    let sel = v.selected_clips();
    assert!(sel.contains(&ClipId(1)) && sel.contains(&ClipId(3)) && sel.contains(&ClipId(2)));
    assert!(!sel.contains(&ClipId(5)), "the looped clip starts at bar 5");
    // On a point marker the range commands do not apply.
    v.set_selected_marker(Some(MarkSel::Marker(1)));
    let a = command(&mut v, &doc, "sequencer.marker_loop_range", None);
    assert!(matches!(a, CommandOutcome::Declined));
}

#[test]
fn add_marker_here_uses_the_tick_of_the_menu() {
    let doc = song_doc();
    let mut v = with_strip(&doc);
    let p = strip_point(&v, 6 * BAR, true);
    handle(&mut v, &doc, &[right_down(p), right_up(p)]);
    let a = command(&mut v, &doc, "sequencer.marker_add_here", None);
    assert_eq!(marker_lists(a.actions())[0][0].tick, 6 * BAR);
}

// ---------------------------------------------------------------- previous and next marker

#[test]
fn previous_and_next_marker_locate_relative_to_the_playhead() {
    let doc = marked();
    let mut v = with_strip(&doc);
    v.set_playhead(2 * BAR);
    let a = command(&mut v, &doc, "transport.next_marker", None);
    assert_eq!(locates(a.actions()), vec![4 * BAR]);
    let a = command(&mut v, &doc, "transport.prev_marker", None);
    assert_eq!(locates(a.actions()), vec![BAR]);
    // On a marker: strictly before and after.
    v.set_playhead(BAR);
    let a = command(&mut v, &doc, "transport.next_marker", None);
    assert_eq!(locates(a.actions()), vec![4 * BAR]);
    let a = command(&mut v, &doc, "transport.prev_marker", None);
    assert!(matches!(a, CommandOutcome::NotApplicable(_)));
    v.set_playhead(9 * BAR);
    let a = command(&mut v, &doc, "transport.next_marker", None);
    assert!(matches!(a, CommandOutcome::NotApplicable(_)));
}

#[test]
fn the_marker_commands_work_with_the_strip_hidden() {
    let doc = marked();
    let reg = registry();
    let mut v = arrangement(&doc, &reg);
    v.set_playhead(0);
    let a = command(&mut v, &doc, "transport.next_marker", None);
    assert_eq!(locates(a.actions()), vec![BAR]);
}

// ---------------------------------------------------------------- the list

fn with_list(doc: &RackDoc) -> ArrangementView {
    let mut v = with_strip(doc);
    v.set_marker_list(true);
    v
}

#[test]
fn the_list_takes_its_width_off_the_timeline_and_gives_it_back() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let outer = v.bounds();
    let x = v.tick_to_x(0);
    v.set_marker_list(true);
    assert!(v.marker_list_visible());
    assert_eq!(v.bounds(), outer, "the view's rectangle is the whole one");
    let list = v.marker_list_rect();
    assert!((list.width() - MARKER_LIST_WIDTH).abs() < 1e-9 && (list.x1 - outer.x1).abs() < 1e-9);
    assert!(v.marker_strip_rect().x1 <= list.x0 + 1e-9);
    assert_eq!(v.tick_to_x(0), x, "the track list stays put");
    v.set_marker_list(false);
    assert_eq!(v.marker_list_rect().width(), 0.0);
    assert!((v.marker_strip_rect().x1 - outer.x1).abs() < 1e-9);
}

#[test]
fn the_list_shows_markers_and_ranges_in_order_with_bar_beat_tick_positions() {
    let mut doc = marked();
    // 3/4 from bar 3: a marker at bar 4 sits 3 beats of 3/4 further.
    doc.song.meter = vec![
        MeterEvent {
            bar: 1,
            num: 4,
            den: 4,
        },
        MeterEvent {
            bar: 3,
            num: 3,
            den: 4,
        },
    ];
    doc.song
        .markers
        .push(marker(3, 2 * BAR + 3 * PPQ + 5, "Pickup"));
    let mut v = with_list(&doc);
    assert_eq!(
        v.list_order(),
        vec![
            MarkSel::Range(1),
            MarkSel::Marker(1),
            MarkSel::Marker(3),
            MarkSel::Marker(2),
            MarkSel::Range(2)
        ]
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    let texts = dl.texts();
    for t in [
        "MARKERS",
        "+ Marker",
        "+ Range",
        "Pickup",
        "1.1.00000",
        "2.1.00000",
        "3.1.00000",
        "4.1.00005",
        "5.3.00000",
        "11.1.00000",
    ] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
}

#[test]
fn an_empty_list_says_so() {
    let doc = song_doc();
    let mut v = with_list(&doc);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    assert!(dl.texts().contains(&"No markers yet"));
}

#[test]
fn clicking_a_marker_row_locates_and_a_range_row_loops_and_locates() {
    let doc = marked();
    let mut v = with_list(&doc);
    let r = v.list_row_rect(MarkSel::Marker(2)).unwrap();
    let p = Point::new(r.x0 + 160.0, r.center().y);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(locates(&a), vec![4 * BAR], "{a:?}");
    assert_eq!(v.selected_marker(), Some(MarkSel::Marker(2)));
    let r = v.list_row_rect(MarkSel::Range(2)).unwrap();
    let p = Point::new(r.x0 + 160.0, r.center().y);
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(loops(&a), vec![((4 * BAR, 8 * BAR), true)]);
    assert_eq!(locates(&a), vec![4 * BAR]);
}

#[test]
fn the_delete_button_of_a_row_deletes_it() {
    let doc = marked();
    let mut v = with_list(&doc);
    let p = v.list_delete_rect(MarkSel::Marker(1)).unwrap().center();
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(marker_lists(&a)[0], vec![doc.song.markers[1].clone()]);
    let p = v.list_delete_rect(MarkSel::Range(1)).unwrap().center();
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(range_lists(&a)[0], vec![doc.song.ranges[1].clone()]);
}

#[test]
fn a_double_click_on_a_name_in_the_list_renames_in_place() {
    let doc = marked();
    let mut v = with_list(&doc);
    let p = v.list_name_rect(MarkSel::Range(2)).unwrap().center();
    handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    assert!(v.is_text_owner());
    assert_eq!(v.marker_rename_text(), Some("Verse"));
    handle(&mut v, &doc, &typed(" 1"));
    let a = handle(&mut v, &doc, &[key_event(Key::Enter)]);
    assert_eq!(range_lists(&a)[0][1].name, "Verse 1");
    // The field is where the name is, so a click on it keeps typing.
    handle(
        &mut v,
        &doc,
        &[down_with(p, PointerButton::Primary, 2), up(p)],
    );
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert!(v.is_text_owner() && a.is_empty(), "{a:?}");
}

#[test]
fn the_add_buttons_add_at_the_playhead_and_from_the_loop() {
    let mut doc = song_doc();
    doc.song.loop_range = (2 * BAR, 6 * BAR);
    let mut v = with_list(&doc);
    v.set_playhead(3 * BAR);
    let p = v.list_add_marker_rect().center();
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(marker_lists(&a)[0], vec![marker(1, 3 * BAR, "Marker 1")]);
    let p = v.list_add_range_rect().center();
    let a = handle(&mut v, &doc, &[down(p), up(p)]);
    assert_eq!(
        range_lists(&a)[0],
        vec![range(1, 2 * BAR, 6 * BAR, "Range 1")]
    );
}

#[test]
fn right_click_on_a_row_asks_for_the_marker_menu() {
    let doc = marked();
    let mut v = with_list(&doc);
    let r = v.list_row_rect(MarkSel::Range(1)).unwrap();
    let p = Point::new(r.x0 + 160.0, r.center().y);
    let a = handle(&mut v, &doc, &[right_down(p), right_up(p)]);
    assert!(a.iter().any(|x| matches!(
        x,
        UiAction::ContextMenu {
            target: ContextTarget::MarkerRange,
            ..
        }
    )));
}

#[test]
fn the_list_scrolls_with_the_wheel_over_it_and_the_timeline_does_not() {
    let mut doc = song_doc();
    doc.song.markers = (0..40)
        .map(|i| marker(i + 1, i64::from(i) * BAR, &format!("M{i}")))
        .collect();
    let mut v = with_list(&doc);
    let first = v.list_row_rect(MarkSel::Marker(1)).unwrap();
    let over = v.marker_list_rect().center();
    let scroll = v.scroll_ticks();
    handle(
        &mut v,
        &doc,
        &[UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -3.0),
            pos: over,
            modifiers: Modifiers::NONE,
        }],
    );
    assert!(
        v.list_row_rect(MarkSel::Marker(1)).is_none(),
        "scrolled out"
    );
    assert_eq!(v.scroll_ticks(), scroll, "the timeline stayed");
    handle(
        &mut v,
        &doc,
        &[UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, 30.0),
            pos: over,
            modifiers: Modifiers::NONE,
        }],
    );
    assert_eq!(v.list_row_rect(MarkSel::Marker(1)), Some(first));
}

#[test]
fn deleting_a_marker_in_the_document_drops_a_stale_selection() {
    let doc = marked();
    let mut v = with_strip(&doc);
    v.set_selected_marker(Some(MarkSel::Marker(2)));
    let mut gone = doc.clone();
    gone.song.markers.remove(1);
    v.sync(&gone, &registry());
    assert_eq!(v.selected_marker(), None);
}

// ---------------------------------------------------------------- preview

/// Writes `target/preview/arrangement_markers.png`:
/// `cargo test -p atomik-ui --test markers -- --ignored`.
#[test]
#[ignore = "writes a preview image; run explicitly"]
fn preview_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    let mut doc = marked();
    doc.song.markers[1].color = Some(edit::PALETTE[3].1);
    doc.song.ranges[1].color = Some(edit::PALETTE[1].1);
    doc.song
        .markers
        .push(marker(3, 5 * BAR + PPQ, "Last chance of the day"));
    let reg = registry();
    let mut v = with_list(&doc);
    v.set_tempo_lane(true);
    v.set_scale(1.5);
    v.set_playhead(2 * BAR);
    v.set_selected_marker(Some(MarkSel::Range(2)));
    let (w, h) = (
        (ARR_BOUNDS.width() * 1.5) as u16,
        (ARR_BOUNDS.height() * 1.5) as u16,
    );
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/arrangement_markers.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

// ---------------------------------------------------------------- marker lines in the grid

#[test]
fn dragging_a_marker_line_in_the_grid_moves_the_marker() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let y = v.lane_rect(&key("lead"), 0).unwrap().center().y;
    let x = v.tick_to_x(4 * BAR);
    let to = Point::new(x + 2.0 * PPQ as f64 * v.px_per_tick() + 1.0, y);
    let a = handle(
        &mut v,
        &doc,
        &[
            down(Point::new(x + 2.0, y)),
            mv(Point::new(x + 20.0, y)),
            mv(to),
            up(to),
        ],
    );
    let lists = marker_lists(&a);
    let moved = lists.last().unwrap().iter().find(|m| m.id == 2).unwrap();
    assert_eq!(moved.tick, 4 * BAR + 2 * PPQ, "snapped like a strip drag");
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn hovering_a_marker_line_hints_the_resize_cursor() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let y = v.lane_rect(&key("lead"), 0).unwrap().center().y;
    let p = Point::new(v.tick_to_x(4 * BAR), y);
    let _ = handle(&mut v, &doc, &[mv(p)]);
    assert_eq!(v.cursor(), atomik_ui::CursorHint::ResizeHorizontal);
}

#[test]
fn pressing_beside_a_marker_line_or_with_another_tool_leaves_the_grid_alone() {
    let doc = marked();
    let mut v = with_strip(&doc);
    let y = v.lane_rect(&key("lead"), 0).unwrap().center().y;
    let x = v.tick_to_x(4 * BAR);
    let beside = Point::new(x + 12.0, y);
    let a = handle(&mut v, &doc, &[down(beside), up(beside)]);
    assert!(marker_lists(&a).is_empty());
    v.set_tool(Tool::Pencil);
    let to = Point::new(x + 80.0, y);
    let a = handle(
        &mut v,
        &doc,
        &[down(Point::new(x + 1.0, y)), mv(to), up(to)],
    );
    assert!(
        marker_lists(&a).is_empty(),
        "the pencil draws over the line"
    );
}

#[test]
fn marker_lines_are_not_grabbed_while_the_strip_is_hidden() {
    let doc = marked();
    let mut v = arrangement(&doc, &registry());
    let y = v.lane_rect(&key("lead"), 0).unwrap().center().y;
    let x = v.tick_to_x(4 * BAR);
    let to = Point::new(x + 80.0, y);
    let a = handle(&mut v, &doc, &[down(Point::new(x, y)), mv(to), up(to)]);
    assert!(marker_lists(&a).is_empty());
}
