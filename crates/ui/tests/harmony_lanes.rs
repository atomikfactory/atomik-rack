//! The key and chord lanes of the arrangement: off by default, they push the track lanes down and
//! sit under the tempo and energy lanes; a click adds a key change or a chord, a drag moves or
//! resizes it, a double click opens the picker, the right button opens a menu, Delete removes the
//! selection, Escape puts a drag back. Every gesture runs through a real document, so "one undo
//! step" is checked on the model. Also a golden of the lanes and one of the picker.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::harmony::{ChordEvent, Harmony, Key, KeyChange};
use atomik_model::theory::{ChordKind, ScaleKind};
use atomik_model::{Command, Document, SongCommand};
use atomik_ui::arrangement::{CHORD_LANE_HEIGHT, KEY_LANE_HEIGHT};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::{
    ArrangementView, DisplayList, HarmonyPickerView, HarmonyTarget, Key as UiKey, Modifiers,
    PickerValue, Theme, UiAction, UiEvent,
};
use support::song::*;
use support::*;

const BAR_PX: f64 = 96.0;

fn setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let d = Document::new(song_doc());
    let mut v = arrangement(d.doc(), &reg);
    v.set_key_lane(true);
    v.set_chord_lane(true);
    v.sync(d.doc(), &reg);
    (d, reg, v)
}

/// Sets the harmony in the document and syncs the view.
fn set(d: &mut Document, reg: &Registry, v: &mut ArrangementView, h: Harmony) {
    d.apply(Command::Song(SongCommand::SetHarmony(h)), reg)
        .unwrap();
    d.end_merge();
    v.sync(d.doc(), reg);
}

/// Feeds events to the view and applies each command it emits to the document before the next
/// event (what the app does); returns every action.
fn play(
    v: &mut ArrangementView,
    d: &mut Document,
    reg: &Registry,
    events: &[UiEvent],
) -> Vec<UiAction> {
    let mut all = Vec::new();
    for e in events {
        let actions = v.handle(e, d.doc(), reg);
        for a in &actions {
            match a {
                UiAction::Command(c) => {
                    d.apply(c.clone(), reg).unwrap();
                }
                UiAction::EndMerge => d.end_merge(),
                _ => {}
            }
        }
        v.sync(d.doc(), reg);
        all.extend(actions);
    }
    all
}

fn bar_x(v: &ArrangementView, bar: f64) -> f64 {
    v.chord_lane_rect().x0 + bar * BAR_PX
}

fn chord_y(v: &ArrangementView) -> f64 {
    v.chord_lane_rect().center().y
}

fn key_y(v: &ArrangementView) -> f64 {
    v.key_lane_rect().center().y
}

fn am_key() -> Key {
    Key::new(9, ScaleKind::NaturalMinor)
}

fn chord(bar: i64, bars: i64, root: u8, quality: ChordKind) -> ChordEvent {
    ChordEvent::new(bar * BAR, bars * BAR, root, quality)
}

fn lane_harmony() -> Harmony {
    Harmony {
        key: Some(am_key()),
        changes: vec![KeyChange {
            tick: 4 * BAR,
            key: Key::new(0, ScaleKind::Major),
        }],
        chords: vec![
            chord(0, 2, 9, ChordKind::MinorSeventh),
            chord(3, 1, 5, ChordKind::MajorSeventh),
        ],
    }
}

fn texts(v: &mut ArrangementView, d: &Document, reg: &Registry) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), reg);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_lanes_are_hidden_by_default_and_push_the_track_lanes_down() {
    let reg = registry();
    let d = Document::new(song_doc());
    let mut v = arrangement(d.doc(), &reg);
    assert!(!v.key_lane_visible() && !v.chord_lane_visible());
    assert_eq!(v.key_lane_rect().height(), 0.0);
    assert_eq!(v.chord_lane_rect().height(), 0.0);
    let before = v.lane_rect(&key("bass"), 0).unwrap();
    v.set_key_lane(true);
    let with_key = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((with_key.y0 - before.y0 - KEY_LANE_HEIGHT).abs() < 1e-9);
    v.set_chord_lane(true);
    let both = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((both.y0 - before.y0 - KEY_LANE_HEIGHT - CHORD_LANE_HEIGHT).abs() < 1e-9);
    assert!((v.key_lane_rect().y1 - v.chord_lane_rect().y0).abs() < 1e-9);
    v.set_key_lane(false);
    v.set_chord_lane(false);
    let back = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((back.y0 - before.y0).abs() < 1e-9);
}

#[test]
fn the_lanes_sit_under_the_tempo_and_energy_lanes() {
    let (d, reg, mut v) = setup();
    v.set_tempo_lane(true);
    v.set_energy_lane(true);
    v.sync(d.doc(), &reg);
    let (tempo, energy) = (v.tempo_lane_rect(), v.energy_lane_rect());
    let (key_lane, chord_lane) = (v.key_lane_rect(), v.chord_lane_rect());
    assert!((energy.y0 - tempo.y1).abs() < 1e-9);
    assert!((key_lane.y0 - energy.y1).abs() < 1e-9);
    assert!((chord_lane.y0 - key_lane.y1).abs() < 1e-9);
    let first = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((first.y0 - chord_lane.y1).abs() < 1.0);
}

#[test]
fn the_lanes_draw_the_keys_and_the_chords() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let t = texts(&mut v, &d, &reg);
    for word in ["KEY", "CHORDS", "A minor", "C major", "Am7", "Fmaj7"] {
        assert!(t.iter().any(|s| s == word), "{word} in {t:?}");
    }
    v.set_playhead(0);
    assert!(texts(&mut v, &d, &reg).iter().any(|s| s == "Am"));
    v.set_playhead(5 * BAR);
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|s| s == "C"), "the key at the playhead: {t:?}");
    assert!(
        t.iter().any(|s| s == "-"),
        "no chord at the playhead: {t:?}"
    );
}

#[test]
fn nothing_is_drawn_while_the_lanes_are_hidden() {
    let reg = registry();
    let mut d = Document::new(song_doc());
    d.apply(Command::Song(SongCommand::SetHarmony(lane_harmony())), &reg)
        .unwrap();
    let mut v = arrangement(d.doc(), &reg);
    let t = texts(&mut v, &d, &reg);
    assert!(!t.iter().any(|s| s == "CHORDS" || s == "Am7"), "{t:?}");
}

// ---------------------------------------------------------------- chords

#[test]
fn a_click_on_the_empty_chord_lane_adds_the_tonic_chord_a_bar_long_in_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    set(
        &mut d,
        &reg,
        &mut v,
        Harmony {
            key: Some(am_key()),
            ..Harmony::default()
        },
    );
    let p = Point::new(bar_x(&v, 2.0), chord_y(&v));
    play(&mut v, &mut d, &reg, &[down(p), up(p)]);
    let h = &d.doc().song.harmony;
    assert_eq!(h.chords.len(), 1);
    let c = h.chords[0];
    assert_eq!((c.tick, c.length), (2 * BAR, BAR));
    assert_eq!((c.root, c.quality), (9, ChordKind::Minor));
    d.undo().unwrap();
    assert!(
        d.doc().song.harmony.chords.is_empty(),
        "the add is one undo step"
    );
}

#[test]
fn a_drag_from_the_press_sets_the_length_of_the_new_chord() {
    let (mut d, reg, mut v) = setup();
    let y = chord_y(&v);
    let (a, b) = (Point::new(bar_x(&v, 1.0), y), Point::new(bar_x(&v, 4.0), y));
    let actions = play(&mut v, &mut d, &reg, &[down(a), mv(b), up(b)]);
    let c = d.doc().song.harmony.chords[0];
    assert_eq!((c.tick, c.length), (BAR, 3 * BAR));
    assert!(actions.iter().any(|a| matches!(a, UiAction::EndMerge)));
    d.undo().unwrap();
    assert!(d.doc().song.harmony.chords.is_empty());
}

#[test]
fn a_new_chord_stops_at_the_next_one() {
    let (mut d, reg, mut v) = setup();
    set(
        &mut d,
        &reg,
        &mut v,
        Harmony {
            chords: vec![chord(2, 1, 0, ChordKind::Major)],
            ..Harmony::default()
        },
    );
    let y = chord_y(&v);
    let p = Point::new(bar_x(&v, 1.5), y);
    let q = Point::new(bar_x(&v, 3.0), y);
    play(&mut v, &mut d, &reg, &[down(p), mv(q), up(q)]);
    let chords = &d.doc().song.harmony.chords;
    assert_eq!(chords.len(), 2);
    assert_eq!(chords[0].end(), chords[1].tick, "no overlap, no gap");
    assert!(d.doc().validate().is_ok(), "{:?}", d.doc().validate().err());
}

#[test]
fn a_drag_moves_a_chord_and_never_over_a_neighbour() {
    let (mut d, reg, mut v) = setup();
    set(
        &mut d,
        &reg,
        &mut v,
        Harmony {
            chords: vec![
                chord(0, 1, 0, ChordKind::Major),
                chord(3, 1, 7, ChordKind::Major),
            ],
            ..Harmony::default()
        },
    );
    let y = chord_y(&v);
    let from = Point::new(bar_x(&v, 0.5), y);
    let to = Point::new(bar_x(&v, 1.5), y);
    play(&mut v, &mut d, &reg, &[down(from), mv(to), up(to)]);
    assert_eq!(d.doc().song.harmony.chords[0].tick, BAR);
    // Onto the next chord: refused, the chord stays where it last fitted.
    let from = Point::new(bar_x(&v, 1.5), y);
    let over = Point::new(bar_x(&v, 3.5), y);
    play(&mut v, &mut d, &reg, &[down(from), mv(over), up(over)]);
    let chords = &d.doc().song.harmony.chords;
    assert_eq!(chords.len(), 2);
    assert!(chords[0].end() <= chords[1].tick, "{chords:?}");
    d.undo().unwrap();
    d.undo().unwrap_or_default();
}

#[test]
fn the_right_end_resizes_a_chord_and_stops_at_the_next_one() {
    let (mut d, reg, mut v) = setup();
    set(
        &mut d,
        &reg,
        &mut v,
        Harmony {
            chords: vec![
                chord(0, 1, 0, ChordKind::Major),
                chord(3, 1, 7, ChordKind::Major),
            ],
            ..Harmony::default()
        },
    );
    let end = v.chord_end_handle(0).unwrap();
    let wider = Point::new(end.x + BAR_PX, end.y);
    play(&mut v, &mut d, &reg, &[down(end), mv(wider), up(wider)]);
    assert_eq!(d.doc().song.harmony.chords[0].length, 2 * BAR);
    let end = v.chord_end_handle(0).unwrap();
    let too_far = Point::new(end.x + 6.0 * BAR_PX, end.y);
    play(&mut v, &mut d, &reg, &[down(end), mv(too_far), up(too_far)]);
    let chords = &d.doc().song.harmony.chords;
    assert_eq!(chords[0].end(), chords[1].tick, "the next chord stops it");
}

#[test]
fn escape_puts_a_drag_back_in_one_step() {
    let (mut d, reg, mut v) = setup();
    set(
        &mut d,
        &reg,
        &mut v,
        Harmony {
            chords: vec![chord(0, 1, 0, ChordKind::Major)],
            ..Harmony::default()
        },
    );
    let before = d.doc().song.harmony.clone();
    let y = chord_y(&v);
    let from = Point::new(bar_x(&v, 0.5), y);
    let to = Point::new(bar_x(&v, 2.5), y);
    play(
        &mut v,
        &mut d,
        &reg,
        &[
            down(from),
            mv(to),
            press(UiKey::Escape, Modifiers::NONE),
            up(to),
        ],
    );
    assert_eq!(d.doc().song.harmony, before);
}

#[test]
fn a_double_click_on_a_chord_opens_the_picker() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let p = v.chord_center(0).unwrap();
    let actions = play(
        &mut v,
        &mut d,
        &reg,
        &[down_with(p, atomik_ui::PointerButton::Primary, 2)],
    );
    assert!(
        actions.contains(&UiAction::HarmonyPicker(HarmonyTarget::Chord {
            tick: 0,
            length: 2 * BAR
        })),
        "{actions:?}"
    );
}

#[test]
fn a_double_click_on_the_empty_lane_adds_a_chord_and_opens_its_picker() {
    let (mut d, reg, mut v) = setup();
    let p = Point::new(bar_x(&v, 5.0), chord_y(&v));
    let actions = play(
        &mut v,
        &mut d,
        &reg,
        &[
            down(p),
            up(p),
            down_with(p, atomik_ui::PointerButton::Primary, 2),
            up(p),
        ],
    );
    assert_eq!(d.doc().song.harmony.chords.len(), 1, "one chord, not two");
    assert!(
        actions.contains(&UiAction::HarmonyPicker(HarmonyTarget::Chord {
            tick: 5 * BAR,
            length: BAR
        }))
    );
}

#[test]
fn the_context_menu_of_a_chord_deletes_and_copies_and_the_lane_pastes() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let p = v.chord_center(3 * BAR).unwrap();
    play(&mut v, &mut d, &reg, &[right_down(p), right_up(p)]);
    assert_eq!(
        v.menu_labels(),
        ["Delete Chord", "Copy Chord", "Play Chords on Click"]
    );
    // Copy.
    let copy = v.param_menu_row_center(1).unwrap();
    play(&mut v, &mut d, &reg, &[down(copy), up(copy)]);
    assert!(v.menu_labels().is_empty(), "the menu closed");
    // Paste on the empty lane at bar 6.
    let empty = Point::new(bar_x(&v, 6.0), chord_y(&v));
    play(&mut v, &mut d, &reg, &[right_down(empty), right_up(empty)]);
    assert_eq!(
        v.menu_labels(),
        ["Add Chord Here", "Paste Chord", "Play Chords on Click"]
    );
    let paste = v.param_menu_row_center(1).unwrap();
    let actions = play(&mut v, &mut d, &reg, &[down(paste), up(paste)]);
    assert_eq!(actions.len(), 1, "{actions:?}");
    let chords = &d.doc().song.harmony.chords;
    assert_eq!(chords.len(), 3);
    assert_eq!(
        (chords[2].tick, chords[2].root, chords[2].quality),
        (6 * BAR, 5, ChordKind::MajorSeventh)
    );
    // Delete.
    let p = v.chord_center(0).unwrap();
    play(&mut v, &mut d, &reg, &[right_down(p), right_up(p)]);
    let delete = v.param_menu_row_center(0).unwrap();
    play(&mut v, &mut d, &reg, &[down(delete), up(delete)]);
    assert_eq!(d.doc().song.harmony.chords.len(), 2);
    assert_eq!(d.doc().song.harmony.chords[0].tick, 3 * BAR);
}

#[test]
fn delete_removes_the_selected_chord_and_copy_keeps_it() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let p = v.chord_center(0).unwrap();
    play(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(v.selected_harmony_tick(), Some(0));
    let out = v.command("edit.copy", None, d.doc(), &reg);
    assert!(matches!(out, CommandOutcome::Handled(ref a) if a.is_empty()));
    let out = v.command("edit.delete", None, d.doc(), &reg);
    let CommandOutcome::Handled(actions) = out else {
        panic!("delete not handled")
    };
    for a in &actions {
        if let UiAction::Command(c) = a {
            d.apply(c.clone(), &reg).unwrap();
        }
    }
    assert_eq!(d.doc().song.harmony.chords.len(), 1);
    v.sync(d.doc(), &reg);
    assert_eq!(v.selected_harmony_tick(), None);
}

// ---------------------------------------------------------------- key lane

#[test]
fn a_click_on_the_key_lane_adds_a_change_with_the_key_already_in_effect() {
    let (mut d, reg, mut v) = setup();
    set(
        &mut d,
        &reg,
        &mut v,
        Harmony {
            key: Some(am_key()),
            ..Harmony::default()
        },
    );
    let p = Point::new(bar_x(&v, 3.0), key_y(&v));
    play(&mut v, &mut d, &reg, &[down(p), up(p)]);
    let h = &d.doc().song.harmony;
    assert_eq!(h.changes.len(), 1);
    assert_eq!(h.changes[0].tick, 3 * BAR);
    assert_eq!(h.changes[0].key, am_key());
    d.undo().unwrap();
    assert!(d.doc().song.harmony.changes.is_empty());
}

#[test]
fn a_tag_is_dragged_to_move_its_change_and_a_double_click_opens_the_picker() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let tag = v.key_tag_center(4 * BAR).unwrap();
    let to = Point::new(tag.x + 2.0 * BAR_PX, tag.y);
    play(&mut v, &mut d, &reg, &[down(tag), mv(to), up(to)]);
    assert_eq!(d.doc().song.harmony.changes[0].tick, 6 * BAR);
    let tag = v.key_tag_center(6 * BAR).unwrap();
    let actions = play(
        &mut v,
        &mut d,
        &reg,
        &[down_with(tag, atomik_ui::PointerButton::Primary, 2)],
    );
    assert!(
        actions.contains(&UiAction::HarmonyPicker(HarmonyTarget::KeyChange {
            tick: 6 * BAR
        }))
    );
}

#[test]
fn the_song_key_tag_opens_the_song_key_picker_and_clears_from_its_menu() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let tag = v.key_tag_center(0).unwrap();
    let actions = play(
        &mut v,
        &mut d,
        &reg,
        &[down_with(tag, atomik_ui::PointerButton::Primary, 2)],
    );
    assert!(actions.contains(&UiAction::HarmonyPicker(HarmonyTarget::SongKey)));
    play(&mut v, &mut d, &reg, &[right_down(tag), right_up(tag)]);
    assert_eq!(v.menu_labels(), ["Clear Song Key", "Copy Key"]);
    let clear = v.param_menu_row_center(0).unwrap();
    play(&mut v, &mut d, &reg, &[down(clear), up(clear)]);
    assert_eq!(d.doc().song.harmony.key, None);
}

#[test]
fn the_menu_of_a_key_change_deletes_it_and_the_empty_lane_pastes_a_key() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let tag = v.key_tag_center(4 * BAR).unwrap();
    play(&mut v, &mut d, &reg, &[right_down(tag), right_up(tag)]);
    assert_eq!(v.menu_labels(), ["Delete Key Change", "Copy Key"]);
    let copy = v.param_menu_row_center(1).unwrap();
    play(&mut v, &mut d, &reg, &[down(copy), up(copy)]);
    let empty = Point::new(bar_x(&v, 9.0), key_y(&v));
    play(&mut v, &mut d, &reg, &[right_down(empty), right_up(empty)]);
    assert_eq!(v.menu_labels(), ["Add Key Change Here", "Paste Key Change"]);
    let paste = v.param_menu_row_center(1).unwrap();
    play(&mut v, &mut d, &reg, &[down(paste), up(paste)]);
    let changes = &d.doc().song.harmony.changes;
    assert_eq!(changes.len(), 2);
    assert_eq!(
        (changes[1].tick, changes[1].key),
        (9 * BAR, Key::new(0, ScaleKind::Major))
    );
    let tag = v.key_tag_center(4 * BAR).unwrap();
    play(&mut v, &mut d, &reg, &[right_down(tag), right_up(tag)]);
    let delete = v.param_menu_row_center(0).unwrap();
    play(&mut v, &mut d, &reg, &[down(delete), up(delete)]);
    assert_eq!(d.doc().song.harmony.changes.len(), 1);
}

// ---------------------------------------------------------------- goldens

fn record_lanes() -> DisplayList {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let p = v.chord_center(0).unwrap();
    play(&mut v, &mut d, &reg, &[down(p), up(p)]);
    v.set_playhead(BAR);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    dl
}

#[test]
fn the_lanes_match_their_golden() {
    starter::check_golden(
        "arrangement_key_chord_lanes_1x.json",
        &record_lanes().to_json(),
    );
}

fn picker(
    value: PickerValue,
    target: HarmonyTarget,
    title: &str,
    existing: bool,
) -> HarmonyPickerView {
    let mut v = HarmonyPickerView::new(Theme::dark(), target, title, value, existing);
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1000.0, 700.0));
    v
}

#[test]
fn the_chord_picker_matches_its_golden() {
    let v = picker(
        PickerValue::Chord {
            root: 9,
            quality: ChordKind::MinorSeventh,
            bass: Some(7),
        },
        HarmonyTarget::Chord {
            tick: 0,
            length: BAR,
        },
        "CHORD AT BAR 1",
        true,
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    starter::check_golden("harmony_picker_chord_1x.json", &dl.to_json());
}

#[test]
fn the_key_picker_matches_its_golden() {
    let v = picker(
        PickerValue::Key(am_key()),
        HarmonyTarget::SongKey,
        "SONG KEY",
        true,
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    starter::check_golden("harmony_picker_key_1x.json", &dl.to_json());
}

#[test]
fn an_unreadable_symbol_shows_its_hint_in_the_picker() {
    let mut v = picker(
        PickerValue::Chord {
            root: 0,
            quality: ChordKind::Major,
            bass: None,
        },
        HarmonyTarget::Chord {
            tick: 0,
            length: BAR,
        },
        "CHORD",
        false,
    );
    for c in ['Q', 'x'] {
        v.handle(&UiEvent::Key {
            key: UiKey::Char(c),
            modifiers: Modifiers::NONE,
            pressed: true,
        });
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(
        dl.texts()
            .iter()
            .any(|t| t.contains("does not start with a note name")),
        "{:?}",
        dl.texts()
    );
}

#[test]
fn a_click_on_a_chord_plays_it_unless_play_chords_on_click_is_off() {
    let (mut d, reg, mut v) = setup();
    set(&mut d, &reg, &mut v, lane_harmony());
    let chord = d.doc().song.harmony.chords[0];
    let p = v.chord_center(chord.tick).unwrap();
    let actions = play(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert!(
        actions.contains(&UiAction::AuditionChord(chord)),
        "{actions:?}"
    );
    assert!(v.plays_chords_on_click());
    // The lane's menu turns it off (a checked row).
    play(&mut v, &mut d, &reg, &[right_down(p), right_up(p)]);
    let row = v.menu_labels().len() - 1;
    let toggle = v.param_menu_row_center(row).unwrap();
    play(&mut v, &mut d, &reg, &[down(toggle), up(toggle)]);
    assert!(!v.plays_chords_on_click());
    let actions = play(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, UiAction::AuditionChord(_))),
        "{actions:?}"
    );
}
