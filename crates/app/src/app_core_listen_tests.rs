//! Listen and grade in the app core: the switch, the practice bar's state, the verdicts the tab
//! editor shows, the trainer asking the accuracy judge, the latency offset and its calibration,
//! and that none of it touches the project. The recording sink has no engine to tap, so the tests
//! attach a listener without a tap and feed it transport views and synthetic plucks (the pure
//! pipeline has its own tests in `listen/tests.rs`).

use super::tests::rack_only_core;
use super::*;
use crate::listen::TransportView;
use crate::listen::tests::{BAR, LEAD, RIFF, SR, STEP, TPF_FULL, frame_of, pluck};
use crate::practice::PracticeAction;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{Clip, ClipContent, ClipId, Command, DeviceKey, Note, SongCommand};
use atomik_tab::grade::Verdict;
use atomik_ui::grading;

const SYNTH: &str = "atomik.poly-synth";

fn clip_with(notes: &[(i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(t, k)| Note::new(t, PPQ / 2, k, 0.8))
                .collect(),
        ),
    }
}

/// A core with a synth track holding the riff in one clip, the tab editor open on it.
fn core_with_riff() -> (AppCore<RecordingSink>, ClipId) {
    let mut c = rack_only_core();
    let key = DeviceKey("a".to_string());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(key.clone()),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key.clone(),
    }))
    .unwrap();
    let notes: Vec<(i64, u8)> = RIFF.iter().map(|(t, k)| (*t, *k as u8)).collect();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key,
        lane: 0,
        clip: clip_with(&notes),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (0, BAR),
        enabled: true,
    }))
    .unwrap();
    let id = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.open_tab_editor(id);
    c.sink_mut().take();
    (c, id)
}

fn practice(c: &mut AppCore<RecordingSink>, a: PracticeAction) {
    c.practice_action(a);
}

/// What a player did: the notes `(tick in the bar, key)` plucked on each lap, `delay` frames late.
fn playing(laps: u64, notes: &[(i64, i32)], delay: usize) -> Vec<f32> {
    let frames = frame_of((laps * BAR as u64) as f64, TPF_FULL) + SR as usize;
    let mut x = vec![0.0f32; frames];
    for lap in 0..laps as i64 {
        for (tick, key) in notes {
            pluck(
                &mut x,
                frame_of((lap * BAR + tick) as f64, TPF_FULL) + delay,
                *key,
            );
        }
    }
    x
}

/// Runs the transport for `laps` laps of the loop and then stops it, feeding the core a frame at
/// a time as the app does. The input has been idle for [`LEAD`] frames first.
fn run(c: &mut AppCore<RecordingSink>, audio: &[f32], laps: u64) {
    for f in (0..LEAD).step_by(STEP) {
        let idle = TransportView {
            playing: false,
            engine_frame: f as u64,
            tick: 0,
            loop_count: 0,
            loop_range: Some((0, BAR)),
        };
        c.listen_consume(&idle, vec![(f as u64, vec![0.0; STEP])]);
    }
    let total = frame_of((laps * BAR as u64) as f64, TPF_FULL);
    let mut f = 0usize;
    while f < total {
        let unwrapped = f as f64 * TPF_FULL;
        let lap = (unwrapped / BAR as f64).floor() as u64;
        let view = TransportView {
            playing: true,
            engine_frame: (f + LEAD) as u64,
            tick: (unwrapped - lap as f64 * BAR as f64).round() as i64,
            loop_count: lap,
            loop_range: Some((0, BAR)),
        };
        let end = (f + STEP).min(audio.len());
        c.listen_consume(&view, vec![((f + LEAD) as u64, audio[f..end].to_vec())]);
        f += STEP;
    }
    let stop = TransportView {
        playing: false,
        engine_frame: (f + LEAD) as u64,
        tick: 0,
        loop_count: laps,
        loop_range: Some((0, BAR)),
    };
    for _ in 0..60 {
        let end = (f + STEP).min(audio.len());
        let chunk = if f < end {
            audio[f..end].to_vec()
        } else {
            vec![0.0; STEP]
        };
        c.listen_consume(&stop, vec![((f + LEAD) as u64, chunk)]);
        f += STEP;
    }
}

fn verdicts(c: &AppCore<RecordingSink>) -> Vec<Verdict> {
    let marks = c.tab_editor().grades().expect("grades shown");
    RIFF.iter()
        .map(|(t, k)| grading::verdict_of(marks, *t, *k as u8).expect("a mark"))
        .collect()
}

#[test]
fn the_switch_turns_listening_on_and_off_and_shows_in_the_bar() {
    let (mut c, _) = core_with_riff();
    assert!(!c.listen_on());
    assert!(!c.practice_state().listen.on);
    practice(&mut c, PracticeAction::ToggleListen);
    assert!(c.listen_on());
    let l = c.practice_state().listen;
    assert!(l.on);
    assert_eq!(l.input, Some((0, true)), "a guitar is one channel: input 1");
    assert_eq!(l.threshold, 90);
    practice(&mut c, PracticeAction::ToggleListen);
    assert!(!c.listen_on());
}

#[test]
fn the_inputs_are_single_channels_then_pairs_and_the_next_one_cycles() {
    let all = listen_ops::listen_inputs(4);
    let labels: Vec<String> = all.iter().map(|i| i.label()).collect();
    assert_eq!(
        labels,
        [
            "Input 1",
            "Input 2",
            "Input 3",
            "Input 4",
            "Inputs 1-2",
            "Inputs 3-4"
        ]
    );
    let (mut c, _) = core_with_riff();
    let first = c.listen_input().unwrap();
    assert_eq!(first.label(), "Input 1");
    practice(&mut c, PracticeAction::ListenInput);
    assert_eq!(c.listen_input().unwrap().label(), "Input 2");
    practice(&mut c, PracticeAction::ListenInput);
    assert_eq!(c.listen_input().unwrap().label(), "Inputs 1-2");
    practice(&mut c, PracticeAction::ListenInput);
    assert_eq!(c.listen_input().unwrap().label(), "Input 1", "it wraps");
}

#[test]
fn a_graded_pass_colours_the_tab_and_fills_the_bar_and_nothing_is_saved() {
    let (mut c, _) = core_with_riff();
    let before = c.doc().clone();
    c.listen_attach_for_test(SR);
    let notes: Vec<(i64, i32)> = RIFF.to_vec();
    run(&mut c, &playing(2, &notes, 0), 2);
    let l = c.practice_state().listen;
    assert_eq!(l.last_percent, Some(100));
    assert_eq!(l.streak, 8, "four notes on each of two passes");
    assert_eq!(l.history_len, 2);
    assert_eq!(l.passes(), [100, 100]);
    assert_eq!(verdicts(&c), [Verdict::Hit; 4]);
    assert_eq!(c.listen_session().passes(), 2);
    // Not saved, no undo step, no document change.
    assert!(c.doc() == &before);
}

#[test]
fn a_missed_and_a_wrong_note_show_on_the_tab() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    // The second note is left out, the third is another pitch.
    let played = [(0, 40), (30_720, 53), (46_080, 55)];
    run(&mut c, &playing(1, &played, 0), 1);
    assert_eq!(
        verdicts(&c),
        [Verdict::Hit, Verdict::Missed, Verdict::Wrong, Verdict::Hit]
    );
    let l = c.practice_state().listen;
    assert_eq!(l.last_percent, Some(50));
    assert_eq!(l.streak, 1);
    // The neck gets the same verdicts by place.
    assert!(c.tab_editor().fretboard_feed().grades.is_some());
}

#[test]
fn clearing_the_results_clears_the_tab_the_bar_and_the_session() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    run(&mut c, &playing(1, &RIFF, 0), 1);
    assert!(c.tab_editor().grades().is_some());
    practice(&mut c, PracticeAction::ListenClear);
    assert!(c.tab_editor().grades().is_none());
    let l = c.practice_state().listen;
    assert_eq!((l.last_percent, l.history_len, l.streak), (None, 0, 0));
    assert_eq!(c.listen_session().passes(), 0);
    assert!(c.tab_editor().fretboard_feed().grades.is_none());
}

#[test]
fn another_clip_shows_no_verdicts_of_the_last_one() {
    let (mut c, id) = core_with_riff();
    c.listen_attach_for_test(SR);
    run(&mut c, &playing(1, &RIFF, 0), 1);
    assert!(c.tab_editor().grades().is_some());
    c.close_tab_editor();
    c.show_transport();
    assert!(c.tab_editor().grades().is_none());
    c.open_tab_editor(id);
    c.show_transport();
    assert!(c.tab_editor().grades().is_some(), "its own clip again");
}

#[test]
fn the_trainer_counts_only_passes_that_reach_the_threshold() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    // Trainer on at 50 %, a step after one clean pass.
    practice(&mut c, PracticeAction::ToggleTrainer);
    practice(
        &mut c,
        PracticeAction::Trainer(atomik_ui::TrainerValue::After, false),
    );
    assert_eq!(c.practice().trainer().after, 1);
    assert_eq!(c.practice().speed(), 50);
    // A pass that misses two of four notes is not clean: the speed stays, the pass counts.
    let sloppy = [(0, 40), (46_080, 55)];
    run(&mut c, &playing(1, &sloppy, 0), 1);
    assert_eq!(c.practice().trainer().passes, 1);
    assert_eq!(c.practice().speed(), 50, "dirty pass");
    // A clean pass steps up.
    run(&mut c, &playing(1, &RIFF, 0), 1);
    assert_eq!(c.practice().trainer().passes, 2);
    assert_eq!(c.practice().speed(), 55);
}

#[test]
fn loop_wraps_are_not_counted_a_second_time_while_passes_are_graded() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    c.sink_mut().state.playing = true;
    c.handle_action(atomik_ui::UiAction::Transport(
        atomik_ui::TransportAction::Play,
    ));
    practice(&mut c, PracticeAction::ToggleTrainer);
    c.sink_mut().state.loop_count = 0;
    c.transport_frame(1.0);
    c.sink_mut().state.loop_count = 3;
    c.transport_frame(2.0);
    assert_eq!(
        c.practice().trainer().passes,
        0,
        "graded passes count through the grade, not the wrap"
    );
}

#[test]
fn without_listening_the_trainer_counts_every_wrap_as_before() {
    let (mut c, _) = core_with_riff();
    c.handle_action(atomik_ui::UiAction::Transport(
        atomik_ui::TransportAction::Play,
    ));
    practice(&mut c, PracticeAction::ToggleTrainer);
    c.sink_mut().state.loop_count = 0;
    c.transport_frame(1.0);
    c.sink_mut().state.loop_count = 3;
    c.transport_frame(2.0);
    assert_eq!(c.practice().trainer().passes, 3);
}

#[test]
fn the_threshold_steps_by_five_inside_its_range_and_reaches_the_judge() {
    let (mut c, _) = core_with_riff();
    practice(&mut c, PracticeAction::ListenThreshold(true));
    assert_eq!(c.listen_threshold(), 95);
    practice(&mut c, PracticeAction::ListenThreshold(true));
    practice(&mut c, PracticeAction::ListenThreshold(true));
    assert_eq!(c.listen_threshold(), 100, "the top");
    for _ in 0..30 {
        practice(&mut c, PracticeAction::ListenThreshold(false));
    }
    assert_eq!(c.listen_threshold(), 50, "the bottom");
    assert_eq!(c.practice_state().listen.threshold, 50);
}

#[test]
fn a_louder_threshold_makes_a_good_pass_dirty() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    practice(&mut c, PracticeAction::ToggleTrainer);
    practice(
        &mut c,
        PracticeAction::Trainer(atomik_ui::TrainerValue::After, false),
    );
    for _ in 0..30 {
        practice(&mut c, PracticeAction::ListenThreshold(true));
    }
    assert_eq!(c.listen_threshold(), 100);
    // Three of four notes: 75 %.
    run(
        &mut c,
        &playing(1, &[(0, 40), (15_360, 45), (30_720, 50)], 0),
        1,
    );
    assert_eq!(c.practice_state().listen.last_percent, Some(75));
    assert_eq!(c.practice().speed(), 50);
}

#[test]
fn the_offset_steps_and_is_handed_to_the_shell_once() {
    let (mut c, _) = core_with_riff();
    assert_eq!(c.take_listen_offset_change(), None);
    practice(&mut c, PracticeAction::ListenOffset(true));
    practice(&mut c, PracticeAction::ListenOffset(true));
    assert_eq!(c.listen_offset_ms(), 10);
    assert_eq!(c.take_listen_offset_change(), Some(10));
    assert_eq!(c.take_listen_offset_change(), None);
    for _ in 0..200 {
        practice(&mut c, PracticeAction::ListenOffset(false));
    }
    assert_eq!(c.listen_offset_ms(), -500, "the limit");
    // Loading the saved value is not a change to save.
    c.set_listen_offset_ms(25);
    assert_eq!(c.listen_offset_ms(), 25);
    assert_eq!(c.take_listen_offset_change(), None);
    c.set_listen_offset_ms(9999);
    assert_eq!(c.listen_offset_ms(), 500);
}

#[test]
fn the_offset_moves_late_notes_onto_the_beat() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    // The player is 50 ms behind the click.
    let late = playing(1, &RIFF, 2_400);
    run(&mut c, &late, 1);
    let first = verdicts(&c);
    assert!(
        first.iter().filter(|v| **v == Verdict::Late).count() >= 3,
        "{first:?}"
    );
    practice(&mut c, PracticeAction::ListenClear);
    c.set_listen_offset_ms(50);
    c.listen_attach_for_test(SR);
    run(&mut c, &late, 1);
    assert_eq!(verdicts(&c), [Verdict::Hit; 4]);
}

#[test]
fn a_calibration_sets_the_offset_from_the_notes_played_along_the_click() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    practice(&mut c, PracticeAction::ListenCalibrate);
    assert!(c.practice_state().listen.calibrating);
    assert!(c.metronome, "the click is on while calibrating");
    // Eight notes on the beats of two laps, played 40 ms late.
    let beats: Vec<(i64, i32)> = (0..4).map(|b| (b * PPQ, 45)).collect();
    run(&mut c, &playing(2, &beats, 1_920), 2);
    assert!(!c.practice_state().listen.calibrating, "it ended by itself");
    let off = c.listen_offset_ms();
    assert!((38..=42).contains(&off), "{off}");
    assert_eq!(c.take_listen_offset_change(), Some(off));
    assert!(!c.metronome, "the click is back off");
    // What was played along the click is not graded.
    assert!(
        c.listen_session().passes() <= 1,
        "at most the rest of the lap after it ended"
    );
}

#[test]
fn a_calibration_with_too_few_notes_says_so_and_keeps_the_offset() {
    let (mut c, _) = core_with_riff();
    c.listen_attach_for_test(SR);
    c.set_listen_offset_ms(15);
    practice(&mut c, PracticeAction::ListenCalibrate);
    run(&mut c, &playing(1, &[(0, 45)], 0), 1);
    assert_eq!(c.listen_offset_ms(), 15);
    assert_eq!(c.take_listen_offset_change(), None);
    assert!(c.last_rejection().unwrap().contains("Not enough notes"));
}

#[test]
fn nothing_is_graded_without_an_open_tab_clip() {
    let (mut c, _) = core_with_riff();
    c.close_tab_editor();
    c.listen_attach_for_test(SR);
    run(&mut c, &playing(1, &RIFF, 0), 1);
    assert_eq!(c.listen_session().passes(), 0);
    assert_eq!(c.practice_state().listen.last_percent, None);
}
