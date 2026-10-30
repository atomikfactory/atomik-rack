//! The segmented time field and the arrangement inspector: the parts of a position and of a
//! length through the meter map, stepping and typing (`+n`, `-n`, dotted entries), what the
//! strip shows for one, several and mixed clips of every kind, and the edits: relative on a
//! multi-selection, one undo step each, Match Values, a drag as one merge group.

mod support;

use atomik_model::{ClipId, Command, RackDoc, SongCommand};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::segment_field::{
    EditKey, FieldValue, SIXTEENTH, SegmentDrag, SegmentEdit, SegmentField, SegmentMode,
};
use atomik_ui::timeline::{Bars, Meter};
use atomik_ui::{ArrangementView, ClipField, DisplayList, Key, Modifiers, UiAction, UiEvent};
use support::audio::*;
use support::song::*;
use support::*;

const BAR: i64 = 4 * PPQ;
const PPQ: i64 = atomik_ui::timeline::PPQ;

fn four_four() -> Bars {
    Bars::single(Meter {
        beats_per_bar: 4,
        beat_unit: 4,
    })
}

fn three_four() -> Bars {
    Bars::single(Meter {
        beats_per_bar: 3,
        beat_unit: 4,
    })
}

// ---------------------------------------------------------------- the field

#[test]
fn a_position_splits_into_bar_beat_sixteenth_and_tick() {
    let f = SegmentField::new(&four_four(), SegmentMode::Position);
    assert_eq!(f.split(0), [1, 1, 1, 0]);
    assert_eq!(f.split(BAR + PPQ + SIXTEENTH + 7), [2, 2, 2, 7]);
    for t in [0, 1, 3839, 3840, PPQ, BAR - 1, BAR, 5 * BAR + 2 * PPQ + 9] {
        assert_eq!(f.join(f.split(t)), t, "tick {t}");
    }
    let f = SegmentField::new(&three_four(), SegmentMode::Position);
    assert_eq!(f.split(3 * PPQ), [2, 1, 1, 0]);
    assert_eq!(f.texts(3 * PPQ + 5)[3], "0005");
}

#[test]
fn a_length_counts_from_zero_in_the_meter_where_it_starts() {
    let f = SegmentField::new(&four_four(), SegmentMode::Length { anchor: 0 });
    assert_eq!(f.split(0), [0, 0, 0, 0]);
    assert_eq!(f.split(BAR + PPQ), [1, 1, 0, 0]);
    assert_eq!(f.split(2 * BAR + SIXTEENTH + 3), [2, 0, 1, 3]);
    let f = SegmentField::new(&three_four(), SegmentMode::Length { anchor: 0 });
    assert_eq!(f.split(3 * PPQ), [1, 0, 0, 0]);
    assert_eq!(f.join([1, 2, 0, 0]), 5 * PPQ);
}

#[test]
fn steps_follow_the_part_and_shift_takes_the_finer_unit() {
    let f = SegmentField::new(&four_four(), SegmentMode::Position);
    assert_eq!(f.step(0, 0, 1, false), BAR);
    assert_eq!(f.step(0, 1, 2, false), 2 * PPQ);
    assert_eq!(f.step(0, 2, -1, false), -SIXTEENTH);
    assert_eq!(f.step(0, 0, 1, true), PPQ, "fine bar is a beat");
    assert_eq!(f.step(0, 3, 1, true), 1, "fine tick is one tick");
    let f = SegmentField::new(&three_four(), SegmentMode::Length { anchor: 0 });
    assert_eq!(f.step(0, 0, 1, false), 3 * PPQ, "a bar of 3/4");
}

#[test]
fn typing_replaces_a_part_adds_ticks_or_sets_the_whole_field() {
    let f = SegmentField::new(&four_four(), SegmentMode::Position);
    let at = BAR + PPQ;
    assert_eq!(f.parse(at, 0, "+240"), Some(at + 240));
    assert_eq!(f.parse(at, 0, "-240"), Some(at - 240));
    assert_eq!(f.parse(at, 0, "5"), Some(4 * BAR + PPQ), "bar 5, same beat");
    assert_eq!(f.parse(at, 1, "4"), Some(BAR + 3 * PPQ));
    assert_eq!(f.parse(0, 0, "3.2.1.0"), Some(2 * BAR + PPQ));
    assert_eq!(
        f.parse(0, 0, "3."),
        Some(2 * BAR),
        "missing parts are the first"
    );
    assert_eq!(f.parse(0, 0, ""), None);
    assert_eq!(f.parse(0, 0, "+"), None);
    assert_eq!(f.parse(0, 0, "1.2.3.4.5"), None);
    let l = SegmentField::new(&four_four(), SegmentMode::Length { anchor: 0 });
    assert_eq!(l.parse(0, 0, "2"), Some(2 * BAR));
    assert_eq!(l.parse(0, 0, "1.2"), Some(BAR + 2 * PPQ));
}

#[test]
fn an_edit_takes_digits_signs_and_the_arrow_keys() {
    let mut e = SegmentEdit::new(0, "3".into());
    assert_eq!(e.key(&Key::Char('+')), EditKey::Typed);
    assert_eq!(e.text, "+", "the first character replaces the shown value");
    e.key(&Key::Char('2'));
    e.key(&Key::Char('4'));
    e.key(&Key::Char('x'));
    assert_eq!(e.text, "+24");
    e.key(&Key::Backspace);
    assert_eq!(e.text, "+2");
    assert_eq!(e.key(&Key::Up), EditKey::Step(1));
    assert_eq!(e.key(&Key::Left), EditKey::Move(-1));
    assert_eq!(e.key(&Key::Enter), EditKey::Commit);
    assert_eq!(e.key(&Key::Escape), EditKey::Cancel);
}

#[test]
fn a_drag_steps_the_part_and_stops_at_the_clamp() {
    let f = SegmentField::new(&four_four(), SegmentMode::Position);
    let mut d = SegmentDrag::new(0, 100.0, BAR);
    // 5 px per step, up is positive.
    assert_eq!(d.update(&f, 90.0, false, |v| v.max(0)), Some(3 * BAR));
    assert_eq!(d.update(&f, 90.0, false, |v| v.max(0)), None);
    assert_eq!(d.update(&f, 140.0, false, |v| v.max(0)), Some(0));
    assert_eq!(
        d.update(&f, 160.0, false, |v| v.max(0)),
        None,
        "held at the end"
    );
}

// ---------------------------------------------------------------- the strip

fn shown(doc: &RackDoc, ids: &[u64]) -> ArrangementView {
    let reg = registry();
    let mut arr = arrangement(doc, &reg);
    arr.set_inspector(true);
    let ids: Vec<ClipId> = ids.iter().map(|i| ClipId(*i)).collect();
    arr.set_selected_clips(&ids);
    arr
}

fn texts(arr: &mut ArrangementView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    arr.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn handle(arr: &mut ArrangementView, doc: &RackDoc, events: &[UiEvent]) -> Vec<UiAction> {
    let reg = registry();
    run(|e| arr.handle(e, doc, &reg), events)
}

fn wheel(pos: Point, dy: f64) -> UiEvent {
    UiEvent::Wheel {
        pos,
        delta: Vec2::new(0.0, dy),
        modifiers: Modifiers::NONE,
    }
}

fn key_down(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn typed(s: &str) -> Vec<UiEvent> {
    s.chars().map(|c| key_down(Key::Char(c))).collect()
}

/// Every `(clip, start, length, offset, loop, merge)` the actions set, batched or not.
type Geo = (ClipId, i64, i64, i64, Option<i64>, Option<u64>);

fn geometry(actions: &[UiAction]) -> Vec<Geo> {
    fn one(c: &SongCommand, out: &mut Vec<Geo>) {
        match c {
            SongCommand::SetClipGeometry {
                id,
                start,
                length,
                offset,
                loop_length,
                merge,
                ..
            } => out.push((*id, *start, *length, *offset, *loop_length, *merge)),
            SongCommand::SetAudioClip {
                id,
                start,
                length,
                merge,
                ..
            } => out.push((*id, *start, *length, 0, None, *merge)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in commands(actions) {
        match c {
            Command::Song(s) => one(&s, &mut out),
            Command::Batch(list) => {
                for c in list {
                    if let Command::Song(s) = c {
                        one(&s, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn batches(actions: &[UiAction]) -> usize {
    commands(actions)
        .iter()
        .filter(|c| matches!(c, Command::Batch(_)))
        .count()
}

#[test]
fn the_strip_takes_room_from_the_lanes_and_hides_again() {
    let doc = song_doc();
    let reg = registry();
    let mut arr = arrangement(&doc, &reg);
    assert!(!arr.inspector_visible());
    assert!(arr.inspector_field_rect(ClipField::Position).is_none());
    let before = arr.bounds();
    arr.set_inspector(true);
    let strip = arr.inspector_rect();
    assert_eq!(strip.y1, before.y1);
    assert!(strip.height() > 20.0);
    assert!(arr.inspector_field_rect(ClipField::Loop).is_some());
    arr.set_inspector(false);
    assert!(arr.inspector_field_rect(ClipField::Position).is_none());
}

#[test]
fn one_clip_shows_its_position_length_and_loop() {
    let doc = song_doc();
    // Clip 5: bar 5, two bars long, looping every bar.
    let mut arr = shown(&doc, &[5]);
    assert_eq!(
        arr.inspector_value(ClipField::Position),
        FieldValue::Value(4 * BAR)
    );
    assert_eq!(
        arr.inspector_value(ClipField::Length),
        FieldValue::Value(2 * BAR)
    );
    assert_eq!(arr.inspector_value(ClipField::Offset), FieldValue::Value(0));
    assert_eq!(arr.inspector_value(ClipField::Loop), FieldValue::Value(BAR));
    let t = texts(&mut arr, &doc);
    for want in [
        "POSITION",
        "LENGTH",
        "LOOP OFFSET",
        "LOOP LENGTH",
        "MATCH",
        "NOTE CLIP",
    ] {
        assert!(t.iter().any(|s| s == want), "{want} in {t:?}");
    }
    assert!(t.iter().any(|s| s == "5"), "bar 5 of the position: {t:?}");
}

#[test]
fn clips_of_every_kind_are_shown_and_mixed_values_dash() {
    let doc = song_doc();
    // Notes clip 1, automation clip 4: both start at 0 and are four bars long.
    let arr = shown(&doc, &[1, 4]);
    assert_eq!(
        arr.inspector_value(ClipField::Position),
        FieldValue::Value(0)
    );
    assert_eq!(
        arr.inspector_value(ClipField::Length),
        FieldValue::Value(4 * BAR)
    );
    let arr = shown(&doc, &[1, 5]);
    assert_eq!(arr.inspector_value(ClipField::Position), FieldValue::Mixed);
    assert_eq!(arr.inspector_value(ClipField::Loop), FieldValue::Mixed);
    let mut arr = shown(&doc, &[]);
    assert_eq!(arr.inspector_value(ClipField::Length), FieldValue::Empty);
    let t = texts(&mut arr, &doc);
    assert!(t.iter().any(|s| s == "no clip selected"), "{t:?}");
}

#[test]
fn position_plus_one_bar_on_several_clips_is_one_batch() {
    let doc = song_doc();
    let mut arr = shown(&doc, &[1, 5, 2]);
    let at = arr.inspector_part_point(ClipField::Position, 0).unwrap();
    let a = handle(&mut arr, &doc, &[wheel(at, 1.0)]);
    let mut g = geometry(&a);
    g.sort_by_key(|x| x.0);
    assert_eq!(batches(&a), 1);
    // Clip 1 starts at 0, clip 2 at bar 2, clip 5 at bar 5: each moves one bar.
    assert_eq!(
        g.iter().map(|x| (x.0.0, x.1)).collect::<Vec<_>>(),
        vec![(1, BAR), (2, 2 * BAR), (5, 5 * BAR)]
    );
    assert!(g.iter().all(|x| x.5.is_none()));
}

#[test]
fn typing_plus_240_adds_240_ticks_to_every_selected_clip() {
    let doc = song_doc();
    let mut arr = shown(&doc, &[1, 5]);
    let at = arr.inspector_part_point(ClipField::Position, 3).unwrap();
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed("+240"));
    events.push(key_down(Key::Enter));
    let a = handle(&mut arr, &doc, &events);
    let mut g = geometry(&a);
    g.sort_by_key(|x| x.0);
    assert_eq!(batches(&a), 1);
    assert_eq!(g[0].1, 240);
    assert_eq!(g[1].1, 4 * BAR + 240);
    assert!(arr.inspector_text().is_none(), "typing ended");
}

#[test]
fn typing_a_new_length_shifts_every_clip_by_the_difference() {
    let doc = song_doc();
    // Clip 1 is 4 bars, clip 5 is 2: typing 5 bars for the first adds a bar to both.
    let mut arr = shown(&doc, &[1, 5]);
    let at = arr.inspector_part_point(ClipField::Length, 0).unwrap();
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed("5"));
    events.push(key_down(Key::Enter));
    let a = handle(&mut arr, &doc, &events);
    let mut g = geometry(&a);
    g.sort_by_key(|x| x.0);
    assert_eq!(g[0].2, 5 * BAR);
    assert_eq!(g[1].2, 3 * BAR);
}

#[test]
fn a_relative_edit_clamps_each_clip_on_its_own() {
    let doc = song_doc();
    // Clip 1 at 0 and clip 5 at bar 5: minus a bar leaves the first and moves the other.
    let mut arr = shown(&doc, &[1, 5]);
    let at = arr.inspector_part_point(ClipField::Position, 0).unwrap();
    let a = handle(&mut arr, &doc, &[wheel(at, -1.0), wheel(at, -1.0)]);
    let g = geometry(&a);
    // The first clip is already at zero and sends nothing; the document has not caught up
    // between the two events, so both steps are one bar back from where the clip is.
    assert!(g.iter().all(|x| x.0 == ClipId(5)), "{g:?}");
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].1, 3 * BAR);
}

#[test]
fn a_dotted_entry_sets_the_position_and_arrow_keys_step_while_typing() {
    let doc = song_doc();
    let mut arr = shown(&doc, &[3]);
    let at = arr.inspector_part_point(ClipField::Position, 1).unwrap();
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed("3.2.1.0"));
    events.push(key_down(Key::Enter));
    let a = handle(&mut arr, &doc, &events);
    let g = geometry(&a);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].1, 2 * BAR + PPQ);
    assert_eq!(batches(&a), 0, "one clip is a single command");

    let events = [down_mods(at, Modifiers::NONE, 2), key_down(Key::Up)];
    let a = handle(&mut arr, &doc, &events);
    assert_eq!(geometry(&a)[0].1, PPQ, "up steps the beat");
    assert!(arr.inspector_text().is_some(), "typing goes on");
    let a = handle(&mut arr, &doc, &[key_down(Key::Escape)]);
    assert!(a.is_empty());
    assert!(arr.inspector_text().is_none());
}

#[test]
fn match_gives_every_clip_the_first_clips_length_and_loop() {
    let doc = song_doc();
    let mut arr = shown(&doc, &[1, 5]);
    let m = arr.inspector_match_rect().unwrap().center();
    let a = handle(&mut arr, &doc, &[down(m), up(m)]);
    let g = geometry(&a);
    // Clip 1 (4 bars, no loop) is the first: clip 5 takes its length and drops its loop.
    assert_eq!(g, vec![(ClipId(5), 4 * BAR, 4 * BAR, 0, None, None)]);
    // One clip has nothing to match.
    let mut arr = shown(&doc, &[1]);
    let m = arr.inspector_match_rect().unwrap().center();
    assert!(handle(&mut arr, &doc, &[down(m), up(m)]).is_empty());
}

#[test]
fn a_drag_is_one_merge_group_with_a_closing_end_merge() {
    let doc = song_doc();
    let mut arr = shown(&doc, &[5]);
    let at = arr.inspector_part_point(ClipField::Position, 0).unwrap();
    let a = handle(
        &mut arr,
        &doc,
        &[
            down(at),
            mv(Point::new(at.x, at.y - 6.0)),
            mv(Point::new(at.x, at.y - 12.0)),
            up(Point::new(at.x, at.y - 12.0)),
        ],
    );
    let g = geometry(&a);
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].1, 5 * BAR);
    assert_eq!(g[1].1, 6 * BAR);
    assert!(g[0].5.is_some() && g[0].5 == g[1].5, "one merge group");
    assert!(matches!(a.last(), Some(UiAction::EndMerge)));
}

#[test]
fn the_loop_length_turns_a_loop_on_and_zero_turns_it_off() {
    let doc = song_doc();
    // Clip 3 has no loop: typing two bars loops it.
    let mut arr = shown(&doc, &[3]);
    let at = arr.inspector_part_point(ClipField::Loop, 0).unwrap();
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed("2"));
    events.push(key_down(Key::Enter));
    let g = geometry(&handle(&mut arr, &doc, &events));
    assert_eq!(g[0].4, Some(2 * BAR));
    // Clip 5 loops every bar: typing 0 stops it.
    let mut arr = shown(&doc, &[5]);
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed("0"));
    events.push(key_down(Key::Enter));
    let g = geometry(&handle(&mut arr, &doc, &events));
    assert_eq!(g[0].4, None);
}

#[test]
fn automation_clips_edit_like_note_clips() {
    let doc = song_doc();
    let mut arr = shown(&doc, &[4]);
    let at = arr.inspector_part_point(ClipField::Length, 0).unwrap();
    let a = handle(&mut arr, &doc, &[wheel(at, 1.0)]);
    let g = geometry(&a);
    assert_eq!(g, vec![(ClipId(4), 0, 5 * BAR, 0, None, None)]);
}

#[test]
fn audio_clips_have_no_offset_or_loop_and_edit_through_the_audio_command() {
    let doc = audio_doc();
    let reg = registry();
    let mut arr = audio_arrangement(&doc, &reg);
    arr.set_bounds(ARR_BOUNDS);
    arr.set_inspector(true);
    arr.set_selected_clips(&[ClipId(10)]);
    assert_eq!(arr.inspector_value(ClipField::Offset), FieldValue::Empty);
    assert_eq!(arr.inspector_value(ClipField::Loop), FieldValue::Empty);
    let at = arr.inspector_part_point(ClipField::Offset, 0).unwrap();
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed("1"));
    events.push(key_down(Key::Enter));
    assert!(
        handle(&mut arr, &doc, &events).is_empty(),
        "the field is dimmed"
    );
    let at = arr.inspector_part_point(ClipField::Position, 0).unwrap();
    let a = handle(&mut arr, &doc, &[wheel(at, 1.0)]);
    let cmds = commands(&a);
    assert!(
        matches!(
            &cmds[..],
            [Command::Song(SongCommand::SetAudioClip { id, start, .. })]
                if *id == ClipId(10) && *start == 2 * BAR
        ),
        "{cmds:?}"
    );
}

#[test]
fn the_view_owns_the_keyboard_only_while_a_value_is_typed() {
    let doc = song_doc();
    let mut arr = shown(&doc, &[1]);
    assert!(!arr.is_text_owner());
    let at = arr.inspector_part_point(ClipField::Position, 0).unwrap();
    handle(&mut arr, &doc, &[down_mods(at, Modifiers::NONE, 2)]);
    assert!(arr.is_text_owner());
    handle(&mut arr, &doc, &[key_down(Key::Escape)]);
    assert!(!arr.is_text_owner());
}

// ---------------------------------------------------------------- points

use atomik_ui::{PointField, PointShown};

fn points_shown(sel: &[usize]) -> (ArrangementView, RackDoc) {
    let doc = song_doc();
    let mut arr = shown(&doc, &[4]);
    arr.set_selected_points(ClipId(4), sel);
    (arr, doc)
}

fn contents(actions: &[UiAction]) -> Vec<(Vec<atomik_model::AutomationPoint>, Option<u64>)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Song(SongCommand::SetClipContent {
                content: atomik_model::ClipContent::Automation(p),
                merge,
                ..
            }) => Some((p, merge)),
            _ => None,
        })
        .collect()
}

fn entry(at: Point, text: &str) -> Vec<UiEvent> {
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed(text));
    events.push(key_down(Key::Enter));
    events
}

#[test]
fn selected_points_replace_the_clip_fields_with_position_and_value() {
    let (mut arr, doc) = points_shown(&[1]);
    assert!(arr.inspector_shows_points());
    assert_eq!(
        arr.inspector_point_value(PointField::Position),
        Some(PointShown::Value((2 * BAR) as f64))
    );
    assert_eq!(
        arr.inspector_point_value(PointField::Value),
        Some(PointShown::Value(6000.0))
    );
    let t = texts(&mut arr, &doc);
    for want in ["AUTOMATION", "1 point", "POSITION", "VALUE"] {
        assert!(t.iter().any(|s| s == want), "{want} in {t:?}");
    }
    assert!(
        !t.iter().any(|s| s == "LOOP LENGTH"),
        "the clip fields are hidden"
    );
    let (arr, _) = points_shown(&[0, 1]);
    assert_eq!(
        arr.inspector_point_value(PointField::Value),
        Some(PointShown::Mixed)
    );
    let (arr, _) = points_shown(&[]);
    assert!(!arr.inspector_shows_points());
}

#[test]
fn typing_a_relative_value_moves_every_point_and_clamps_each_to_the_lane_range() {
    let (mut arr, doc) = points_shown(&[0, 1]);
    let at = arr.inspector_point_part(PointField::Value, 0).unwrap();
    let c = contents(&handle(&mut arr, &doc, &entry(at, "+100")));
    assert_eq!(c.len(), 1, "one undo step");
    assert!((c[0].0[0].value - 400.0).abs() < 1e-3);
    assert!((c[0].0[1].value - 6100.0).abs() < 1e-3);
    // A huge entry stops each point at the top of the range.
    let (mut arr, doc) = points_shown(&[0, 1]);
    let c = contents(&handle(&mut arr, &doc, &entry(at, "+9999999")));
    let top = c[0].0[0].value;
    assert!(top > 300.0 && (c[0].0[1].value - top).abs() < 1e-3, "{c:?}");
}

#[test]
fn point_positions_move_by_the_same_ticks_and_a_collision_sends_nothing() {
    let (mut arr, doc) = points_shown(&[1, 2]);
    let at = arr.inspector_point_part(PointField::Position, 3).unwrap();
    let c = contents(&handle(&mut arr, &doc, &entry(at, "+240")));
    assert_eq!(c[0].0[1].tick, 2 * BAR + 240);
    assert_eq!(c[0].0[2].tick, 3 * BAR + 240);
    // Point 0 typed onto bar 2 is free; bar 3 is where point 1 sits and sends nothing.
    let (mut arr, doc) = points_shown(&[0]);
    let at = arr.inspector_point_part(PointField::Position, 0).unwrap();
    assert!(!contents(&handle(&mut arr, &doc, &entry(at, "2"))).is_empty());
    assert!(contents(&handle(&mut arr, &doc, &entry(at, "3"))).is_empty());
}

#[test]
fn a_point_drag_is_one_merge_group_and_the_wheel_is_one_step() {
    let (mut arr, doc) = points_shown(&[1]);
    let at = arr.inspector_point_part(PointField::Value, 0).unwrap();
    let a = handle(
        &mut arr,
        &doc,
        &[
            down(at),
            mv(Point::new(at.x, at.y - 6.0)),
            up(Point::new(at.x, at.y - 6.0)),
        ],
    );
    let c = contents(&a);
    assert_eq!(c.len(), 1);
    assert!(c[0].1.is_some());
    assert!(c[0].0[1].value > 6000.0);
    assert!(matches!(a.last(), Some(UiAction::EndMerge)));
    let c = contents(&handle(&mut arr, &doc, &[wheel(at, 1.0)]));
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].1, None);
}
