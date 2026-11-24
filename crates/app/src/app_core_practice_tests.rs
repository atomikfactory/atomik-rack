//! The practice session in the app core: the speed reaches the engine and nothing else, it is not
//! saved and resets when a project opens, the count-in belongs to a plain Play, the speed trainer
//! counts the engine's loop passes through a replaceable judge, and the practice loop comes from
//! the open pane.

use super::tests::rack_only_core;
use super::*;
use crate::practice::{PassJudge, PassReport, PracticeAction};
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::{PPQ, Ticks};
use atomik_engine::TransportCommand;
use atomik_tab::sheet::{SheetBlock, parse_sheet};
use atomik_ui::{TrainerValue, TransportAction, UiAction};

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

fn core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(DeviceKey("a".to_string())),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: DeviceKey("a".to_string()),
    }))
    .unwrap();
    c.sink_mut().take();
    c
}

fn transports(calls: &[SinkCall]) -> Vec<TransportCommand> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Transport(t) => Some(*t),
            _ => None,
        })
        .collect()
}

fn speeds(calls: &[SinkCall]) -> Vec<f32> {
    transports(calls)
        .into_iter()
        .filter_map(|t| match t {
            TransportCommand::Speed(f) => Some(f),
            _ => None,
        })
        .collect()
}

fn do_practice(c: &mut AppCore<RecordingSink>, a: PracticeAction) -> Vec<SinkCall> {
    c.practice_action(a);
    c.sink_mut().take()
}

fn loop_on(c: &mut AppCore<RecordingSink>) {
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (0, 2 * BAR),
        enabled: true,
    }))
    .unwrap();
    c.sink_mut().take();
}

#[test]
fn the_speed_goes_to_the_engine_as_a_factor_and_only_when_it_changes() {
    let mut c = core();
    assert_eq!(
        speeds(&do_practice(&mut c, PracticeAction::Speed(50))),
        [0.5]
    );
    assert_eq!(c.practice().speed(), 50);
    assert!(
        speeds(&do_practice(&mut c, PracticeAction::Speed(50))).is_empty(),
        "no change, no message"
    );
    assert_eq!(
        speeds(&do_practice(&mut c, PracticeAction::SpeedUp)),
        [0.55]
    );
    assert_eq!(
        speeds(&do_practice(&mut c, PracticeAction::SpeedDown)),
        [0.5]
    );
    do_practice(&mut c, PracticeAction::Speed(5));
    assert_eq!(c.practice().speed(), 25, "the slowest is a quarter");
    do_practice(&mut c, PracticeAction::Speed(900));
    assert_eq!(c.practice().speed(), 150, "the fastest is one and a half");
}

#[test]
fn the_speed_is_not_saved_and_a_project_opens_at_full_speed() {
    let mut c = core();
    do_practice(&mut c, PracticeAction::Speed(50));
    let json = atomik_model::save_project(c.doc());
    assert!(
        !json.to_lowercase().contains("practice"),
        "the project holds no practice state"
    );
    assert!(c.open_str(&json, None).is_ok());
    assert_eq!(c.practice().speed(), 100);
    let calls = c.sink_mut().take();
    assert!(
        speeds(&calls).is_empty(),
        "the new engine plays at 100% without being told"
    );
    do_practice(&mut c, PracticeAction::Speed(75));
    c.new_document();
    assert_eq!(c.practice().speed(), 100);
    assert!(!c.practice().trainer().enabled);
}

#[test]
fn a_rebuild_in_place_keeps_the_speed() {
    let mut c = core();
    do_practice(&mut c, PracticeAction::Speed(75));
    c.rebuild_engine();
    assert_eq!(speeds(&c.sink_mut().take()), [0.75]);
}

#[test]
fn recording_is_refused_while_the_speed_is_not_full() {
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: DeviceKey("a".to_string()),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    do_practice(&mut c, PracticeAction::Speed(50));
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
    assert!(!c.is_recording());
    assert!(c.last_rejection().unwrap().contains("100%"));
}

#[test]
fn the_count_in_belongs_to_a_plain_play() {
    let mut c = core();
    c.handle_action(UiAction::Transport(TransportAction::Play));
    assert_eq!(transports(&c.sink_mut().take()), [TransportCommand::Play]);
    c.handle_action(UiAction::Transport(TransportAction::Stop));
    c.sink_mut().take();
    do_practice(&mut c, PracticeAction::CountIn(1));
    c.handle_action(UiAction::Transport(TransportAction::Play));
    let plan = c
        .song_timing()
        .unwrap()
        .pre_roll(Ticks(0), 1, f64::from(c.doc().sample_rate))
        .unwrap();
    assert_eq!(
        transports(&c.sink_mut().take()),
        [TransportCommand::PlayWithPreRoll(plan)]
    );
    assert_eq!(plan.beats, 4);
    c.handle_action(UiAction::Transport(TransportAction::Stop));
    c.sink_mut().take();
    do_practice(&mut c, PracticeAction::CountIn(2));
    c.handle_action(UiAction::Transport(TransportAction::TogglePlay));
    match transports(&c.sink_mut().take()).as_slice() {
        [TransportCommand::PlayWithPreRoll(p)] => assert_eq!(p.beats, 8),
        other => panic!("{other:?}"),
    }
    // Back to none: a plain Play again.
    c.handle_action(UiAction::Transport(TransportAction::Stop));
    do_practice(&mut c, PracticeAction::CountIn(0));
    c.handle_action(UiAction::Transport(TransportAction::Play));
    assert_eq!(transports(&c.sink_mut().take()), [TransportCommand::Play]);
}

/// Scripts the engine's loop counter and runs one frame.
fn loops(c: &mut AppCore<RecordingSink>, n: u64) {
    c.sink_mut().state.loop_count = n;
    c.transport_frame(1.0);
}

#[test]
fn the_trainer_steps_up_after_the_passes_the_engine_completes() {
    let mut c = core();
    loop_on(&mut c);
    c.handle_action(UiAction::Transport(TransportAction::Play));
    c.sink_mut().take();
    assert_eq!(
        speeds(&do_practice(&mut c, PracticeAction::ToggleTrainer)),
        [0.5]
    );
    loops(&mut c, 0);
    c.sink_mut().take();
    loops(&mut c, 1);
    assert_eq!(c.practice().trainer().passes, 1);
    assert!(speeds(&c.sink_mut().take()).is_empty(), "one pass of two");
    loops(&mut c, 2);
    assert_eq!(speeds(&c.sink_mut().take()), [0.55]);
    assert_eq!(c.practice().speed(), 55);
    // Several wraps in one frame count each.
    loops(&mut c, 6);
    assert_eq!(c.practice().speed(), 65);
    assert_eq!(c.practice_state().speed, 65);
}

#[test]
fn the_trainer_needs_a_running_transport_and_does_not_count_a_stopped_one() {
    let mut c = core();
    loop_on(&mut c);
    do_practice(&mut c, PracticeAction::ToggleTrainer);
    loops(&mut c, 10);
    loops(&mut c, 20);
    assert_eq!(c.practice().trainer().passes, 0);
    assert_eq!(c.practice().speed(), 50);
}

struct Never;

impl PassJudge for Never {
    fn is_clean(&mut self, _: &PassReport) -> bool {
        false
    }
}

#[test]
fn a_replaced_judge_decides_which_passes_are_clean() {
    let mut c = core();
    loop_on(&mut c);
    c.set_pass_judge(Box::new(Never));
    c.handle_action(UiAction::Transport(TransportAction::Play));
    do_practice(&mut c, PracticeAction::ToggleTrainer);
    loops(&mut c, 0);
    loops(&mut c, 9);
    assert_eq!(c.practice().trainer().passes, 9);
    assert_eq!(c.practice().speed(), 50, "no pass was clean");
}

#[test]
fn the_trainer_asks_for_the_loop_and_its_values_edit_in_place() {
    let mut c = core();
    do_practice(&mut c, PracticeAction::ToggleTrainer);
    assert!(c.last_rejection().unwrap().contains("loop"));
    do_practice(&mut c, PracticeAction::Trainer(TrainerValue::Start, true));
    assert_eq!(
        c.practice().speed(),
        55,
        "a new start restarts the running trainer"
    );
    do_practice(&mut c, PracticeAction::Trainer(TrainerValue::After, true));
    assert_eq!(c.practice().trainer().after, 3);
}

fn linked_sheet(first: u32, second: u32) -> atomik_tab::sheet::ChordSheet {
    let mut sheet =
        parse_sheet("[Verse]\nG       D\nThe lantern swings\nEm      C\nalong the road\n")
            .unwrap()
            .sheet;
    let mut bars = [first, second].into_iter();
    for b in &mut sheet.blocks {
        if let SheetBlock::Line { bar, .. } = b {
            *bar = bars.next();
        }
    }
    sheet
}

#[test]
fn loop_selection_needs_a_pane_and_loops_the_bars_of_the_linked_lines() {
    let mut c = core();
    do_practice(&mut c, PracticeAction::LoopSelection);
    assert!(c.last_rejection().unwrap().contains("Tab Editor"));
    c.apply_command(Command::Song(SongCommand::SetSheets(vec![linked_sheet(
        2, 4,
    )])))
    .unwrap();
    c.open_chord_sheet();
    do_practice(&mut c, PracticeAction::LoopSelection);
    // From bar 2 to the end of the line linked to bar 4, as long as the gap before it (two bars).
    assert_eq!(c.doc().song.loop_range, (BAR, 5 * BAR));
    assert!(c.doc().song.loop_enabled);
    // The practice bars show the loop.
    assert!(c.practice_state().looping);
}

#[test]
fn a_sheet_without_links_has_no_loop_to_make() {
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::SetSheets(vec![
        parse_sheet("[Verse]\nG\nA line\n").unwrap().sheet,
    ])))
    .unwrap();
    c.open_chord_sheet();
    do_practice(&mut c, PracticeAction::LoopSelection);
    assert!(c.last_rejection().unwrap().contains("linked"));
    assert!(!c.doc().song.loop_enabled);
}
