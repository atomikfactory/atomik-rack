//! Mixer snapshots in `AppCore`: the commands the Mixer menu runs (store, update, recall, next and
//! previous, compare, delete), the current snapshot and the hint that survives edits, the menu
//! state, one undo step each, and the engine seeing a recall as ordinary param changes (no
//! routing rebuild, so the strips' own smoothers apply it without clicks).

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::{Command, ConsoleCommand, DeviceKey};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// Two synths `a` and `b`, each with its automatic channel (`channel` and `channel-2`).
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .expect("instrument added");
    }
    c.sink_mut().take();
    c
}

fn set(c: &mut AppCore<RecordingSink>, device: &str, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: key(device),
        param: param.into(),
        value,
        merge: None,
    })
    .expect("param set");
}

fn value(c: &AppCore<RecordingSink>, device: &str, param: &str) -> f32 {
    c.doc()
        .param_value(&key(device), param, c.registry())
        .unwrap()
}

fn names(c: &AppCore<RecordingSink>) -> Vec<String> {
    c.doc()
        .console
        .snapshots
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

/// Snapshots `A` (fader 0.3, pan -0.5) and `B` (fader 0.9), the mix left as `B`.
fn two_snapshots() -> AppCore<RecordingSink> {
    let mut c = core();
    set(&mut c, "channel", "fader", 0.3);
    set(&mut c, "channel", "pan", -0.5);
    assert!(c.store_snapshot(None));
    set(&mut c, "channel", "fader", 0.9);
    set(&mut c, "channel", "pan", 0.0);
    assert!(c.store_snapshot(None));
    c
}

#[test]
fn store_names_the_snapshots_a_b_then_numbers_and_refuses_a_taken_name() {
    let mut c = core();
    for _ in 0..3 {
        assert!(c.store_snapshot(None));
    }
    assert_eq!(names(&c), ["A", "B", "1"]);
    assert!(c.store_snapshot(Some("  Verse ".into())));
    assert_eq!(names(&c).last().unwrap(), "Verse");
    assert!(!c.store_snapshot(Some("verse".into())));
    assert!(c.last_rejection().unwrap().contains("unique"));
    assert_eq!(c.doc().console.snapshots.len(), 4);
}

#[test]
fn a_recall_reaches_the_engine_as_param_changes_and_rebuilds_nothing() {
    let mut c = two_snapshots();
    let ch = c.device_id(&key("channel")).unwrap();
    let fader = c
        .registry()
        .get("atomik.channel")
        .unwrap()
        .descriptor
        .param_by_key("fader")
        .unwrap()
        .id;
    c.sink_mut().take();
    assert!(c.recall_snapshot(Some(1)));
    let calls = c.sink_mut().take();
    assert!(
        calls.iter().any(|x| matches!(
            x,
            SinkCall::Param { id, param, plain } if *id == ch && *param == fader && (*plain - 0.3).abs() < 1e-6
        )),
        "{calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|x| matches!(x, SinkCall::Routing(_) | SinkCall::Reset { .. })),
        "a recall rebuilds nothing"
    );
    assert_eq!(value(&c, "channel", "fader"), 0.3);
}

#[test]
fn recall_update_compare_and_delete_are_one_undo_step_each() {
    let mut c = two_snapshots();
    let step = |c: &mut AppCore<RecordingSink>, f: &dyn Fn(&mut AppCore<RecordingSink>) -> bool| {
        let before = c.doc().clone();
        assert!(f(c));
        assert_ne!(c.doc(), &before);
        assert!(c.undo());
        assert_eq!(c.doc(), &before, "one step");
        assert!(c.redo());
    };
    step(&mut c, &|c| c.recall_snapshot(Some(1)));
    set(&mut c, "channel", "fader", 0.55);
    step(&mut c, &|c| c.update_snapshot(Some(1)));
    step(&mut c, &|c| c.compare_snapshot(Some(2)));
    step(&mut c, &|c| c.compare_snapshot(None));
    step(&mut c, &|c| c.end_compare());
    step(&mut c, &|c| c.delete_snapshot(Some(2)));
}

#[test]
fn the_current_snapshot_is_the_exact_match_else_the_last_one_used() {
    let mut c = two_snapshots();
    // The mix is B.
    assert_eq!(c.current_snapshot(), Some(2));
    assert!(c.recall_snapshot(Some(1)));
    assert_eq!(c.current_snapshot(), Some(1));
    // A moved fader: no snapshot matches, the last recalled stays current.
    set(&mut c, "channel", "fader", 0.61);
    assert_eq!(c.current_snapshot(), Some(1));
    // Recalling the current snapshot puts back what moved, and is a step.
    let moved = c.doc().clone();
    assert!(c.recall_snapshot(None));
    assert_ne!(c.doc(), &moved);
    assert_eq!(value(&c, "channel", "fader"), 0.3);
    // Recalling the mix that is already there is not a step and not an error.
    let before = c.doc().clone();
    assert!(c.recall_snapshot(None));
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().is_none() || !c.last_rejection().unwrap().contains("matches"));
}

#[test]
fn the_hint_is_forgotten_with_the_document_and_when_its_snapshot_is_deleted() {
    let mut c = two_snapshots();
    assert!(c.recall_snapshot(Some(1)));
    set(&mut c, "channel", "fader", 0.61);
    assert_eq!(c.current_snapshot(), Some(1));
    assert!(c.delete_snapshot(Some(1)));
    assert_eq!(c.current_snapshot(), None);
    assert!(c.recall_snapshot(Some(2)));
    set(&mut c, "channel", "fader", 0.62);
    assert_eq!(c.current_snapshot(), Some(2));
    c.new_document();
    assert_eq!(c.current_snapshot(), None);
}

#[test]
fn next_and_previous_walk_the_list_and_wrap() {
    let mut c = two_snapshots();
    set(&mut c, "channel", "fader", 0.5);
    assert!(c.store_snapshot(None));
    // The mix is "1" (the last stored); next wraps to A, previous goes back to 1.
    assert_eq!(c.current_snapshot(), Some(3));
    assert!(c.recall_adjacent(true));
    assert_eq!(c.current_snapshot(), Some(1));
    assert!(c.recall_adjacent(true));
    assert_eq!(c.current_snapshot(), Some(2));
    assert!(c.recall_adjacent(false));
    assert!(c.recall_adjacent(false));
    assert_eq!(c.current_snapshot(), Some(3), "wrapped backwards");
    // With none current: next is the first, previous the last.
    set(&mut c, "channel-2", "fader", 0.4);
    c.apply_command(Command::Console(ConsoleCommand::DeleteSnapshot { id: 3 }))
        .unwrap();
    assert_eq!(c.current_snapshot(), None);
    assert!(c.recall_adjacent(false));
    assert_eq!(c.current_snapshot(), Some(2));
}

#[test]
fn compare_flips_and_the_menu_state_follows() {
    let mut c = two_snapshots();
    let m = c.snapshot_menu();
    assert!(m.can_store && m.can_recall && m.can_cycle && m.can_compare && m.can_delete);
    assert!(!m.can_update, "the mix is B");
    assert!(!m.comparing);
    set(&mut c, "channel", "fader", 0.61);
    assert!(c.snapshot_menu().can_update);
    // Compare starts with the current snapshot (B, the hint).
    assert!(c.compare_snapshot(None));
    assert!(c.snapshot_menu().comparing);
    assert_eq!(c.doc().console.compare.as_ref().unwrap().snapshot, 2);
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    assert!(c.compare_snapshot(None));
    assert_eq!(
        value(&c, "channel", "fader"),
        0.61,
        "back to the working mix"
    );
    assert!(c.end_compare());
    assert!(!c.snapshot_menu().comparing);
    assert!(!c.end_compare());
    assert!(c.last_rejection().unwrap().contains("compared"));
}

#[test]
fn commands_without_a_snapshot_say_what_to_do() {
    let mut c = core();
    assert!(!c.update_snapshot(None));
    assert!(c.last_rejection().unwrap().contains("Store or recall"));
    assert!(!c.recall_snapshot(None));
    assert!(!c.delete_snapshot(None));
    assert!(!c.recall_adjacent(true));
    assert!(c.last_rejection().unwrap().contains("no snapshots"));
    assert!(!c.compare_snapshot(None));
    let m = c.snapshot_menu();
    assert!(m.can_store);
    assert!(!(m.can_update || m.can_recall || m.can_cycle || m.can_compare || m.can_delete));
}

#[test]
fn sixty_four_snapshots_are_the_most() {
    let mut c = core();
    for _ in 0..64 {
        assert!(c.store_snapshot(None));
    }
    assert!(!c.snapshot_menu().can_store);
    assert!(!c.store_snapshot(None));
    assert!(c.last_rejection().unwrap().contains("at most 64"));
}

#[test]
fn undo_and_redo_keep_the_hint() {
    let mut c = two_snapshots();
    assert!(c.recall_snapshot(Some(1)));
    set(&mut c, "channel", "fader", 0.61);
    assert!(c.undo());
    assert_eq!(c.current_snapshot(), Some(1));
    assert!(c.undo());
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    assert!(c.redo());
    assert_eq!(value(&c, "channel", "fader"), 0.3);
}

// ---- Morph ---------------------------------------------------------------------------------

/// Plays the morph with frames every 50 ms from `t0`, until it ends.
fn play(c: &mut AppCore<RecordingSink>, t0: f64) -> usize {
    let mut frames = 0;
    let mut now = t0;
    while c.snapshot_morphing() {
        c.transport_frame(now);
        now += 0.05;
        frames += 1;
        assert!(frames < 1000, "the morph never ends");
    }
    frames
}

#[test]
fn a_morph_moves_the_mix_to_the_other_snapshot_over_the_morph_time_in_one_undo_step() {
    let mut c = two_snapshots();
    assert!(c.recall_snapshot(Some(1)));
    let start = c.doc().clone();
    c.set_morph_seconds(2);
    assert!(c.morph_snapshot(None, true));
    assert!(c.snapshot_morphing());
    // The first frame is the start of the clock, half way is half way.
    c.transport_frame(10.0);
    assert!((value(&c, "channel", "fader") - 0.3).abs() < 1e-5);
    c.transport_frame(11.0);
    assert!((value(&c, "channel", "fader") - 0.6).abs() < 1e-5);
    assert!(c.snapshot_morphing());
    c.transport_frame(11.5);
    c.transport_frame(12.0);
    assert!(!c.snapshot_morphing());
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    assert_eq!(c.current_snapshot(), Some(2));
    assert!(c.undo());
    assert_eq!(c.doc(), &start, "the whole morph is one undo step");
}

#[test]
fn a_morph_reaches_the_engine_as_param_changes_and_rebuilds_nothing() {
    let mut c = two_snapshots();
    assert!(c.recall_snapshot(Some(1)));
    c.sink_mut().take();
    assert!(c.morph_snapshot(Some(2), true));
    let frames = play(&mut c, 0.0);
    assert!(
        frames > 10,
        "a two second morph takes many frames: {frames}"
    );
    let calls = c.sink_mut().take();
    let params = calls
        .iter()
        .filter(|x| matches!(x, SinkCall::Param { .. }))
        .count();
    assert!(params >= 10, "{params} param changes");
    assert!(
        !calls
            .iter()
            .any(|x| matches!(x, SinkCall::Routing(_) | SinkCall::Reset { .. })),
        "a morph rebuilds nothing"
    );
}

#[test]
fn another_edit_takes_the_mix_over_and_ends_the_morph() {
    let mut c = two_snapshots();
    assert!(c.recall_snapshot(Some(1)));
    assert!(c.morph_snapshot(None, true));
    c.transport_frame(0.0);
    c.transport_frame(1.0);
    let mid = value(&c, "channel", "fader");
    set(&mut c, "channel", "pan", 0.7);
    assert!(!c.snapshot_morphing());
    c.transport_frame(5.0);
    assert_eq!(value(&c, "channel", "fader"), mid, "it stays where it was");
}

#[test]
fn a_morph_needs_a_snapshot_that_is_not_the_mix_and_no_compare() {
    let mut c = core();
    assert!(!c.morph_snapshot(None, true));
    assert!(c.last_rejection().unwrap().contains("Store a snapshot"));
    assert!(!c.snapshot_menu().can_morph);
    assert!(c.store_snapshot(None));
    assert!(!c.morph_snapshot(None, true));
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("already that snapshot")
    );
    assert!(!c.snapshot_menu().can_morph, "the only snapshot is the mix");
    set(&mut c, "channel", "fader", 0.4);
    assert!(c.snapshot_menu().can_morph, "a moved mix can morph back");

    let mut c = two_snapshots();
    assert!(c.snapshot_menu().can_morph);
    assert!(c.compare_snapshot(None));
    assert!(!c.snapshot_menu().can_morph);
    assert!(!c.morph_snapshot(None, true));
    assert!(c.last_rejection().unwrap().contains("compare"));
    assert!(c.end_compare());
    assert!(c.morph_snapshot(Some(1), true));
    assert!(!c.snapshot_menu().can_morph, "one at a time");
    assert!(!c.morph_snapshot(Some(1), true));
    assert!(c.last_rejection().unwrap().contains("already playing"));
}

#[test]
fn a_morph_starts_from_the_mix_as_it_is_even_when_no_snapshot_holds_it() {
    let mut c = two_snapshots();
    // The mix is neither snapshot, and no snapshot is current.
    set(&mut c, "channel", "fader", 0.6);
    set(&mut c, "channel", "pan", 0.5);
    set(&mut c, "channel", "mute", 1.0);
    assert!(c.recall_snapshot(Some(1)));
    set(&mut c, "channel", "fader", 0.6);
    let before = c.doc().clone();
    assert!(c.morph_snapshot(Some(2), true));
    c.transport_frame(0.0);
    assert_eq!(
        value(&c, "channel", "fader"),
        0.6,
        "the first frame does not jump to the snapshot the mix last came from"
    );
    c.transport_frame(1.0);
    let f = value(&c, "channel", "fader");
    assert!((f - 0.75).abs() < 1e-5, "half way from 0.6 to 0.9: {f}");
    c.transport_frame(2.0);
    assert!(!c.snapshot_morphing());
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    assert_eq!(c.current_snapshot(), Some(2));
    assert!(c.undo());
    assert_eq!(c.doc(), &before, "the morph is one undo step");
}

#[test]
fn with_no_current_snapshot_next_is_the_first_and_previous_the_last() {
    let mut c = two_snapshots();
    set(&mut c, "channel", "fader", 0.5);
    assert_eq!(c.current_snapshot(), Some(2), "the hint stays");
    // Deleting the hinted snapshot leaves none current.
    assert!(c.delete_snapshot(Some(2)));
    assert_eq!(c.current_snapshot(), None);
    assert!(c.morph_snapshot(None, true));
    play(&mut c, 0.0);
    assert_eq!(c.current_snapshot(), Some(1));
}

#[test]
fn the_neighbours_wrap_round_for_next_and_previous() {
    let mut c = two_snapshots();
    assert_eq!(c.current_snapshot(), Some(2));
    assert!(c.morph_snapshot(None, true));
    play(&mut c, 0.0);
    assert_eq!(c.current_snapshot(), Some(1), "next wraps to the first");
    assert!(c.morph_snapshot(None, false));
    play(&mut c, 0.0);
    assert_eq!(c.current_snapshot(), Some(2), "previous wraps to the last");
}

#[test]
fn the_morph_time_is_one_of_the_menu_choices() {
    let mut c = two_snapshots();
    assert_eq!(c.snapshot_menu().morph_seconds, 2);
    c.set_morph_seconds(8);
    assert_eq!(c.snapshot_menu().morph_seconds, 8);
    assert!(c.recall_snapshot(Some(1)));
    assert!(c.morph_snapshot(None, true));
    c.transport_frame(0.0);
    c.transport_frame(4.0);
    assert!(c.snapshot_morphing(), "eight seconds are not over at four");
    c.transport_frame(8.0);
    assert!(!c.snapshot_morphing());
}

// ---- Effect params -------------------------------------------------------------------------

#[test]
fn the_current_snapshot_can_hold_the_effect_params() {
    let mut c = two_snapshots();
    assert!(!c.snapshot_menu().effects);
    assert!(c.snapshot_menu().can_effects);
    assert!(c.toggle_snapshot_effects(None));
    assert!(c.snapshot_menu().effects);
    assert!(c.doc().console.snapshot(2).unwrap().state.effects);
    assert!(c.undo());
    assert!(!c.doc().console.snapshot(2).unwrap().state.effects);
    assert!(c.redo());
    assert!(c.toggle_snapshot_effects(None));
    assert!(!c.snapshot_menu().effects);
}

#[test]
fn a_new_core_has_no_morph_and_a_new_document_ends_one() {
    let mut c = two_snapshots();
    assert!(!c.snapshot_morphing());
    assert!(c.recall_snapshot(Some(1)));
    assert!(c.morph_snapshot(None, true));
    c.new_document();
    assert!(!c.snapshot_morphing());
}

// ---- Marker recall -------------------------------------------------------------------------

fn marker(id: u32, tick: i64, name: &str) -> atomik_model::Marker {
    atomik_model::Marker {
        id,
        tick,
        name: name.into(),
        color: None,
    }
}

/// `two_snapshots` with markers `Verse` at 1000 and `Chorus` at 5000, `A` recalling at `Verse`
/// and `B` at `chorus`; the mix is `B`'s.
fn marked() -> AppCore<RecordingSink> {
    let mut c = two_snapshots();
    c.apply_command(Command::Song(atomik_model::SongCommand::SetMarkers(vec![
        marker(1, 1000, "Verse"),
        marker(2, 5000, "Chorus"),
    ])))
    .expect("markers");
    for (id, name) in [(1, "Verse"), (2, "chorus")] {
        c.apply_command(Command::Console(ConsoleCommand::SetSnapshotMarker {
            id,
            marker: Some(name.into()),
        }))
        .expect("marker set");
    }
    c
}

#[test]
fn the_playhead_crossing_a_marker_recalls_its_snapshot() {
    let mut c = marked();
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    // Not playing: nothing happens however far the playhead is.
    c.snapshot_marker_frame(false, 1200);
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    // Playing from before the marker: the frame that crosses it recalls.
    c.snapshot_marker_frame(true, 100);
    c.snapshot_marker_frame(true, 900);
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    c.snapshot_marker_frame(true, 1300);
    assert_eq!(value(&c, "channel", "fader"), 0.3, "Verse recalls A");
    assert_eq!(c.current_snapshot(), Some(1));
    // The next frames do not recall again, so an edit made after the marker stays.
    set(&mut c, "channel", "fader", 0.5);
    c.snapshot_marker_frame(true, 1500);
    assert_eq!(value(&c, "channel", "fader"), 0.5);
    // A later marker, matched ignoring case.
    c.snapshot_marker_frame(true, 5200);
    assert_eq!(value(&c, "channel", "fader"), 0.9, "Chorus recalls B");
}

#[test]
fn a_start_on_a_marker_and_a_wrap_to_one_recall_it_and_a_locate_past_one_does_not() {
    let mut c = marked();
    // Starting right at the marker.
    c.snapshot_marker_frame(true, 1000);
    assert_eq!(value(&c, "channel", "fader"), 0.3);
    // The song plays on past Chorus, then the loop wraps back to just after it.
    c.snapshot_marker_frame(false, 0);
    set(&mut c, "channel", "fader", 0.5);
    c.snapshot_marker_frame(true, 20000);
    assert_eq!(value(&c, "channel", "fader"), 0.5);
    c.snapshot_marker_frame(true, 5100);
    assert_eq!(
        value(&c, "channel", "fader"),
        0.9,
        "a wrap that lands past Chorus recalls it"
    );
    // Starting well past a marker does not recall it.
    c.snapshot_marker_frame(false, 0);
    set(&mut c, "channel", "fader", 0.5);
    c.snapshot_marker_frame(true, 4900);
    assert_eq!(value(&c, "channel", "fader"), 0.5);
}

#[test]
fn a_compare_or_a_morph_in_progress_is_left_alone_by_marker_recall() {
    let mut c = marked();
    assert!(c.compare_snapshot(Some(2)));
    let comparing = c.doc().clone();
    c.snapshot_marker_frame(true, 500);
    c.snapshot_marker_frame(true, 1300);
    assert_eq!(c.doc(), &comparing);
    assert!(c.end_compare());
    assert!(c.morph_snapshot(Some(1), true));
    c.snapshot_marker_frame(true, 5000);
    c.snapshot_marker_frame(true, 5100);
    assert!(c.snapshot_morphing());
}

#[test]
fn a_marker_with_no_snapshot_or_a_snapshot_with_no_marker_does_nothing() {
    let mut c = marked();
    c.apply_command(Command::Song(atomik_model::SongCommand::SetMarkers(vec![
        marker(1, 1000, "Bridge"),
    ])))
    .expect("markers");
    c.snapshot_marker_frame(true, 500);
    c.snapshot_marker_frame(true, 1500);
    assert_eq!(value(&c, "channel", "fader"), 0.9);
    let mut c = two_snapshots();
    c.snapshot_marker_frame(true, 0);
    c.snapshot_marker_frame(true, 9000);
    assert_eq!(value(&c, "channel", "fader"), 0.9);
}
