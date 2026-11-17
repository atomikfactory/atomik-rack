//! The Chord Trainer pane in the app core: opening and closing with the other editors, the pane's
//! commands from any focus, the drills on an injected clock without the song transport, the click
//! through the preview voice, listening with synthetic chroma, and that the scores are the
//! user's (saved with the window state) and never the project's.

use super::tests::rack_only_core;
use super::*;
use crate::sink::{RecordingSink, SinkCall};
use crate::ui_state::UiState;
use atomik_dsp::listen::PlayedNote;
use atomik_tab::grade;
use atomik_ui::{ChordTrainerView, Key, Modifiers, PointerButton, TrainerButton, UiEvent};

fn core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    c.set_input_channels(2);
    c.open_chord_trainer();
    c.set_trainer_seed(3);
    c
}

fn trainer_cmd(c: &mut AppCore<RecordingSink>, id: &str, arg: Option<&str>) {
    // From the rack's focus: the trainer's commands reach the pane whichever pane has the focus.
    assert!(
        c.pane_command(Pane::Rack, id, arg).is_some(),
        "{id} {arg:?} was not taken"
    );
}

/// Sets 120 BPM with two beats a chord (a change every second).
fn one_second_chords(c: &mut AppCore<RecordingSink>) {
    for _ in 0..12 {
        trainer_cmd(c, "editor.trainer_bpm", Some("up"));
    }
    for _ in 0..2 {
        trainer_cmd(c, "editor.trainer_beats", Some("down"));
    }
    assert_eq!(c.chord_trainer().config().bpm, 120);
}

fn calls(c: &AppCore<RecordingSink>) -> Vec<SinkCall> {
    c.sink().calls.clone()
}

fn count(c: &AppCore<RecordingSink>, pred: impl Fn(&SinkCall) -> bool) -> usize {
    c.sink().calls.iter().filter(|x| pred(x)).count()
}

/// A note of the tracker with the chroma of chord `i` of the pane's set.
fn note_of(c: &AppCore<RecordingSink>, i: usize) -> PlayedNote {
    let mask = grade::pc_mask(&c.chord_trainer().chords()[i].pitch_classes());
    let mut chroma = [0.03f32; 12];
    for (pc, v) in chroma.iter_mut().enumerate() {
        if mask >> pc & 1 == 1 {
            *v = 1.0;
        }
    }
    PlayedNote {
        frame: 0,
        key: None,
        cents: 0.0,
        clarity: 0.0,
        level: 0.1,
        chroma,
    }
}

/// Lets the drill hear chord `i` as played at `t` on the frame clock: the frame at `t + 0.2` s
/// receives a note that started 0.2 s (9600 frames at 48 kHz) before.
fn hear_at(c: &mut AppCore<RecordingSink>, t: f64, i: usize) {
    let rate = u64::from(c.doc().sample_rate);
    let behind = rate / 5;
    c.transport_frame(t + 0.2);
    let note = note_of(c, i);
    c.trainer_heard(&note, note.frame + behind);
}

#[test]
fn opening_the_pane_closes_the_other_editors_and_they_close_it() {
    let mut c = rack_only_core();
    c.open_chord_sheet();
    assert!(c.chord_sheet_open());
    c.open_chord_trainer();
    assert!(c.chord_trainer_open());
    assert!(!c.chord_sheet_open(), "one at a time");
    c.open_chord_sheet();
    assert!(!c.chord_trainer_open());
    c.open_chord_trainer();
    assert!(c.close_open_editor());
    assert!(!c.chord_trainer_open());
    assert!(!c.close_open_editor(), "nothing else is open");
}

#[test]
fn the_panes_commands_reach_it_from_any_focus_and_only_while_it_is_open() {
    let mut c = core();
    trainer_cmd(&mut c, "editor.trainer_bpm", Some("up"));
    assert_eq!(c.chord_trainer().config().bpm, 65);
    trainer_cmd(&mut c, "editor.trainer_capo", Some("up"));
    assert_eq!(c.chord_trainer().capo(), 1);
    c.close_chord_trainer();
    assert!(
        c.pane_command(Pane::Arrangement, "editor.trainer_bpm", Some("up"))
            .is_none(),
        "a closed pane takes nothing"
    );
    assert_eq!(c.chord_trainer().config().bpm, 65);
}

#[test]
fn the_pane_handles_events_through_the_core() {
    let mut c = core();
    let p = c
        .chord_trainer()
        .button_center(TrainerButton::KeyUp)
        .unwrap();
    c.handle_pane_event(
        Pane::ChordTrainer,
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
    );
    assert_eq!(c.chord_trainer().key(), (1, false));
    let _ = Key::Escape;
    let _: &ChordTrainerView = c.chord_trainer();
}

#[test]
fn a_timed_drill_runs_on_the_frame_clock_without_the_song_transport() {
    let mut c = core();
    one_second_chords(&mut c);
    assert!(!c.is_playing());
    let before = c.sink().calls.len();
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(100.0);
    assert!(c.chord_trainer().running());
    assert_eq!(c.chord_trainer().drill().unwrap().current(), 0);
    c.transport_frame(101.0);
    assert_eq!(c.chord_trainer().drill().unwrap().current(), 1);
    c.transport_frame(130.0);
    let d = c.chord_trainer().drill().unwrap();
    assert_eq!(d.changes(), 30);
    assert!((d.changes_per_minute(130.0) - 60.0).abs() < 0.5);
    assert!((d.elapsed(130.0) - 30.0).abs() < 1e-9);
    // The song transport was never touched.
    assert!(!c.is_playing());
    let transport = c.sink().calls[before..]
        .iter()
        .filter(|x| matches!(x, SinkCall::Transport(_)))
        .count();
    assert_eq!(transport, 0);
}

#[test]
fn the_click_is_a_preview_stream_that_follows_the_drill() {
    let mut c = core();
    assert!(!c.chord_trainer_active());
    trainer_cmd(&mut c, "editor.trainer_start", None);
    assert!(c.chord_trainer_active(), "frames must keep coming");
    c.transport_frame(10.0);
    assert!(c.trainer_click_running());
    assert_eq!(count(&c, |x| matches!(x, SinkCall::PreviewStart { .. })), 1);
    // More frames do not start it again.
    c.transport_frame(11.0);
    c.transport_frame(12.0);
    assert_eq!(count(&c, |x| matches!(x, SinkCall::PreviewStart { .. })), 1);
    // A new tempo is a new session and a new click.
    trainer_cmd(&mut c, "editor.trainer_bpm", Some("up"));
    c.transport_frame(13.0);
    assert!(c.trainer_click_running());
    assert_eq!(count(&c, |x| matches!(x, SinkCall::PreviewStart { .. })), 2);
    // Stopping the drill stops the click.
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(14.0);
    assert!(!c.trainer_click_running());
    assert!(calls(&c).last() == Some(&SinkCall::PreviewStop));
}

#[test]
fn a_listen_drill_has_no_click_and_closing_the_pane_stops_everything() {
    let mut c = core();
    trainer_cmd(&mut c, "editor.trainer_mode", Some("listen"));
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(5.0);
    assert!(c.chord_trainer().running());
    assert!(!c.trainer_click_running());
    assert_eq!(count(&c, |x| matches!(x, SinkCall::PreviewStart { .. })), 0);
    trainer_cmd(&mut c, "editor.trainer_mode", Some("timed"));
    c.transport_frame(6.0);
    assert!(c.trainer_click_running());
    c.close_chord_trainer();
    assert!(!c.trainer_click_running());
    assert!(!c.chord_trainer().running());
}

#[test]
fn the_input_the_drill_listens_to_is_the_practice_bars() {
    let mut c = core();
    c.transport_frame(1.0);
    assert_eq!(c.chord_trainer().input(), Some("Input 1"));
    c.practice_action(crate::practice::PracticeAction::ListenInput);
    c.transport_frame(2.0);
    assert_eq!(c.chord_trainer().input(), Some("Input 2"));
    // With no interface input a listening drill is refused.
    let mut none = rack_only_core();
    none.set_input_channels(0);
    none.open_chord_trainer();
    trainer_cmd(&mut none, "editor.trainer_mode", Some("listen"));
    none.transport_frame(1.0);
    assert!(none.chord_trainer().input().is_none());
    assert!(
        none.pane_command(Pane::Rack, "editor.trainer_start", None)
            .is_some()
    );
    none.transport_frame(2.0);
    assert!(!none.chord_trainer().running());
}

#[test]
fn a_verified_drill_hears_the_chords_through_synthetic_chroma_and_scores_them() {
    let mut c = core();
    one_second_chords(&mut c);
    trainer_cmd(&mut c, "editor.trainer_mode", Some("verified"));
    trainer_cmd(&mut c, "editor.trainer_order", Some("pair"));
    let (a, b) = c.chord_trainer().pair();
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(100.0);
    for k in 0..10u32 {
        let t = 100.0 + f64::from(k);
        c.transport_frame(t + 0.05);
        let chord = c.chord_trainer().drill().unwrap().current();
        let heard = if k == 3 {
            if chord == a { b } else { a }
        } else {
            chord
        };
        hear_at(&mut c, t + 0.1, heard);
    }
    c.transport_frame(110.5);
    trainer_cmd(&mut c, "editor.trainer_start", None);
    let (hits, judged) = c.chord_trainer().drill().unwrap().hits();
    assert!(judged >= 9, "{judged}");
    assert_eq!(judged - hits, 1, "the wrong chord of window 3");
    let scores = c.take_trainer_scores_change().expect("the session counted");
    assert_eq!(scores.len(), 1);
    let (_, best) = scores.rows().next().unwrap();
    assert!(best.hit_rate > 0.8 && best.hit_rate < 1.0);
    assert_eq!(best.sessions, 1);
    assert!(c.take_trainer_scores_change().is_none());
}

#[test]
fn a_listen_drill_advances_when_the_chord_is_heard() {
    let mut c = core();
    trainer_cmd(&mut c, "editor.trainer_mode", Some("listen"));
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(50.0);
    // A wrong chord first, then the right one.
    hear_at(&mut c, 51.0, 4);
    assert_eq!(c.chord_trainer().drill().unwrap().current(), 0);
    hear_at(&mut c, 52.0, 0);
    assert_eq!(c.chord_trainer().drill().unwrap().current(), 1);
    hear_at(&mut c, 54.0, 1);
    let d = c.chord_trainer().drill().unwrap();
    assert_eq!(d.current(), 2);
    assert_eq!(d.outcomes().len(), 1);
    assert!((d.outcomes()[0].seconds.unwrap() - 2.0).abs() < 1e-6);
}

#[test]
fn latency_compensation_places_what_was_heard_earlier() {
    let mut c = core();
    trainer_cmd(&mut c, "editor.trainer_mode", Some("listen"));
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(50.0);
    // The player's offset of 300 ms moves the note before the chord was shown: not it.
    c.set_listen_offset_ms(300);
    hear_at(&mut c, 50.2, 0);
    assert_eq!(c.chord_trainer().drill().unwrap().current(), 0);
    c.set_listen_offset_ms(0);
    hear_at(&mut c, 50.2, 0);
    assert_eq!(c.chord_trainer().drill().unwrap().current(), 1);
}

#[test]
fn the_pane_never_touches_the_project() {
    let mut c = core();
    let before = c.doc().clone();
    let depth = c.document().undo_depth();
    one_second_chords(&mut c);
    trainer_cmd(&mut c, "editor.trainer_mode", Some("verified"));
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(0.0);
    for k in 0..8u32 {
        hear_at(&mut c, f64::from(k) + 0.1, 0);
    }
    c.transport_frame(9.0);
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.close_chord_trainer();
    assert_eq!(*c.doc(), before);
    assert_eq!(c.document().undo_depth(), depth, "no undo step");
}

#[test]
fn the_scores_are_saved_with_the_window_state_and_come_back() {
    let mut c = core();
    one_second_chords(&mut c);
    trainer_cmd(&mut c, "editor.trainer_mode", Some("verified"));
    trainer_cmd(&mut c, "editor.trainer_order", Some("pair"));
    trainer_cmd(&mut c, "editor.trainer_start", None);
    c.transport_frame(0.0);
    for k in 0..8u32 {
        c.transport_frame(f64::from(k) + 0.05);
        let chord = c.chord_trainer().drill().unwrap().current();
        hear_at(&mut c, f64::from(k) + 0.1, chord);
    }
    c.transport_frame(9.0);
    trainer_cmd(&mut c, "editor.trainer_start", None);
    let scores = c.take_trainer_scores_change().expect("scores");
    let mut ui = UiState::default();
    ui.set_trainer_scores(&scores);
    let dir = std::env::temp_dir().join(format!("atomik-trainer-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui-state.json");
    ui.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("chord_trainer_scores"));
    let back = UiState::load(&path);
    assert_eq!(back.trainer_scores(), scores);
    let _ = std::fs::remove_dir_all(&dir);
    // A new core starts from them.
    let mut fresh = core();
    fresh.set_trainer_scores(back.trainer_scores());
    assert_eq!(fresh.chord_trainer().scores(), &scores);
    assert!(fresh.take_trainer_scores_change().is_none());
}

#[test]
fn a_state_file_with_nonsense_scores_is_brought_into_range() {
    let json = r#"{"chord_trainer_scores":{"Am|C":{"cpm":1e9,"hit_rate":7.0,"sessions":3},
        "":{"cpm":1.0,"hit_rate":1.0,"sessions":1}}}"#;
    let ui: UiState = serde_json::from_str(json).unwrap();
    let scores = ui.trainer_scores();
    assert_eq!(scores.len(), 1);
    let b = scores.get("Am|C").unwrap();
    assert_eq!((b.cpm, b.hit_rate), (1000.0, 1.0));
    // No scores: the key is not written.
    assert!(
        !serde_json::to_string(&UiState::default())
            .unwrap()
            .contains("chord_trainer_scores")
    );
}
