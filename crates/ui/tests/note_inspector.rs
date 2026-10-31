//! The note inspector strip under the piano roll: what it shows for none, one, several and
//! mixed notes, relative multi-edits clamped per note, typing (names, `+n`, dotted positions),
//! one undo step per value and one merge group per drag.

mod support;

use atomik_model::{ClipContent, Command, Note, RackDoc, SongCommand};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::segment_field::FieldValue;
use atomik_ui::{DisplayList, Key, Modifiers, NoteField, PianoRollView, UiAction, UiEvent};
use support::song::*;
use support::*;

fn roll(selected: &[usize]) -> (PianoRollView, RackDoc) {
    let doc = song_doc();
    let reg = registry();
    let mut v = piano_roll(&doc, &reg);
    v.set_inspector(true);
    v.set_selected_notes(selected);
    (v, doc)
}

fn handle(v: &mut PianoRollView, doc: &RackDoc, events: &[UiEvent]) -> Vec<UiAction> {
    let reg = registry();
    run(|e| v.handle(e, doc, &reg), events)
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

fn wheel(pos: Point, dy: f64) -> UiEvent {
    UiEvent::Wheel {
        pos,
        delta: Vec2::new(0.0, dy),
        modifiers: Modifiers::NONE,
    }
}

/// Double-click `field`, type `text`, Enter.
fn entry(at: Point, text: &str) -> Vec<UiEvent> {
    let mut events = vec![down_mods(at, Modifiers::NONE, 2)];
    events.extend(typed(text));
    events.push(key_down(Key::Enter));
    events
}

/// The note lists the actions set, with their merge ids.
fn lists(actions: &[UiAction]) -> Vec<(Vec<Note>, Option<u64>)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Song(SongCommand::SetClipContent {
                content: ClipContent::Notes(n),
                merge,
                ..
            }) => Some((n.into_vec(), merge)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_strip_shares_the_view_with_the_roll_and_shows_the_notes() {
    let doc = song_doc();
    let reg = registry();
    let mut v = piano_roll(&doc, &reg);
    let full = v.bounds();
    assert!(!v.inspector_visible() && v.inspector_field_rect(NoteField::Pitch).is_none());
    v.set_inspector(true);
    assert_eq!(v.bounds(), full, "the host's rectangle is unchanged");
    assert!(v.inspector_rect().height() > 20.0);
    assert!(v.inspector_field_rect(NoteField::Velocity).is_some());
    v.set_selected_notes(&[2]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let t: Vec<&str> = dl.texts();
    for want in ["NOTE", "POSITION", "LENGTH", "VELOCITY", "G2", "1 note"] {
        assert!(t.contains(&want), "{want} in {t:?}");
    }
    v.set_inspector(false);
    assert!(v.inspector_field_rect(NoteField::Pitch).is_none());
}

#[test]
fn fields_show_one_value_a_dash_for_mixed_and_nothing_without_a_selection() {
    let (v, _) = roll(&[]);
    assert_eq!(v.inspector_value(NoteField::Pitch), FieldValue::Empty);
    let (v, _) = roll(&[1]);
    assert_eq!(v.inspector_value(NoteField::Pitch), FieldValue::Value(36));
    assert_eq!(
        v.inspector_value(NoteField::Position),
        FieldValue::Value(PPQ)
    );
    let (v, _) = roll(&[0, 1]);
    assert_eq!(v.inspector_value(NoteField::Pitch), FieldValue::Value(36));
    assert_eq!(v.inspector_value(NoteField::Position), FieldValue::Mixed);
    assert_eq!(v.inspector_value(NoteField::Velocity), FieldValue::Mixed);
}

#[test]
fn a_wheel_notch_on_pitch_moves_every_selected_note_one_step_in_one_command() {
    let (mut v, doc) = roll(&[0, 2]);
    let at = v.inspector_part_point(NoteField::Pitch, 0).unwrap();
    let l = lists(&handle(&mut v, &doc, &[wheel(at, 1.0)]));
    assert_eq!(l.len(), 1, "one command is one undo step");
    assert_eq!(l[0].1, None);
    let before = bass_line();
    assert_eq!(l[0].0[0].key, before[0].key + 1);
    assert_eq!(l[0].0[2].key, before[2].key + 1);
    assert_eq!(l[0].0[1], before[1]);
}

#[test]
fn pitch_is_clamped_per_note_and_the_others_still_move() {
    let (mut v, doc) = roll(&[0, 5]);
    let at = v.inspector_part_point(NoteField::Pitch, 0).unwrap();
    let l = lists(&handle(&mut v, &doc, &entry(at, "+100")));
    assert_eq!((l[0].0[0].key, l[0].0[5].key), (127, 127));
    // Down by 40: the lower note (36) stops at 0, the other (48) goes to 8.
    let (mut v, doc) = roll(&[0, 5]);
    let l = lists(&handle(&mut v, &doc, &entry(at, "-40")));
    assert_eq!((l[0].0[0].key, l[0].0[5].key), (0, 8));
}

#[test]
fn a_note_name_a_number_or_a_relative_entry_sets_the_pitch() {
    let (mut v, doc) = roll(&[2]);
    let at = v.inspector_part_point(NoteField::Pitch, 0).unwrap();
    // C4 is 60, so C#3 is 49.
    assert_eq!(
        lists(&handle(&mut v, &doc, &entry(at, "C#3")))[0].0[2].key,
        49
    );
    assert_eq!(
        lists(&handle(&mut v, &doc, &entry(at, "72")))[0].0[2].key,
        72
    );
    assert_eq!(
        lists(&handle(&mut v, &doc, &entry(at, "+7")))[0].0[2].key,
        50
    );
}

#[test]
fn typing_a_new_length_shifts_three_selected_notes_by_the_difference_in_one_step() {
    let (mut v, doc) = roll(&[0, 1, 2]);
    let at = v.inspector_part_point(NoteField::Length, 0).unwrap();
    let l = lists(&handle(&mut v, &doc, &entry(at, "1")));
    assert_eq!(l.len(), 1);
    let before = bass_line();
    // Typing 1 replaces the bar part only: the first note gains a bar, so does each other.
    let delta = 4 * PPQ;
    for (i, b) in before.iter().enumerate().take(3) {
        assert_eq!(l[0].0[i].length, b.length + delta, "note {i}");
    }
    assert_eq!(l[0].0[3].length, before[3].length);
}

#[test]
fn position_clamps_at_zero_and_keeps_the_notes_sorted_with_the_selection() {
    let (mut v, doc) = roll(&[0, 1]);
    let at = v.inspector_part_point(NoteField::Position, 0).unwrap();
    let l = lists(&handle(&mut v, &doc, &[wheel(at, -1.0)]));
    assert_eq!(l[0].0.iter().filter(|n| n.tick == 0).count(), 2);
    // Moving note 0 a bar forward re-sorts the list: the selection follows the note.
    let (mut v, doc) = roll(&[0]);
    let l = lists(&handle(&mut v, &doc, &[wheel(at, 1.0)]));
    assert!(
        l[0].0
            .windows(2)
            .all(|w| (w[0].tick, w[0].key) <= (w[1].tick, w[1].key))
    );
    let moved = v.selected_notes();
    assert_eq!(l[0].0[moved[0]].tick, 4 * PPQ);
}

#[test]
fn velocity_is_relative_clamped_to_1_and_127_and_typed_as_a_number() {
    let (mut v, doc) = roll(&[0, 15]);
    let at = v.inspector_part_point(NoteField::Velocity, 0).unwrap();
    let l = lists(&handle(&mut v, &doc, &entry(at, "+100")));
    assert!((l[0].0[0].velocity - 1.0).abs() < 1e-6);
    assert!((l[0].0[15].velocity - 1.0).abs() < 1e-6);
    let (mut v, doc) = roll(&[3]);
    let l = lists(&handle(&mut v, &doc, &entry(at, "64")));
    assert!((l[0].0[3].velocity - 64.0 / 127.0).abs() < 1e-6);
}

#[test]
fn a_drag_is_one_merge_group_closed_by_end_merge_and_arrow_keys_step_while_typing() {
    let (mut v, doc) = roll(&[0]);
    let at = v.inspector_part_point(NoteField::Pitch, 0).unwrap();
    let a = handle(
        &mut v,
        &doc,
        &[
            down(at),
            mv(Point::new(at.x, at.y - 6.0)),
            mv(Point::new(at.x, at.y - 12.0)),
            up(Point::new(at.x, at.y - 12.0)),
        ],
    );
    let l = lists(&a);
    assert_eq!(l.len(), 2);
    assert!(l[0].1.is_some() && l[0].1 == l[1].1);
    assert_eq!((l[0].0[0].key, l[1].0[0].key), (37, 38));
    assert!(matches!(a.last(), Some(UiAction::EndMerge)));

    let (mut v, doc) = roll(&[0]);
    let a = handle(
        &mut v,
        &doc,
        &[down_mods(at, Modifiers::NONE, 2), key_down(Key::Up)],
    );
    assert_eq!(lists(&a)[0].0[0].key, 37);
    assert!(v.is_text_owner());
    handle(&mut v, &doc, &[key_down(Key::Escape)]);
    assert!(!v.is_text_owner());
}

#[test]
fn nothing_is_sent_without_a_selection_or_when_the_value_does_not_change() {
    let (mut v, doc) = roll(&[]);
    let at = v.inspector_part_point(NoteField::Pitch, 0).unwrap();
    let a = handle(
        &mut v,
        &doc,
        &[wheel(at, 1.0), down_mods(at, Modifiers::NONE, 2)],
    );
    assert!(a.is_empty());
    assert!(!v.is_text_owner());
    let (mut v, doc) = roll(&[0]);
    assert!(lists(&handle(&mut v, &doc, &entry(at, "36"))).is_empty());
}
